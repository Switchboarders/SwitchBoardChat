pub mod add_new_call_center;
pub mod agreement;
pub mod delete_call_center;
pub mod update_call_center;

pub mod broadcast_message;
pub mod change_respondents;
pub mod erase_caller;
pub mod web_unban;
pub mod list_macros;
pub mod send_message;
pub mod update_macro;
pub mod web_sites;
use axum::Json;
use matrix_sdk::Client;
use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Deserialize, Serialize, Debug)]
enum QueryStatus {
    Success,
    SuccessWithWarnings,
    Warning,
    Error,
}
impl fmt::Display for QueryStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            QueryStatus::Success => write!(f, "Success"),
            QueryStatus::SuccessWithWarnings => write!(f, "SuccessWithWarnings"),
            QueryStatus::Warning => write!(f, "Warning"),
            QueryStatus::Error => write!(f, "Error"),
        }
    }
}
#[derive(Deserialize, Serialize, Debug)]
pub struct ReturnQuery {
    querystatus: QueryStatus,
    message: String,
    vec_answer: Vec<String>,
    vec_error: Vec<String>,
}
pub fn return_query(
    querystatus: QueryStatus,
    message: String,
    answer: String,
) -> Json<ReturnQuery> {
    let vec_answer;
    if answer.len() > 0 {
        vec_answer = vec![answer]
    } else {
        vec_answer = vec![]
    }
    Json(ReturnQuery {
        querystatus,
        message,
        vec_answer: vec_answer,
        vec_error: vec![],
    })
}

/// Completes a respondent's user name written without its homeserver: in a SwitchBoard calendar "@bob" (or "bob")
/// means "@bob:<the bridge's own homeserver>". Full ids ("@bob:example.org") are kept as they are.
pub fn complete_user_id(user: &str, own_server: &str) -> String {
    let user = user.trim();
    if user.contains(':') || own_server.is_empty() {
        return user.to_string();
    }
    let local = user.strip_prefix('@').unwrap_or(user);
    format!("@{local}:{own_server}")
}

/// The bridge's homeserver name (from its own Matrix id)
pub fn own_server_name(client: &Client) -> String {
    client.user_id().map(|u| u.server_name().to_string()).unwrap_or_default()
}

/// A requester may only act on a call center whose privilege is not above their own
pub fn may_act_on(requester_privilege: u64, target_privilege: u64) -> bool {
    target_privilege <= requester_privilege
}

pub fn does_user_have_permission(
    privilege_level: u64,
    payload: &serde_json::Value,
    requester_token: &str,
) -> Result<(), Json<ReturnQuery>> {
    // Check if user token match itself
    let api_token: &str;
    let user_modify_itself: bool;
    if let Some(api_token_value) = payload.get("api_token") {
        match api_token_value.as_str() {
            Some(some) => api_token = some,
            None => {
                return Err(return_query(
                    QueryStatus::Error,
                    "Api token can't be parsed to str".to_string(),
                    "".to_string(),
                ));
            }
        }
    } else {
        return Err(return_query(
            QueryStatus::Error,
            "api_token not in the request".to_string(),
            "".to_string(),
        ));
    }
    user_modify_itself = requester_token == api_token;
    if !user_modify_itself && privilege_level < 100 {
        tracing::warn!("Privilege level is too low");
        return Err(return_query(
            QueryStatus::Error,
            "You can't add new call center, privilege level is to low.".to_string(),
            "".to_string(),
        ));
    }

    Ok(())
}

pub async fn execute_command(
    client: Client,
    call_center_id: u64,
    privilege_level: u64,
    token: &str,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    // Accessing command
    if let Some(command) = payload.get("command") {
        if let Some(command) = command.as_str() {
            tracing::debug!("sbchat_api command: {command}");
            match command {
                "change_respondents" => {
                    return change_respondents::change_respondents(
                        privilege_level,
                        token,
                        client,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                "send_message" => {
                    return send_message::send_message(
                        privilege_level,
                        client,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                "broadcast_message" => {
                    return broadcast_message::broadcast_message(
                        privilege_level,
                        client,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                "add_new_call_center" => {
                    return add_new_call_center::add_new_call_center(
                        privilege_level,
                        token,
                        payload,
                    )
                    .await;
                }
                "update_call_center" => {
                    return update_call_center::update_call_center(privilege_level, token, payload)
                        .await;
                }
                "delete_call_center" => {
                    return delete_call_center::delete_call_center(privilege_level, token, payload)
                        .await;
                }
                "web_unban" => {
                    return web_unban::web_unban(privilege_level, payload).await;
                }
                "erase_caller" => {
                    return erase_caller::erase_caller(
                        privilege_level,
                        token,
                        client,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                "list_macros" => {
                    return list_macros::list_macros(
                        privilege_level,
                        token,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                "list_agreement_signatures" => {
                    return agreement::list_agreement_signatures(privilege_level, token, call_center_id, payload).await;
                }
                "send_agreement" => {
                    return agreement::send_agreement(privilege_level, token, client, call_center_id, payload).await;
                }
                "create_web_pairing" => {
                    return web_sites::create_web_pairing(privilege_level, token, payload).await;
                }
                "list_web_sites" => {
                    return web_sites::list_web_sites(privilege_level, token, payload).await;
                }
                "remove_web_site" => {
                    return web_sites::remove_web_site(privilege_level, token, payload).await;
                }
                "update_macro" => {
                    return update_macro::update_macro(
                        privilege_level,
                        token,
                        call_center_id,
                        payload,
                    )
                    .await;
                }
                _ => {
                    return Json(ReturnQuery {
                        querystatus: QueryStatus::Error,
                        message: "Invalid command".to_string(),
                        vec_answer: vec![],
                        vec_error: vec![],
                    });
                }
            }
        }
    }
    Json(ReturnQuery {
        querystatus: QueryStatus::Error,
        message: "Invalid command".to_string(),
        vec_answer: vec![],
        vec_error: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::{complete_user_id, may_act_on};

    #[test]
    fn nobody_acts_on_a_call_center_above_their_own_privilege() {
        assert!(may_act_on(100, 100));
        assert!(may_act_on(100, 0));
        assert!(!may_act_on(100, 101));
        assert!(!may_act_on(0, 100));
    }

    #[test]
    fn short_user_names_get_the_bridge_homeserver() {
        assert_eq!(complete_user_id("@bob", "example.org"), "@bob:example.org");
        assert_eq!(complete_user_id(" bob ", "example.org"), "@bob:example.org");
        assert_eq!(complete_user_id("@bob:example.org", "example.org"), "@bob:example.org");
        assert_eq!(complete_user_id("@bob:other.org", "example.org"), "@bob:other.org");
    }
}
