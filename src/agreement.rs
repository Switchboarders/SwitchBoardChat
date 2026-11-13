//! Volunteers' confidentiality agreement, signed in Element (Quebec Law 25).
//!
//! In their user room ("User room / Salle Utilisateur") a volunteer writes `!entente` (or `!agreement`) to read the
//! agreement and `!signer Prénom Nom` (or `!sign Full Name`) to sign it. A signature is the person's own Matrix account
//! putting their typed name on the exact text, identified by its SHA-256 fingerprint (Civil Code of Québec art. 2827,
//! Act to establish a legal framework for information technology s. 39). The bridge records who, the typed name, the
//! server's timestamp, the device and whether the bridge verified it, the version and the fingerprint, and answers
//! with a receipt (the volunteer's copy). Any change to the text changes the fingerprint, so people sign again.
//!
//! Configuration: PRIVACY_OFFICER (required; how to reach the person in charge of the protection of personal information,
//! usually a generic email address; put into the text), AGREEMENT_VERSION (label, default below), AGREEMENT_FR_FILE / AGREEMENT_EN_FILE
//! (replace the built-in text). Unsigned volunteers are reminded once a day (main.rs).

use crate::db_manager::{AgreementSignature, DbManager};
use crate::rooms;
use crate::userroom::UserRoom;
use matrix_sdk::Client;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::ruma::UserId;
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

const DEFAULT_VERSION: &str = "2026-10-13";
const BUILTIN_FR: &str = include_str!("agreement/entente.fr.md");
const BUILTIN_EN: &str = include_str!("agreement/entente.en.md");

#[derive(Debug, Clone, PartialEq)]
pub struct Agreement {
    pub version: String,
    pub fr: String,
    pub en: String,
    /// SHA-256 (hex) of exactly what is shown to the volunteer (French, then English)
    pub sha256: String,
}

impl Agreement {
    fn build(version: &str, fr: &str, en: &str, officer: &str) -> Agreement {
        let fr = fr.trim().replace("{RESPONSABLE}", officer);
        let en = en.trim().replace("{RESPONSABLE}", officer);
        let sha256 = format!("{:x}", Sha256::digest(format!("{fr}\n\n{en}").as_bytes()));
        Agreement { version: version.to_string(), fr, en, sha256 }
    }

    /// The agreement in force, or None when PRIVACY_OFFICER is not set (the feature is then off).
    pub fn current() -> Option<Agreement> {
        let officer = std::env::var("PRIVACY_OFFICER").ok().filter(|v| !v.trim().is_empty())?;
        let read = |var: &str, builtin: &str| match std::env::var(var) {
            Ok(path) if !path.is_empty() => std::fs::read_to_string(&path).unwrap_or_else(|err| {
                error!("Agreement: cannot read {var}={path} ({err}); using the built-in text");
                builtin.to_string()
            }),
            _ => builtin.to_string(),
        };
        let version = std::env::var("AGREEMENT_VERSION").ok().filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_VERSION.to_string());
        Some(Self::build(&version, &read("AGREEMENT_FR_FILE", BUILTIN_FR), &read("AGREEMENT_EN_FILE", BUILTIN_EN), officer.trim()))
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Show,
    Sign(String),
    /// `!signer` without a usable name
    SignWithoutName,
}

/// Recognises the agreement commands (first word decides, case-insensitive).
pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    let (word, rest) = match text.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim()),
        None => (text, ""),
    };
    match word.to_lowercase().as_str() {
        "!entente" | "!agreement" => Some(Command::Show),
        "!signer" | "!sign" => {
            let name = rest.split_whitespace().collect::<Vec<_>>().join(" ");
            if name.chars().filter(|c| c.is_alphabetic()).count() >= 2 && name.chars().count() <= 120 {
                Some(Command::Sign(name))
            } else {
                Some(Command::SignWithoutName)
            }
        }
        _ => None,
    }
}

const NOT_CONFIGURED: &str = "L'entente de confidentialité n'est pas encore configurée. Avisez la coordination. / The confidentiality agreement is not configured yet. Please tell the coordinators.";

fn how_to_sign() -> String {
    "✍️ Pour signer, écrivez ici : !signer suivi de votre prénom et nom (par exemple : !signer Marie Tremblay).\n\
     ✍️ To sign, write here: !sign followed by your first and last name (for example: !sign Marie Tremblay)."
        .to_string()
}

fn format_time(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono_tz::America::Toronto.timestamp_opt(unix, 0).single() {
        Some(t) => t.format("%Y-%m-%d %H:%M (%Z)").to_string(),
        None => unix.to_string(),
    }
}

/// The messages that present the agreement (French text, English text, how to sign).
pub fn presentation(a: &Agreement) -> Vec<String> {
    vec![
        format!("📄 Version {} — empreinte / fingerprint {}\n\n{}", a.version, a.sha256, a.fr),
        a.en.clone(),
        how_to_sign(),
    ]
}

/// The receipt the volunteer keeps.
pub fn receipt(s: &AgreementSignature) -> String {
    let device = match (&s.device_id, s.device_verified) {
        (Some(d), true) => format!("{d} (vérifié / verified ✅)"),
        (Some(d), false) => format!("{d} (non vérifié / not verified)"),
        (None, _) => "inconnu / unknown".to_string(),
    };
    format!(
        "✅ Entente de confidentialité signée / Confidentiality agreement signed\n\
         Nom / Name : {}\n\
         Compte / Account : {}\n\
         Appareil / Device : {}\n\
         Date : {}\n\
         Version : {}\n\
         Empreinte du texte / Text fingerprint : {}\n\
         Référence / Reference : {}\n\
         Gardez ce message : c'est votre copie. / Keep this message: it is your copy.",
        s.typed_name,
        s.respondant_username,
        device,
        format_time(s.signed_at),
        s.version,
        s.text_sha256,
        s.event_id
    )
}

async fn say(client: &Client, room_id: &str, text: &str) {
    if let Err(err) = rooms::send_message_to_room_using_id(client.clone(), text, room_id).await {
        error!("Agreement: could not send a message to the user room: {err}");
    }
}

/// Handles `!entente` / `!signer` written by `sender` in their user room.
pub async fn handle(
    client: Client,
    room_id: String,
    sender: String,
    command: Command,
    event_id: String,
    server_ts_secs: i64,
    encryption_info: Option<EncryptionInfo>,
) {
    let Some(agreement) = Agreement::current() else {
        say(&client, &room_id, NOT_CONFIGURED).await;
        return;
    };
    let db = DbManager::new();
    match command {
        Command::Show => {
            for text in presentation(&agreement) {
                say(&client, &room_id, &text).await;
            }
        }
        Command::SignWithoutName => {
            say(&client, &room_id, &how_to_sign()).await;
        }
        Command::Sign(typed_name) => {
            match db.has_signed_agreement(&sender, &agreement.sha256) {
                Ok(true) => {
                    if let Ok(Some(previous)) = db.get_latest_agreement_signature(&sender) {
                        say(&client, &room_id, &format!("Vous avez déjà signé cette version de l'entente. / You already signed this version of the agreement.\n\n{}", receipt(&previous))).await;
                    }
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    error!("Agreement: database error: {err}");
                    say(&client, &room_id, "⚠️ La signature n'a pas pu être enregistrée. Réessayez plus tard. / The signature could not be recorded. Please try again later.").await;
                    return;
                }
            }
            let device_id = encryption_info.as_ref().and_then(|i| i.sender_device.as_ref()).map(|d| d.to_string());
            let mut device_verified = false;
            if let (Some(device_id), Ok(user_id)) = (&device_id, UserId::parse(sender.as_str())) {
                if let Ok(Some(device)) = client.encryption().get_device(&user_id, device_id.as_str().into()).await {
                    device_verified = device.is_verified();
                }
            }
            let signature = AgreementSignature {
                respondant_username: sender.clone(),
                typed_name,
                version: agreement.version.clone(),
                text_sha256: agreement.sha256.clone(),
                signed_at: server_ts_secs,
                event_id,
                device_id,
                device_verified,
            };
            match db.add_agreement_signature(&signature) {
                Ok(_) => {
                    info!("Agreement: {} signed version {} ({})", sender, agreement.version, &agreement.sha256[..12]);
                    say(&client, &room_id, &receipt(&signature)).await;
                }
                Err(err) => {
                    error!("Agreement: could not record the signature: {err}");
                    say(&client, &room_id, "⚠️ La signature n'a pas pu être enregistrée. Réessayez plus tard. / The signature could not be recorded. Please try again later.").await;
                }
            }
        }
    }
}

/// Posts the agreement in the user room of each volunteer in `users` who has not signed the current text.
/// Returns how many were sent. Used by the daily reminder and by SwitchBoard ("send now").
pub async fn send_to_unsigned(client: &Client, users: Vec<String>) -> usize {
    let Some(agreement) = Agreement::current() else {
        warn!("Agreement: PRIVACY_OFFICER is not set, the confidentiality agreement is off");
        return 0;
    };
    let db = DbManager::new();
    let user_room = UserRoom::new(client.clone());
    let mut sent = 0;
    for user in users {
        if matches!(db.has_signed_agreement(&user, &agreement.sha256), Ok(true)) || !user_room.is_user_valid(&user).await {
            continue;
        }
        let existing_room = match user_room.get_user_verification_room_if_exist(&user).await {
            Ok(existing) => existing,
            Err(err) => {
                error!("Agreement: database error while looking for the user room : {err}");
                continue;
            }
        };
        let room_id = match existing_room {
            Some(id) => id,
            None => match user_room.create_user_verification_room(&user).await {
                Ok(id) => id,
                Err(err) => {
                    error!("Agreement: no user room for {user}: {err}");
                    continue;
                }
            },
        };
        let intro = if matches!(db.get_latest_agreement_signature(&user), Ok(Some(_))) {
            "📄 L'entente de confidentialité a changé : merci de lire la nouvelle version et de la signer. / The confidentiality agreement changed: please read the new version and sign it."
        } else {
            "📄 Merci de lire et de signer l'entente de confidentialité des bénévoles (une seule fois, directement ici). / Please read and sign the volunteers' confidentiality agreement (only once, right here)."
        };
        say(client, &room_id, intro).await;
        for text in presentation(&agreement) {
            say(client, &room_id, &text).await;
        }
        sent += 1;
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse("!entente"), Some(Command::Show));
        assert_eq!(parse("  !AGREEMENT please"), Some(Command::Show));
        assert_eq!(parse("!signer   Marie-Ève   Tremblay "), Some(Command::Sign("Marie-Ève Tremblay".into())));
        assert_eq!(parse("!sign Jo Lee"), Some(Command::Sign("Jo Lee".into())));
        assert_eq!(parse("!signer"), Some(Command::SignWithoutName));
        assert_eq!(parse("!signer 12"), Some(Command::SignWithoutName));
        assert_eq!(parse("bonjour"), None);
        assert_eq!(parse("! signer Marie"), None);
    }

    #[test]
    fn fingerprint_follows_the_text() {
        let a = Agreement::build("v1", BUILTIN_FR, BUILTIN_EN, "Marie Tremblay, vie.privee@example.org");
        assert!(a.fr.contains("Marie Tremblay, vie.privee@example.org") && !a.fr.contains("{RESPONSABLE}"));
        assert!(!a.en.contains("{RESPONSABLE}"));
        assert_eq!(a.sha256.len(), 64);
        // Same text, same fingerprint; any change (even the officer) gives a new one, so people sign again
        assert_eq!(a.sha256, Agreement::build("v2", BUILTIN_FR, BUILTIN_EN, "Marie Tremblay, vie.privee@example.org").sha256);
        assert_ne!(a.sha256, Agreement::build("v1", BUILTIN_FR, BUILTIN_EN, "Autre Personne, x@example.org").sha256);
        assert_ne!(a.sha256, Agreement::build("v1", &BUILTIN_FR.replace("3 ans", "5 ans"), BUILTIN_EN, "Marie Tremblay, vie.privee@example.org").sha256);
    }

    #[test]
    fn receipt_names_everything_needed_as_proof() {
        let s = AgreementSignature {
            respondant_username: "@marie:example.org".into(),
            typed_name: "Marie Tremblay".into(),
            version: "2026-10-13".into(),
            text_sha256: "ab".repeat(32),
            signed_at: 1_791_900_000,
            event_id: "$evt".into(),
            device_id: Some("ABCDEF".into()),
            device_verified: true,
        };
        let r = receipt(&s);
        for part in ["Marie Tremblay", "@marie:example.org", "ABCDEF", "2026-10-13", &"ab".repeat(32), "$evt", "vérifié"] {
            assert!(r.contains(part), "missing {part}");
        }
    }
}
