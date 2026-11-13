//! Commands a respondent types in a conversation room (`!ok`, `!aide`, `!fin`, `!bloquer`, …).
//!
//! Parsing is separate from running so it can be tested: the first word after `!` decides, whatever follows
//! (`!help me` is `!help`), and every command gets a visible answer in the room.

use crate::web_chat;

#[derive(Debug, PartialEq)]
pub enum RoomCommand {
    /// Private room: remove the other respondents
    Ok,
    /// Opt the SMS caller out
    Stop,
    /// List the commands and macros (`french` for `!aide`)
    Help { french: bool },
    /// Erase the caller / end the web chat
    Forget,
    /// End and delete a web chat
    End,
    /// Block an abusive web visitor, for this many days (None = the default)
    Ban(Option<u64>),
    /// Anything else that starts with `!`
    Unknown(String),
}

/// None when the text is not a command (it does not start with `!` followed by a word)
pub fn parse(body: &str) -> Option<RoomCommand> {
    let text = body.trim_start();
    if !text.starts_with('!') {
        return None;
    }
    if let Some(days) = web_chat::parse_ban_command(text) {
        return Some(RoomCommand::Ban(days));
    }
    let rest = &text[1..];
    let word = if rest.starts_with(char::is_whitespace) { String::new() } else { rest.split_whitespace().next().unwrap_or("").to_lowercase() };
    if word.is_empty() {
        return None; // "! hello" is an ordinary sentence
    }
    Some(match word.as_str() {
        "ok" => RoomCommand::Ok,
        "stop" => RoomCommand::Stop,
        "help" => RoomCommand::Help { french: false },
        "aide" => RoomCommand::Help { french: true },
        "oubli" | "forget" => RoomCommand::Forget,
        "fin" | "end" => RoomCommand::End,
        other => RoomCommand::Unknown(other.to_string()),
    })
}

pub const PRIVATE_ROOM_DONE: &str = "Salle privée : les autres répondants ont été retirés de cette salle. / Private room: the other respondents were removed from this room.";
pub const PRIVATE_ROOM_FAILED: &str = "Impossible de retirer les autres répondants. / Could not remove the other respondents.";
pub const STOP_NOT_FOR_WEB: &str = "!stop ne s'applique pas à un clavardage web (le visiteur n'a pas de numéro). Pour un visiteur abusif, écrivez !bloquer [jours]. / !stop does not apply to a web chat (the visitor has no phone number). For an abusive visitor, type !ban [days].";
pub const WEB_ONLY: &str = "Cette commande s'applique seulement à un clavardage web. Pour un appelant SMS : !stop (désabonner) ou !oubli (effacer). / This command only applies to a web chat. For an SMS caller: !stop (opt out) or !forget (erase).";

pub fn unknown(word: &str) -> String {
    format!("Commande inconnue : !{word}. Écrivez !aide pour la liste. / Unknown command: !{word}. Type !help for the list.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_word_decides() {
        assert_eq!(parse("!ok"), Some(RoomCommand::Ok));
        assert_eq!(parse("  !OK please"), Some(RoomCommand::Ok));
        assert_eq!(parse("!help"), Some(RoomCommand::Help { french: false }));
        assert_eq!(parse("!Help me"), Some(RoomCommand::Help { french: false }));
        assert_eq!(parse("!aide"), Some(RoomCommand::Help { french: true }));
        assert_eq!(parse("!stop"), Some(RoomCommand::Stop));
        assert_eq!(parse("!oubli"), Some(RoomCommand::Forget));
        assert_eq!(parse("!forget"), Some(RoomCommand::Forget));
        assert_eq!(parse("!fin"), Some(RoomCommand::End));
        assert_eq!(parse("!end"), Some(RoomCommand::End));
    }

    #[test]
    fn the_ban_command_keeps_its_duration() {
        assert_eq!(parse("!ban"), Some(RoomCommand::Ban(None)));
        assert_eq!(parse("!bloquer 3j"), Some(RoomCommand::Ban(Some(3))));
        assert_eq!(parse("!ban 7"), Some(RoomCommand::Ban(Some(7))));
    }

    #[test]
    fn unknown_words_and_plain_text() {
        assert_eq!(parse("!bonjour"), Some(RoomCommand::Unknown("bonjour".into())));
        assert_eq!(parse("!"), None);
        assert_eq!(parse("! really?"), None);
        assert_eq!(parse("hello !ok"), None);
        assert_eq!(parse("#macro"), None);
        assert_eq!(parse(""), None);
        assert!(unknown("xyz").contains("!xyz") && unknown("xyz").contains(" / "));
    }

    #[test]
    fn every_reply_is_bilingual() {
        for text in [PRIVATE_ROOM_DONE, PRIVATE_ROOM_FAILED, STOP_NOT_FOR_WEB, WEB_ONLY] {
            assert!(text.contains(" / "), "{text}");
        }
    }
}
