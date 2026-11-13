use crate::{db_manager::DbManager, web_server::QueryParams};
use chrono::prelude::*;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// An inbound text waiting in the queue (no respondent available, Matrix down...) is
/// deleted after this long, answered or not: the queue is not an archive (Law 25).
pub const FROM_QUEUE_MAX_AGE_SECS: i64 = 24 * 3600;

pub struct FromQueueManager {
    transmitter: mpsc::Sender<QueryParams>,
}

pub struct FromQueueManagerEntry {
    pub id: u64,
    pub queryparams: QueryParams,
    pub processing_level: String,
    pub error_log: String,
}

impl std::fmt::Debug for FromQueueManagerEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromQueueManagerEntry")
            .field("id", &self.id)
            .field("queryparams", &"<redacted>")
            .field("processing_level", &self.processing_level)
            .field("error_log", &"<redacted>")
            .finish()
    }
}

impl FromQueueManager {
    // ================================
    // CONSTRUCTOR
    // ================================

    pub async fn new(transmitter: mpsc::Sender<QueryParams>) -> FromQueueManager {
        Self { transmitter }
    }

    // ================================
    // MAIN PROCESSING LOOP
    // ================================

    pub async fn loop_queue(&self) -> anyhow::Result<()> {
        // Initial sleep to make sure transmitter is enabled
        sleep(Duration::from_millis(5000)).await;
        // Maximum time in second before an item in the queue with the Processing state is no longer considered being in process (has failed without notice)
        let maximum_time_sec_processing = 75;
        let db_manager = DbManager::new();
        loop {
            // Check if items has been processed for too long
            let queue_content = match db_manager.get_from_voip_ms_queue_content() {
                Ok(content) => content,
                Err(err) => {
                    error!("Database error while reading the inbound queue, retrying next tick : {err}");
                    sleep(Duration::from_millis(60000)).await;
                    continue;
                }
            };
            if let Some(queue_content) = queue_content {
                for queue_item in queue_content {
                    // Check if the queue item has been processing for a while now.
                    if queue_item.processing_level == "Processing" {
                        info!("Inbound queue item {} is still processing", queue_item.id);
                        let Ok(item_timestamp) = queue_item.queryparams.get_timestamp() else {
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
                            match self.reprocess_queue_item(queue_item, &db_manager).await {
                                Ok(_) => {}
                                Err(err) => {
                                    error!(
                                        "Error while trying to late processing queue item {:?}",
                                        err
                                    );
                                }
                            }
                        }
                        // If the queue item is not processing and also not reprocessing, it's likely an
                        // unprocessed error, so we reprocess it
                        // TODO ajouter erreur permanente
                    } else {
                        match self.reprocess_queue_item(queue_item, &db_manager).await {
                            Ok(_) => {}
                            Err(err) => {
                                error!("Error while trying to reprocess queue item {:?}", err);
                            }
                        }
                    }
                }
            }
            sleep(Duration::from_millis(60000)).await;
        }
    }

    // ================================
    // QUEUE ITEM REPROCESSING
    // ================================

    async fn reprocess_queue_item(
        &self,
        queue_entry: FromQueueManagerEntry,
        db_manager: &DbManager,
    ) -> anyhow::Result<()> {
        // Note : the whole counting things does not work for now because the only info the message spread
        // is QueryParams. Would need to pass the new processing state but I'm lazy...
        // Normal while nobody is on duty: the text waits and is retried every minute (not an error)
        if queue_entry.processing_level.contains("No Respondent") {
            debug!("Retrying inbound queue item {} (nobody on duty)", queue_entry.id);
        } else {
            warn!("Retrying inbound queue item {} ({})", queue_entry.id, queue_entry.processing_level);
        }
        // Delete from database
        let mut processing_level;
        // If this error has been reprocessed, we change the processing level adding to reprocessing counter
        if queue_entry.processing_level.starts_with("Reprocessing <|") {
            // Get current count
            let reprocessing_count =
                self.get_current_reprocessing_count(queue_entry.processing_level.clone());
            // Get the last error before reprocessing
            let last_error = self.get_reprocessing_last_error(queue_entry.processing_level);
            processing_level = format!("Reprocessing <|{}|> {}", reprocessing_count, last_error);
            debug!(processing_level);
        } else {
            // We change processing level to Reprocessing
            processing_level = "Reprocessing <|0|> ".to_string();
            processing_level.push_str(queue_entry.processing_level.as_str());
        }

        db_manager
            .change_processing_level_from_voip_ms_queue_item(
                queue_entry.id as i64,
                processing_level,
                queue_entry.error_log,
            )
            .map_err(|err| anyhow::anyhow!("Database error while updating an inbound queue item : {err}"))?;
        debug!("Resending inbound queue item {}", queue_entry.id);
        // Reprocess entry
        if self.transmitter.is_closed() {
            error!("Transmitter closed !!");
        }
        let tx_result = self.transmitter.send(queue_entry.queryparams).await;
        match tx_result {
            Ok(_) => {
                debug!("transmission ok");
            }
            Err(err) => {
                error!(
                    "transmission error(check if you use return instead of continue in the while loop) : {:?}",
                    err
                );
                panic!("");
            }
        }
        Ok(())
    }

    // ================================
    // STRING PARSING UTILITIES
    // ================================

    pub fn get_current_reprocessing_count(&self, processing_str: String) -> i32 {
        let Some(mut first_index) = processing_str.find("<|") else {
            return -1;
        };
        first_index += 2;
        let Some(mut second_index) = processing_str[first_index..].find("|>") else {
            return -1;
        };
        second_index = second_index + first_index;
        let answer = &processing_str[first_index..second_index];
        let parsed_answer: i32 = answer.parse().expect("Failed to parse number");
        parsed_answer
    }

    pub fn get_reprocessing_last_error(&self, processing_str: String) -> String {
        let Some(index) = processing_str.find("|>") else {
            return "(error)".to_string();
        };
        processing_str[index + 2..].to_string()
    }
}
