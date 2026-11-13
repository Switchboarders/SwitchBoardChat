
use crate::api::{QueryStatus, ReturnQuery};
use crate::db_manager::{DbManager, Macro};
use axum::Json;
use tracing::{error, info, warn};

pub async fn update_macro(
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
    let mut message: String = "".to_string();
    let mut new_macro_id= "".to_string();

    //Object {"id_macro": String("2"), "macro_name": String("test2"), "macro_tags": String(""),
    // "macro_tags_config": Null, "macro_text": String("bloublou")}
    if let Some(macro_info) = payload.get("macro_info") {
        let id_macro_i64 = macro_info
            .get("id_macro")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok());
        let id_macro: Option<u64> = match id_macro_i64 {
            // -1 means "new macro"
            Some(-1) => None,
            Some(id) if id >= 0 => Some(id as u64),
            _ => {
                warn!("update_macro: id_macro malformed or out of range");
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Error,
                    message: "macro_info.id_macro is required (-1 for a new macro, otherwise a positive id)".to_string(),
                    vec_answer: vec![],
                    vec_error: vec![],
                });
            }
        };
        let macro_name = match macro_info.get("macro_name").and_then(|v| v.as_str()) {
            Some(name) => name.to_string(),
            None => {
                warn!("update_macro: missing macro_name");
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Error,
                    message: "macro_info.macro_name is required".to_string(),
                    vec_answer: vec![],
                    vec_error: vec![],
                });
            }
        };
        let macro_text = match macro_info.get("macro_text").and_then(|v| v.as_str()) {
            Some(text) => text.to_string(),
            None => {
                warn!("update_macro: missing macro_text");
                return Json(ReturnQuery {
                    querystatus: QueryStatus::Error,
                    message: "macro_info.macro_text is required".to_string(),
                    vec_answer: vec![],
                    vec_error: vec![],
                });
            }
        };
        let macro_tags = macro_info
            .get("macro_tags")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let macro_tags_config = macro_info
            .get("macro_tags_config")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let my_macro = Macro {
            id_macro,
            id_call_center: call_center_id,
            macro_name,
            macro_text,
            macro_tags,
            macro_tags_config,
        };
        //println!("{:?}",my_macro);
        let add_or_replace_macro_result = db_manager.add_or_replace_macro(my_macro);
        if let Err(err) = add_or_replace_macro_result {
            error!("update_macro: failed to save macro: {err}");
            return Json(ReturnQuery {
                querystatus: QueryStatus::Error,
                message: format!("Failed to save macro: {err}"),
                vec_answer: vec![],
                vec_error: vec![],
            });
        } else {
            let (new_macro_id_opt,dbmessage) = add_or_replace_macro_result.unwrap();
            message = dbmessage;
            match new_macro_id_opt {
                Some(some) => new_macro_id = some.to_string(),
                None => {
                    new_macro_id = "".to_string();
                }
            }
        }
    }

    Json(ReturnQuery {
        querystatus: QueryStatus::Success,
        message: message,
        vec_answer: vec![new_macro_id],
        vec_error: vec![],
    })
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
