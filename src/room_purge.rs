//! Purges deleted rooms from Synapse (Quebec Law 25: erasure and retention).
//!
//! Leaving and forgetting a room is not enough: Synapse keeps its encrypted events, and the
//! bot's crypto store keeps the keys to read them. When it can, the bridge therefore asks
//! Synapse to delete the room and purge its history (`DELETE /_synapse/admin/v2/rooms/{id}`).
//!
//! That needs a Synapse admin token:
//! - with MAS (`MAS_ADMIN_CLIENT_ID` / `_SECRET` set): MAS refuses the Synapse admin scope to the
//!   admin client itself, so a personal session acting as the bot, with the Synapse admin scope,
//!   is created for each purge and revoked right after (it also expires on its own in 2 minutes).
//!   The bot's MAS user must be allowed to request admin (`mas-cli manage promote-admin`).
//! - without MAS: the bot's own token, which works when the bot user is a Synapse server admin.
//!
//! When neither works the room is only left and forgotten, and a warning says so.

use crate::mas_admin::MasAdmin;
use anyhow::{Result, anyhow};
use matrix_sdk::Client;
use matrix_sdk::reqwest;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};

/// Scopes of the per-purge MAS personal session
const PURGE_SCOPE: &str = "urn:matrix:client:api:* urn:synapse:admin:*";
/// Lifetime of that session's token, in seconds (revoked earlier when the purge is done)
const PURGE_TOKEN_SECONDS: u32 = 120;
/// How long to wait for Synapse to report the purge finished
const PURGE_WAIT: Duration = Duration::from_secs(90);

/// Time between removing the members and purging: respondents' clients must sync the removal first,
/// or the purged room stays in their room list (`ROOM_PURGE_DELAY_SECS`, default 120)
pub fn purge_delay() -> Duration {
    let secs = std::env::var("ROOM_PURGE_DELAY_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    Duration::from_secs(secs)
}

/// Warn only once when purging is not possible, not for every room
static UNAVAILABLE_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_unavailable(reason: &str) {
    if !UNAVAILABLE_WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            "Deleted rooms are left and forgotten but NOT purged from Synapse ({reason}): their encrypted \
             history stays in the Synapse database. See docs/law25.md (Purging rooms from Synapse)."
        );
    }
}

/// Where the admin token comes from, and how to give it back
enum AdminToken {
    /// MAS personal session: revoked after use
    Personal { mas: MasAdmin, session_id: String, token: String },
    /// The bot's own token (the bot is a Synapse admin, no MAS)
    Own(String),
}

impl AdminToken {
    fn token(&self) -> &str {
        match self {
            AdminToken::Personal { token, .. } | AdminToken::Own(token) => token,
        }
    }

    async fn release(self) {
        if let AdminToken::Personal { mas, session_id, .. } = self {
            if let Err(err) = mas.revoke_personal_session(&session_id).await {
                warn!("Room purge: could not revoke the temporary admin session (it expires in 2 minutes): {err}");
            }
        }
    }
}

async fn admin_token(client: &Client) -> Result<Option<AdminToken>> {
    match MasAdmin::connect(client).await? {
        Some(mas) => {
            let Some(bot) = client.user_id().map(|u| u.localpart().to_owned()) else {
                return Err(anyhow!("the bot is not logged in"));
            };
            let Some(user) = mas.find_user(&bot).await? else {
                return Err(anyhow!("the bot user was not found in MAS"));
            };
            let (session_id, token) = mas
                .create_personal_session(&user.id, "switchboard-chat room purge", PURGE_SCOPE, PURGE_TOKEN_SECONDS)
                .await?;
            Ok(Some(AdminToken::Personal { mas, session_id, token }))
        }
        None => {
            // Under MAS the bot's own (compatibility) token is never a Synapse admin token
            if client.oauth().server_metadata().await.is_ok() {
                return Ok(None);
            }
            Ok(client.access_token().map(AdminToken::Own))
        }
    }
}

/// Deletes the room on Synapse and purges its history. Returns Ok(false) when no admin token can be
/// had (purging not configured): the caller has already left and forgotten the room.
pub async fn purge_room(client: &Client, room_id: &str) -> Result<bool> {
    let token = match admin_token(client).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            warn_unavailable("no MAS admin client configured and the bot is not a Synapse admin");
            return Ok(false);
        }
        Err(err) => {
            warn_unavailable("the MAS admin client could not get a Synapse admin token");
            return Err(err);
        }
    };
    let result = delete_and_wait(client, token.token(), room_id).await;
    token.release().await;
    match result {
        Ok(true) => Ok(true),
        Ok(false) => {
            warn_unavailable("Synapse refused the admin call: the bot is not a server admin");
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

/// Ok(false) when Synapse answers 401/403 (not an admin)
async fn delete_and_wait(client: &Client, token: &str, room_id: &str) -> Result<bool> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let homeserver = client.homeserver();
    let mut url = homeserver.join("_synapse/admin/v2/rooms/")?;
    url.path_segments_mut()
        .map_err(|_| anyhow!("homeserver URL cannot be a base"))?
        .pop_if_empty()
        .push(room_id);
    let response = http
        .delete(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(json!({ "purge": true, "block": false }).to_string())
        .send()
        .await?;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_str(&response.text().await.unwrap_or_default()).unwrap_or(Value::Null);
    match status {
        200 => {}
        401 | 403 => return Ok(false),
        // Already gone (purged earlier, or never existed on this server)
        404 => return Ok(true),
        _ => return Err(anyhow!("Synapse admin API error {status}: {}", body["error"])),
    }
    let Some(delete_id) = body["delete_id"].as_str() else {
        return Err(anyhow!("Synapse did not return a delete_id"));
    };

    let mut status_url = homeserver.join("_synapse/admin/v2/rooms/delete_status/")?;
    status_url
        .path_segments_mut()
        .map_err(|_| anyhow!("homeserver URL cannot be a base"))?
        .pop_if_empty()
        .push(delete_id);
    let deadline = tokio::time::Instant::now() + PURGE_WAIT;
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let response = http.get(status_url.clone()).bearer_auth(token).send().await?;
        let body: Value = serde_json::from_str(&response.text().await.unwrap_or_default()).unwrap_or(Value::Null);
        match body["status"].as_str() {
            Some("complete") => {
                info!("Room purge: room deleted and purged from Synapse");
                return Ok(true);
            }
            Some("failed") => return Err(anyhow!("Synapse could not purge the room: {}", body["error"])),
            _ if tokio::time::Instant::now() >= deadline => {
                // Synapse keeps purging in the background; nothing more to do here
                warn!("Room purge: still running on Synapse after {}s", PURGE_WAIT.as_secs());
                return Ok(true);
            }
            _ => {}
        }
    }
}
