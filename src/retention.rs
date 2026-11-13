//! Retention and erasure for SMS conversations (Quebec Law 25).
//!
//! An SMS room (and the caller's encrypted number) is deleted after `SMS_IDLE_RETENTION_DAYS`
//! (default 7) without a text from the caller. A returning caller simply gets a new room.
//! `erase_caller` is the right to erasure: callable through the admin API (`erase_caller`)
//! or by a respondent typing `!oubli` / `!forget` in the room.

use crate::db_manager::DbManager;
use crate::{rooms, web_chat};
use matrix_sdk::Client;
use std::env;
use std::time::Duration;
use tracing::{error, info};

pub const ROOM_NOTICE_ERASED: &str = "Conversation effacée à la demande. Le robot quitte la salle : supprimez-la aussi de votre côté. / Conversation erased on request. The bot is leaving: please delete the room on your side too.";
pub const ROOM_NOTICE_EXPIRED: &str = "Conversation expirée et supprimée (période de conservation terminée). Supprimez la salle de votre côté. / Conversation expired and deleted (retention period over). Please delete the room on your side too.";

pub fn sms_retention_days() -> u64 {
    env::var("SMS_IDLE_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|d| *d > 0)
        .unwrap_or(7)
}

/// Leaves and forgets every SMS room idle for longer than the retention period.
pub async fn purge_idle_sms_rooms(client: &Client) {
    let days = sms_retention_days();
    match DbManager::new().fetch_idle_sms_room_ids(days) {
        Ok(ids) => {
            for id in ids {
                info!("Retention: SMS room idle for more than {days} days, deleting");
                if let Err(err) =
                    rooms::send_message_to_room_using_id(client.clone(), ROOM_NOTICE_EXPIRED, &id).await
                {
                    error!("Retention: notice not sent: {err}");
                }
                web_chat::forget_room(client, &id).await;
            }
        }
        Err(err) => error!("Retention: could not list idle rooms: {err}"),
    }
}

/// Runs forever, once an hour.
pub async fn run(client: Client) {
    // Let the first Matrix sync finish so the rooms are known to the client
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        purge_idle_sms_rooms(&client).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

/// Right to erasure for one caller of one call center. Returns how many rooms were deleted.
/// The opt-out entry (hash only) is kept on purpose.
pub async fn erase_caller(client: &Client, call_center_id: u64, did: u64) -> anyhow::Result<usize> {
    let room_ids = DbManager::new().erase_caller(call_center_id, did)?;
    for id in &room_ids {
        if let Err(err) = rooms::send_message_to_room_using_id(client.clone(), ROOM_NOTICE_ERASED, id).await {
            error!("Erasure: notice not sent: {err}");
        }
        web_chat::forget_room(client, id).await;
    }
    info!(
        "Erasure: caller {} erased from call center {call_center_id} ({} rooms)",
        crate::crypto::short_id(did),
        room_ids.len()
    );
    Ok(room_ids.len())
}
