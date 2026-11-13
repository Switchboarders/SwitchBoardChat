//! Abuse controls for web chat sessions that do not rely on the visitor's IP address:
//!
//! 1. **Block marker.** When a respondent bans a visitor, the banned browser receives a signed
//!    "blocked until" marker (`issued.until.mac`) and keeps it; it sends the marker when it asks for a
//!    new session, and the server refuses it. The marker identifies nobody (it is just two dates and a
//!    signature) and is only ever created for a visitor who was banned. `web_unban all` revokes every
//!    marker issued before it.
//! 2. **Proof-of-work challenge.** Before a session is created the browser may have to find a nonce such
//!    that `SHA-256("<challenge>:<nonce>")` starts with N zero bits. It costs one visitor a moment and a
//!    script that opens many sessions a lot of computing time. The challenge is signed, tied to the
//!    requesting address, short-lived and single-use. It is required according to `WEB_CHALLENGE`:
//!    `auto` (default: only when there has been abuse lately or the address opens many chats),
//!    `always`, or `off`.
//!
//! 3. **Under-attack mode.** A distributed flood has one request per address, so per-address limits see
//!    nothing. The server therefore also counts *all* chat-start requests: above
//!    `WEB_ATTACK_STARTS_PER_MIN` (30) in a minute it switches to under-attack mode for
//!    `WEB_ATTACK_HOLD_MIN` (15) minutes after the last excess: the challenge is required from everyone, a
//!    little harder, and chats that never send a message are closed sooner.
//! 4. **Room cap.** At most `WEB_MAX_NEW_ROOMS_PER_MIN` (10) new Matrix rooms (hence new-chat pings to the
//!    respondents) per minute, whatever the number of visitors.
//!
//! Settings: `WEB_CHALLENGE`, `WEB_CHALLENGE_BASE_BITS` (12), `WEB_CHALLENGE_MAX_BITS` (18),
//! `WEB_CHALLENGE_WINDOW_HOURS` (6), `WEB_ATTACK_STARTS_PER_MIN`, `WEB_ATTACK_HOLD_MIN`,
//! `WEB_MAX_NEW_ROOMS_PER_MIN`.

use crate::crypto::{ct_eq, web_mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::sync::{LazyLock, Mutex};

/// How long a challenge can be solved, in seconds
const CHALLENGE_LIFETIME: i64 = 180;

// ========================================
// BLOCK MARKER
// ========================================

pub fn make_marker(issued: i64, until: i64) -> String {
    format!("{issued}.{until}.{}", web_mac(&format!("ban:{issued}:{until}")))
}

/// True if the marker is genuine, still running at `now`, and issued after `valid_after`
/// (the last time an administrator revoked every marker).
pub fn marker_valid(marker: &str, now: i64, valid_after: i64) -> bool {
    let mut parts = marker.splitn(3, '.');
    let (Some(issued), Some(until), Some(mac)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(issued_n), Ok(until_n)) = (issued.parse::<i64>(), until.parse::<i64>()) else {
        return false;
    };
    ct_eq(mac, &web_mac(&format!("ban:{issued}:{until}"))) && until_n > now && issued_n >= valid_after
}

// ========================================
// PROOF OF WORK
// ========================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChallengeMode {
    Off,
    Auto,
    Always,
}

pub fn challenge_mode() -> ChallengeMode {
    match env::var("WEB_CHALLENGE").unwrap_or_default().trim().to_lowercase().as_str() {
        "off" | "false" | "0" | "no" => ChallengeMode::Off,
        "always" | "on" | "true" | "1" => ChallengeMode::Always,
        _ => ChallengeMode::Auto,
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Window in which recent bans raise the difficulty (`WEB_CHALLENGE_WINDOW_HOURS`, default 6)
pub fn window_secs() -> i64 {
    env_u32("WEB_CHALLENGE_WINDOW_HOURS", 6).max(1) as i64 * 3600
}

/// Zero bits required, or None when no challenge is needed. Difficulty grows by 2 bits for each recent
/// ban and for each chat this address opened in the last hour, and by 4 while the whole service is under
/// attack, between the base and the maximum (each extra bit doubles the work; 12 bits is instant, 18 bits
/// takes a couple of seconds on a computer and several on a phone).
pub fn challenge_bits(mode: ChallengeMode, recent_bans: i64, ip_sessions: usize, attack: bool) -> Option<u32> {
    let needed = match mode {
        ChallengeMode::Off => false,
        ChallengeMode::Always => true,
        ChallengeMode::Auto => recent_bans > 0 || ip_sessions >= 2 || attack,
    };
    if !needed {
        return None;
    }
    let max = env_u32("WEB_CHALLENGE_MAX_BITS", 18).clamp(8, 28);
    let base = env_u32("WEB_CHALLENGE_BASE_BITS", 12).clamp(4, max);
    let extra = 2 * (recent_bans.clamp(0, 10) as u32) + 2 * (ip_sessions.min(10) as u32) + if attack { 4 } else { 0 };
    Some((base + extra).min(max))
}

// ========================================
// GLOBAL PRESSURE (under-attack mode, room cap)
// ========================================

/// Counts events per second over the last 60 seconds in a fixed-size ring: constant memory, however
/// large the flood.
pub struct RateWindow {
    buckets: [(u64, u32); 60],
}

impl RateWindow {
    pub const fn new() -> Self {
        Self { buckets: [(0, 0); 60] }
    }

    /// Events in the 60 seconds up to and including `sec`
    pub fn count(&self, sec: u64) -> u32 {
        self.buckets
            .iter()
            .filter(|(s, _)| *s != 0 && sec.saturating_sub(*s) < 60 && *s <= sec)
            .map(|(_, c)| *c)
            .sum()
    }

    /// Records one event at `sec` and returns the count over the last 60 seconds
    pub fn hit(&mut self, sec: u64) -> u32 {
        let slot = &mut self.buckets[(sec % 60) as usize];
        if slot.0 != sec {
            *slot = (sec, 0);
        }
        slot.1 = slot.1.saturating_add(1);
        self.count(sec)
    }
}

static START_REQUESTS: Mutex<RateWindow> = Mutex::new(RateWindow::new());
static ATTACK_UNTIL: Mutex<u64> = Mutex::new(0);
static NEW_ROOMS: Mutex<RateWindow> = Mutex::new(RateWindow::new());

fn attack_threshold() -> u32 {
    env_u32("WEB_ATTACK_STARTS_PER_MIN", 30).max(1)
}

fn attack_hold_secs() -> u64 {
    env_u32("WEB_ATTACK_HOLD_MIN", 15).max(1) as u64 * 60
}

/// Chat-start requests allowed per minute in total before the under-attack mode starts
/// (decision part, testable without globals): returns the new "attack until" second.
fn attack_until_after(count: u32, threshold: u32, now_sec: u64, hold_secs: u64, current: u64) -> u64 {
    if count > threshold { now_sec + hold_secs } else { current }
}

/// Called for every chat-start request. Returns true while the service is under attack.
pub fn note_start_request(now_sec: u64) -> bool {
    let count = START_REQUESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).hit(now_sec);
    let mut until = ATTACK_UNTIL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let was = *until > now_sec;
    *until = attack_until_after(count, attack_threshold(), now_sec, attack_hold_secs(), *until);
    if !was && *until > now_sec {
        tracing::warn!("Web chat: under attack mode ON ({count} chat starts in the last minute)");
    }
    *until > now_sec
}

pub fn under_attack(now_sec: u64) -> bool {
    *ATTACK_UNTIL.lock().unwrap_or_else(std::sync::PoisonError::into_inner) > now_sec
}

/// Room cap: true (and counted) if fewer than `cap` rooms were created in the last minute
pub fn allow_new_room(now_sec: u64, cap: u32) -> bool {
    let mut w = NEW_ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if w.count(now_sec) >= cap {
        return false;
    }
    w.hit(now_sec);
    true
}

pub fn max_new_rooms_per_min() -> u32 {
    env_u32("WEB_MAX_NEW_ROOMS_PER_MIN", 10).max(1)
}

/// Minutes a chat may stay open without a single message before it is closed (shorter under attack)
pub fn no_message_timeout_secs(attack: bool) -> u64 {
    let minutes = env_u32("WEB_NO_MESSAGE_TIMEOUT_MIN", 10).max(1) as u64;
    if attack { minutes.min(2) * 60 } else { minutes * 60 }
}

pub fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut bits = 0;
    for b in bytes {
        if *b == 0 {
            bits += 8;
        } else {
            bits += b.leading_zeros();
            break;
        }
    }
    bits
}

/// A challenge for `ip_key` (see `web_chat::ip_key`): `expires.salt.bits.mac`
pub fn make_challenge(ip_key: &str, bits: u32, now: i64) -> String {
    let expires = now + CHALLENGE_LIFETIME;
    let salt = crate::crypto::generate_token(16);
    format!("{expires}.{salt}.{bits}.{}", web_mac(&format!("pow:{expires}:{salt}:{bits}:{ip_key}")))
}

static USED_CHALLENGES: LazyLock<Mutex<HashMap<String, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Checks a solution: genuine, for this address, not expired, hard enough (`needed_bits`), never used
/// before, and `SHA-256("<challenge>:<nonce>")` really has enough leading zero bits. Consumes the challenge.
pub fn verify_solution(challenge: &str, nonce: &str, ip_key: &str, needed_bits: u32, now: i64) -> bool {
    if nonce.is_empty() || nonce.len() > 24 || challenge.len() > 200 {
        return false;
    }
    let mut parts = challenge.splitn(4, '.');
    let (Some(expires), Some(salt), Some(bits), Some(mac)) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Ok(expires_n), Ok(bits_n)) = (expires.parse::<i64>(), bits.parse::<u32>()) else {
        return false;
    };
    if expires_n <= now || bits_n < needed_bits || bits_n > 40 {
        return false;
    }
    if !ct_eq(mac, &web_mac(&format!("pow:{expires}:{salt}:{bits}:{ip_key}"))) {
        return false;
    }
    let digest = Sha256::digest(format!("{challenge}:{nonce}").as_bytes());
    if leading_zero_bits(&digest) < bits_n {
        return false;
    }
    let mut used = USED_CHALLENGES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    used.retain(|_, e| *e > now);
    used.insert(salt.to_string(), expires_n).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::init_test_env;

    fn solve(challenge: &str, bits: u32) -> String {
        (0u64..)
            .map(|n| n.to_string())
            .find(|n| leading_zero_bits(&Sha256::digest(format!("{challenge}:{n}").as_bytes())) >= bits)
            .unwrap()
    }

    #[test]
    fn a_block_marker_is_genuine_and_expires() {
        init_test_env();
        let m = make_marker(1000, 5000);
        assert!(marker_valid(&m, 2000, 0));
        assert!(!marker_valid(&m, 5000, 0)); // over
        assert!(!marker_valid(&m, 2000, 1001)); // revoked by a later "unban all"
        assert!(marker_valid(&m, 2000, 1000));
        // forged: changed end date, changed signature, garbage
        let forged = m.replace(".5000.", ".9999999999.");
        assert!(!marker_valid(&forged, 2000, 0));
        let mut bad_sig = m.clone();
        bad_sig.pop();
        bad_sig.push('0');
        assert!(!marker_valid(&bad_sig, 2000, 0) || bad_sig == m);
        for junk in ["", "a.b.c", "1.2", "x.y.z", "1.2.3.4"] {
            assert!(!marker_valid(junk, 0, 0), "{junk}");
        }
    }

    #[test]
    fn leading_zero_bits_counts_bits() {
        assert_eq!(leading_zero_bits(&[0xff]), 0);
        assert_eq!(leading_zero_bits(&[0x0f]), 4);
        assert_eq!(leading_zero_bits(&[0x00, 0x80]), 8);
        assert_eq!(leading_zero_bits(&[0x00, 0x00, 0x01]), 23);
        assert_eq!(leading_zero_bits(&[0, 0]), 16);
    }

    #[test]
    fn difficulty_follows_the_abuse_pressure() {
        // defaults: base 12, max 18 (the test does not set the env variables)
        assert_eq!(challenge_bits(ChallengeMode::Off, 5, 5, true), None);
        assert_eq!(challenge_bits(ChallengeMode::Auto, 0, 0, false), None); // calm: nothing
        assert_eq!(challenge_bits(ChallengeMode::Auto, 0, 1, false), None); // a first chat from an address
        assert_eq!(challenge_bits(ChallengeMode::Auto, 0, 2, false), Some(16)); // refreshing
        assert_eq!(challenge_bits(ChallengeMode::Auto, 1, 0, false), Some(14)); // a recent ban
        assert_eq!(challenge_bits(ChallengeMode::Auto, 0, 0, true), Some(16)); // the whole service is under attack
        assert_eq!(challenge_bits(ChallengeMode::Auto, 9, 9, false), Some(18)); // capped
        assert_eq!(challenge_bits(ChallengeMode::Always, 0, 0, false), Some(12));
    }

    #[test]
    fn the_rate_window_counts_the_last_minute_in_constant_memory() {
        let mut w = RateWindow::new();
        assert_eq!(w.count(1000), 0);
        for _ in 0..5 {
            w.hit(1000);
        }
        assert_eq!(w.hit(1000), 6);
        assert_eq!(w.hit(1010), 7);
        assert_eq!(w.count(1059), 7); // still inside the minute
        assert_eq!(w.count(1060), 1); // 1000 has aged out, 1010 remains
        assert_eq!(w.count(1070), 0);
        // a bucket reused a minute later starts from zero
        assert_eq!(w.hit(1060), 2);
        assert_eq!(w.count(1060), 2);
        for i in 0..100_000 {
            w.hit(2000 + (i % 3));
        }
        assert_eq!(w.count(2002), 100_000); // counts without storing events
    }

    #[test]
    fn the_attack_mode_starts_above_the_threshold_and_holds() {
        assert_eq!(attack_until_after(30, 30, 1000, 900, 0), 0); // at the threshold: calm
        assert_eq!(attack_until_after(31, 30, 1000, 900, 0), 1900); // above: attack for 15 min
        assert_eq!(attack_until_after(5, 30, 1500, 900, 1900), 1900); // calm again, but the hold continues
        assert_eq!(attack_until_after(40, 30, 1800, 900, 1900), 2700); // renewed by new excess
    }

    #[test]
    fn the_room_cap_limits_new_rooms_per_minute() {
        // distinct seconds far from other tests; the cap applies to the global window
        let base = 5_000_000;
        assert!(allow_new_room(base, 2));
        assert!(allow_new_room(base, 2));
        assert!(!allow_new_room(base + 1, 2)); // third in the same minute
        assert!(allow_new_room(base + 61, 2)); // a minute later
    }

    #[test]
    fn chats_without_a_message_are_closed_sooner_under_attack() {
        assert_eq!(no_message_timeout_secs(false), 600);
        assert_eq!(no_message_timeout_secs(true), 120);
    }

    #[test]
    fn a_solved_challenge_is_accepted_once() {
        init_test_env();
        let now = 1_000_000;
        let c = make_challenge("203.0.113.9", 8, now);
        let nonce = solve(&c, 8);
        assert!(verify_solution(&c, &nonce, "203.0.113.9", 8, now));
        assert!(!verify_solution(&c, &nonce, "203.0.113.9", 8, now), "replay must fail");
    }

    #[test]
    fn a_challenge_is_bound_to_address_time_and_difficulty() {
        init_test_env();
        let now = 2_000_000;
        let c = make_challenge("203.0.113.9", 8, now);
        let nonce = solve(&c, 8);
        assert!(!verify_solution(&c, &nonce, "203.0.113.10", 8, now), "another address");
        assert!(!verify_solution(&c, &nonce, "203.0.113.9", 8, now + CHALLENGE_LIFETIME + 1), "expired");
        assert!(!verify_solution(&c, &nonce, "203.0.113.9", 12, now), "easier than currently required");
        assert!(!verify_solution(&c, "0", "203.0.113.9", 8, now) || leading_zero_bits(&Sha256::digest(format!("{c}:0").as_bytes())) >= 8);
        let forged = c.replacen(".8.", ".1.", 1);
        assert!(!verify_solution(&forged, &nonce, "203.0.113.9", 1, now), "bits changed");
        assert!(!verify_solution("", "1", "x", 1, now));
        assert!(!verify_solution(&c, "", "203.0.113.9", 8, now));
        // a wrong nonce is refused (and does not burn the challenge)
        let c2 = make_challenge("203.0.113.9", 12, now);
        let wrong = (0u64..).map(|n| n.to_string()).find(|n| leading_zero_bits(&Sha256::digest(format!("{c2}:{n}").as_bytes())) < 12).unwrap();
        assert!(!verify_solution(&c2, &wrong, "203.0.113.9", 12, now));
        assert!(verify_solution(&c2, &solve(&c2, 12), "203.0.113.9", 12, now));
    }
}
