use crate::DbManager;
use crate::toqueuemanager::reqwest::Response;
use chrono::DateTime;
use chrono::Local;
use chrono::NaiveDateTime;
use chrono::Utc;
use matrix_sdk::reqwest;
use serde_json::Value;
use tokio::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

pub struct ToQueueManager {}

pub struct ToQueueManagerEntry {
    pub id_to_voipms_queue: u64,
    pub send_message_url: String,
    pub timestamp: String,
    pub processing_level: String,
    pub error_log: String,
}

impl std::fmt::Debug for ToQueueManagerEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToQueueManagerEntry")
            .field("id_to_voipms_queue", &self.id_to_voipms_queue)
            .field("send_message_url", &"<redacted>")
            .field("timestamp", &self.timestamp)
            .field("processing_level", &self.processing_level)
            .field("error_log", &"<redacted>")
            .finish()
    }
}

impl ToQueueManagerEntry {
    // ================================
    // UTILITY METHODS
    // ================================

    pub fn get_timestamp(&self) -> anyhow::Result<NaiveDateTime> {
        let timestamp = NaiveDateTime::parse_from_str(&self.timestamp, "%Y-%m-%d %H:%M:%S")?;
        return Ok(timestamp);
    }
}

const MAX_RESEND_ATTEMPTS: u32 = 10;
const MAX_RESEND_AGE_SECS: i64 = 3600;
const RESEND_ATTEMPT_PREFIX: &str = "Resend attempt ";
const GAVE_UP_ATTEMPTS: &str = "Gave up: too many resend attempts";
const GAVE_UP_TOO_OLD: &str = "Gave up: message too old to resend";
const CANCELLED_OPTED_OUT: &str = "Cancelled: caller opted out";

/// Number of resend attempts already made, from a "Resend attempt N" processing level
fn resend_attempts(processing_level: &str) -> u32 {
    processing_level
        .strip_prefix(RESEND_ATTEMPT_PREFIX)
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Some(true) if the queued voip.ms URL targets a caller who opted out (STOP); None on a database error
fn is_to_opted_out_caller(send_message_url: &str) -> Option<bool> {
    let Ok(url) = url::Url::parse(send_message_url) else {
        return Some(false);
    };
    let param = |name: &str| -> Option<u64> {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.parse().ok())
    };
    let (Some(call_center_did), Some(caller_did)) = (param("did"), param("dst")) else {
        return Some(false);
    };
    match DbManager::new().is_did_denied_by_call_center_did(call_center_did, caller_did) {
        Ok(denied) => Some(denied),
        Err(err) => {
            error!("Database error while checking the deny list : {err}");
            None
        }
    }
}

//id_to_voipms_queue,send_message_url,timestamp,processing_level,error_log
impl ToQueueManager {
    // ================================
    // MAIN PROCESSING LOOP
    // ================================

    pub async fn loop_queue(&self) -> anyhow::Result<()> {
        // Change the processing level of older in process queue item to "Reprocessing"
        self.review_processing_state().await;
        let do_not_resend = vec![
            "Processing",
            "Voip.ms return error",
            "Test mode",
            GAVE_UP_ATTEMPTS,
            GAVE_UP_TOO_OLD,
            CANCELLED_OPTED_OUT,
        ];
        let db_manager = DbManager::new();
        loop {
            // Queued SMS are only kept to retry: nothing outlives MAX_RESEND_AGE_SECS
            // (inbound texts waiting for a respondent are kept 24 hours)
            if let Err(err) = db_manager.purge_old_queue_items(
                crate::fromqueuemanager::FROM_QUEUE_MAX_AGE_SECS,
                MAX_RESEND_AGE_SECS,
            ) {
                error!("Could not purge the queues : {err}");
            }
            // Fetch queue from db
            let queue_content = match db_manager.get_to_voip_ms_queue_content() {
                Ok(content) => content,
                Err(err) => {
                    error!("Database error while reading the outbound queue, retrying next tick : {err}");
                    sleep(Duration::from_millis(30000)).await;
                    continue;
                }
            };
            if let Some(queue_content) = queue_content {
                info!("Start listing To Queue");
                // Browse each queue item
                for queue_item in queue_content {
                    // Certain items with specific error processing levels, including in process items are excluded from the resend
                    // This avoid looping over certain message that caused errors (infinite loops)
                    if !do_not_resend.contains(&queue_item.processing_level.as_str()) {
                        info!("Queue item to resend: {}", queue_item.processing_level);
                        // Never resend forever: an error after voip.ms accepted the message
                        // would otherwise send it again every 30 seconds.
                        let attempts = resend_attempts(&queue_item.processing_level);
                        let too_old = queue_item
                            .get_timestamp()
                            .map(|created| {
                                (Utc::now().naive_utc() - created).num_seconds()
                                    > MAX_RESEND_AGE_SECS
                            })
                            .unwrap_or(true);
                        let opted_out = if attempts < MAX_RESEND_ATTEMPTS && !too_old {
                            match is_to_opted_out_caller(&queue_item.send_message_url) {
                                Some(opted_out) => opted_out,
                                // Database error: try this item again next tick
                                None => continue,
                            }
                        } else {
                            false
                        };
                        let final_level = if attempts >= MAX_RESEND_ATTEMPTS {
                            Some(GAVE_UP_ATTEMPTS)
                        } else if too_old {
                            Some(GAVE_UP_TOO_OLD)
                        } else if opted_out {
                            Some(CANCELLED_OPTED_OUT)
                        } else {
                            None
                        };
                        let id_to_voipms_queue: i64 =
                            queue_item.id_to_voipms_queue.try_into().unwrap_or_else(|err| {
                                error!("Failed trying into queue_item.id_to_voipms_queue : {err}");
                                -1
                            });
                        let next_level = match final_level {
                            Some(level) => level.to_string(),
                            None => format!("{RESEND_ATTEMPT_PREFIX}{}", attempts + 1),
                        };
                        if let Err(err) = db_manager.change_processing_level_to_voip_ms_queue_item(
                            id_to_voipms_queue,
                            next_level,
                            queue_item.error_log.clone(),
                        ) {
                            // Skip the item: resending without counting the attempt could loop
                            error!("Database error, queue item skipped until next tick : {err}");
                            continue;
                        }
                        if let Some(level) = final_level {
                            error!(
                                "To Queue Item with id {} not resent: {level}",
                                queue_item.id_to_voipms_queue
                            );
                            continue;
                        }
                        // Send web request (the queued url has no password: add it back)
                        let url = match crate::voip_ms::restore_password(&queue_item.send_message_url) {
                            Ok(url) => url,
                            Err(err) => {
                                error!("Cannot rebuild the queued voip.ms request : {err}");
                                continue;
                            }
                        };
                        let result = crate::voip_ms::http_client().get(url).send().await;
                        let res: Response;
                        match result {
                            Ok(ok) => {
                                info!("Resend answered with status {}", ok.status());
                                res = ok;
                            }
                            Err(err) => {
                                error!("Error : {}", err.without_url());
                                continue;
                            }
                        }
                        info!("Status: {}", res.status());
                        // Fetch request body content
                        let body_result = res.text().await;
                        let body: String;
                        match body_result {
                            Ok(ok) => {
                                body = ok;
                            }
                            Err(err) => {
                                error!("Error fetching body {}", err);
                                continue;
                            }
                        }
                        // parse the resulting json
                        let json_result: Result<Value, serde_json::Error> =
                            serde_json::from_str(&body);
                        let json: Value;
                        match json_result {
                            Ok(value) => json = value,
                            Err(err) => {
                                error!("Error parsing json {}", err);
                                continue;
                            }
                        }
                        // verify that the request was a success, if so, remove the item from queue
                        // TODO queue garbage collector that would clean unprocessed errors from db
                        // and send them to log.
                        if json["status"] == "success" {
                            info!(
                                "To Queue Item with id {} removed from queue.",
                                queue_item.id_to_voipms_queue
                            );
                            db_manager
                                .remove_to_voip_ms_queue_item(
                                    queue_item.id_to_voipms_queue.try_into().unwrap_or_else(|err| {error!("Failed tying into queue_item.id_to_voipms_queue : {err}");-1}),
                                )
                                .unwrap_or_else(|err| {
                                    error!("Database error : {err}");
                                });
                        }
                    }
                }
                info!("End listing To Queue\n");
            }
            sleep(Duration::from_millis(30000)).await;
        }
    }

    // ================================
    // QUEUE STATE MANAGEMENT
    // ================================

    // Review processing state for queue items with processing_level = "Processing"
    // If they are past a certain time, they are made available for a reprocess
    pub async fn review_processing_state(&self) {
        let maximum_time_sec_processing = 60;
        let db_manager = DbManager::new();
        let queue_content = match db_manager.get_to_voip_ms_queue_content() {
            Ok(content) => content,
            Err(err) => {
                error!("Database error while reviewing the outbound queue : {err}");
                return;
            }
        };
        if let Some(queue_content) = queue_content {
            info!("Start listing To Queue");
            for queue_item in queue_content {
                if queue_item.processing_level == "Processing" {
                    let Ok(item_timestamp) = queue_item.get_timestamp() else {
                        error!("Error with timestamp parsing !");
                        continue;
                    };

                    let local: DateTime<Local> = Local::now();
                    let time_since_first_process = item_timestamp
                        .signed_duration_since(local.naive_local())
                        .num_seconds()
                        .abs();
                    // Test if the item processing has been processing for a certain time
                    // if so, we need to reprocess it since it's probably an error that didn't
                    // generate any error message
                    if time_since_first_process > maximum_time_sec_processing {
                        error!(
                            "Reprocessing id_to_voipms_queue : {}",
                            queue_item.id_to_voipms_queue
                        );
                        db_manager
                            .change_processing_level_to_voip_ms_queue_item(
                                queue_item.id_to_voipms_queue.try_into().unwrap_or_else(|err|{error!("Failed trying into queue_item.id_to_voipms_queue : {err}");-1}),
                                "Reprocessing".to_string(),
                                queue_item.error_log,
                            )
                            .unwrap_or_else(|err| {
                                error!("Database error : {err}");
                            });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resend_attempts() {
        assert_eq!(resend_attempts("Network error"), 0);
        assert_eq!(resend_attempts("Reprocessing"), 0);
        assert_eq!(resend_attempts("Resend attempt 3"), 3);
        assert_eq!(resend_attempts(GAVE_UP_ATTEMPTS), 0);
    }
}
