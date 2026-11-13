use crate::api;
use crate::api::ReturnQuery;
use crate::crypto::hash_token;
use crate::db_manager::DbManager;
use anyhow::anyhow;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::routing::get;
use axum::routing::post;
use axum::{
    Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::Json,
};
use chrono::NaiveDateTime;
use matrix_sdk::Client;
//use matrix_sdk::crypto::Session;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::web_chat;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::{env, sync::Arc};
use tokio::sync::mpsc;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
// ========================================
// DATA STRUCTURES
// ========================================

/// Response struct for API endpoints
#[derive(Serialize)]
struct ApiResponse {
    message: String,
    received_params: QueryParams,
    path_param: Option<String>,
}

/// Main web server struct containing the message transmitter
pub struct WebServer {
    transmitter: mpsc::Sender<QueryParams>,
    client: Client,
}

/// Application state shared across handlers
#[derive(Clone)]
pub(crate) struct AppState {
    tx: mpsc::Sender<QueryParams>,
    pub(crate) client: Client,
}

/// Struct for handling query parameters from VoIP.ms webhooks
#[derive(Deserialize, Serialize, Clone)]
pub struct QueryParams {
    pub to: Option<String>,
    pub from: Option<String>,
    pub message: Option<String>,
    pub id: Option<String>,
    pub timestamp: Option<String>,
    pub sbchatapikey: Option<String>,
    pub is_reprocessing_error: Option<bool>,
}

impl WebServer {
    // ========================================
    // INITIALIZATION METHODS
    // ========================================

    /// Creates a new WebServer instance with the provided message transmitter
    pub fn new(transmitter: mpsc::Sender<QueryParams>, client: Client) -> WebServer {
        Self {
            transmitter,
            client,
        }
    }

    // ========================================
    // SERVER MANAGEMENT METHODS
    // ========================================

    /// Binds and starts the HTTPS server with TLS configuration
    pub async fn bind_server(&self) -> Result<()> {
        // Prepare transmitter to be shipped in router state
        let state = Arc::new(AppState {
            tx: self.transmitter.clone(),
            client: self.client.clone(),
        });

        // Build the application router
        // Web chat routes (wp-chat widget). CORS applies to these routes only.
        let web_routes: Router<Arc<AppState>> = Router::new()
            .route("/web/session", post(web_chat::create_session))
            .route("/web/message", post(web_chat::send_message))
            .route("/web/stream", get(web_chat::stream))
            .route("/web/history", get(web_chat::history))
            .route("/web/end", post(web_chat::end))
            // the WP Chat plugin links its website with a pairing code (server to server: no CORS needed)
            .route("/web/pair", post(crate::web_sites::pair))
            .layer(DefaultBodyLimit::max(8 * 1024)) // a message is at most 1000 characters: refuse anything bigger early
            .layer(web_chat::cors_layer())
            .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, REQUEST_TIMEOUT)); // the SSE body is not limited, only the time to answer
        let api_routes: Router<Arc<AppState>> = Router::new()
            .route("/sbchat_api", post(handle_sbchat_api_query_params))
            .layer(DefaultBodyLimit::max(SBCHAT_API_BODY_MAX))
            .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, API_REQUEST_TIMEOUT));
        let end_points: Router = Router::new()
            .route("/voip_api", get(handle_voip_api_query_params))
            .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, REQUEST_TIMEOUT))
            .merge(api_routes)
            .merge(web_routes)
            .with_state(state)
            .layer(ConcurrencyLimitLayer::new(CONCURRENT_REQUESTS_MAX))
            .layer(TraceLayer::new_for_http());

        let current_dir: String = env::var("APP_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
        info!("Current dir : {:?}", current_dir);

        let cert_path: PathBuf = (current_dir.clone() + "/web_server_keys/cert.pem").into();
        let key_path: PathBuf = (current_dir + "/web_server_keys/key.pem").into();
        info!("cert_path,key_path: {:?},{:?}", cert_path, key_path);

        // Using axum-server for easier TLS integration
        let config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path).await?;

        self.suggest_endpoint();

        // The listener is recorded on each request: X-Forwarded-For is only believed on the proxy one
        let mut tls = axum_server::bind_rustls("0.0.0.0:3443".parse()?, config);
        limit_header_time(tls.http_builder());
        let tls = tls.serve(
            end_points
                .clone()
                .layer(axum::Extension(web_chat::ViaProxy(false)))
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        // Only for reverse proxy. Do not expose !
        let http_bind = env::var("HTTP_BIND").unwrap_or_else(|_| "0.0.0.0:8088".to_string());
        let mut http = axum_server::bind(http_bind.parse()?);
        limit_header_time(http.http_builder());
        let http = http.serve(
            end_points
                .layer(axum::Extension(web_chat::ViaProxy(true)))
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        let (resulttls, resulthttp) = tokio::join!(tls, http);
        resulttls.expect("Error running tls server");
        resulthttp.expect("Error running http server");

        Ok(())
    }

    // ========================================
    // UTILITY AND HELPER METHODS
    // ========================================

    /// Announces the listening ports. It used to print one ready-to-use /voip_api URL per call
    /// center, which put each call center's inbound key in the logs.
    pub fn suggest_endpoint(&self) {
        println!("HTTPS server listening on https://localhost:3443 (plain HTTP on 8088 for a reverse proxy)");
    }
}

/// Drop connections that do not send their request headers in time (HTTP/1)
fn limit_header_time(builder: &mut hyper_util::server::conn::auto::Builder<hyper_util::rt::TokioExecutor>) {
    builder
        .http1()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
}

/// Numbers, text and key never reach a log through `{:?}`
impl std::fmt::Debug for QueryParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |v: &Option<String>| if v.is_some() { "<redacted>" } else { "None" };
        f.debug_struct("QueryParams")
            .field("to", &redacted(&self.to))
            .field("from", &redacted(&self.from))
            .field("message", &redacted(&self.message))
            .field("id", &self.id)
            .field("timestamp", &self.timestamp)
            .field("sbchatapikey", &redacted(&self.sbchatapikey))
            .field("is_reprocessing_error", &self.is_reprocessing_error)
            .finish()
    }
}

impl QueryParams {
    // ========================================
    // QUERY PARAMETER UTILITY METHODS
    // ========================================

    /// Parses and returns the timestamp as a NaiveDateTime
    pub fn get_timestamp(&self) -> anyhow::Result<NaiveDateTime> {
        let Some(atimestep) = self.timestamp.clone() else {
            error!("Struct countain a None timestamp");
            return Err(anyhow!("Struct countain a None timestamp"));
        };

        let timestamp = NaiveDateTime::parse_from_str(&atimestep, "%Y-%m-%d %H:%M:%S")?;
        return Ok(timestamp);
    }
}

// ========================================
// REQUEST HANDLERS
// ========================================

/// Wrong keys allowed per address and window before the address is refused outright
const KEY_FAILURES_MAX: u32 = 20;
const KEY_FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);

/// Counts wrong keys (webhook) or tokens (admin API) per client address, so a guessing or flooding
/// client is refused before it costs a database lookup.
struct FailureLimiter {
    what: &'static str,
    map: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, (u32, std::time::Instant)>>,
}

impl FailureLimiter {
    fn new(what: &'static str) -> Self {
        Self { what, map: std::sync::Mutex::new(std::collections::HashMap::new()) }
    }

    /// True when this address has sent too many wrong keys lately
    fn blocked(&self, ip: std::net::IpAddr, now: std::time::Instant) -> bool {
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&ip)
            .is_some_and(|(count, since)| *count >= KEY_FAILURES_MAX && now.duration_since(*since) < KEY_FAILURE_WINDOW)
    }

    fn note_failure(&self, ip: std::net::IpAddr, now: std::time::Instant) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 10_000 {
            map.retain(|_, (_, since)| now.duration_since(*since) < KEY_FAILURE_WINDOW);
        }
        let entry = map.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1) >= KEY_FAILURE_WINDOW {
            *entry = (0, now);
        }
        entry.0 += 1;
        if entry.0 == KEY_FAILURES_MAX {
            tracing::warn!("{}: too many wrong keys from one address, refusing it for a while", self.what);
        }
    }
}

static VOIP_KEY_FAILURES: std::sync::LazyLock<FailureLimiter> =
    std::sync::LazyLock::new(|| FailureLimiter::new("SMS webhook"));
static API_TOKEN_FAILURES: std::sync::LazyLock<FailureLimiter> =
    std::sync::LazyLock::new(|| FailureLimiter::new("Admin API"));

/// Largest admin API request accepted (a respondent list or a broadcast fits easily)
const SBCHAT_API_BODY_MAX: usize = 64 * 1024;
/// Handlers answering a browser or voip.ms must answer within this time
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Admin API commands can invite or message many users
const API_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// Requests handled at the same time per listener; the rest wait their turn
const CONCURRENT_REQUESTS_MAX: usize = 256;
/// A client must send its request headers within this time (slow-loris)
const HEADER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Handler for processing VoIP.ms webhook query parameters
/// Processes incoming SMS/MMS messages and forwards them to the message queue
async fn handle_voip_api_query_params(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Query(params): Query<QueryParams>,
) -> (StatusCode, &'static str) {
    // VoIP.ms webhook format:
    // If selected, SMS/MMS Messages received by your DID will send a GET request to the URL callback provided.
    // Available Variables for your URL:
    // {ID} The ID of the SMS/MMS message
    // {TIMESTAMP} The date and time the message was received
    // {FROM} The phone number that sent you the message
    // {TO} The DID number that received the message
    // {MESSAGE} The content of the message
    // {MEDIA} Comma-separated list of media files
    // Example: https://mysite.com/sms.php?to={TO}&from={FROM}&message={MESSAGE}&files={MEDIA}&id={ID}&date={TIMESTAMP}

    // The key is checked here, before queueing: requests with a wrong key must not be able to fill
    // the queue and delay real texts (voip.ms does not retry).
    let now = std::time::Instant::now();
    let ip = addr.ip();
    if VOIP_KEY_FAILURES.blocked(ip, now) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    let Some(key) = params.sbchatapikey.clone().filter(|k| !k.is_empty() && k.len() <= 128) else {
        VOIP_KEY_FAILURES.note_failure(ip, now);
        return (StatusCode::FORBIDDEN, "forbidden");
    };
    let known = tokio::task::spawn_blocking(move || DbManager::new().get_call_center_id_by_hashed_token(key)).await;
    match known {
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) => {
            VOIP_KEY_FAILURES.note_failure(ip, now);
            return (StatusCode::FORBIDDEN, "forbidden");
        }
        Ok(Err(err)) => {
            error!("SMS webhook: database error while checking the key: {err}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "error");
        }
        Err(err) => {
            error!("SMS webhook: key check task failed: {err}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "error");
        }
    }

    // Wait for room in the queue (a burst of real texts), but not forever
    match tokio::time::timeout(std::time::Duration::from_secs(20), state.tx.send(params)).await {
        Ok(Ok(())) => (StatusCode::OK, "ok"),
        Ok(Err(_)) => {
            error!("SMS webhook: the SMS listener is gone, text not queued");
            (StatusCode::INTERNAL_SERVER_ERROR, "error")
        }
        Err(_) => {
            error!("SMS webhook: queue full for 20 s, text not queued");
            (StatusCode::SERVICE_UNAVAILABLE, "busy")
        }
    }
}

/// Admin API (SwitchBoard). The bearer token is checked before the body is parsed, so an
/// unauthenticated client cannot make the server parse large JSON documents.
async fn handle_sbchat_api_query_params(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<ReturnQuery>, StatusCode> {
    let now = std::time::Instant::now();
    let ip = addr.ip();
    if API_TOKEN_FAILURES.blocked(ip, now) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let Some(token) = bearer_token(&headers) else {
        API_TOKEN_FAILURES.note_failure(ip, now);
        return Err(StatusCode::UNAUTHORIZED);
    };
    let lookup = token.clone();
    let found = tokio::task::spawn_blocking(move || DbManager::new().get_call_center_id_by_token(lookup))
        .await
        .map_err(|err| {
            error!("Admin API: token check task failed: {err}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|err| {
            error!("Admin API: database error while checking the token: {err}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let Some((call_center_id, privilege_level)) = found else {
        API_TOKEN_FAILURES.note_failure(ip, now);
        return Err(StatusCode::UNAUTHORIZED);
    };
    let payload: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = api::execute_command(
        state.client.clone(),
        call_center_id,
        privilege_level,
        &token,
        payload,
    )
    .await;
    Ok(result)
}

/// The token of an `Authorization: Bearer <token>` header, if it has a plausible length
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    (!token.is_empty() && token.len() <= 512).then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_sending_wrong_webhook_keys_is_refused_for_a_while() {
        let ip: std::net::IpAddr = "198.51.100.200".parse().unwrap();
        let other: std::net::IpAddr = "198.51.100.201".parse().unwrap();
        let t0 = std::time::Instant::now();
        let limiter = FailureLimiter::new("test");
        for _ in 0..KEY_FAILURES_MAX - 1 {
            limiter.note_failure(ip, t0);
        }
        assert!(!limiter.blocked(ip, t0));
        limiter.note_failure(ip, t0);
        assert!(limiter.blocked(ip, t0));
        assert!(!limiter.blocked(other, t0));
        assert!(!limiter.blocked(ip, t0 + KEY_FAILURE_WINDOW));
    }

    #[test]
    fn bearer_token_is_taken_only_from_a_well_formed_header() {
        let mut h = HeaderMap::new();
        assert_eq!(bearer_token(&h), None);
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(bearer_token(&h), None);
        h.insert("authorization", "Bearer ".parse().unwrap());
        assert_eq!(bearer_token(&h), None);
        h.insert("authorization", "Bearer abc123".parse().unwrap());
        assert_eq!(bearer_token(&h).as_deref(), Some("abc123"));
        h.insert("authorization", format!("Bearer {}", "a".repeat(600)).parse().unwrap());
        assert_eq!(bearer_token(&h), None);
    }
}
