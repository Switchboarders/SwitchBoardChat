use crate::db_manager::{DbManager, Macro};
use anyhow::Result;
use tracing::error;

pub fn get_macro_by_call_center_id(macro_name: String, id_call_center: u64) -> Option<Macro> {
    let db_manager = DbManager::new();
    let result = db_manager.get_macro_by_call_center_id(&macro_name, &id_call_center);
    match result {
        Ok(ok) => {
            return ok;
        }
        Err(err) => {
            error!("Database error : {err}");
            return None;
        }
    }
}
pub fn get_newcomer_macro_text(id_call_center: u64) -> Option<String> {
    let db_manager = DbManager::new();
    let result = db_manager.get_macro_by_tag(&id_call_center, "newcomer".to_string());
    match result {
        Ok(ok) => match ok {
            Some(some) => return Some(some.macro_text),
            None => return None,
        },
        Err(err) => {
            error!("Database error : {err}");
            return None;
        }
    }
}

/// The macro sent automatically to a web visitor after their first message (tag `newcomerweb`)
pub fn get_newcomer_web_macro_text(id_call_center: u64) -> Option<String> {
    match DbManager::new().get_macro_by_tag(&id_call_center, "newcomerweb".to_string()) {
        Ok(found) => found.map(|m| m.macro_text),
        Err(err) => {
            error!("Database error : {err}");
            None
        }
    }
}

pub fn get_no_respondent_macro_text(id_call_center: u64) -> String {
    let db_manager = DbManager::new();
    let result = db_manager.get_macro_by_tag(&id_call_center, "no_respondent".to_string());
    match result {
        Ok(ok) => match ok {
            Some(some) => return some.macro_text,
            None => return "No respondent is available for now. One should answer shortly. / Aucun répondant n'est disponible pour le moment. Quelqu'un vous répondra prochainement.".to_string(),
        },
        Err(err) => {
            error!("Database error : {err}");
            return "No respondent is available for now. One should answer shortly. / Aucun répondant n'est disponible pour le moment. Quelqu'un vous répondra prochainement.".to_string();
        }
    }
}
/*pub fn add_simple_macro(macro_name: String, id_call_center: u64, macro_text: String) -> Result<()> {
    let db_manager = DbManager::new();
    let my_macro = Macro {
        id_macro: None,
        id_call_center,
        macro_name,
        macro_text,
        macro_tags: None,
        macro_tags_config: None,
    };
    db_manager
        .add_or_replace_macro(my_macro)
        .unwrap_or_else(|err| {
            error!("Database error : {err}");
            exit(1);
        });

    Ok(())
}*/
pub fn format_macro_name(macro_name: String) -> String {
    macro_name
        .trim_start_matches(" ")
        .trim_end_matches(" ")
        .trim_start_matches("#")
        .to_lowercase()
}

/// Reply to HELP/AIDE (required by carriers): who is texting and how to stop.
/// Call centers should set their own macro tagged "help" that names their organization.
pub fn get_help_macro_text(id_call_center: u64) -> String {
    let db_manager = DbManager::new();
    let result = db_manager.get_macro_by_tag(&id_call_center, "help".to_string());
    match result {
        Ok(ok) => match ok {
            Some(some) => return some.macro_text,
            None => return "Our support team's text line. Reply STOP to opt out, START to resume. / Ligne texto de notre équipe. ARRET pour vous désabonner, REPRENDRE pour revenir.".to_string(),
        },
        Err(err) => {
            error!("Database error : {err}");
            return "Our support team's text line. Reply STOP to opt out, START to resume. / Ligne texto de notre équipe. ARRET pour vous désabonner, REPRENDRE pour revenir.".to_string();
        }
    }
}
