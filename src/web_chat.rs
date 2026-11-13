//! Web chat channel: lets a website visitor (the wp-chat WordPress widget) talk to the respondents.
//!
//! Privacy design (Quebec Law 25):
//! - Message text is never written to disk, a queue or a log. It only passes through memory.
//! - The visitor is identified by a random session token (kept in the widget's memory and sent in an
//!   `Authorization: Bearer` header, so browsers that block third-party cookies still work); only its
//!   SHA-256 is kept, in memory. A cookie is only set when `WEB_SESSION_COOKIE=true` (older widgets).
//! - The Matrix room needs a caller number, so each session gets a random pseudo number that
//!   cannot be a real phone number (>= `WEB_DID_MIN`). The room row is deleted when the chat ends.
//! - Sessions end on request, on respondent `!fin` / `!end`, or after `WEB_IDLE_TIMEOUT_MIN`.
//! - Sessions live in memory only: at startup every leftover web room is left and deleted.
//!
//! Routes (CORS limited to `WEB_ALLOWED_ORIGINS`):
//!   POST /web/session, POST /web/message {text}, GET /web/stream (SSE), POST /web/end

use crate::crypto::{generate_token, hash_ip, hash_token};
use crate::db_manager::DbManager;
use crate::web_server::AppState;
use crate::{help, names, rooms};
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use matrix_sdk::Client;
use rand::{Rng, rngs::OsRng};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::env;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{error, info, warn};

/// Pseudo caller numbers for web chats start here. Real phone numbers have at most 15 digits.
pub const WEB_DID_MIN: u64 = 100_000_000_000_000_000;
const COOKIE_NAME: &str = "wpchat";
const MAX_MESSAGE_CHARS: usize = 1000;
/// The conversation is kept in memory only (never on disk) so a page reload can restore it; it is
/// dropped with the session. Oldest lines go first past this many.
const MAX_HISTORY: usize = 300;
/// Per session: at most this many messages per window
const RATE_MESSAGES: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(30);
/// Live streams one session may hold open (a few browser tabs); more would only tie up connections
const MAX_STREAMS_PER_SESSION: usize = 4;
/// Per network address: at most this many new sessions per hour (`WEB_SESSIONS_PER_IP_PER_HOUR`, default 5)
fn max_sessions_per_ip_per_hour() -> usize {
    env::var("WEB_SESSIONS_PER_IP_PER_HOUR").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(5)
}

#[derive(Clone, Debug)]
enum WebEvent {
    Message { seq: u64, text: String },
    /// The visitor was banned: JSON with the block marker the browser must keep
    Banned(String),
    Closed,
}

struct WebSession {
    call_center_id: u64,
    call_center_did: u64,
    pseudo_did: u64,
    /// Keyed hash of the visitor's network address (never the address): used by `!ban`
    ip_hash: String,
    /// Another keyed hash of the same address, only for the room name shown to respondents: the same network
    /// always gets the same name (like an SMS caller's number), so a returning abusive visitor is recognisable.
    name_hash: String,
    /// Matrix room, created on the first visitor message. Locked during creation.
    room: tokio::sync::Mutex<Option<String>>,
    tx: broadcast::Sender<WebEvent>,
    state: Mutex<SessionState>,
}

/// One line of the in-memory conversation
struct HistoryItem {
    seq: u64,
    /// true = written by the visitor
    mine: bool,
    text: String,
    /// A respondent line already pushed to a connected stream
    delivered: bool,
}

struct SessionState {
    last_activity: Instant,
    recent_messages: VecDeque<Instant>,
    history: VecDeque<HistoryItem>,
    next_seq: u64,
    /// When the last live stream closed (None while one is connected)
    disconnected_since: Option<Instant>,
    created: Instant,
    /// True once the visitor's first message was delivered
    messaged: bool,
}

#[derive(Default)]
struct Registry {
    by_hash: HashMap<String, Arc<WebSession>>,
    by_room: HashMap<String, String>,
    sessions_per_ip: HashMap<IpAddr, VecDeque<Instant>>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

// ========================================
// CONFIGURATION AND HELPERS
// ========================================

fn allowed_origins() -> Vec<String> {
    env::var("WEB_ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn idle_timeout() -> Duration {
    let minutes: u64 = env::var("WEB_IDLE_TIMEOUT_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2880);
    Duration::from_secs(minutes * 60)
}

fn max_sessions() -> usize {
    env::var("WEB_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200)
}

/// Also set the legacy `wpchat` cookie (needed only by widgets older than 0.5.0).
fn session_cookie_enabled() -> bool {
    matches!(
        env::var("WEB_SESSION_COOKIE").unwrap_or_default().to_lowercase().as_str(),
        "true" | "1" | "on" | "yes"
    )
}

/// A visitor whose live stream has been closed this long has left: the chat is ended.
fn abandon_after() -> Duration {
    let minutes: u64 = env::var("WEB_ABANDON_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|m| *m > 0)
        .unwrap_or(5);
    Duration::from_secs(minutes * 60)
}

fn is_abandoned(disconnected_since: Option<Instant>, receivers: usize, now: Instant, grace: Duration) -> bool {
    receivers == 0 && disconnected_since.is_some_and(|t| now.saturating_duration_since(t) > grace)
}

pub fn cors_layer() -> CorsLayer {
    let origins: Vec<HeaderValue> = allowed_origins()
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, HeaderName::from_static("x-wpchat-block"), HeaderName::from_static("last-event-id")])
        .allow_credentials(true)
}

/// The Origin must be allowed on every call, whatever carries the session (header or cookie).
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    allowed_origins().iter().any(|o| o == origin.trim_end_matches('/'))
}

fn error_response(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == COOKIE_NAME).then(|| value.to_string())
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ")?.trim().to_string())
}

/// The session token: `Authorization: Bearer` first, else the legacy cookie, else (for
/// `navigator.sendBeacon`, which cannot set headers) a token passed in the request body.
fn session_token(headers: &HeaderMap, body_token: Option<&str>) -> Option<String> {
    bearer_token(headers)
        .or_else(|| cookie_token(headers))
        .or_else(|| body_token.map(str::to_string))
        .filter(|t| !t.is_empty() && t.len() <= 128)
}

fn token_from_body(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()?
        .get("token")?
        .as_str()
        .map(str::to_string)
}

fn lookup_session(headers: &HeaderMap) -> Option<(String, Arc<WebSession>)> {
    lookup_by_token(&session_token(headers, None)?)
}

fn lookup_by_token(token: &str) -> Option<(String, Arc<WebSession>)> {
    let hash = hash_token(token);
    let session = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner).by_hash.get(&hash).cloned()?;
    Some((hash, session))
}

fn set_cookie_header(token: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/web; HttpOnly; Secure; SameSite=None; Max-Age={max_age}"
    ))
    .expect("cookie is ascii")
}

// ========================================
// SHORT-LIVED CACHES: a flood must not turn into a database flood
// ========================================

const CACHE_TTL: Duration = Duration::from_secs(10);

fn stale(at: Option<Instant>) -> bool {
    at.is_none_or(|t| t.elapsed() > CACHE_TTL)
}

static BLOCK_CACHE: LazyLock<Mutex<(Option<Instant>, HashSet<String>)>> =
    LazyLock::new(|| Mutex::new((None, HashSet::new())));
static MARKERS_CACHE: Mutex<(Option<Instant>, i64)> = Mutex::new((None, 0));
static BANS_CACHE: Mutex<(Option<Instant>, i64)> = Mutex::new((None, 0));
/// (when read, call center id, its DID, has active respondents)
static CONFIG_CACHE: Mutex<(Option<Instant>, u64, Option<u64>, bool)> = Mutex::new((None, 0, None, false));
/// The call center's `no_respondent` macro, cached like the rest (nights bring many "nobody is on duty" answers)
static NO_RESPONDENT_CACHE: Mutex<(Option<Instant>, u64, Option<String>)> = Mutex::new((None, 0, None));

/// Is this address blocked? Reads the whole (small) block list at most every 10 seconds.
fn is_blocked_cached(ip_hash: &str, now_ts: i64) -> bool {
    let mut cache = BLOCK_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if stale(cache.0) {
        match DbManager::new().list_active_web_block_hashes(now_ts) {
            Ok(list) => {
                cache.1 = list.into_iter().collect();
                cache.0 = Some(Instant::now());
            }
            Err(err) => error!("Web chat: could not read the block list: {err}"),
        }
    }
    cache.1.contains(ip_hash)
}

/// A new block takes effect at once (the cached list would otherwise miss it for up to 10 seconds)
fn block_cache_add(ip_hash: &str) {
    BLOCK_CACHE.lock().unwrap_or_else(PoisonError::into_inner).1.insert(ip_hash.to_string());
}

/// After an unban: forget every cached answer that depended on the blocks
fn block_cache_reset() {
    BLOCK_CACHE.lock().unwrap_or_else(PoisonError::into_inner).0 = None;
    MARKERS_CACHE.lock().unwrap_or_else(PoisonError::into_inner).0 = None;
    BANS_CACHE.lock().unwrap_or_else(PoisonError::into_inner).0 = None;
}

fn markers_valid_after_cached() -> i64 {
    let mut c = MARKERS_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if stale(c.0) {
        c.1 = DbManager::new().get_web_state("markers_valid_after").ok().flatten().unwrap_or(0);
        c.0 = Some(Instant::now());
    }
    c.1
}

fn recent_bans_cached(now_ts: i64) -> i64 {
    let mut c = BANS_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if stale(c.0) {
        c.1 = DbManager::new()
            .count_recent_web_blocks(now_ts - crate::web_abuse::window_secs())
            .unwrap_or(0);
        c.0 = Some(Instant::now());
    }
    c.1
}

/// (DID of the call center, does it have active respondents), or None if the call center does not exist
fn call_center_cached(id: u64) -> Option<(u64, bool)> {
    let mut c = CONFIG_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if stale(c.0) || c.1 != id {
        let db = DbManager::new();
        c.2 = match db.get_call_center_by_id(id) {
            Ok(Some(cc)) => Some(cc.call_center_did),
            _ => None,
        };
        c.3 = matches!(db.get_call_center_respondant(id, true), Ok(Some(r)) if !r.is_empty());
        c.1 = id;
        c.0 = Some(Instant::now());
    }
    c.2.map(|did| (did, c.3))
}

fn no_respondent_text_cached(id: u64) -> Option<String> {
    let mut c = NO_RESPONDENT_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if stale(c.0) || c.1 != id {
        c.2 = match DbManager::new().get_macro_by_tag(&id, "no_respondent".to_string()) {
            Ok(Some(m)) if !m.macro_text.trim().is_empty() => Some(m.macro_text),
            _ => None,
        };
        c.1 = id;
        c.0 = Some(Instant::now());
    }
    c.2.clone()
}

/// "Nobody is on duty": 503 `no_respondent`, with the call center's `no_respondent` macro as `message` when one is
/// set (the widget shows it instead of its generic sentence, the same text SMS callers get).
fn no_respondent_response(call_center_id: u64) -> Response {
    let mut body = json!({ "error": "no_respondent" });
    if let Some(text) = no_respondent_text_cached(call_center_id) {
        body["message"] = json!(text);
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

/// Keyed hash used for the visitor's display name. Domain-separated from the block-list hash, and keyed with
/// PHONE_HASH_SECRET: without that secret nobody can go from a name back to an address, even by trying all of them.
fn visitor_name_hash(ip: IpAddr) -> String {
    hash_ip(&format!("display-name:{}", ip_key(ip)))
}

/// What a block applies to: the full IPv4 address, or the /64 network of an IPv6 address (a visitor
/// controls the last 64 bits and could otherwise change them at will).
fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// Days a `!ban` lasts when none is given (`WEB_BAN_DAYS`, default 1)
fn ban_default_days() -> u64 {
    env::var("WEB_BAN_DAYS").ok().and_then(|v| v.parse().ok()).filter(|d| *d > 0).unwrap_or(1)
}

/// Longest ban a respondent can set, in days (`WEB_BAN_MAX_DAYS`, default 30)
fn ban_max_days() -> u64 {
    env::var("WEB_BAN_MAX_DAYS").ok().and_then(|v| v.parse().ok()).filter(|d| *d > 0).unwrap_or(30)
}

/// Parses `!ban`, `!ban 7`, `!bloquer 3j`, `!ban 2d`: Some(days asked, if any) for a ban command.
pub fn parse_ban_command(text: &str) -> Option<Option<u64>> {
    let mut words = text.trim().split_whitespace();
    let command = words.next()?.to_lowercase();
    if command != "!ban" && command != "!bloquer" {
        return None;
    }
    let days = words
        .next()
        .and_then(|w| w.trim_end_matches(|c| matches!(c, 'd' | 'D' | 'j' | 'J')).parse::<u64>().ok());
    Some(days)
}

/// Bounds a requested duration between 1 day and the maximum
fn clamp_ban_days(asked: Option<u64>, default: u64, max: u64) -> u64 {
    asked.unwrap_or(default).clamp(1, max)
}

/// Which listener a request came in on: the plain-HTTP one meant for the reverse proxy
/// (8088), or the HTTPS one that is reachable directly (3443).
#[derive(Clone, Copy, Debug)]
pub struct ViaProxy(pub bool);

/// `TRUSTED_PROXIES`: comma-separated addresses or networks (e.g. `172.18.0.0/16, ::1`) of the
/// reverse proxy. Empty: any peer of the proxy listener counts (that listener must stay private).
fn trusted_proxies() -> Vec<ipnet::IpNet> {
    static LIST: LazyLock<Vec<ipnet::IpNet>> = LazyLock::new(|| parse_proxy_list(&env::var("TRUSTED_PROXIES").unwrap_or_default()));
    LIST.clone()
}

fn parse_proxy_list(raw: &str) -> Vec<ipnet::IpNet> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<ipnet::IpNet>().ok().or_else(|| s.parse::<IpAddr>().ok().map(ipnet::IpNet::from)))
        .collect()
}

/// The visitor's address. X-Forwarded-For is only believed on the proxy listener, from a trusted
/// peer: on the directly reachable listener anyone could write it and dodge bans and rate limits.
fn client_ip(headers: &HeaderMap, addr: SocketAddr, via_proxy: bool, trusted: &[ipnet::IpNet]) -> IpAddr {
    let peer = addr.ip();
    let peer_trusted = via_proxy && (trusted.is_empty() || trusted.iter().any(|n| n.contains(&peer)));
    if !peer_trusted {
        return peer;
    }
    // Behind the reverse proxy the peer is the proxy: use the last X-Forwarded-For entry it appended.
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(peer)
}

pub fn is_web_did(did: u64) -> bool {
    did >= WEB_DID_MIN
}

impl WebSession {
    /// Appends a line to the conversation and returns its number
    fn record(&self, mine: bool, text: &str) -> u64 {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::record_locked(&mut st, mine, text, mine)
    }

    fn record_locked(st: &mut SessionState, mine: bool, text: &str, delivered: bool) -> u64 {
        let seq = st.next_seq;
        st.next_seq += 1;
        st.history.push_back(HistoryItem { seq, mine, text: text.to_string(), delivered });
        while st.history.len() > MAX_HISTORY {
            st.history.pop_front();
        }
        seq
    }

    fn touch(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).last_activity = Instant::now();
    }
}

// ========================================
// HANDLERS
// ========================================

#[derive(Deserialize)]
pub struct MessageBody {
    text: String,
}

/// POST /web/session: creates (or resumes) a session and returns its token.
pub async fn create_session(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::Extension(ViaProxy(via_proxy)): axum::Extension<ViaProxy>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !origin_allowed(&headers) {
        return error_response(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    // Cheap, in-memory checks first: a flood must cost as little as possible per request
    let now_ts = chrono::Utc::now().timestamp();
    let attack = crate::web_abuse::note_start_request(now_ts as u64);
    let ip = client_ip(&headers, addr, via_proxy, &trusted_proxies());
    let ip_hash = hash_ip(&ip_key(ip));
    let name_hash = visitor_name_hash(ip);
    if is_blocked_cached(&ip_hash, now_ts) {
        return error_response(StatusCode::FORBIDDEN, "blocked");
    }
    // A browser that was banned keeps a signed marker and sends it, so a new VPN does not help
    if let Some(marker) = headers.get("x-wpchat-block").and_then(|v| v.to_str().ok()) {
        if crate::web_abuse::marker_valid(marker, now_ts, markers_valid_after_cached()) {
            return error_response(StatusCode::FORBIDDEN, "blocked");
        }
    }
    if lookup_session(&headers).is_some() {
        let token = session_token(&headers, None).unwrap_or_default();
        return Json(json!({ "resumed": true, "token": token })).into_response();
    }
    let Some(call_center_id) = env::var("WEB_CALL_CENTER_ID").ok().and_then(|v| v.parse::<u64>().ok())
    else {
        error!("WEB_CALL_CENTER_ID is not set or invalid");
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "not_configured");
    };
    let Some((call_center_did, has_respondents)) = call_center_cached(call_center_id) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "not_configured");
    };
    if !has_respondents {
        return no_respondent_response(call_center_id);
    }

    // Proof of work: after abuse, when this address keeps opening chats, or while under attack
    {
        let ip_sessions = {
            let reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
            let t = Instant::now();
            reg.sessions_per_ip
                .get(&ip)
                .map(|d| d.iter().filter(|s| t.saturating_duration_since(**s) <= Duration::from_secs(3600)).count())
                .unwrap_or(0)
        };
        let recent_bans = recent_bans_cached(now_ts);
        if let Some(bits) =
            crate::web_abuse::challenge_bits(crate::web_abuse::challenge_mode(), recent_bans, ip_sessions, attack)
        {
            let key = ip_key(ip);
            let solved = serde_json::from_slice::<serde_json::Value>(&body).ok().is_some_and(|v| {
                match (v.get("challenge").and_then(|c| c.as_str()), v.get("nonce").and_then(|n| n.as_str())) {
                    (Some(c), Some(n)) => crate::web_abuse::verify_solution(c, n, &key, bits, now_ts),
                    _ => false,
                }
            });
            if !solved {
                return (
                    StatusCode::PRECONDITION_REQUIRED,
                    Json(json!({
                        "error": "challenge_required",
                        "bits": bits,
                        "challenge": crate::web_abuse::make_challenge(&key, bits, now_ts),
                    })),
                )
                    .into_response();
            }
        }
    }

    let token = generate_token(48);
    let hash = hash_token(&token);
    let now = Instant::now();
    {
        let mut reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        if reg.by_hash.len() >= max_sessions() {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "busy");
        }
        let recent = reg.sessions_per_ip.entry(ip).or_default();
        while recent.front().is_some_and(|t| now - *t > Duration::from_secs(3600)) {
            recent.pop_front();
        }
        if recent.len() >= max_sessions_per_ip_per_hour() {
            return error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
        }
        recent.push_back(now);

        let mut pseudo_did;
        loop {
            pseudo_did = OsRng.gen_range(WEB_DID_MIN..WEB_DID_MIN * 9);
            if !reg.by_hash.values().any(|s| s.pseudo_did == pseudo_did) {
                break;
            }
        }
        let (tx, _rx) = broadcast::channel(32);
        reg.by_hash.insert(
            hash,
            Arc::new(WebSession {
                call_center_id,
                call_center_did,
                pseudo_did,
                ip_hash,
                name_hash,
                room: tokio::sync::Mutex::new(None),
                tx,
                state: Mutex::new(SessionState {
                    last_activity: now,
                    recent_messages: VecDeque::new(),
                    history: VecDeque::new(),
                    next_seq: 1,
                    disconnected_since: Some(now),
                    created: now,
                    messaged: false,
                }),
            }),
        );
    }
    let mut response = Json(json!({ "resumed": false, "token": token })).into_response();
    if session_cookie_enabled() {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, set_cookie_header(&token, idle_timeout().as_secs()));
    }
    response
}

/// POST /web/message: forwards a visitor message to the respondents' Matrix room.
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MessageBody>,
) -> Response {
    if !origin_allowed(&headers) {
        return error_response(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    let Some((session_hash, session)) = lookup_session(&headers) else {
        return error_response(StatusCode::UNAUTHORIZED, "no_session");
    };
    let text = body.text.trim();
    if text.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "empty");
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "too_long");
    }
    {
        let mut st = session.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        while st.recent_messages.front().is_some_and(|t| now - *t > RATE_WINDOW) {
            st.recent_messages.pop_front();
        }
        if st.recent_messages.len() >= RATE_MESSAGES {
            return error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
        }
        st.recent_messages.push_back(now);
        st.last_activity = now;
    }

    // Create the room on the first message (nobody is bothered for visitors who never write)
    let mut room_guard = session.room.lock().await;
    let mut prefix = "";
    if room_guard.is_none() {
        let db = DbManager::new();
        let invites = match db.get_call_center_respondant(session.call_center_id, true) {
            Ok(Some(r)) if !r.is_empty() => r,
            _ => return no_respondent_response(session.call_center_id),
        };
        // At most WEB_MAX_NEW_ROOMS_PER_MIN new rooms a minute (so respondents are never flooded with pings)
        if !crate::web_abuse::allow_new_room(chrono::Utc::now().timestamp() as u64, crate::web_abuse::max_new_rooms_per_min()) {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "busy");
        }
        // Same network address, same name: respondents recognise a visitor who comes back (like an SMS number)
        let room_name = format!("Web {}", names::hash_to_line(&session.name_hash));
        let room_id = match rooms::create_room(room_name, state.client.clone(), invites).await {
            Ok(id) => id,
            Err(err) => {
                error!("Web chat: could not create room: {err}");
                return error_response(StatusCode::BAD_GATEWAY, "room_creation_failed");
            }
        };
        if let Err(err) = db.add_room_id_to_did(
            &room_id,
            &session.call_center_id,
            &session.pseudo_did,
            &session.call_center_did,
        ) {
            error!("Web chat: could not record room: {err}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "db");
        }
        REGISTRY.lock().unwrap_or_else(PoisonError::into_inner).by_room.insert(room_id.clone(), session_hash.clone());
        *room_guard = Some(room_id);
        prefix = help::WEB_ROOM_HINT;
    }
    let room_id = room_guard.clone().expect("room set above");
    drop(room_guard);

    let outgoing = format!("{prefix}{text}");
    match rooms::send_message_to_room_using_id(state.client.clone(), &outgoing, &room_id).await {
        Ok(()) => {
            session.state.lock().unwrap_or_else(PoisonError::into_inner).messaged = true;
            session.record(true, text);
            // Respondents owe an answer; a brand new room also gets the "new chat" ping
            crate::attention::inbound(&room_id);
            if !prefix.is_empty() {
                tokio::spawn(crate::attention::announce_new_room(
                    state.client.clone(),
                    room_id.clone(),
                    crate::attention::Kind::Web,
                ));
                // The "newcomerweb" macro answers a new visitor straight away
                let call_center_id = session.call_center_id;
                if let Some(welcome) = tokio::task::spawn_blocking(move || crate::macros::get_newcomer_web_macro_text(call_center_id))
                    .await
                    .ok()
                    .flatten()
                {
                    deliver_to_room(&room_id, welcome.clone());
                    let notice = format!("Macro newcomerweb envoyé automatiquement. / Sent automatically.\n{welcome}");
                    if let Err(err) = rooms::send_message_to_room_using_id(state.client.clone(), &notice, &room_id).await {
                        warn!("Web chat: newcomerweb notice not sent: {err}");
                    }
                }
            }
            Json(json!({})).into_response()
        }
        Err(err) => {
            warn!("Web chat: message not delivered to room: {err}");
            error_response(StatusCode::BAD_GATEWAY, "delivery_failed")
        }
    }
}

/// GET /web/stream: server-sent events with the respondents' messages.
pub async fn stream(headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return error_response(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    let Some((_, session)) = lookup_session(&headers) else {
        return error_response(StatusCode::UNAUTHORIZED, "no_session");
    };
    if session.tx.receiver_count() >= MAX_STREAMS_PER_SESSION {
        return error_response(StatusCode::TOO_MANY_REQUESTS, "too_many_streams");
    }
    session.touch();
    info!("Web chat: stream opened (resume after {:?})", headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    // Subscribe first, then drain what arrived while the visitor was disconnected
    let rx = session.tx.subscribe();
    session.state.lock().unwrap_or_else(PoisonError::into_inner).disconnected_since = None;
    // Dropped with the stream: records when the visitor's connection closed
    let guard = StreamGuard(session.clone());
    // Resume after the line the browser already shows (Last-Event-ID); without it, send only what was never delivered
    let after: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let replay: Vec<WebEvent> = {
        let mut st = session.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.history
            .iter_mut()
            .filter(|i| !i.mine && after.map_or(!i.delivered, |a| i.seq > a))
            .map(|i| {
                i.delivered = true;
                WebEvent::Message { seq: i.seq, text: i.text.clone() }
            })
            .collect()
    };
    let to_event = |ev: WebEvent| -> Event {
        match ev {
            WebEvent::Message { seq, text } => Event::default()
                .event("message")
                .id(seq.to_string())
                .data(json!({ "text": text }).to_string()),
            WebEvent::Banned(data) => Event::default().event("banned").data(data),
            WebEvent::Closed => Event::default().event("closed").data("{}"),
        }
    };
    let initial = tokio_stream::iter(replay);
    let live = BroadcastStream::new(rx).filter_map(|r| r.ok());
    let events = initial.chain(live).map(move |ev| {
        let _keep = &guard;
        Ok::<Event, Infallible>(to_event(ev))
    });
    let mut response = Sse::new(events).keep_alive(KeepAlive::default()).into_response();
    // Tells nginx not to buffer this response even if the proxy block forgot `proxy_buffering off`
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// GET /web/history: the conversation so far, to restore the window after a reload or a page change.
pub async fn history(headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return error_response(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    let Some((_, session)) = lookup_session(&headers) else {
        return error_response(StatusCode::UNAUTHORIZED, "no_session");
    };
    session.touch();
    let (messages, last) = {
        let st = session.state.lock().unwrap_or_else(PoisonError::into_inner);
        let messages: Vec<serde_json::Value> = st
            .history
            .iter()
            .map(|i| json!({ "seq": i.seq, "from": if i.mine { "me" } else { "them" }, "text": i.text }))
            .collect();
        (messages, st.next_seq - 1)
    };
    Json(json!({ "messages": messages, "last": last })).into_response()
}

struct StreamGuard(Arc<WebSession>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).disconnected_since = Some(Instant::now());
    }
}

/// POST /web/end: the visitor ends the chat; everything is deleted. The token comes from the
/// `Authorization` header, or from a JSON body `{"token":"…"}` (used by `navigator.sendBeacon`
/// when the page is closed).
pub async fn end(State(state): State<Arc<AppState>>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !origin_allowed(&headers) {
        return error_response(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    let body_token = token_from_body(&body);
    let found = session_token(&headers, body_token.as_deref()).and_then(|t| lookup_by_token(&t));
    if let Some((hash, session)) = found {
        // Not tied to this request: removing members and the room must finish even if the browser goes away
        let client = state.client.clone();
        tokio::spawn(async move {
            end_session(&client, &hash, &session, "Le visiteur a mis fin au clavardage. / The visitor ended the chat.")
                .await;
        });
    }
    let mut response = Json(json!({})).into_response();
    if session_cookie_enabled() {
        response.headers_mut().insert(header::SET_COOKIE, set_cookie_header("", 0));
    }
    response
}

// ========================================
// SESSION LIFECYCLE
// ========================================

/// Removes the session, tells the visitor and the room, then leaves the room and deletes its row.
async fn end_session(client: &Client, hash: &str, session: &Arc<WebSession>, room_notice: &str) {
    {
        let mut reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        reg.by_hash.remove(hash);
        reg.by_room.retain(|_, h| h != hash);
    }
    let _ = session.tx.send(WebEvent::Closed);
    let room_id = session.room.lock().await.take();
    if let Some(room_id) = room_id {
        if let Err(err) = rooms::send_message_to_room_using_id(client.clone(), room_notice, &room_id).await {
            warn!("Web chat: closing notice not sent: {err}");
        }
        forget_room(client, &room_id).await;
    }
}

/// Makes the room disappear for everyone: removes every other member (joined or merely invited), then the
/// bridge leaves and forgets it, and, when it has Synapse admin access, has Synapse delete the room and
/// purge its history (see room_purge).
pub(crate) async fn forget_room(client: &Client, room_id: &str) {
    crate::attention::forget(room_id);
    if let Ok(parsed) = <&matrix_sdk::ruma::RoomId>::try_from(room_id) {
        if let Some(room) = client.get_room(parsed) {
            let own = client.user_id().map(|u| u.to_owned());
            match room.members(matrix_sdk::RoomMemberships::ACTIVE).await {
                Ok(members) => {
                    for member in members {
                        if Some(member.user_id().to_owned()) == own {
                            continue;
                        }
                        let kick = room.kick_user(
                            member.user_id(),
                            Some("Clavardage terminé et supprimé. / Chat ended and deleted."),
                        );
                        match tokio::time::timeout(Duration::from_secs(10), kick).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(err)) => warn!("Web chat: could not remove a member from the room: {err}"),
                            Err(_) => warn!("Web chat: removing a member timed out"),
                        }
                    }
                }
                Err(err) => warn!("Web chat: could not list room members: {err}"),
            }
            if let Err(err) = room.leave().await {
                warn!("Web chat: could not leave room: {err}");
            }
            if let Err(err) = room.forget().await {
                warn!("Web chat: could not forget room: {err}");
            }
        }
    }
    if let Err(err) = DbManager::new().delete_room(room_id) {
        error!("Web chat: could not delete room row: {err}");
    }
    // In its own task: it can take a minute, and must not stop half way if the caller (an HTTP request) is cancelled
    let client = client.clone();
    let room_id = room_id.to_string();
    tokio::spawn(async move {
        // A restart during this delay leaves the room unpurged (it stays left and forgotten)
        tokio::time::sleep(crate::room_purge::purge_delay()).await;
        if let Err(err) = crate::room_purge::purge_room(&client, &room_id).await {
            error!("Room purge failed, the room's history stays on Synapse: {err}");
        }
    });
}

/// Called from the Matrix callback: a respondent's text for the visitor of this room.
/// Returns false if no web session owns the room.
pub fn deliver_to_room(room_id: &str, text: String) -> bool {
    let session = {
        let reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        reg.by_room.get(room_id).and_then(|h| reg.by_hash.get(h)).cloned()
    };
    let Some(session) = session else {
        return false;
    };
    session.touch();
    // Keep the line (for reloads), then push it live if a stream is connected
    let mut st = session.state.lock().unwrap_or_else(PoisonError::into_inner);
    let seq = WebSession::record_locked(&mut st, false, &text, false);
    info!("Web chat: reply #{seq} for {} live stream(s)", session.tx.receiver_count());
    if session.tx.receiver_count() > 0 && session.tx.send(WebEvent::Message { seq, text }).is_ok() {
        if let Some(item) = st.history.back_mut() {
            item.delivered = true;
        }
    }
    true
}

/// Called from the Matrix callback when a respondent types `!fin` / `!end` in a web room.
pub async fn end_session_for_room(client: &Client, room_id: &str) -> bool {
    let found = {
        let reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        reg.by_room
            .get(room_id)
            .and_then(|h| reg.by_hash.get(h).map(|s| (h.clone(), s.clone())))
    };
    let Some((hash, session)) = found else {
        return false;
    };
    end_session(client, &hash, &session, "Clavardage terminé. / Chat ended.").await;
    true
}

/// A respondent typed `!ban [days]` in a web room: block the visitor's connection and end every open
/// chat from it. Returns false if no web session owns the room.
pub async fn ban_for_room(client: &Client, room_id: &str, asked_days: Option<u64>) -> bool {
    let found = {
        let reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        reg.by_room
            .get(room_id)
            .and_then(|h| reg.by_hash.get(h).map(|s| s.ip_hash.clone()))
    };
    let Some(ip_hash) = found else {
        return false;
    };
    let days = clamp_ban_days(asked_days, ban_default_days(), ban_max_days());
    let until = chrono::Utc::now().timestamp() + (days as i64) * 86_400;
    if let Err(err) = DbManager::new().add_web_block(&ip_hash, until) {
        error!("Web chat: could not record the block: {err}");
    }
    block_cache_add(&ip_hash);
    // End every open chat of that connection (a banned visitor may have several tabs open)
    let sessions: Vec<(String, Arc<WebSession>)> = {
        let reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        reg.by_hash
            .iter()
            .filter(|(_, s)| s.ip_hash == ip_hash)
            .map(|(h, s)| (h.clone(), s.clone()))
            .collect()
    };
    info!("Web chat: a respondent blocked a connection for {days} day(s) ({} chat(s) ended)", sessions.len());
    // The banned browser keeps this marker, so changing network does not help it start a new chat
    let now_ts = chrono::Utc::now().timestamp();
    let banned = json!({ "until": until, "marker": crate::web_abuse::make_marker(now_ts, until) }).to_string();
    for (_, session) in &sessions {
        let _ = session.tx.send(WebEvent::Banned(banned.clone()));
    }
    let (fr, en) = if days == 1 { ("1 jour".to_string(), "1 day".to_string()) } else { (format!("{days} jours"), format!("{days} days")) };
    let notice = format!("Visiteur bloqué pour {fr}; clavardage terminé. / Visitor blocked for {en}; chat ended.");
    for (hash, session) in sessions {
        end_session(client, &hash, &session, &notice).await;
    }
    true
}

/// Admin: lifts the block of one IP address, or of every address ("all", which also revokes the block
/// markers kept in browsers). Returns how many address blocks were lifted.
pub fn unban(ip: &str) -> anyhow::Result<usize> {
    let db = DbManager::new();
    let result = if ip.trim().eq_ignore_ascii_case("all") {
        // also revokes every block marker kept in browsers
        db.set_web_state("markers_valid_after", chrono::Utc::now().timestamp())?;
        db.clear_web_blocks()?
    } else {
        let parsed: IpAddr = ip.trim().parse()?;
        usize::from(db.remove_web_block(&hash_ip(&ip_key(parsed)))?)
    };
    block_cache_reset();
    Ok(result)
}

/// At startup: web sessions are memory only, so any web room left in the database is an orphan.
pub async fn purge_orphan_rooms(client: &Client) {
    match DbManager::new().fetch_room_ids_by_channel("web") {
        Ok(ids) => {
            for id in ids {
                if REGISTRY.lock().unwrap_or_else(PoisonError::into_inner).by_room.contains_key(&id) {
                    continue; // a live session already owns it
                }
                info!("Web chat: purging orphan room");
                forget_room(client, &id).await;
            }
        }
        Err(err) => error!("Web chat: could not list orphan rooms: {err}"),
    }
}

/// Ends sessions idle for longer than `WEB_IDLE_TIMEOUT_MIN`. Runs forever.
pub async fn run_cleanup(client: Client) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if let Err(err) = DbManager::new().purge_expired_web_blocks(chrono::Utc::now().timestamp()) {
            error!("Web chat: could not purge expired blocks: {err}");
        }
        let timeout = idle_timeout();
        let grace = abandon_after();
        let attack = crate::web_abuse::under_attack(chrono::Utc::now().timestamp() as u64);
        let silent_after = Duration::from_secs(crate::web_abuse::no_message_timeout_secs(attack));
        let expired: Vec<(String, Arc<WebSession>, EndReason)> = {
            let mut reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            reg.sessions_per_ip.retain(|_, v| {
                while v.front().is_some_and(|t| now - *t > Duration::from_secs(3600)) {
                    v.pop_front();
                }
                !v.is_empty()
            });
            reg.by_hash
                .iter()
                .filter_map(|(h, s)| {
                    let st = s.state.lock().unwrap_or_else(PoisonError::into_inner);
                    let reason = if now - st.last_activity > timeout {
                        EndReason::Idle
                    } else if !st.messaged && is_abandoned(st.disconnected_since, s.tx.receiver_count(), now, grace) {
                        EndReason::Left
                    } else if !st.messaged && now.saturating_duration_since(st.created) > silent_after {
                        EndReason::Silent
                    } else {
                        return None;
                    };
                    Some((h.clone(), s.clone(), reason))
                })
                .collect()
        };
        for (hash, session, reason) in expired {
            let notice = match reason {
                EndReason::Idle => "Clavardage fermé pour inactivité. / Chat closed for inactivity.",
                EndReason::Left => "Le visiteur a quitté (connexion perdue). / The visitor left (connection lost).",
                EndReason::Silent => "Clavardage fermé : aucun message envoyé. / Chat closed: no message was sent.",
            };
            info!("Web chat: session ended ({reason:?})");
            end_session(&client, &hash, &session, notice).await;
        }
    }
}

#[derive(Debug)]
enum EndReason {
    Idle,
    Left,
    Silent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_reply_reaches_the_new_stream_after_a_page_change() {
        unsafe { std::env::set_var("WEB_ALLOWED_ORIGINS", "https://a.example, https://b.example/") };
        let now = Instant::now();
        let (tx, _rx) = broadcast::channel(32);
        let session = Arc::new(WebSession {
            call_center_id: 1,
            call_center_did: 1,
            pseudo_did: WEB_DID_MIN + 7,
            ip_hash: String::new(),
            name_hash: String::new(),
            room: tokio::sync::Mutex::new(Some("!room:test".into())),
            tx,
            state: Mutex::new(SessionState {
                last_activity: now,
                recent_messages: VecDeque::new(),
                history: VecDeque::new(),
                next_seq: 1,
                disconnected_since: None,
                created: now,
                messaged: true,
            }),
        });
        session.record(true, "Allo");
        let hash = hash_token("tok-page-change");
        {
            let mut reg = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
            reg.by_hash.insert(hash.clone(), session.clone());
            reg.by_room.insert("!room:test".into(), hash);
        }
        let h = headers(&[("origin", "https://a.example"), ("authorization", "Bearer tok-page-change")]);
        let first = stream(h.clone()).await; // page 1
        drop(first); // the visitor changes page
        let mut h2 = h.clone();
        h2.insert("last-event-id", HeaderValue::from_static("1"));
        let second = stream(h2).await; // page 2
        assert!(deliver_to_room("!room:test", "Bonjour".into()));
        let mut body = second.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(3), futures_util::StreamExt::next(&mut body))
            .await
            .expect("the new stream got nothing")
            .expect("stream ended")
            .expect("body error");
        let text = String::from_utf8_lossy(&chunk).to_string();
        assert!(text.contains("Bonjour") && text.contains("id: 2"), "{text}");
    }

    #[test]
    fn visitor_names_follow_the_network_and_need_the_secret() {
        crate::crypto::init_test_env();
        let a: IpAddr = "203.0.113.7".parse().unwrap();
        let b: IpAddr = "203.0.113.8".parse().unwrap();
        let v6a: IpAddr = "2001:db8:1:2::10".parse().unwrap();
        let v6b: IpAddr = "2001:db8:1:2:ffff::99".parse().unwrap(); // same /64 network
        assert_eq!(visitor_name_hash(a), visitor_name_hash(a), "same address, same name");
        assert_ne!(visitor_name_hash(a), visitor_name_hash(b));
        assert_eq!(visitor_name_hash(v6a), visitor_name_hash(v6b), "an IPv6 visitor keeps the name inside their /64");
        // not the block-list value, and not a plain hash of the address (the server secret is mixed in)
        assert_ne!(visitor_name_hash(a), hash_ip(&ip_key(a)));
        use sha2::{Digest, Sha256};
        assert_ne!(visitor_name_hash(a), format!("{:x}", Sha256::digest(b"203.0.113.7")));
        assert_ne!(visitor_name_hash(a), format!("{:x}", Sha256::digest(b"display-name:203.0.113.7")));
    }

    #[test]
    fn history_numbers_lines_and_stays_bounded() {
        let now = Instant::now();
        let mut st = SessionState {
            last_activity: now,
            recent_messages: VecDeque::new(),
            history: VecDeque::new(),
            next_seq: 1,
            disconnected_since: None,
            created: now,
            messaged: false,
        };
        for i in 0..(MAX_HISTORY + 5) {
            let seq = WebSession::record_locked(&mut st, i % 2 == 0, "x", false);
            assert_eq!(seq, i as u64 + 1);
        }
        assert_eq!(st.history.len(), MAX_HISTORY);
        assert_eq!(st.history.front().unwrap().seq, 6);
        assert_eq!(st.next_seq, MAX_HISTORY as u64 + 6);
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn web_dids_never_collide_with_phone_numbers() {
        assert!(!is_web_did(999_999_999_999_999)); // 15 digits, the longest phone number
        assert!(is_web_did(WEB_DID_MIN));
        assert!(WEB_DID_MIN * 9 < i64::MAX as u64); // fits a SQLite integer
    }

    #[test]
    fn cookie_token_is_found_among_other_cookies() {
        let h = headers(&[("cookie", "a=1; wpchat=abc123; b=2")]);
        assert_eq!(cookie_token(&h).as_deref(), Some("abc123"));
        assert_eq!(cookie_token(&headers(&[("cookie", "a=1")])), None);
        assert_eq!(cookie_token(&HeaderMap::new()), None);
    }

    #[test]
    fn origin_is_checked_against_the_allow_list() {
        // SAFETY: tests in this module that touch env vars run in this single test
        unsafe { env::set_var("WEB_ALLOWED_ORIGINS", "https://a.example, https://b.example/") };
        assert!(origin_allowed(&headers(&[("origin", "https://a.example")])));
        assert!(origin_allowed(&headers(&[("origin", "https://b.example")])));
        assert!(!origin_allowed(&headers(&[("origin", "https://evil.example")])));
        assert!(!origin_allowed(&HeaderMap::new()));
    }

    #[test]
    fn the_session_token_comes_from_the_header_then_the_cookie_then_the_body() {
        let both = headers(&[("authorization", "Bearer from-header"), ("cookie", "wpchat=from-cookie")]);
        assert_eq!(session_token(&both, Some("from-body")).as_deref(), Some("from-header"));
        let cookie = headers(&[("cookie", "wpchat=from-cookie")]);
        assert_eq!(session_token(&cookie, Some("from-body")).as_deref(), Some("from-cookie"));
        assert_eq!(session_token(&HeaderMap::new(), Some("from-body")).as_deref(), Some("from-body"));
        assert_eq!(session_token(&HeaderMap::new(), None), None);
        // not a bearer scheme, empty and oversized tokens are ignored
        assert_eq!(session_token(&headers(&[("authorization", "Basic abc")]), None), None);
        assert_eq!(session_token(&headers(&[("authorization", "Bearer ")]), None), None);
        assert_eq!(session_token(&HeaderMap::new(), Some(&"x".repeat(129))), None);
    }

    #[test]
    fn a_beacon_body_carries_the_token() {
        assert_eq!(token_from_body(br#"{"token":"abc"}"#).as_deref(), Some("abc"));
        assert_eq!(token_from_body(b""), None);
        assert_eq!(token_from_body(b"not json"), None);
        assert_eq!(token_from_body(br#"{"token":5}"#), None);
    }

    #[test]
    fn a_block_applies_to_the_ipv4_address_or_the_ipv6_network() {
        let a: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(ip_key(a), "203.0.113.9");
        let x: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let y: IpAddr = "2001:db8:1:2:1:2:3:4".parse().unwrap(); // same /64, other suffix
        let z: IpAddr = "2001:db8:1:3::1".parse().unwrap(); // other /64
        assert_eq!(ip_key(x), ip_key(y));
        assert_ne!(ip_key(x), ip_key(z));
    }

    #[test]
    fn the_ban_command_is_parsed_and_bounded() {
        assert_eq!(parse_ban_command("!ban"), Some(None));
        assert_eq!(parse_ban_command("  !BAN 7 "), Some(Some(7)));
        assert_eq!(parse_ban_command("!bloquer 3j"), Some(Some(3)));
        assert_eq!(parse_ban_command("!ban 2d"), Some(Some(2)));
        assert_eq!(parse_ban_command("!bloquer abc"), Some(None)); // unreadable duration: default
        assert_eq!(parse_ban_command("!ok"), None);
        assert_eq!(parse_ban_command("ban"), None);
        assert_eq!(parse_ban_command(""), None);
        assert_eq!(clamp_ban_days(None, 1, 30), 1);
        assert_eq!(clamp_ban_days(Some(0), 1, 30), 1);
        assert_eq!(clamp_ban_days(Some(7), 1, 30), 7);
        assert_eq!(clamp_ban_days(Some(100000), 1, 30), 30);
    }

    #[test]
    fn a_visitor_is_gone_when_the_stream_stays_closed() {
        let now = Instant::now();
        let grace = Duration::from_secs(300);
        let long_ago = now.checked_sub(Duration::from_secs(600));
        assert!(is_abandoned(long_ago, 0, now, grace));
        assert!(!is_abandoned(long_ago, 1, now, grace)); // a stream is connected again
        assert!(!is_abandoned(None, 0, now, grace)); // connected
        assert!(!is_abandoned(Some(now), 0, now, grace)); // just disconnected
    }

    #[test]
    fn cookie_is_secure_and_http_only() {
        let c = set_cookie_header("tok", 60);
        let c = c.to_str().unwrap();
        assert!(c.contains("HttpOnly") && c.contains("Secure") && c.contains("SameSite=None"));
    }

    #[test]
    fn forwarded_for_uses_the_proxy_appended_entry() {
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9")]);
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(client_ip(&h, addr, true, &[]), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(client_ip(&HeaderMap::new(), addr, true, &[]), addr.ip());
    }

    #[test]
    fn forwarded_for_is_ignored_on_the_direct_listener_and_from_untrusted_peers() {
        let h = headers(&[("x-forwarded-for", "203.0.113.9")]);
        let peer: SocketAddr = "198.51.100.7:1".parse().unwrap();
        // HTTPS listener (reachable directly): the header is the attacker's word
        assert_eq!(client_ip(&h, peer, false, &[]), peer.ip());
        // proxy listener with a trusted list: only the listed proxy is believed
        let trusted = parse_proxy_list("172.18.0.0/16, ::1");
        assert_eq!(client_ip(&h, peer, true, &trusted), peer.ip());
        let proxy: SocketAddr = "172.18.0.5:1".parse().unwrap();
        assert_eq!(client_ip(&h, proxy, true, &trusted), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(parse_proxy_list("10.0.0.1, junk, ").len(), 1);
    }
}
