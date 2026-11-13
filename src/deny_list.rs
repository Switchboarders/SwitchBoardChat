// Deny list: callers who texted STOP (or were opted out by a respondent with !stop) no
// longer receive any SMS from the call center (respondent replies, macros or automatic
// messages) until they text START (or REPRENDRE). HELP/AIDE always gets the help reply.
use crate::db_manager::DbManager;
use crate::utils::string_utils::retirer_accents;
use tracing::error;

// Includes every keyword the US FCC requires to be honored (stop, quit, end, revoke,
// opt out, cancel, unsubscribe) plus the French equivalents.
const OPT_OUT_KEYWORDS: [&str; 10] = [
    "stop",
    "stopall",
    "end",
    "cancel",
    "unsubscribe",
    "quit",
    "revoke",
    "optout",
    "arret",
    "arreter",
];
const OPT_IN_KEYWORDS: [&str; 3] = ["start", "unstop", "reprendre"];
const HELP_KEYWORDS: [&str; 3] = ["help", "aide", "info"];

// SMS sent to callers are bilingual and must fit in one SMS (160 characters, see
// voip_ms::SMS_MAX_CHARS); a test checks it.
const OPT_OUT_REPLY: &str = "You're unsubscribed, no more messages. Reply START to resume. / Vous êtes désabonné, plus aucun message. Répondez REPRENDRE pour revenir.";
const OPT_IN_REPLY: &str = "You're subscribed again. Reply STOP to stop. / Vous êtes réabonné. Répondez ARRET pour arrêter.";
/// Sent to the caller when a respondent opts them out with !stop
pub const RESPONDENT_OPT_OUT_REPLY: &str = "As requested, you're unsubscribed. Reply START to resume. / À votre demande, vous êtes désabonné. Répondez REPRENDRE pour revenir.";

/// Shown in the room when the caller opts out
pub const ROOM_NOTICE_OPTED_OUT: &str = "The caller opted out (STOP) and will no longer receive messages. / L'appelant s'est désabonné (ARRET) et ne recevra plus de messages.";
/// Shown in the room when the caller opts back in
pub const ROOM_NOTICE_OPTED_IN: &str = "The caller opted back in and can receive messages again. / L'appelant s'est réabonné et peut à nouveau recevoir des messages.";
/// Shown in the room when a respondent tries to message an opted-out caller
pub const ROOM_NOTICE_NOT_SENT: &str = "Message not sent: this caller opted out (STOP). They must text START to receive messages again. / Message non envoyé : cet appelant s'est désabonné (ARRET). Il doit écrire REPRENDRE pour recevoir à nouveau des messages.";
/// Shown in the room after a respondent's !stop
pub const ROOM_NOTICE_RESPONDENT_OPT_OUT: &str = "The caller was added to the deny list and was told by SMS. They will no longer receive messages unless they text START. / L'appelant a été ajouté à la liste de refus et en a été informé par SMS. Il ne recevra plus de messages à moins d'écrire REPRENDRE.";
/// Shown in the room after !stop for a caller already on the deny list
pub const ROOM_NOTICE_ALREADY_OPTED_OUT: &str = "This caller is already on the deny list. / Cet appelant est déjà sur la liste de refus.";

/// Sent once to every caller (appended to the first automatic SMS when there is one),
/// so they know how to opt out, and how long their texts are kept (Law 25), before anyone
/// writes to them. It is longer than one SMS: voip_ms splits it.
fn first_contact_notice_text() -> String {
    let days = crate::retention::sms_retention_days();
    format!(
        "Private. Your texts are deleted after {days} days. Reply STOP to opt out, HELP for help. / Privé. Vos textos sont supprimés après {days} jours. Répondez ARRET pour vous désabonner, AIDE pour de l'aide."
    )
}

/// Whether this caller still has to receive the first-contact opt-out notice
pub fn needs_first_contact_notice(call_center_id: u64, caller_did: u64) -> bool {
    match DbManager::new().is_caller_notified(call_center_id, caller_did) {
        Ok(notified) => !notified,
        Err(err) => {
            // Cannot tell: do not send the notice now, the next message asks again
            error!("Database error : {err}");
            false
        }
    }
}

/// Appends the first-contact notice to an automatic SMS if still `pending`.
/// Returns the text and whether the notice is included.
pub fn with_first_contact_notice(text: String, pending: bool) -> (String, bool) {
    if pending {
        (format!("{text}\n{}", first_contact_notice_text()), true)
    } else {
        (text, false)
    }
}

/// The notice alone, for callers whose first message triggered no other automatic SMS
pub fn first_contact_notice() -> String {
    first_contact_notice_text()
}

/// Records that the caller received the first-contact notice
pub fn mark_notified(call_center_id: u64, caller_did: u64) {
    DbManager::new()
        .mark_caller_notified(call_center_id, caller_did)
        .unwrap_or_else(|err| {
            error!("Database error : {err}");
        });
}

#[derive(Debug, PartialEq)]
pub enum DenyCheck {
    /// Not on the deny list: process the message normally
    Allowed,
    /// On the deny list and not opting back in: drop the message silently
    Denied,
    /// Texted an opt-out keyword. `reply` is Some only the first time, so the
    /// confirmation is sent once
    OptOut { reply: Option<&'static str> },
    /// Was on the deny list and texted an opt-in keyword
    OptIn { reply: &'static str },
    /// Texted HELP/AIDE: always gets the help reply. If `denied`, the message is
    /// then dropped like any other message from an opted-out caller.
    Help { denied: bool },
}

enum Keyword {
    OptOut,
    OptIn,
    Help,
}

/// Matches the whole message against the keywords, ignoring case, accents,
/// spaces and punctuation ("Stop.", "ARRÊT!", " aide " all match).
fn parse_keyword(message: &str) -> Option<Keyword> {
    let word: String = retirer_accents(&message.to_lowercase())
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    if OPT_OUT_KEYWORDS.contains(&word.as_str()) {
        Some(Keyword::OptOut)
    } else if OPT_IN_KEYWORDS.contains(&word.as_str()) {
        Some(Keyword::OptIn)
    } else if HELP_KEYWORDS.contains(&word.as_str()) {
        Some(Keyword::Help)
    } else {
        None
    }
}

/// Applies opt-out/opt-in keywords to the deny list and tells the caller what to do
/// with this inbound message.
/// A database error is returned to the caller, who should leave the message queued for a retry.
pub fn check_inbound(
    call_center_id: u64,
    caller_did: u64,
    message: &str,
) -> anyhow::Result<DenyCheck> {
    let db_manager = DbManager::new();
    let is_denied = || db_manager.is_did_denied(call_center_id, caller_did);
    Ok(match parse_keyword(message) {
        Some(Keyword::OptOut) => {
            let denied_by = format!("caller sms: {}", message.trim());
            let newly_denied = db_manager
                .deny_did(call_center_id, caller_did, &denied_by)?;
            let reply = OPT_OUT_REPLY;
            DenyCheck::OptOut {
                reply: newly_denied.then_some(reply),
            }
        }
        Some(Keyword::OptIn) => {
            let was_denied = db_manager
                .did_undeny(call_center_id, caller_did)?;
            if !was_denied {
                // A regular caller writing "start": just a normal message
                return Ok(DenyCheck::Allowed);
            }
            DenyCheck::OptIn {
                reply: OPT_IN_REPLY,
            }
        }
        Some(Keyword::Help) => DenyCheck::Help {
            denied: is_denied()?,
        },
        None => {
            if is_denied()? {
                DenyCheck::Denied
            } else {
                DenyCheck::Allowed
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_opt_out(message: &str) -> bool {
        matches!(parse_keyword(message), Some(Keyword::OptOut))
    }
    fn is_opt_in(message: &str) -> bool {
        matches!(parse_keyword(message), Some(Keyword::OptIn))
    }
    fn is_help(message: &str) -> bool {
        matches!(parse_keyword(message), Some(Keyword::Help))
    }

    #[test]
    fn test_keywords() {
        assert!(is_opt_out("STOP"));
        assert!(is_opt_out(" Stop. "));
        assert!(is_opt_out("Unsubscribe!"));
        assert!(is_opt_out("Opt out"));
        assert!(is_opt_out("REVOKE"));
        assert!(is_opt_out("ARRÊT"));
        assert!(is_opt_out("arrêter"));
        assert!(is_opt_in("start"));
        assert!(is_opt_in("UNSTOP"));
        assert!(is_opt_in("Reprendre"));
        assert!(is_help("Aide"));
        assert!(is_help("HELP!"));
        assert!(!is_opt_in("help"));
        assert!(!is_opt_out("please stop calling me"));
        assert!(parse_keyword("please stop calling me").is_none());
        assert!(parse_keyword("j'ai besoin d'aide").is_none());
        assert!(parse_keyword("").is_none());
    }

    #[test]
    fn test_sms_fit_in_one_message() {
        for text in [
            OPT_OUT_REPLY,
            OPT_IN_REPLY,
            RESPONDENT_OPT_OUT_REPLY,
        ] {
            assert!(
                text.chars().count() <= crate::voip_ms::SMS_MAX_CHARS,
                "{} chars: {text}",
                text.chars().count()
            );
        }
    }

    #[test]
    fn first_contact_notice_states_retention_in_two_sms_at_most() {
        let notice = first_contact_notice_text();
        assert!(notice.contains("7 days") && notice.contains("7 jours"));
        assert!(notice.chars().count() <= 2 * crate::voip_ms::SMS_MAX_CHARS);
    }
}
