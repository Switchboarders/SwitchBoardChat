use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use axum::Json;

pub async fn delete_call_center(
    privilege_level: u64,
    requester_token: &str,
    payload: serde_json::Value,
) -> Json<ReturnQuery> {
    if privilege_level < 100 {
        tracing::warn!("Privilege level is too low");
        return super::return_query(
            QueryStatus::Error,
            "You can't delete this call center, privilege level is to low.".to_string(),
            "".to_string(),
        );
    }
    let db_manager = DbManager::new();
    let api_token;
    if let Some(api_token_value) = payload.get("api_token") {
        match api_token_value.as_str() {
            Some(token) => api_token = token,
            None => {
                return super::return_query(
                    QueryStatus::Error,
                    "Api token to be deleted can't be parsed to str".to_string(),
                    "".to_string(),
                );
            }
        }

        match db_manager.get_call_center_id_by_token(api_token.to_string()) {
            Ok(None) => {
                return super::return_query(
                    QueryStatus::Warning,
                    "Token does not exist !".to_string(),
                    "".to_string(),
                );
            }
            // Nobody deletes a call center with more privilege than their own (e.g. the admin)
            Ok(Some((_, target_privilege))) if !super::may_act_on(privilege_level, target_privilege) => {
                return super::return_query(
                    QueryStatus::Error,
                    "Insufficent privilege".to_string(),
                    "".to_string(),
                );
            }
            Ok(Some(_)) => {}
            Err(err) => {
                return super::return_query(
                    QueryStatus::Error,
                    format!(
                        "Database error when trying to check if call center is already existing {}",
                        err
                    ),
                    "".to_string(),
                );
            }
        }
        match db_manager.delete_call_center_by_token(api_token.to_string()) {
            Ok(_) => {
                return super::return_query(
                    QueryStatus::Success,
                    "Call center deleted.".to_string(),
                    "".to_string(),
                );
            }
            Err(err) => {
                return super::return_query(
                    QueryStatus::Error,
                    format!(
                        "Database error when trying to check if call center was deleted {}",
                        err
                    ),
                    "".to_string(),
                );
            }
        }
    }
    return super::return_query(
        QueryStatus::Error,
        "Something wrong happened".to_string(),
        "".to_string(),
    );
}
