use crate::api::{QueryStatus, ReturnQuery};
use crate::retention;
use axum::Json;
use matrix_sdk::Client;
use tracing::warn;

/// Right to erasure. Payload: {"command":"erase_caller","api_token":"...","caller_number":"4185551234"}
/// Erases the caller's conversations and records for the call center that owns the token.
pub async fn erase_caller(
    privilege_level: u64,
    requester_token: &str,
    client: Client,
    call_center_id: u64,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    if let Err(err) = super::does_user_have_permission(privilege_level, &payload, requester_token) {
        warn!("User forbidden !");
        return err;
    }
    let Some(number) = payload
        .get("caller_number")
        .and_then(|v| v.as_str())
        .map(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
        .and_then(|digits| digits.parse::<u64>().ok())
    else {
        return super::return_query(
            QueryStatus::Error,
            "caller_number is missing or not a phone number".to_string(),
            "".to_string(),
        );
    };
    match retention::erase_caller(&client, call_center_id, number).await {
        Ok(rooms) => super::return_query(
            QueryStatus::Success,
            format!("Caller erased ({rooms} conversation(s) deleted). The opt-out entry, if any, is kept."),
            "".to_string(),
        ),
        Err(err) => super::return_query(
            QueryStatus::Error,
            format!("Erasure failed: {err}"),
            "".to_string(),
        ),
    }
}
