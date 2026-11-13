//! Makes sure respondents notice a new conversation (web chat or SMS) and do not forget a waiting one.
//!
//! A Matrix invite alone often produces no notification (push registration of the app, client
//! settings, and later messages do not notify a user who has not joined). So the bridge:
//! 1. waits (briefly) for the invited respondents to join the new room (Synapse
//!    `auto_accept_invites` makes that instant), then posts a short bilingual message that
//!    **mentions** every joined respondent: a mention is a highlight, which notifies with sound even
//!    when the room is set to "mentions only";
//! 2. while a visitor/caller is **waiting** (a message arrived and no respondent has typed or written
//!    since), posts a reminder after 1, 2 and 5 minutes, then every hour.
//!
//! The messages contain no personal information. Settings: `ATTENTION_ENABLED` (default on),
//! `ATTENTION_JOIN_WAIT_SECS` (default 20), `ATTENTION_HOURLY_MAX` (default 24 hourly reminders).

use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::events::typing::SyncTypingEvent;
use matrix_sdk::{Client, Room, RoomMemberships};
use std::collections::HashMap;
use std::env;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Web,
    Sms,
}

struct Waiting {
    since: Instant,
    /// Reminders already sent for this wait
    sent: u32,
    /// The conversation was already answered: this is a follow-up message in a conversation someone is handling
    followup: bool,
}

static WAITING: LazyLock<Mutex<HashMap<String, Waiting>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Rooms where a respondent has already typed or written: their next waits are follow-ups, not new conversations.
/// (In memory: after a bridge restart, the first wait of an ongoing conversation is treated as new.)
static ANSWERED: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Minutes a follow-up message (in a conversation a respondent is already handling) may wait before the room is
/// reminded (`ATTENTION_FOLLOWUP_MIN`, default 10). The fast 1/2/5-minute reminders are only for unanswered
/// conversations: in a live exchange they pinged everyone whenever a reply took more than a minute.
fn followup_secs() -> u64 {
    env::var("ATTENTION_FOLLOWUP_MIN").ok().and_then(|v| v.parse::<u64>().ok()).filter(|m| *m > 0).unwrap_or(10) * 60
}

// ========================================
// SETTINGS
// ========================================

fn enabled() -> bool {
    !matches!(
        env::var("ATTENTION_ENABLED").unwrap_or_default().to_lowercase().as_str(),
        "false" | "0" | "off" | "no"
    )
}

fn join_wait() -> Duration {
    Duration::from_secs(
        env::var("ATTENTION_JOIN_WAIT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    )
}

fn hourly_max() -> u32 {
    env::var("ATTENTION_HOURLY_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24)
}

/// When reminder number `sent + 1` of a wait is due: the fast schedule for a conversation nobody has answered yet,
/// one follow-up reminder (then hourly) for a conversation a respondent is already handling.
fn due_secs_for(followup: bool, sent: u32, hourly_max: u32, followup_secs: u64) -> Option<u64> {
    if !followup {
        return due_secs(sent, hourly_max);
    }
    match sent {
        0 => Some(followup_secs),
        n if n <= hourly_max => Some(followup_secs + 3600 * n as u64),
        _ => None,
    }
}

/// Seconds after the start of the wait at which reminder number `sent + 1` is due:
/// 60 s, 120 s, 300 s, then 1 h, 2 h, 3 h … (at most `hourly_max` hourly reminders).
fn due_secs(sent: u32, hourly_max: u32) -> Option<u64> {
    match sent {
        0 => Some(60),
        1 => Some(120),
        2 => Some(300),
        n if n - 2 <= hourly_max => Some(3600 * (n - 2) as u64),
        _ => None,
    }
}

fn age_label(secs: u64) -> (String, String) {
    if secs >= 3600 {
        let h = secs / 3600;
        (
            format!("{h} heure{}", if h > 1 { "s" } else { "" }),
            format!("{h} hour{}", if h > 1 { "s" } else { "" }),
        )
    } else {
        let m = (secs / 60).max(1);
        (
            format!("{m} minute{}", if m > 1 { "s" } else { "" }),
            format!("{m} minute{}", if m > 1 { "s" } else { "" }),
        )
    }
}

// ========================================
// STATE (called from the web chat, the SMS flow and the Matrix handlers)
// ========================================

/// A visitor or caller message reached the room: respondents owe an answer.
/// An already running wait keeps its start time (no reminder storm).
pub fn inbound(room_id: &str) {
    if !enabled() {
        return;
    }
    WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(room_id.to_string())
        .or_insert(Waiting { since: Instant::now(), sent: 0, followup: ANSWERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(room_id) });
}

/// A respondent typed or wrote in the room: someone is on it.
pub fn respondent_active(room_id: &str) {
    WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(room_id);
    ANSWERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(room_id.to_string());
}

/// The room is gone (chat ended, deleted, expired).
pub fn forget(room_id: &str) {
    WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(room_id);
    ANSWERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(room_id);
    TYPING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(room_id);
}

/// Rooms where a respondent is typing right now, and when we last heard so. A typing notice expires on its
/// own on the Matrix side, so a missed "stopped typing" must not silence the reminders for long.
static TYPING: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const TYPING_STALE: Duration = Duration::from_secs(40);

fn set_typing(room_id: &str, typing: bool) {
    let mut map = TYPING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if typing {
        map.insert(room_id.to_string(), Instant::now());
    } else {
        map.remove(room_id);
    }
}

/// Someone (other than the bridge) is typing in the room: a reminder would only be noise.
pub fn typing_now(room_id: &str) -> bool {
    TYPING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(room_id)
        .is_some_and(|t| t.elapsed() < TYPING_STALE)
}

/// Matrix handler: who is typing in a room changed.
pub async fn on_typing(event: SyncTypingEvent, room: Room, client: Client) {
    let me = client.user_id();
    let someone = event.content.user_ids.iter().any(|u| Some(u.as_ref()) != me);
    set_typing(room.room_id().as_str(), someone);
    if someone {
        respondent_active(room.room_id().as_str());
    }
}

// ========================================
// MESSAGES
// ========================================

/// Most names written in one message; the rest are still mentioned, just not listed.
const MAX_NAMES: usize = 15;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A respondent's name: the display name, else `@localpart` of the Matrix id.
fn display_label(user_id: &str, display: Option<&str>) -> String {
    match display.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => d.to_string(),
        None => format!("@{}", user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)),
    }
}

/// The ping as (plain text, HTML): the message, then the joined respondents by name (clickable pills
/// in Element). Names are HTML-escaped; at most `MAX_NAMES` are listed.
fn compose_ping(text: &str, members: &[(String, Option<String>)]) -> (String, String) {
    if members.is_empty() {
        return (text.to_string(), html_escape(text));
    }
    let shown = &members[..members.len().min(MAX_NAMES)];
    let more = members.len() - shown.len();
    let mut plain = format!("{text}\n👥 ");
    let mut html = format!("{}<br>👥 ", html_escape(text));
    for (i, (id, display)) in shown.iter().enumerate() {
        let label = display_label(id, display.as_deref());
        if i > 0 {
            plain.push_str(", ");
            html.push_str(", ");
        }
        plain.push_str(&label);
        html.push_str(&format!("<a href=\"https://matrix.to/#/{}\">{}</a>", html_escape(id), html_escape(&label)));
    }
    if more > 0 {
        plain.push_str(&format!(" +{more}"));
        html.push_str(&format!(" +{more}"));
    }
    (plain, html)
}

/// Posts `text` in the room, mentioning every respondent who has joined it. With `only_if_joined`,
/// nothing is posted while nobody but the bot is in the room (a reminder nobody can be notified of).
/// Returns false when the room is unknown to this client.
async fn send_ping(client: &Client, room_id: &str, text: &str, only_if_joined: bool) -> bool {
    let Ok(parsed) = <&RoomId>::try_from(room_id) else {
        return false;
    };
    let Some(room) = client.get_room(parsed) else {
        return false;
    };
    let me = client.user_id().map(|u| u.to_owned());
    // (user id, display name) of every respondent who has joined
    let joined: Vec<(OwnedUserId, Option<String>)> = match room.members(RoomMemberships::JOIN).await {
        Ok(members) => members
            .into_iter()
            .filter(|m| Some(m.user_id()) != me.as_deref())
            .map(|m| (m.user_id().to_owned(), m.display_name().map(str::to_string)))
            .collect(),
        Err(err) => {
            warn!("Attention: could not list room members: {err}");
            Vec::new()
        }
    };
    if only_if_joined && joined.is_empty() {
        return true;
    }
    // Names in the text make the notification easier to spot (and match clients that look for the
    // display name); m.mentions makes it a highlight with sound.
    let named: Vec<(String, Option<String>)> = joined.iter().map(|(u, d)| (u.to_string(), d.clone())).collect();
    let (plain, html) = compose_ping(text, &named);
    let content = RoomMessageEventContent::text_html(plain, html)
        .add_mentions(Mentions::with_user_ids(joined.into_iter().map(|(u, _)| u)));
    match timeout(Duration::from_secs(10), room.send(content)).await {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            warn!("Attention: message not sent: {err}");
            true // the room exists; do not drop the wait for a transient error
        }
        Err(_) => {
            warn!("Attention: timed out sending a message");
            true
        }
    }
}

/// Right after a new room is created: wait for the invited respondents to join, then ping them.
/// Without auto-accepted invites nobody joins within the wait and the ping is posted anyway
/// (it will be seen when they accept the invitation).
pub async fn announce_new_room(client: Client, room_id: String, kind: Kind) {
    if !enabled() {
        return;
    }
    let deadline = Instant::now() + join_wait();
    if let Ok(parsed) = <&RoomId>::try_from(room_id.as_str()) {
        while Instant::now() < deadline {
            let joined = client.get_room(parsed).map(|r| r.joined_members_count()).unwrap_or(0);
            if joined >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let text = match kind {
        Kind::Web => "🔔 Nouveau clavardage Web en attente. / New web chat waiting.",
        Kind::Sms => "🔔 Nouvelle conversation SMS en attente. / New SMS conversation waiting.",
    };
    if typing_now(&room_id) {
        info!("Attention: a respondent is already typing, no ping needed");
        return;
    }
    info!("Attention: announcing a new {kind:?} room");
    send_ping(&client, &room_id, text, false).await;
}

/// Runs forever: posts the reminders that are due.
pub async fn run(client: Client) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !enabled() {
            continue;
        }
        let max = hourly_max();
        let follow = followup_secs();
        let due: Vec<(String, u64, bool)> = {
            let mut waiting = WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let mut out = Vec::new();
            for (room, w) in waiting.iter_mut() {
                let elapsed = now.duration_since(w.since).as_secs();
                if let Some(at) = due_secs_for(w.followup, w.sent, max, follow) {
                    // Someone is already typing an answer: no reminder now (it stays due for later)
                    if elapsed >= at && !typing_now(room) {
                        w.sent += 1;
                        out.push((room.clone(), elapsed, w.followup));
                    }
                }
            }
            out
        };
        for (room_id, elapsed, followup) in due {
            let (fr, en) = age_label(elapsed);
            let text = if followup {
                format!("⏰ Le dernier message attend une réponse depuis {fr}. / The last message has been waiting for an answer for {en}.")
            } else {
                format!("⏰ Personne n’a répondu depuis {fr}. / Nobody has answered for {en}.")
            };
            if !send_ping(&client, &room_id, &text, true).await {
                forget(&room_id); // the room no longer exists for this client
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_are_due_at_1_2_5_minutes_then_hourly() {
        assert_eq!(due_secs(0, 24), Some(60));
        assert_eq!(due_secs(1, 24), Some(120));
        assert_eq!(due_secs(2, 24), Some(300));
        assert_eq!(due_secs(3, 24), Some(3600));
        assert_eq!(due_secs(4, 24), Some(7200));
        assert_eq!(due_secs(26, 24), Some(24 * 3600)); // the 24th hourly reminder
        assert_eq!(due_secs(27, 24), None); // capped
        assert_eq!(due_secs(3, 0), None); // hourly reminders switched off
    }

    #[test]
    fn a_conversation_already_answered_gets_a_slow_follow_up_reminder() {
        let f = 600;
        assert_eq!(due_secs_for(false, 0, 24, f), Some(60), "new conversation: 1 minute");
        assert_eq!(due_secs_for(true, 0, 24, f), Some(600), "follow-up: 10 minutes, not 1");
        assert_eq!(due_secs_for(true, 1, 24, f), Some(600 + 3600));
        assert_eq!(due_secs_for(true, 25, 24, f), None);
        // the state: a reply marks the room as answered, the next inbound message is a follow-up
        inbound("!f:x");
        assert!(!WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)["!f:x"].followup);
        respondent_active("!f:x");
        inbound("!f:x");
        assert!(WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)["!f:x"].followup);
        forget("!f:x");
        inbound("!f:x");
        assert!(!WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)["!f:x"].followup, "a deleted room starts over");
        forget("!f:x");
    }

    #[test]
    fn typing_silences_a_room_until_it_stops() {
        assert!(!typing_now("!t:x"));
        set_typing("!t:x", true);
        assert!(typing_now("!t:x"));
        assert!(!typing_now("!other:x"));
        set_typing("!t:x", false);
        assert!(!typing_now("!t:x"));
    }

    #[test]
    fn the_ping_names_the_respondents() {
        let members = vec![
            ("@alice:example.org".to_string(), Some("Alice S.".to_string())),
            ("@bob:example.org".to_string(), None),
            ("@bad:example.org".to_string(), Some("<b>Eve & co</b>".to_string())),
            ("@blank:example.org".to_string(), Some("  ".to_string())),
        ];
        let (plain, html) = compose_ping("🔔 New chat.", &members);
        assert_eq!(plain, "🔔 New chat.\n👥 Alice S., @bob, <b>Eve & co</b>, @blank");
        assert!(html.starts_with("🔔 New chat.<br>👥 "));
        assert!(html.contains("<a href=\"https://matrix.to/#/@alice:example.org\">Alice S.</a>"));
        assert!(html.contains("<a href=\"https://matrix.to/#/@bob:example.org\">@bob</a>"));
        // a display name cannot inject markup
        assert!(html.contains("&lt;b&gt;Eve &amp; co&lt;/b&gt;") && !html.contains("<b>"));
    }

    #[test]
    fn nobody_joined_means_the_plain_message() {
        assert_eq!(compose_ping("hi", &[]), ("hi".to_string(), "hi".to_string()));
    }

    #[test]
    fn a_long_team_is_cut_with_a_count() {
        let members: Vec<(String, Option<String>)> =
            (0..20).map(|i| (format!("@u{i}:example.org"), None)).collect();
        let (plain, _) = compose_ping("x", &members);
        assert!(plain.ends_with("@u14 +5"));
        assert!(!plain.contains("@u15"));
    }

    #[test]
    fn age_is_written_in_both_languages() {
        assert_eq!(age_label(60), ("1 minute".to_string(), "1 minute".to_string()));
        assert_eq!(age_label(300), ("5 minutes".to_string(), "5 minutes".to_string()));
        assert_eq!(age_label(3600), ("1 heure".to_string(), "1 hour".to_string()));
        assert_eq!(age_label(7300), ("2 heures".to_string(), "2 hours".to_string()));
    }

    #[test]
    fn a_wait_starts_once_and_ends_with_respondent_activity() {
        let room = "!test-attention-1:example.org";
        inbound(room);
        let first = WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(room).map(|w| w.since).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        inbound(room); // a second message must not restart the clock
        assert_eq!(WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(room).map(|w| w.since), Some(first));
        respondent_active(room);
        assert!(WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(room).is_none());
        inbound(room); // a later message starts a new wait
        assert!(WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(room).is_some());
        forget(room);
        assert!(WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(room).is_none());
    }
}
