//! SwitchBoard API for the volunteers' confidentiality agreement (src/agreement.rs).
//!
//! `list_agreement_signatures`: the agreement in force (version, fingerprint, text) and, for each respondent of the
//! call center, their latest signature and whether it covers the current text. `send_agreement`: posts the agreement
//! now to the call center's respondents who have not signed it.
use crate::agreement::Agreement;
use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use axum::Json;
use matrix_sdk::Client;
use serde_json::json;
use tracing::warn;

fn reply(querystatus: QueryStatus, message: String) -> Json<ReturnQuery> {
    Json(ReturnQuery { querystatus, message, vec_answer: vec![], vec_error: vec![] })
}

fn respondents(call_center_id: u64) -> Vec<String> {
    DbManager::new().get_call_center_respondant(call_center_id, false).ok().flatten().unwrap_or_default()
}

pub async fn list_agreement_signatures(
    privilege_level: u64,
    requester_token: &str,
    call_center_id: u64,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    if let Err(err) = super::does_user_have_permission(privilege_level, &payload, requester_token) {
        warn!("User forbidden !");
        return err;
    }
    let agreement = Agreement::current();
    let db = DbManager::new();
    let mut people = Vec::new();
    let mut users = respondents(call_center_id);
    users.sort();
    users.dedup();
    for user in users {
        let latest = db.get_latest_agreement_signature(&user).ok().flatten();
        let current = match (&agreement, &latest) {
            (Some(a), Some(s)) => s.text_sha256 == a.sha256,
            _ => false,
        };
        people.push(json!({ "respondent": user, "signed_current": current, "latest": latest }));
    }
    let answer = json!({
        "configured": agreement.is_some(),
        "version": agreement.as_ref().map(|a| a.version.clone()),
        "sha256": agreement.as_ref().map(|a| a.sha256.clone()),
        "text_fr": agreement.as_ref().map(|a| a.fr.clone()),
        "text_en": agreement.as_ref().map(|a| a.en.clone()),
        "respondents": people,
    });
    reply(QueryStatus::Success, answer.to_string())
}

pub async fn send_agreement(
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
    if Agreement::current().is_none() {
        return reply(QueryStatus::Error, "PRIVACY_OFFICER is not set on the SwitchBoard Chat server: the agreement is off".to_string());
    }
    let sent = crate::agreement::send_to_unsigned(&client, respondents(call_center_id)).await;
    reply(QueryStatus::Success, json!({ "sent": sent }).to_string())
}
