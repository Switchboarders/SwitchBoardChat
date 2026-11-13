use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use crate::rooms;
use crate::userroom::UserRoom;
use axum::Json;
use matrix_sdk::{
    Client,
    ruma::{OwnedUserId, UserId},
};
use tracing::{error, info, warn};

pub async fn send_message(
    privilege_level: u64,
    client: Client,
    call_center_id: u64,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    let message = match payload.get("message") {
        Some(some) => match some.as_str() {
            Some(some) => some,
            None => {
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Error,
                    message: "Could not parse message.".to_string(),
                    vec_answer: vec![],
                    vec_error: vec![],
                });
            }
        },
        None => {
            return Json(ReturnQuery {
                querystatus: QueryStatus::Error,
                message: "No message was submitted.".to_string(),
                vec_answer: vec![],
                vec_error: vec![],
            });
        }
    };
    if let Some(respondent_opt) = payload.get("respondent") {
        if let Some(respondent_values) = respondent_opt.as_array() {
            // Cast serde values to str
            // "@bob" in a calendar means "@bob:<our homeserver>"
            let own_server = super::own_server_name(&client);
            let respondent_full: Vec<String> = respondent_values
                .iter()
                .map(|value| super::complete_user_id(value.as_str().unwrap_or("error unwraping to str"), &own_server))
                .collect();
            if respondent_full.len() > MAX_RECIPIENTS {
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Error,
                    message: format!("At most {MAX_RECIPIENTS} respondents per message"),
                    vec_answer: vec![],
                    vec_error: vec![],
                });
            }
            // The bot only writes to users of its own homeserver, or to respondents this call center
            // already has: a call center token must not turn the bot into a messenger to anyone.
            let known_respondents = DbManager::new()
                .get_call_center_respondant(call_center_id, false)
                .ok()
                .flatten()
                .unwrap_or_default();
            let respondent: Vec<&str> = respondent_full.iter().map(|s| s.as_str()).collect();
            // Test if respondent are valid usernames
            let mut validated_respondent: Vec<String> = vec![];
            let mut invalid_respondent: Vec<String> = vec![];
            let user_room = UserRoom::new(client.clone());
            for user in respondent.clone() {
                // Does the user is valid and exist ?
                match UserId::parse(user) {
                    Ok(user_id) if !may_message(&user_id, &own_server, &known_respondents) => {
                        warn!("send_message: {user_id} is neither local nor a respondent of this call center, skipped");
                        invalid_respondent.push(user_id.to_string());
                    }
                    Ok(user_id) => {
                        let useridstr = user_id.to_string();
                        match user_room
                            .send_message_to_user_room(&message.to_string(), &useridstr)
                            .await
                        {
                            Ok(_) => {
                                validated_respondent.push(useridstr);
                            }
                            Err(_) => {
                                invalid_respondent.push(useridstr);
                            }
                        }
                    }
                    Err(err) => {
                        error!("Could not parse respondent user name {err}");
                        invalid_respondent.push(user.to_string());
                    }
                }
            }

            info!(
                "Sent a message to the following respondents : {:?}",
                validated_respondent
            );

            if invalid_respondent.len() > 0 {
                return Json(ReturnQuery {
                    querystatus: QueryStatus::SuccessWithWarnings,
                    message: format!("{:?}", validated_respondent),
                    vec_answer: validated_respondent,
                    vec_error: invalid_respondent,
                });
            } else {
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Success,
                    message: format!("{:?}", validated_respondent),
                    vec_answer: validated_respondent,
                    vec_error: vec![],
                });
            }
        } else {
            error!("Unable to parse respondent array");
            return Json(ReturnQuery {
                querystatus: QueryStatus::Error,
                message: "Unable to parse respondent array".to_string(),
                vec_answer: vec![],
                vec_error: vec![],
            });
        }
    } else {
        return Json(ReturnQuery {
            querystatus: QueryStatus::Warning,
            message: "No respondent were submitted.".to_string(),
            vec_answer: vec![],
            vec_error: vec![],
        });
    }
}

const MAX_RECIPIENTS: usize = 50;

/// A user the bot may write to on a call center's behalf
fn may_message(user: &UserId, own_server: &str, known_respondents: &[String]) -> bool {
    user.server_name().as_str() == own_server || known_respondents.iter().any(|r| r == user.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_users_or_known_respondents_get_messages() {
        let known = vec!["@far:other.org".to_string()];
        let local = UserId::parse("@carol:example.org").unwrap();
        let far_known = UserId::parse("@far:other.org").unwrap();
        let stranger = UserId::parse("@spam:evil.example").unwrap();
        assert!(may_message(&local, "example.org", &known));
        assert!(may_message(&far_known, "example.org", &known));
        assert!(!may_message(&stranger, "example.org", &known));
    }
}
