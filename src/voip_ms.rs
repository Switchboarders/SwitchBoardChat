use crate::config::load_config;
use crate::voip_ms::reqwest::Response;
use crate::{config::Config, db_manager::DbManager};
use anyhow::{Error, anyhow};
use matrix_sdk::reqwest;
use serde_json::Value;
use tracing::{error, info};
use url::Url;

/// One HTTP client for every voip.ms call: connect 10 s, whole request 30 s
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|err| {
                error!("Could not build the voip.ms HTTP client with timeouts : {err}");
                reqwest::Client::new()
            })
    });
    &CLIENT
}

pub struct VoipMs {
    voip_ms_url: String,
    config: Config,
}

impl VoipMs {
    // ========================================
    // INITIALIZATION METHODS
    // ========================================

    /// Creates a new VoipMs instance with the default API URL
    pub fn new() -> Self {
        let voip_ms_url = String::from("https://voip.ms/api/v1/rest.php");
        let config = load_config();
        Self {
            voip_ms_url,
            config,
        }
    }

    // ========================================
    // UTILITY METHODS
    // ========================================

    /// Constructs a base URL with authentication and method parameters
    fn get_base_url(&self, id_call_center: u64, method: &str) -> anyhow::Result<Url> {
        let mut url = Url::parse(&self.voip_ms_url)?;
        let dbmanager = DbManager::new();
        let (voip_ms_username, voip_ms_api_password) = dbmanager
            .get_call_center_credentials(id_call_center)
            .map_err(|err| {
                anyhow!("No credentials for id_call_center {id_call_center} in table call_center : {err}")
            })?;

        url.query_pairs_mut()
            .append_pair("api_username", voip_ms_username.as_str())
            .append_pair("api_password", voip_ms_api_password.as_str())
            .append_pair("method", method);

        Ok(url)
    }

    // ========================================
    // CONNECTION AND TESTING METHODS
    // ========================================

    /// Tests the connection to VoIP.ms API by calling the getIP method
    pub async fn test_connection(&self, id_call_center: u64) -> anyhow::Result<bool> {
        let url = self.get_base_url(id_call_center, "getIP")?;

        let result = http_client().get(url).send().await;
        let res: Response;

        match result {
            Ok(ok) => {
                res = ok;
            }
            Err(err) => {
                // reqwest errors embed the request URL, which holds the voip.ms password
                let err = err.without_url();
                error!("Network error : {err}");
                return Err(anyhow!("Network error : {err}"));
            }
        }

        info!("Status: {}", res.status());
        let body = res.text().await?;

        let json: Value = serde_json::from_str(&body)?;
        if json["status"] != "success" {
            error!("Could not connect : status {:?}", json["status"]);
            return Ok(false);
        }
        Ok(true)
    }

    // ========================================
    // SMS MESSAGING METHODS
    // ========================================

    /// Sends an SMS message via VoIP.ms API. Messages longer than one SMS are split
    /// into several (voip.ms refuses them with "sms_toolong" otherwise).
    pub async fn send_sms(
        &self,
        id_call_center: u64,
        call_center_did: u64,
        caller_did: u64,
        message: String,
    ) -> anyhow::Result<()> {
        let parts = split_sms(&message);
        if parts.len() > 1 {
            info!("Message too long for one SMS, sending it in {} parts.", parts.len());
        }
        for part in parts {
            self.send_single_sms(id_call_center, call_center_did, caller_did, part)
                .await?;
        }
        Ok(())
    }

    async fn send_single_sms(
        &self,
        id_call_center: u64,
        call_center_did: u64,
        caller_did: u64,
        message: String,
    ) -> anyhow::Result<()> {
        let mut url = self.get_base_url(id_call_center, "sendSMS")?;
        url.query_pairs_mut()
            .append_pair("did", call_center_did.to_string().as_str())
            //.append_pair("did", "1234") // Error test
            .append_pair("dst", caller_did.to_string().as_str())
            .append_pair("message", message.as_str());

        // The queue (kept for retries) never stores the password: it is added back at resend
        let db_manager = DbManager::new();
        let id_to_voipms_queue =
            db_manager.add_to_voip_ms_queue_item(redact_password(&url), "Processing".to_string())?;

        // For developpement, you might want to mute voip.ms requests
        if self.config.mute_voipms_request {
            // Simulate an api call
            db_manager
                .change_processing_level_to_voip_ms_queue_item(
                    id_to_voipms_queue,
                    "Test mode".to_string(),
                    "Message not sent because the send_sms method is in test mode.".to_string(),
                )
                .unwrap_or_else(|err| { error!("Database error : {err}"); });

            let result = self.test_connection(1).await;
            match result {
                Ok(_ok) => {}
                Err(err) => {
                    error!("Test mode with no connection\n {:?}", err);
                    db_manager
                        .change_processing_level_to_voip_ms_queue_item(
                            id_to_voipms_queue,
                            "Test mode with no connection".to_string(),
                            format!("{:?}", err),
                        )
                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                    return Ok(());
                }
            }
            return Ok(());
        }
        // Here, were not in test mode and message will be sent !

        let result = http_client().get(url).send().await;
        let res: Response;
        match result {
            Ok(ok) => {
                res = ok;
            }
            Err(err) => {
                let err = err.without_url(); // the URL holds the password and the message
                error!("Network error\n {err}");
                db_manager
                    .change_processing_level_to_voip_ms_queue_item(
                        id_to_voipms_queue,
                        "Network error".to_string(),
                        err.to_string(),
                    )
                    .unwrap_or_else(|err| { error!("Database error : {err}"); });
                return Err(anyhow!("Network error{err}"));
            }
        }

        // TODO Monitor error here and also timeout
        info!("Status: {}", res.status());
        let body = res.text().await?;

        let json: Value = serde_json::from_str(&body)?;
        if json["status"] != "success" {
            // Only voip.ms's error code is kept: the full answer could echo the message
            let code = json["status"].to_string();
            error!("voip.ms refused the SMS: status {code}");
            db_manager.change_processing_level_to_voip_ms_queue_item(
                id_to_voipms_queue,
                "Voip.ms return error".to_string(),
                code,
            )?;
            return Err(anyhow!(
                "Could not connect to voip.ms: status {:?}",
                json["status"]
            ));
        }

        info!("SMS sent to caller {}.", crate::crypto::short_id(caller_did));
        db_manager.remove_to_voip_ms_queue_item(id_to_voipms_queue)?;
        Ok(())
    }
}

const PASSWORD_PLACEHOLDER: &str = "__stored_encrypted__";

/// The URL as stored in the retry queue: the voip.ms password is replaced by a placeholder
pub fn redact_password(url: &Url) -> String {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "api_password" { PASSWORD_PLACEHOLDER.to_string() } else { v.to_string() };
            (k.to_string(), v)
        })
        .collect();
    let mut clean = url.clone();
    clean.query_pairs_mut().clear().extend_pairs(pairs);
    clean.to_string()
}

/// Puts the real password back into a queued URL, ready to send
pub fn restore_password(stored: &str) -> anyhow::Result<Url> {
    let url = Url::parse(stored)?;
    let username = url
        .query_pairs()
        .find(|(k, _)| k == "api_username")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow!("queued url has no api_username"))?;
    // Old queue entries may still hold the real password: leave them as they are
    if !url.query_pairs().any(|(k, v)| k == "api_password" && v == PASSWORD_PLACEHOLDER) {
        return Ok(url);
    }
    let password = DbManager::new()
        .get_voip_ms_password_by_username(&username)?
        .ok_or_else(|| anyhow!("no voip.ms account for the queued url"))?;
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "api_password" { password.clone() } else { v.to_string() };
            (k.to_string(), v)
        })
        .collect();
    let mut restored = url.clone();
    restored.query_pairs_mut().clear().extend_pairs(pairs);
    Ok(restored)
}

/// Maximum length of one SMS accepted by voip.ms sendSMS
pub const SMS_MAX_CHARS: usize = 160;

/// Splits a message into SMS-sized parts, cutting between words when possible
fn split_sms(message: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest: Vec<char> = message.trim().chars().collect();
    while rest.len() > SMS_MAX_CHARS {
        let cut = rest[..=SMS_MAX_CHARS]
            .iter()
            .rposition(|c| c.is_whitespace())
            .filter(|&i| i > 0)
            .unwrap_or(SMS_MAX_CHARS);
        parts.push(rest[..cut].iter().collect::<String>().trim_end().to_string());
        rest = rest[cut..].iter().collect::<String>().trim_start().chars().collect();
    }
    if !rest.is_empty() || parts.is_empty() {
        parts.push(rest.into_iter().collect());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_urls_never_hold_the_password() {
        let mut url = Url::parse("https://voip.ms/api/v1/rest.php").unwrap();
        url.query_pairs_mut()
            .append_pair("api_username", "me@x.ca")
            .append_pair("api_password", "s3cret&odd")
            .append_pair("dst", "4185551234")
            .append_pair("message", "hello there");
        let stored = redact_password(&url);
        assert!(!stored.contains("s3cret") && stored.contains("dst=4185551234"));
        assert!(stored.contains("message=hello"));
    }

    #[test]
    fn test_split_sms() {
        assert_eq!(split_sms("Bonjour"), vec!["Bonjour"]);
        let exact: String = "é".repeat(SMS_MAX_CHARS);
        assert_eq!(split_sms(&exact), vec![exact.clone()]);

        let long = "mot ".repeat(100);
        let parts = split_sms(&long);
        assert!(parts.len() > 1);
        for part in &parts {
            assert!(part.chars().count() <= SMS_MAX_CHARS);
            assert!(!part.starts_with(' ') && !part.ends_with(' '));
        }
        assert_eq!(parts.join(" "), long.trim());

        // A single word longer than one SMS is cut anyway
        let word = "a".repeat(SMS_MAX_CHARS * 2 + 5);
        let parts = split_sms(&word);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.concat(), word);
    }
}
