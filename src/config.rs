use std::env;

#[derive(Clone)]
pub struct Config {
    pub phone_hash_secret: String,
    pub homeserver_url: String,
    pub matrix_username: String,
    pub matrix_password: String,
    pub mute_voipms_request: bool,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("phone_hash_secret", &"<redacted>")
            .field("homeserver_url", &self.homeserver_url)
            .field("matrix_username", &"<redacted>")
            .field("matrix_password", &"<redacted>")
            .field("mute_voipms_request", &self.mute_voipms_request)
            .finish()
    }
}
/// Values shipped in `.env-example` or commonly left in place: never accepted as secrets
const PLACEHOLDERS: &[&str] = &[
    "anysecretyouwant",
    "myuserpassword",
    "changeme",
    "change-me",
    "secret",
    "password",
    "generate-with-openssl-rand-hex-32",
];

/// A secret the bridge refuses to start with
fn is_placeholder(value: &str) -> bool {
    let v = value.trim().to_lowercase();
    v.is_empty() || PLACEHOLDERS.contains(&v.as_str())
}

/// Checks the secrets before anything else runs. Err = refuse to start (the message names the
/// variable, never its value); the Vec holds warnings for secrets that work but are weak.
/// Length limits are loose on purpose for existing installs: changing `PHONE_HASH_SECRET` makes
/// the opt-out list forget everyone, so a short one is only a warning above 16 characters.
pub fn check_secrets(get: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    for name in ["PHONE_HASH_SECRET", "MATRIX_PASSWORD", "PHONE_ENC_KEY"] {
        if get(name).as_deref().is_none_or(is_placeholder) {
            return Err(format!(
                "{name} is missing or still a placeholder: generate one with `openssl rand -hex 32` (MATRIX_PASSWORD: the bot account's real password)"
            ));
        }
    }
    let hash_len = get("PHONE_HASH_SECRET").unwrap_or_default().trim().chars().count();
    if hash_len < 16 {
        return Err("PHONE_HASH_SECRET is too short (at least 16 characters, 64 recommended: `openssl rand -hex 32`)".into());
    }
    if hash_len < 32 {
        warnings.push("PHONE_HASH_SECRET is shorter than 32 characters: use `openssl rand -hex 32` for new installations".into());
    }
    if get("MATRIX_PASSWORD").unwrap_or_default().chars().count() < 12 {
        warnings.push("MATRIX_PASSWORD is shorter than 12 characters: give the bot account a long random password".into());
    }
    if let Some(mas) = get("MAS_ADMIN_CLIENT_SECRET").filter(|v| !v.trim().is_empty()) {
        if is_placeholder(&mas) {
            return Err("MAS_ADMIN_CLIENT_SECRET is a placeholder".into());
        }
        if mas.trim().chars().count() < 16 {
            warnings.push("MAS_ADMIN_CLIENT_SECRET is shorter than 16 characters".into());
        }
    }
    Ok(warnings)
}

pub fn load_config() -> Config {
    let mute_voipms_request = match env::var("MUTE_VOIPMS_REQUEST") {
        Ok(ok) => ok.to_lowercase() == "true",
        Err(_err) => false,
    };
    Config {
        phone_hash_secret: env::var("PHONE_HASH_SECRET").expect("PHONE_HASH_SECRET must be set"),
        homeserver_url: env::var("HOMESERVER_URL").expect("HOMESERVER_URL must be set"),
        matrix_username: env::var("MATRIX_USERNAME").expect("MATRIX_USERNAME must be set"),
        matrix_password: env::var("MATRIX_PASSWORD").expect("MATRIX_PASSWORD must be set"),
        mute_voipms_request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn placeholder_or_missing_secrets_refuse_to_start() {
        let good_key = "a".repeat(64);
        let good_hash = "h".repeat(40);
        let ok = [("PHONE_HASH_SECRET", good_hash.as_str()), ("MATRIX_PASSWORD", "a-long-bot-password"), ("PHONE_ENC_KEY", good_key.as_str())];
        assert_eq!(check_secrets(env_of(&ok)), Ok(vec![]));
        let placeholder = [("PHONE_HASH_SECRET", "anysecretyouwant"), ok[1], ok[2]];
        assert!(check_secrets(env_of(&placeholder)).unwrap_err().contains("PHONE_HASH_SECRET"));
        let bad_password = [ok[0], ("MATRIX_PASSWORD", "myuserpassword"), ok[2]];
        assert!(check_secrets(env_of(&bad_password)).is_err());
        assert!(check_secrets(env_of(&[ok[0], ok[1]])).unwrap_err().contains("PHONE_ENC_KEY"));
        let short = [("PHONE_HASH_SECRET", "short-secret"), ok[1], ok[2]];
        assert!(check_secrets(env_of(&short)).is_err());
        let weak = [("PHONE_HASH_SECRET", "twenty-chars-secret!"), ("MATRIX_PASSWORD", "pw-e2e-1234"), ok[2]];
        assert_eq!(check_secrets(env_of(&weak)).unwrap().len(), 2);
        let mas = [ok[0], ok[1], ok[2], ("MAS_ADMIN_CLIENT_SECRET", "changeme")];
        assert!(check_secrets(env_of(&mas)).is_err());
    }

    #[test]
    fn debug_output_hides_secrets() {
        let config = Config {
            phone_hash_secret: "hash-secret-value".into(),
            homeserver_url: "https://matrix.example.org".into(),
            matrix_username: "bot-user".into(),
            matrix_password: "hunter2-password".into(),
            mute_voipms_request: false,
        };
        let shown = format!("{config:?}");
        for secret in ["hash-secret-value", "hunter2-password", "bot-user"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("matrix.example.org"));
    }
}
