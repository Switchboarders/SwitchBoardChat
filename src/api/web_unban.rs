use crate::api::{QueryStatus, ReturnQuery};
use crate::web_chat;
use axum::Json;

/// Lifts a web chat block. Payload: {"command":"web_unban","ip":"203.0.113.9"} or {"ip":"all"}.
/// Needs privilege level 100 or more: blocks are server-wide, not per call center.
pub async fn web_unban(privilege_level: u64, payload: serde_json::Value) -> Json<ReturnQuery> {
    if privilege_level < 100 {
        return super::return_query(
            QueryStatus::Error,
            "Privilege level is too low to lift a web chat block.".to_string(),
            "".to_string(),
        );
    }
    let Some(ip) = payload.get("ip").and_then(|v| v.as_str()) else {
        return super::return_query(
            QueryStatus::Error,
            "ip is missing: give an IP address, or \"all\".".to_string(),
            "".to_string(),
        );
    };
    let all = ip.trim().eq_ignore_ascii_case("all");
    match web_chat::unban(ip) {
        Ok(n) if all => super::return_query(
            QueryStatus::Success,
            format!("{n} block(s) lifted; every block marker kept in a browser before now is revoked too."),
            "".to_string(),
        ),
        Ok(0) => super::return_query(QueryStatus::Warning, "No block found for this address.".to_string(), "".to_string()),
        Ok(n) => super::return_query(QueryStatus::Success, format!("{n} block(s) lifted."), "".to_string()),
        Err(err) => super::return_query(QueryStatus::Error, format!("Could not lift the block: {err}"), "".to_string()),
    }
}
