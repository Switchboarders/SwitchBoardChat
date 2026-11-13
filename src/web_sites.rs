//! Websites linked to a call center (web chat), without editing the server's settings.
//!
//! SwitchBoard asks for a one-time pairing code for a committee (`create_web_pairing`, valid 30 minutes) and
//! shows it to the committee's webmaster as one string: `https://<this server>#XXXX-XXXX-XXXX`. The WP Chat
//! plugin sends the code with its website's address to `POST /web/pair`; from then on the browsers of that
//! website may open web chats (CORS) and every new chat goes to that committee. `WEB_ALLOWED_ORIGINS` and
//! `WEB_CALL_CENTER_ID` keep working as before for sites configured on the server.
//!
//! Codes are stored hashed, used once, and the pairing endpoint is rate limited per address.

use crate::db_manager::DbManager;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// How long a pairing code can be used
pub const CODE_VALIDITY_SECS: i64 = 30 * 60;
/// Pairing attempts per address and hour (a code has 60 bits: guessing is hopeless, this keeps logs quiet)
const PAIR_ATTEMPTS_PER_HOUR: usize = 10;
/// Unambiguous letters and digits (no 0/O, 1/I/L)
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// origin -> call center, loaded from the database on first use and after every change
static SITES: LazyLock<RwLock<Option<HashMap<String, u64>>>> = LazyLock::new(|| RwLock::new(None));
static ATTEMPTS: LazyLock<Mutex<HashMap<IpAddr, VecDeque<Instant>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `https://Example.org:443/path` -> `https://example.org`. Only https (http for localhost, for tests and
/// development); no user, path, query or fragment.
pub fn normalize_origin(input: &str) -> Option<String> {
    let url = url::Url::parse(input.trim()).ok()?;
    let host = url.host_str()?.to_lowercase();
    let local = host == "localhost" || host == "127.0.0.1";
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return None,
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if !matches!(url.path(), "" | "/") || url.query().is_some() {
        return None;
    }
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// A new code, e.g. `K7QM-3XRA-PW2D` (12 characters from 31: about 59 bits)
pub fn generate_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let chars: Vec<char> = (0..12).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
    format!(
        "{}-{}-{}",
        chars[0..4].iter().collect::<String>(),
        chars[4..8].iter().collect::<String>(),
        chars[8..12].iter().collect::<String>()
    )
}

/// The code as stored: hashed, ignoring case, spaces and dashes (people retype it)
pub fn code_hash(code: &str) -> String {
    let clean: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    crate::crypto::hash_token(&format!("web-pairing:{clean}"))
}

fn load() -> HashMap<String, u64> {
    match DbManager::new().web_sites() {
        Ok(rows) => rows.into_iter().map(|(origin, id, _)| (origin, id)).collect(),
        Err(err) => {
            warn!("Web sites: could not read the linked websites: {err}");
            HashMap::new()
        }
    }
}

/// Re-reads the linked websites (after a pairing or a removal)
pub fn reload() {
    let sites = load();
    *SITES.write().unwrap_or_else(PoisonError::into_inner) = Some(sites);
}

/// The call center a linked website sends its chats to
pub fn call_center_for(origin: &str) -> Option<u64> {
    let origin = normalize_origin(origin)?;
    {
        let guard = SITES.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(sites) = guard.as_ref() {
            return sites.get(&origin).copied();
        }
    }
    reload();
    SITES.read().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|s| s.get(&origin).copied())
}

/// A new pairing code for this call center (shown once by SwitchBoard)
pub fn create_pairing(id_call_center: u64) -> anyhow::Result<(String, i64)> {
    let code = generate_code();
    let expires_at = chrono::Utc::now().timestamp() + CODE_VALIDITY_SECS;
    DbManager::new().add_web_pairing(id_call_center, &code_hash(&code), expires_at)?;
    Ok((code, expires_at))
}

pub fn remove(id_call_center: u64, origin: &str) -> anyhow::Result<bool> {
    let origin = normalize_origin(origin).unwrap_or_else(|| origin.trim().to_string());
    let removed = DbManager::new().remove_web_site(id_call_center, &origin)?;
    reload();
    if removed {
        info!("Web sites: {origin} unlinked from call center {id_call_center}");
    }
    Ok(removed)
}

pub fn list(id_call_center: u64) -> anyhow::Result<Vec<(String, i64)>> {
    Ok(DbManager::new()
        .web_sites()?
        .into_iter()
        .filter(|(_, id, _)| *id == id_call_center)
        .map(|(origin, _, at)| (origin, at))
        .collect())
}

fn too_many_attempts(ip: IpAddr) -> bool {
    let mut attempts = ATTEMPTS.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    let q = attempts.entry(ip).or_default();
    while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600)) {
        q.pop_front();
    }
    if q.len() >= PAIR_ATTEMPTS_PER_HOUR {
        return true;
    }
    q.push_back(now);
    false
}

/// `POST /web/pair {"code": "...", "origin": "https://www.example.org"}`, called by the WP Chat plugin from the
/// website's server. Answers `{"ok": true}` or `{"error": "invalid_code" | "bad_origin" | "too_many_attempts"}`.
pub async fn pair(ConnectInfo(addr): ConnectInfo<SocketAddr>, _headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if too_many_attempts(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too_many_attempts" }))).into_response();
    }
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let code = request["code"].as_str().unwrap_or_default();
    let Some(origin) = request["origin"].as_str().and_then(normalize_origin) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_origin" }))).into_response();
    };
    if code.chars().filter(|c| c.is_ascii_alphanumeric()).count() != 12 {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "invalid_code" }))).into_response();
    }
    match DbManager::new().redeem_web_pairing(&code_hash(code), &origin, chrono::Utc::now().timestamp()) {
        Ok(Some(id)) => {
            reload();
            info!("Web sites: {origin} linked to call center {id}");
            Json(json!({ "ok": true, "origin": origin })).into_response()
        }
        Ok(None) => (StatusCode::FORBIDDEN, Json(json!({ "error": "invalid_code" }))).into_response(),
        Err(err) => {
            warn!("Web sites: pairing failed: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "server_error" }))).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_normalized_and_checked() {
        assert_eq!(normalize_origin("https://WWW.Example.org/").as_deref(), Some("https://www.example.org"));
        assert_eq!(normalize_origin("https://example.org:8443").as_deref(), Some("https://example.org:8443"));
        assert_eq!(normalize_origin("https://example.org:443").as_deref(), Some("https://example.org"));
        assert_eq!(normalize_origin("http://localhost:8085").as_deref(), Some("http://localhost:8085"));
        assert_eq!(normalize_origin("http://example.org"), None, "plain http only for localhost");
        assert_eq!(normalize_origin("https://example.org/blog"), None, "an origin has no path");
        assert_eq!(normalize_origin("https://user:pw@example.org"), None);
        assert_eq!(normalize_origin("javascript:alert(1)"), None);
        assert_eq!(normalize_origin(""), None);
    }

    #[test]
    fn codes_are_readable_and_hashed_loosely() {
        let code = generate_code();
        assert_eq!(code.len(), 14);
        assert!(code.chars().all(|c| c == '-' || ALPHABET.contains(&(c as u8))));
        assert_ne!(generate_code(), code);
        assert_eq!(code_hash("k7qm-3xra-pw2d"), code_hash(" K7QM 3XRA PW2D "));
        assert_ne!(code_hash("K7QM-3XRA-PW2D"), code_hash("K7QM-3XRA-PW2E"));
    }

    #[test]
    fn pairing_attempts_are_limited_per_address() {
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        for _ in 0..PAIR_ATTEMPTS_PER_HOUR {
            assert!(!too_many_attempts(ip));
        }
        assert!(too_many_attempts(ip));
        assert!(!too_many_attempts("203.0.113.10".parse().unwrap()));
    }
}
