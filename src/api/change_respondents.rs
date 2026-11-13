use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use crate::rooms;
use axum::Json;
use matrix_sdk::{
    Client,
    ruma::{OwnedUserId, UserId},
};
use tracing::{error, info, warn};

pub async fn change_respondents(
    privilege_level: u64,
    requester_token: &str,
    client: Client,
    call_center_id: u64,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    let does_user_has_permission =
        super::does_user_have_permission(privilege_level, &payload, requester_token);
    match does_user_has_permission {
        Ok(_) => {
            info!("User has permission");
        }
        Err(err) => {
            warn!("User forbidden !");
            return err;
        }
    }
    let db_manager = DbManager::new();
    if let Some(respondent_opt) = payload.get("respondent") {
        if let Some(respondent_values) = respondent_opt.as_array() {
            // Cast serde values to str
            // "@bob" in a calendar means "@bob:<our homeserver>"
            let own_server = super::own_server_name(&client);
            let respondent_full: Vec<String> = respondent_values
                .iter()
                .map(|value| super::complete_user_id(value.as_str().unwrap_or("error unwraping to str"), &own_server))
                .collect();
            let respondent: Vec<&str> = respondent_full.iter().map(|s| s.as_str()).collect();
            // Test if respondent are valid usernames
            let mut validated_respondent: Vec<String> = vec![];
            let mut invalid_respondent: Vec<String> = vec![];

            for user in respondent.clone() {
                // Does the user is valid and exist ?
                match UserId::parse(user) {
                    Ok(user_id) => {
                        let mut useridstr = user_id.to_string();
                        if !rooms::do_user_is_from_homeserver(&client.clone(), &user_id).await {
                            warn!(
                                "User {} has invalid homeserver name or is from another matrix server",
                                user
                            );
                            useridstr.push_str(
                                " has invalid homeserver name or is from another matrix server",
                            );
                            invalid_respondent.push(useridstr);
                            continue;
                        }

                        if !rooms::do_user_exist(&client.clone(), &user_id).await {
                            warn!("User {} does not exist", user);
                            useridstr.push_str(" do not exist");
                            invalid_respondent.push(useridstr);
                            continue;
                        }

                        // If we are here, the user is valid

                        validated_respondent.push(useridstr);
                    }
                    Err(err) => {
                        warn!("Parsing error for user {} : {} ", user, err);
                        let mut useridstr = user.to_string();
                        useridstr.push_str(" invalid username format");
                        invalid_respondent.push(useridstr);
                        continue;
                    }
                }
            }

            info!(
                "Changing to the following respondents : {:?}",
                validated_respondent
            );
            if invalid_respondent.len() > 0 {
                warn!("Invalid respondents : {:?}", invalid_respondent);
            }
            // Check if we got no respondent
            if validated_respondent.len() == 0 {
                warn!("Empty respondent array, removing all respondent");
                db_manager
                    .set_call_center_respondant(call_center_id, validated_respondent)
                    .unwrap_or_else(|err| { error!("Database error : {err}"); });

                return Json(ReturnQuery {
                    querystatus: QueryStatus::Warning,
                    message: format!(
                        "Empty respondent array. No valid respondant ! Removing all respondent ! Invalid respondents : {:?}",
                        invalid_respondent
                    ),
                    vec_answer: vec![],
                    vec_error: invalid_respondent,
                });
            }
            // If we have respondents :
            db_manager
                .set_call_center_respondant(call_center_id, validated_respondent.clone())
                .unwrap_or_else(|err| { error!("Database error : {err}"); });
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

    // let ip = public_ip::addr().await;
    // match ip {
    //     Some(address) => {
    //         return Json(format!("{}", address.to_string()));
    //     }
    //     None => {
    //         return Json("Error fetching external ip address".to_string());
    //     }
    // }
}
