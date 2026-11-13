// Automatic SMS: messages the app sends on its own (no human pressed send).
// Every one of them goes through `send` so that an automated sender on the other
// end (auto-responder, another bot) can never ping-pong with us forever.
use crate::VoipMs;
use crate::db_manager::DbManager;
use chrono::Utc;
use tracing::{error, info};

#[derive(Debug, Clone, Copy)]
pub enum AutoSms {
    /// "No respondent available" macro
    NoRespondent,
    /// Welcome macro for a new caller
    Welcome,
    /// STOP / START confirmation
    DenyListConfirmation,
    /// Reply to HELP / AIDE
    Help,
    /// One-time "reply STOP to opt out" notice, when no other automatic SMS carried it
    FirstContactNotice,
}

impl AutoSms {
    fn kind(self) -> &'static str {
        match self {
            AutoSms::NoRespondent => "no_respondent",
            AutoSms::Welcome => "welcome",
            AutoSms::DenyListConfirmation => "deny_list_confirmation",
            AutoSms::Help => "help",
            AutoSms::FirstContactNotice => "first_contact_notice",
        }
    }

    /// (maximum count, per window in seconds), per caller and per call center
    fn limit(self) -> (u32, i64) {
        match self {
            AutoSms::NoRespondent => (1, 3600),
            AutoSms::Welcome => (1, 24 * 3600),
            AutoSms::DenyListConfirmation => (3, 24 * 3600),
            AutoSms::Help => (2, 3600),
            AutoSms::FirstContactNotice => (1, 24 * 3600),
        }
    }
}

/// Sends an automatic SMS unless the caller opted out (confirmations and HELP excepted)
/// or the rate limit for this kind is reached. The send is recorded *before* it
/// happens, so a failure or crash can never cause it to be resent in a loop.
/// Returns true if the SMS was handed to voip.ms (false if skipped).
pub async fn send(
    kind: AutoSms,
    call_center_id: u64,
    call_center_did: u64,
    caller_did: u64,
    message: String,
) -> bool {
    let db_manager = DbManager::new();
    // On a database error nothing is sent (we cannot check opt-out or the rate limit).
    let denied = if matches!(kind, AutoSms::DenyListConfirmation | AutoSms::Help) {
        false
    } else {
        match db_manager.is_did_denied(call_center_id, caller_did) {
            Ok(denied) => denied,
            Err(err) => {
                error!("Database error, automatic SMS {kind:?} not sent : {err}");
                return false;
            }
        }
    };
    if denied {
        info!("Automatic SMS {kind:?} not sent to {}: caller opted out.", crate::crypto::short_id(caller_did));
        return false;
    }

    let (max, window_secs) = kind.limit();
    let sent = match db_manager.count_auto_sms_since(
        call_center_id,
        caller_did,
        kind.kind(),
        Utc::now().timestamp() - window_secs,
    ) {
        Ok(sent) => sent,
        Err(err) => {
            error!("Database error, automatic SMS {kind:?} not sent : {err}");
            return false;
        }
    };
    if sent >= max {
        info!(
            "Automatic SMS {kind:?} not sent to {}: already sent {sent} in the last {}h.",
            crate::crypto::short_id(caller_did),
            window_secs / 3600
        );
        return false;
    }

    if let Err(err) = db_manager.log_auto_sms(call_center_id, caller_did, kind.kind()) {
        error!("Database error, automatic SMS {kind:?} not sent : {err}");
        return false;
    }
    match VoipMs::new()
        .send_sms(call_center_id, call_center_did, caller_did, message)
        .await
    {
        Ok(_) => info!("Automatic SMS {kind:?} sent to {}", crate::crypto::short_id(caller_did)),
        // Non critical error
        Err(err) => error!("Automatic SMS {kind:?} not sent to {} : {err}", crate::crypto::short_id(caller_did)),
    }
    true
}
