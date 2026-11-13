use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::DbManager;
use crate::{macros, rooms};
use axum::Json;
use matrix_sdk::{
    Client,
    ruma::{OwnedUserId, UserId},
};
use tracing::{error, info, warn};

pub async fn list_macros(
    privilege_level: u64,
    requester_token: &str,
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
    let macros_result = db_manager.get_all_macros_by_call_center_id(&call_center_id);
    match macros_result {
        Ok(macros) => {
            let json: String = serde_json::to_string(&macros).unwrap();
            return Json(ReturnQuery {
                querystatus: QueryStatus::Success,
                message: json,
                vec_answer: vec![],
                vec_error: vec![],
            });
        }
        Err(err) => {
            return Json(ReturnQuery {
                querystatus: QueryStatus::Error,
                message: format!("Error while getting call center macros ! {:?}", err),
                vec_answer: vec![],
                vec_error: vec![],
            });
        }
    }
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
//}
