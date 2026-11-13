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

pub async fn broadcast_message(
    privilege_level: u64,
    client: Client,
    call_center_id: u64,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    // Only master admin can broadcast !
    if privilege_level < 101 {
        tracing::warn!("Privilege level is too low");
        return Json(ReturnQuery {
            querystatus: QueryStatus::Error,
            message: "You can't send a broadcast message, privilege level is to low.".to_string(),
            vec_answer: vec![],
            vec_error: vec![],
        });
    }
    let db = DbManager::new();
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
    let respondent_opt = db.get_all_respondant_user_id(false);
    if let Ok(respondent_opt) = respondent_opt {
        if let Some(respondent) = respondent_opt {
            // Test if respondent are valid usernames
            let mut validated_respondent: Vec<String> = vec![];
            let mut invalid_respondent: Vec<String> = vec![];
            let user_room = UserRoom::new(client.clone());
            for user in respondent.clone() {
                // Does the user is valid and exist ?
                match UserId::parse(user.clone()) {
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
