// Help text for the !aide (French) and !help (English) room commands.
use crate::db_manager::{DbManager, Macro};
use tracing::error;

/// Start of the first message of every new room: tells respondents about !ok and !help
pub const NEW_ROOM_HINT: &str = "Pour obtenir une salle privée écrire !ok / For a private room type !ok\nPour la liste des commandes et des macros écrire !aide / For the list of commands and macros type !help\n";

/// Start of the first message of a web chat room: what the room is, then the commands a respondent needs
/// (private room, abusive visitor, end the chat, help), each in French and English.
/// Web visitors have no phone number and get no SMS.
pub const WEB_ROOM_HINT: &str = "[Clavardage web / Web chat] Vos réponses sont envoyées au visiteur dans le navigateur. / Your replies are sent to the visitor in the browser.\n\
Pour une salle privée écrire !ok / For a private room type !ok\n\
Visiteur abusif : écrire !bloquer [jours] / Abusive visitor: type !ban [days]\n\
Pour terminer et supprimer la conversation écrire !fin / To end and delete the conversation type !end\n\
Pour la liste des commandes et des macros écrire !aide / For the list of commands and macros type !help\n";

const MACRO_PREVIEW_CHARS: usize = 80;

/// (command, French description, English description). Listed most used first:
/// the everyday commands, then the ones that rarely apply.
const FREQUENT_COMMANDS: [(&str, &str, &str); 2] = [
    (
        "!ok",
        "passer en salle privée avec l'appelant : retire les autres répondants de la salle",
        "switch to a private room with the caller: removes the other respondents from the room",
    ),
    (
        "!aide / !help",
        "afficher cette liste (!aide en français, !help en anglais)",
        "show this list (!help in English, !aide in French)",
    ),
];
const OCCASIONAL_COMMANDS: [(&str, &str, &str); 4] = [
    (
        "!stop",
        "désabonner l'appelant : il ne recevra plus aucun texto tant qu'il n'écrit pas REPRENDRE (ou START)",
        "opt the caller out: they will receive no more SMS until they text START (or REPRENDRE)",
    ),
    (
        "!oubli / !forget",
        "effacer la conversation et toutes les données de l'appelant (droit à l'effacement) ; le robot quitte la salle, supprimez-la aussi de votre côté",
        "erase the conversation and all the caller's data (right to erasure); the bot leaves the room, delete it on your side too",
    ),
    (
        "!bloquer [jours] / !ban [days]",
        "clavardage web seulement : bloquer la connexion du visiteur (1 jour par défaut, 30 jours au plus) et terminer le clavardage",
        "web chat only: block the visitor's connection (1 day by default, 30 days at most) and end the chat",
    ),
    (
        "!fin / !end",
        "clavardage web seulement : terminer et supprimer la conversation",
        "web chat only: end and delete the conversation",
    ),
];

/// Builds the help message listing every command and every macro of the call center
pub fn get_help_text(id_call_center: u64, french: bool) -> String {
    let macros = DbManager::new()
        .get_all_macros_by_call_center_id(&id_call_center)
        .unwrap_or_else(|err| {
            error!("Database error : {err}");
            Vec::new()
        });
    format_help(french, macros)
}

/// Answer to a conversation command (!ok, !stop, ...) typed in a respondent's personal room
pub const CONVERSATION_ONLY: &str = "Cette commande s'utilise dans une salle de conversation avec un appelant, pas dans votre salle personnelle. Écrivez !aide pour la liste. / This command is used in a conversation room with a caller, not in your personal room. Type !help for the list.";

/// Help shown in a respondent's personal room (verification and confidentiality agreement):
/// the commands of that room, then the conversation commands and the macros of each of
/// the respondent's call centers (`(call center number, macros)`)
pub fn get_user_room_help_text(respondant_username: &str, french: bool) -> String {
    let db_manager = DbManager::new();
    let db_error = |err: rusqlite::Error| {
        error!("Database error : {err}");
    };
    let call_centers = db_manager
        .get_call_centers_of_respondant(respondant_username)
        .unwrap_or_else(|err| {
            db_error(err);
            Vec::new()
        });
    let mut groups = Vec::new();
    for id_call_center in call_centers {
        let number = db_manager
            .get_call_center_by_id(id_call_center)
            .unwrap_or_else(|err| {
                db_error(err);
                None
            })
            .map(|call_center| call_center.call_center_did)
            .unwrap_or_default();
        let macros = db_manager
            .get_all_macros_by_call_center_id(&id_call_center)
            .unwrap_or_else(|err| {
                db_error(err);
                Vec::new()
            });
        groups.push((number, macros));
    }
    format_user_room_help(french, groups)
}

fn format_user_room_help(french: bool, groups: Vec<(u64, Vec<Macro>)>) -> String {
    let mut text = String::new();
    text.push_str(if french {
        "Dans une salle de conversation avec un appelant, tout message est envoyé par texto à l'appelant.\n"
    } else {
        "In a conversation room with a caller, any message is sent to the caller by SMS.\n"
    });

    let several = groups.len() > 1;
    if groups.is_empty() {
        text.push('\n');
        text.push_str(if french {
            "Vous n'êtes inscrit à aucune ligne pour le moment, donc aucune macro.\n"
        } else {
            "You are not on any line yet, so no macros.\n"
        });
    }
    for (number, macros) in groups {
        text.push('\n');
        if several {
            text.push_str(&if french {
                format!("Ligne {number} :\n")
            } else {
                format!("Line {number}:\n")
            });
        }
        push_macros(&mut text, french, macros);
    }

    text.push('\n');
    push_all_commands(&mut text, french);

    // One-time things last: the agreement is signed once
    text.push('\n');
    text.push_str(if french {
        "Ici, dans votre salle personnelle :\n\
         \x20 !entente / !agreement : lire l'entente de confidentialité\n\
         \x20 !signer Prénom Nom / !sign First Last : signer l'entente\n\
         Tout autre message écrit ici vérifie vos appareils (sessions Element)."
    } else {
        "Here, in your personal room:\n\
         \x20 !agreement / !entente: read the confidentiality agreement\n\
         \x20 !sign First Last / !signer Prénom Nom: sign the agreement\n\
         Any other message written here checks your devices (Element sessions)."
    });
    text
}

/// Name of a macro tag in the reader's language. Tagged macros are also sent
/// automatically (welcome message, no respondent, help reply).
fn tag_label(tag: &str, french: bool) -> String {
    match (tag, french) {
        ("newcomer", true) => "envoyée automatiquement aux nouveaux appelants".to_string(),
        ("newcomer", false) => "sent automatically to new callers".to_string(),
        ("newcomerweb", true) => "envoyée automatiquement aux nouveaux visiteurs du clavardage web".to_string(),
        ("newcomerweb", false) => "sent automatically to new web chat visitors".to_string(),
        ("no_respondent", true) => "envoyée automatiquement si aucun répondant n'est disponible".to_string(),
        ("no_respondent", false) => "sent automatically when no respondent is available".to_string(),
        ("help", true) => "envoyée automatiquement en réponse à AIDE / HELP".to_string(),
        ("help", false) => "sent automatically in reply to HELP / AIDE".to_string(),
        (other, _) => other.to_string(),
    }
}

fn macro_preview(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > MACRO_PREVIEW_CHARS {
        let cut: String = single_line.chars().take(MACRO_PREVIEW_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        single_line
    }
}

fn push_commands(text: &mut String, french: bool, commands: &[(&str, &str, &str)]) {
    for (command, description_fr, description_en) in commands {
        let description = if french { description_fr } else { description_en };
        text.push_str(&format!("  {command} : {description}\n"));
    }
}

fn is_automatic(a_macro: &Macro) -> bool {
    a_macro.macro_tags.as_deref().is_some_and(|tag| !tag.is_empty())
}

/// Macros typed by hand first (by name), then the ones that are mostly sent automatically
fn push_macros(text: &mut String, french: bool, mut macros: Vec<Macro>) {
    macros.sort_by(|a, b| {
        (is_automatic(a), &a.macro_name).cmp(&(is_automatic(b), &b.macro_name))
    });
    if macros.is_empty() {
        text.push_str(if french {
            "Aucune macro n'est définie.\n"
        } else {
            "No macro is defined.\n"
        });
    } else {
        text.push_str(if french {
            "Macros (écrire #nom envoie le texto de la macro à l'appelant) :\n"
        } else {
            "Macros (typing #name sends the macro's text to the caller):\n"
        });
        for a_macro in &macros {
            text.push_str(&format!(
                "  #{} : {}",
                a_macro.macro_name,
                macro_preview(&a_macro.macro_text)
            ));
            if let Some(tag) = a_macro.macro_tags.as_deref().filter(|tag| !tag.is_empty()) {
                text.push_str(&format!(" [{}]", tag_label(tag, french)));
            }
            text.push('\n');
        }
    }
}

/// Commands, most used first, then the ones that rarely apply under their own heading
fn push_all_commands(text: &mut String, french: bool) {
    text.push_str(if french { "Commandes :\n" } else { "Commands:\n" });
    push_commands(text, french, &FREQUENT_COMMANDS);
    text.push_str(if french { "Plus rarement :\n" } else { "Less often:\n" });
    push_commands(text, french, &OCCASIONAL_COMMANDS);
}

/// What a respondent uses the most comes first: plain messages, macros, !ok
fn format_help(french: bool, macros: Vec<Macro>) -> String {
    let mut text = String::new();
    text.push_str(if french {
        "Tout message écrit ici est envoyé par texto à l'appelant.\n\n"
    } else {
        "Any message written here is sent to the caller by SMS.\n\n"
    });
    push_macros(&mut text, french, macros);
    text.push('\n');
    push_all_commands(&mut text, french);
    text.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_macro(name: &str, text: &str, tags: Option<&str>) -> Macro {
        Macro {
            id_macro: None,
            id_call_center: 1,
            macro_name: name.to_string(),
            macro_text: text.to_string(),
            macro_tags: tags.map(|tag| tag.to_string()),
            macro_tags_config: None,
        }
    }

    #[test]
    fn test_help_in_both_languages() {
        let macros = vec![
            a_macro("salut", "Bonjour,\nmerci de nous écrire", None),
            a_macro("accueil", "Bienvenue", Some("newcomer")),
        ];
        let french = format_help(true, macros);
        assert!(french.starts_with("Tout message écrit ici est envoyé par texto"));
        // Most used first: hand-typed macros, automatic macros, everyday commands, rare commands
        let order: Vec<usize> = ["#salut", "#accueil", "!ok", "!aide / !help", "Plus rarement", "!stop", "!oubli", "!fin"]
            .iter()
            .map(|needle| french.find(needle).unwrap_or_else(|| panic!("missing {needle}")))
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "wrong order: {french}");
        assert!(french.contains("!ok"));
        assert!(french.contains("!stop"));
        assert!(french.contains("!aide / !help"));
        // Text on one line, tag explained
        assert!(french.contains("#salut : Bonjour, merci de nous écrire\n"));
        assert!(french.contains("#accueil : Bienvenue [envoyée automatiquement aux nouveaux appelants]"));

        let english = format_help(false, vec![a_macro("hello", "Hi", None)]);
        assert!(english.starts_with("Any message written here is sent to the caller by SMS"));
        assert!(english.contains("Less often:"));
        assert!(english.contains("#hello : Hi\n"));
        assert!(english.contains("opt the caller out"));
    }

    #[test]
    fn test_help_without_macros_and_long_macro() {
        assert!(format_help(true, vec![]).contains("Aucune macro n'est définie."));
        assert!(format_help(false, vec![]).contains("No macro is defined."));

        let long = format_help(false, vec![a_macro("long", &"mot ".repeat(50), None)]);
        let line = long.lines().find(|line| line.starts_with("  #long")).unwrap();
        assert!(line.ends_with('…'));
        assert!(line.chars().count() < MACRO_PREVIEW_CHARS + 20);
    }

    #[test]
    fn test_user_room_help() {
        let one = format_user_room_help(true, vec![(5551234567, vec![a_macro("salut", "Bonjour", None)])]);
        assert!(one.starts_with("Dans une salle de conversation avec un appelant"));
        // Conversation things first, the one-time agreement commands last
        assert!(one.find("#salut").unwrap() < one.find("!ok").unwrap());
        assert!(one.find("!ok").unwrap() < one.find("!stop").unwrap());
        assert!(one.find("!stop").unwrap() < one.find("!entente").unwrap());
        for command in ["!entente", "!signer", "!aide / !help", "!ok", "!stop"] {
            assert!(one.contains(command), "missing {command}");
        }
        assert!(one.contains("vérifie vos appareils"));
        assert!(one.contains("#salut : Bonjour"));
        // Only one line: no line header
        assert!(!one.contains("Ligne 5551234567"));
        // The room's own commands keep their two-space indent
        assert!(one.contains("\n  !entente / !agreement"));

        let several = format_user_room_help(
            false,
            vec![(1111111111, vec![a_macro("a", "A", None)]), (2222222222, vec![])],
        );
        assert!(several.starts_with("In a conversation room with a caller"));
        assert!(several.contains("Line 1111111111:\n"));
        assert!(several.contains("Line 2222222222:\nNo macro is defined."));

        assert!(format_user_room_help(true, vec![]).contains("aucune macro"));
        assert!(CONVERSATION_ONLY.contains(" / "));
    }

    #[test]
    fn test_new_room_hint_mentions_both_commands() {
        assert!(NEW_ROOM_HINT.contains("!ok"));
        assert!(NEW_ROOM_HINT.contains("!aide"));
        assert!(NEW_ROOM_HINT.contains("!help"));
        assert!(NEW_ROOM_HINT.ends_with('\n'));
    }

    #[test]
    fn test_web_room_hint_lists_the_respondent_commands_in_both_languages() {
        for command in ["!ok", "!bloquer", "!ban", "!fin", "!end", "!aide", "!help"] {
            assert!(WEB_ROOM_HINT.contains(command), "missing {command}");
        }
        // every command line carries its French and English halves
        for line in WEB_ROOM_HINT.lines() {
            assert!(line.contains(" / "), "line is not bilingual: {line}");
        }
        assert!(WEB_ROOM_HINT.ends_with('\n'));
    }
}
