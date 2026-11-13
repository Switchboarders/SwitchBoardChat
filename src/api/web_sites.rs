//! Websites of a call center (web chat pairing), for SwitchBoard. See src/web_sites.rs.
//!
//!   {"command": "create_web_pairing", "api_token": "<call center token>"}
//!       -> message: {"code": "K7QM-3XRA-PW2D", "expires_at": <unix seconds>, "public_url": "<WEB_PUBLIC_URL or "">"}
//!   {"command": "list_web_sites", "api_token": "..."}   -> message: [{"origin": "...", "linked_at": ...}]
//!   {"command": "remove_web_site", "api_token": "...", "origin": "https://www.example.org"}
//!
//! A committee acts on its own call center; an admin token (privilege 100) may act on any call center, by
//! giving that call center's token as api_token (same rule as the other commands).

use crate::api::{QueryStatus, ReturnQuery, may_act_on};
use crate::db_manager::DbManager;
use axum::Json;
use serde_json::json;
use tracing::warn;

/// The call center the command is about (the one of payload.api_token), if the requester may act on it
fn target(privilege_level: u64, requester_token: &str, payload: &serde_json::Value) -> Result<u64, Json<ReturnQuery>> {
    super::does_user_have_permission(privilege_level, payload, requester_token)?;
    let token = payload["api_token"].as_str().unwrap_or_default().to_string();
    match DbManager::new().get_call_center_id_by_token(token) {
        Ok(Some((id, target_privilege))) if may_act_on(privilege_level, target_privilege) || payload["api_token"] == requester_token => Ok(id),
        Ok(Some(_)) => Err(super::return_query(QueryStatus::Error, "Insufficient privilege".into(), String::new())),
        Ok(None) => Err(super::return_query(QueryStatus::Error, "Unknown call center (api_token)".into(), String::new())),
        Err(err) => {
            warn!("web sites: database error: {err}");
            Err(super::return_query(QueryStatus::Error, "Database error".into(), String::new()))
        }
    }
}

pub async fn create_web_pairing(privilege_level: u64, requester_token: &str, payload: serde_json::Value) -> Json<ReturnQuery> {
    let id = match target(privilege_level, requester_token, &payload) {
        Ok(id) => id,
        Err(err) => return err,
    };
    match crate::web_sites::create_pairing(id) {
        Ok((code, expires_at)) => {
            // The address visitors' browsers use for the web chat (behind the reverse proxy), when it differs
            // from the API address SwitchBoard knows (often https://host:3443, with a self-signed certificate)
            let public_url = std::env::var("WEB_PUBLIC_URL").unwrap_or_default().trim().trim_end_matches('/').to_string();
            super::return_query(
                QueryStatus::Success,
                json!({ "code": code, "expires_at": expires_at, "public_url": public_url }).to_string(),
                String::new(),
            )
        }
        Err(err) => {
            warn!("web sites: could not create a pairing code: {err}");
            super::return_query(QueryStatus::Error, "Could not create a pairing code".into(), String::new())
        }
    }
}

pub async fn list_web_sites(privilege_level: u64, requester_token: &str, payload: serde_json::Value) -> Json<ReturnQuery> {
    let id = match target(privilege_level, requester_token, &payload) {
        Ok(id) => id,
        Err(err) => return err,
    };
    match crate::web_sites::list(id) {
        Ok(sites) => {
            let list: Vec<_> = sites.into_iter().map(|(origin, at)| json!({ "origin": origin, "linked_at": at })).collect();
            super::return_query(QueryStatus::Success, json!(list).to_string(), String::new())
        }
        Err(err) => {
            warn!("web sites: could not list: {err}");
            super::return_query(QueryStatus::Error, "Database error".into(), String::new())
        }
    }
}

pub async fn remove_web_site(privilege_level: u64, requester_token: &str, payload: serde_json::Value) -> Json<ReturnQuery> {
    let id = match target(privilege_level, requester_token, &payload) {
        Ok(id) => id,
        Err(err) => return err,
    };
    let Some(origin) = payload["origin"].as_str() else {
        return super::return_query(QueryStatus::Error, "origin is required".into(), String::new());
    };
    match crate::web_sites::remove(id, origin) {
        Ok(true) => super::return_query(QueryStatus::Success, "Website unlinked".into(), String::new()),
        Ok(false) => super::return_query(QueryStatus::Warning, "This website was not linked to this call center".into(), String::new()),
        Err(err) => {
            warn!("web sites: could not remove: {err}");
            super::return_query(QueryStatus::Error, "Database error".into(), String::new())
        }
    }
}
