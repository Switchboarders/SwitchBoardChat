use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use axum::Json;
use email_address::EmailAddress;
use matrix_sdk::Client;
use serde_json::json;

pub async fn add_new_call_center(
    privilege_level: u64,
    requester_token: &str,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    if privilege_level < 100 {
        tracing::warn!("Privilege level is too low");
        return super::return_query(
            QueryStatus::Error,
            "You can't add new call center, privilege level is to low.".to_string(),
            "".to_string(),
        );
    }
    let incoming_privilege_level = privilege_level;
    let db_manager = DbManager::new();
    if let Some(voip_ms_username_value) = payload.get("voip_ms_username")
        && let Some(voip_ms_api_password_value) = payload.get("voip_ms_api_password")
        && let Some(privilege_level_value) = payload.get("privilege_level")
        && let Some(call_center_did_value) = payload.get("call_center_did")
        && let Some(renew_api_token_value) = payload.get("renew_api_token")
    {
        let voip_ms_username;
        let voip_ms_api_password;
        let privilege_level;
        let renew_api_token;
        let call_center_did: u64;
        match voip_ms_username_value.as_str() {
            Some(name) => voip_ms_username = name,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "voip_ms_username can't be parsed to str".to_string(),
                    "".to_string(),
                );
            }
        }
        match voip_ms_api_password_value.as_str() {
            Some(password) => voip_ms_api_password = password,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "voip_ms_api_password can't be parsed to str".to_string(),
                    "".to_string(),
                );
            }
        }
        match call_center_did_value.as_str() {
            Some(did) => {
                call_center_did = match did.parse() {
                    Ok(ok) => ok,
                    Err(err) => {
                        return super::return_query(
                            QueryStatus::Error,
                            "call_center_did can't be parsed to u64".to_string(),
                            "".to_string(),
                        );
                    }
                };
            }
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "call_center_did can't be parsed to str".to_string(),
                    "".to_string(),
                );
            }
        }
        match privilege_level_value.as_u64() {
            Some(privilege) => privilege_level = privilege,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "privilege_level can't be parsed to u64".to_string(),
                    "".to_string(),
                );
            }
        }
        match renew_api_token_value.as_bool() {
            Some(token) => renew_api_token = token,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "renew_api_token can't be parsed to bool".to_string(),
                    "".to_string(),
                );
            }
        }

        if !EmailAddress::is_valid(voip_ms_username) {
            return super::return_query(
                QueryStatus::Error,
                "voip_ms_username not a valid email".to_string(),
                "".to_string(),
            );
        }
        // Insure you can't assign privilege higher than the user can
        if privilege_level > incoming_privilege_level {
            return super::return_query(
                QueryStatus::Error,
                format!("Insufficent privilege"),
                "".to_string(),
            );
        }

        match db_manager.create_or_update_call_center(
            voip_ms_username,
            voip_ms_api_password,
            call_center_did,
            privilege_level,
            renew_api_token,
        ) {
            Ok(result) => {
                return super::return_query(QueryStatus::Success, result.1, result.0);
            }
            Err(err) => {
                return super::return_query(
                    QueryStatus::Error,
                    format!("Error while creating call center (db issue) {err}"),
                    "".to_string(),
                );
            }
        }

        //if let Some(privilege_level) = payload.get("privilege_level") {}
    }
    tracing::warn!("voip_ms_username and/or voip_ms_api_password absent and/or privilege_level");
    super::return_query(
        QueryStatus::Error,
        "voip_ms_username and/or voip_ms_api_password absent and/or privilege_level".to_string(),
        "".to_string(),
    )
}
