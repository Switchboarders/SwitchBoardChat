use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use axum::Json;
use email_address::EmailAddress;
use matrix_sdk::Client;
use serde_json::json;
use tracing::info;
use tracing::warn;
pub async fn update_call_center(
    privilege_level: u64,
    requester_token: &str,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    let does_user_has_permission =
        super::does_user_have_permission(privilege_level, &payload, &requester_token);
    match does_user_has_permission {
        Ok(_) => {
            info!("User has permission");
        }
        Err(err) => {
            warn!("User forbidden !");
            return err;
        }
    }
    if let Some(voip_ms_username_value) = payload.get("voip_ms_username")
        && let Some(voip_ms_api_password_value) = payload.get("voip_ms_api_password")
        && let Some(call_center_did_value) = payload.get("call_center_did")
        && let Some(api_token_value) = payload.get("api_token")
    {
        let voip_ms_username;
        let voip_ms_api_password;
        let call_center_did: u64;
        let api_token;

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
        match api_token_value.as_str() {
            Some(token) => api_token = token,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "api_token_value can't be parsed to str".to_string(),
                    "".to_string(),
                );
            }
        }

        let db_manager = DbManager::new();
        // The target must not have more privilege than the requester, and its new number must not
        // already belong to another call center (0 means no number)
        match db_manager.get_call_center_id_by_token(api_token.to_string()) {
            Ok(Some((target_id, target_privilege))) => {
                if !super::may_act_on(privilege_level, target_privilege) {
                    return super::return_query(QueryStatus::Error, "Insufficent privilege".to_string(), "".to_string());
                }
                match db_manager.did_used_by_another_call_center(call_center_did, target_id) {
                    Ok(false) => {}
                    Ok(true) => {
                        return super::return_query(
                            QueryStatus::Error,
                            "This number already belongs to another call center".to_string(),
                            "".to_string(),
                        );
                    }
                    Err(err) => {
                        warn!("Database error while checking the number: {err}");
                        return super::return_query(QueryStatus::Error, "Error with SBChat database".to_string(), "".to_string());
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                warn!("Database error while looking up the call center: {err}");
                return super::return_query(QueryStatus::Error, "Error with SBChat database".to_string(), "".to_string());
            }
        }
        let update_result = db_manager.update_call_center_using_token(
            api_token,
            voip_ms_username,
            voip_ms_api_password,
            call_center_did,
        );
        match update_result {
            Ok(ok) => {
                if ok {
                    return super::return_query(
                        QueryStatus::Success,
                        format!("Token for {} properly updated", voip_ms_username),
                        "".to_string(),
                    );
                } else {
                    return super::return_query(
                        QueryStatus::Error,
                        "Token is not associated to a SBChat user !".to_string(),
                        "".to_string(),
                    );
                }
            }
            Err(err) => {
                return super::return_query(
                    QueryStatus::Error,
                    "Error with SBChat database ??".to_string(),
                    "".to_string(),
                );
            }
        }
    }

    super::return_query(
        QueryStatus::Error,
        "voip_ms_username and/or voip_ms_api_password absent and/or privilege_level".to_string(),
        "".to_string(),
    )
}
