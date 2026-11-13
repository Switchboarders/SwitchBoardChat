use crate::attention;
use crate::auto_sms::{self, AutoSms};
use crate::config;
use crate::crypto;
use crate::db_manager::DbManager;
use crate::deny_list::{self, DenyCheck};
use crate::help;
use crate::macros;
use crate::persistent_session::PersistentSession;
use crate::web_server::QueryParams;
use crate::{names, rooms};
use anyhow::{Result, anyhow};
use chrono::Local;
use std::process::exit;
use tokio::sync::mpsc;
use tracing::{error, info};
#[derive(Debug)]
pub struct Matrix {
    pub session: PersistentSession,
    receiver: Option<mpsc::Receiver<QueryParams>>,
}
impl Matrix {
    pub async fn new(
        receiver: Option<mpsc::Receiver<QueryParams>>,
        session: PersistentSession,
    ) -> Self {
        Self { session, receiver }
    }
    pub async fn listen_sms_message(mut self) -> Result<()> {
        info!("Start SMS listening");
        let mut receiver = self.receiver.take().unwrap_or_else(|| {
            error!("Unable to initiate receiver to listen to sms messages from web server");
            exit(1);
        });
        // Load .env variables
        let config = config::load_config();
        // Spawn a task to receive messages asynchronously
        let listening_task = tokio::spawn(async move {
            while let Some(msg) = receiver.recv().await {
                let is_reprocessing_error: bool;
                match msg.is_reprocessing_error {
                    Some(some) => {
                        is_reprocessing_error = some;
                    }
                    None => {
                        is_reprocessing_error = false;
                    }
                }
                info!("SMS received for the call center");
                let queue_message = msg.clone();
                let msg_clone_1 = msg.clone();
                let msg_clone_2 = msg.clone();
                if let Some(token) = msg.sbchatapikey {
                    //let hashed_token = hash_token(token.as_str());
                    let db_manager = DbManager::new();
                    // Using the token to figure out who the call center is
                    let call_center_id = match db_manager
                        .get_call_center_id_by_hashed_token(token) {
 Ok(v) => v,
 Err(err) => {
 error!("Database error : {err}");
 continue;
 }
 };
                    let Some((call_center_id, _)) = call_center_id else {
                        // TODO Danger here that someone would try to bruteforce filling the log
                        info!("User not authorized");
                        continue;
                    };
                    let call_center = match db_manager.get_call_center_by_id(call_center_id) {
                        Ok(ok) => match ok {
                            Some(some) => some,
                            None => {
                                info!("User not authorized, he does not exist.");
                                continue;
                            }
                        },
                        Err(err) => {
                            info!("User not authorized, db error {err}");
                            continue;
                        }
                    };
                    let call_center_did_opt = match msg_clone_2.to {
                        Some(some) => some,
                        None => {
                            info!("User not authorized, call center did is None");
                            continue;
                        }
                    };
                    let call_center_did: u64 = match call_center_did_opt.parse() {
                        Ok(ok) => ok,
                        Err(err) => {
                            info!(
                                "User not authorized, could not parse call center did to u64 {err}"
                            );
                            continue;
                        }
                    };

                    if call_center.call_center_did != call_center_did {
                        info!("Token exist but is not associated with this DID");
                        continue;
                    }

                    // Check if already existing queue item has already the same id from voip.ms and delete them
                    // They are reprocessed (avoid duplicates)

                    let mut first_arrival = None;
                    if let Some(id_from_voipms) = msg_clone_1.id {
                        // A retried text: remove the old copy but keep its first arrival time (24 h purge, Law 25)
                        first_arrival = match db_manager
                            .take_from_voip_ms_queue_item_from_voipms_id(id_from_voipms) {
 Ok(v) => v,
 Err(err) => {
 error!("Database error : {err}");
 continue;
 }
 };
                    }
                    // Adding to queue in case the message does not get throught
                    let queue_id = db_manager
                        .add_from_voip_ms_queue_item_since(queue_message, "Processing".to_string(), first_arrival);
                    let queue_id = match queue_id {
                        Ok(id) => id,
                        Err(err) => {
                            error!("Database error while queueing an SMS : {err}");
                            continue;
                        }
                    };

                    info!("Call center id : {}", call_center_id);

                    let Some(mut caller_message) = msg.message.clone() else {
                        error!("Invalid QueryParams.message");
                        db_manager
                            .change_processing_level_from_voip_ms_queue_item(
                                queue_id,
                                "Invalid QueryParams.message".to_string(),
                                "".to_string(),
                            )
                            .unwrap_or_else(|err| { error!("Database error : {err}"); });
                        continue;
                    };

                    let room_id: String;
                    let mut message_prefix = String::new();
                    let mut new_room_created = false;
                    if let Some(caller_did) = msg.from {
                        if let Some(call_center_did) = msg.to {
                            // Verify caller did is valid
                            let caller_did = caller_did.parse();
                            let parsed_caller_did: u64;
                            match caller_did {
                                Ok(caller_did) => {
                                    parsed_caller_did = caller_did;
                                }
                                Err(err) => {
                                    error!("Error parsing caller did {}", err);
                                    db_manager
                                        .change_processing_level_from_voip_ms_queue_item(
                                            queue_id,
                                            "Error parsing caller did".to_string(),
                                            err.to_string(),
                                        )
                                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    continue;
                                }
                            }
                            // Verify call center did is valid
                            let call_center_did = call_center_did.parse();
                            let parsed_call_center_did: u64;
                            match call_center_did {
                                Ok(call_center_did) => {
                                    parsed_call_center_did = call_center_did;
                                }
                                Err(err) => {
                                    error!("Error parsing caller did {}", err);
                                    db_manager
                                        .change_processing_level_from_voip_ms_queue_item(
                                            queue_id,
                                            "Error parsing call center did".to_string(),
                                            err.to_string(),
                                        )
                                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    continue;
                                }
                            }
                            // Deny list: callers who texted STOP get no SMS from us (not even automatic
                            // ones) and their messages are dropped until they text HELP.
                            let deny_check = match deny_list::check_inbound(
                                call_center_id,
                                parsed_caller_did,
                                &caller_message,
                            ) {
                                Ok(check) => check,
                                Err(err) => {
                                    // Stays "Processing" in the queue: retried by the queue manager
                                    error!("Database error while checking the deny list : {err}");
                                    continue;
                                }
                            };
                            match deny_check {
                                DenyCheck::Allowed => {}
                                DenyCheck::Help { denied } => {
                                    if !is_reprocessing_error {
                                        auto_sms::send(
                                            AutoSms::Help,
                                            call_center_id,
                                            parsed_call_center_did,
                                            parsed_caller_did,
                                            macros::get_help_macro_text(call_center_id),
                                        )
                                        .await;
                                    }
                                    if denied {
                                        db_manager
                                            .remove_from_voip_ms_queue_item(queue_id)
                                            .unwrap_or_else(|err| error!("Database error : {err}"));
                                        continue;
                                    }
                                    // Otherwise respondents also see the message: the caller may need help
                                }
                                DenyCheck::Denied => {
                                    info!(
                                        "Caller {} for call center {call_center_id} is on the deny list, message dropped.",
                                        crypto::short_id(parsed_caller_did)
                                    );
                                    db_manager
                                        .remove_from_voip_ms_queue_item(queue_id)
                                        .unwrap_or_else(|err| error!("Database error : {err}"));
                                    continue;
                                }
                                DenyCheck::OptOut { reply } => {
                                    // reply is None when already denied: stay silent
                                    if let Some(reply) = reply {
                                        auto_sms::send(
                                            AutoSms::DenyListConfirmation,
                                            call_center_id,
                                            parsed_call_center_did,
                                            parsed_caller_did,
                                            reply.to_string(),
                                        )
                                        .await;
                                        // Tell the respondents, only if a room already exists
                                        // (no point creating a room for a caller who just left)
                                        let room_id_query = db_manager.fetch_room_id_from_did(
                                            call_center_id,
                                            parsed_caller_did,
                                            parsed_call_center_did,
                                        );
                                        match room_id_query {
                                            Ok(Some(existing_room_id)) => {
                                                if let Err(err) =
                                                    rooms::send_message_to_room_using_id(
                                                        self.session.get_client().await,
                                                        deny_list::ROOM_NOTICE_OPTED_OUT,
                                                        &existing_room_id,
                                                    )
                                                    .await
                                                {
                                                    error!("Opt-out notice not sent to room : {err}");
                                                }
                                            }
                                            Ok(None) => {}
                                            Err(err) => error!("Database error : {err}"),
                                        }
                                    }
                                    db_manager
                                        .remove_from_voip_ms_queue_item(queue_id)
                                        .unwrap_or_else(|err| error!("Database error : {err}"));
                                    continue;
                                }
                                DenyCheck::OptIn { reply } => {
                                    auto_sms::send(
                                        AutoSms::DenyListConfirmation,
                                        call_center_id,
                                        parsed_call_center_did,
                                        parsed_caller_did,
                                        reply.to_string(),
                                    )
                                    .await;
                                    // Respondents see the notice instead of the raw keyword
                                    caller_message = deny_list::ROOM_NOTICE_OPTED_IN.to_string();
                                }
                            }
                            // Every caller is told once how to opt out, before anyone writes to them
                            let mut notice_pending = deny_list::needs_first_contact_notice(
                                call_center_id,
                                parsed_caller_did,
                            );
                            // Check if there are actually respondents that can answer the sms. If not,
                            // we must send a message to the user using the proper macro(or default macro).
                            // Make sure the macro is not sent if it is reprocessed.
                            // If no one can answer, the message should be kept in the FROM queue until
                            // new respondent are in. They should receive the message then.
                            let invites = match db_manager
                                .get_call_center_respondant(call_center_id, true) {
 Ok(v) => v,
 Err(err) => {
 error!("Database error : {err}");
 continue;
 }
 };
                            let invites = match invites {
                                Some(some) => some,
                                // Here we have no invites scheduled
                                None => {
                                    // If we are not reprocessing, send back to the caller a message using macro or default.
                                    if !is_reprocessing_error {
                                        let (my_macro, with_notice) =
                                            deny_list::with_first_contact_notice(
                                                macros::get_no_respondent_macro_text(
                                                    call_center_id,
                                                ),
                                                notice_pending,
                                            );
                                        if auto_sms::send(
                                            AutoSms::NoRespondent,
                                            call_center_id,
                                            parsed_call_center_did,
                                            parsed_caller_did,
                                            my_macro,
                                        )
                                        .await
                                            && with_notice
                                        {
                                            deny_list::mark_notified(
                                                call_center_id,
                                                parsed_caller_did,
                                            );
                                        }
                                    }
                                    // Put the message into the FROM Queue
                                    db_manager
                                        .change_processing_level_from_voip_ms_queue_item(
                                            queue_id,
                                            "No Respondent".to_string(),
                                            "".to_string(),
                                        )
                                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    continue;
                                }
                            };
                            // Look for room id using the caller did and the call center did
                            let room_id_query: Result<Option<String>, rusqlite::Error> = db_manager
                                .fetch_room_id_from_did(
                                    call_center_id,
                                    parsed_caller_did,
                                    parsed_call_center_did,
                                );
                            match room_id_query {
                                Ok(room_id_from_db) => {
                                    match room_id_from_db {
                                        // The caller has already SMSed the call center so we use the existant room.
                                        Some(room_id_exist) => {
                                            room_id = room_id_exist;
                                            // We need to update respondants, but a respondant might be in a private room
                                            // with the caller. If so, do not update respondants.
                                            // After some time, we consider that the caller is no longer in a private room
                                            // so we can update respondants accordingly

                                            // Calculate time difference since last access of the room
                                            let last_accessed_room_timestamp = match db_manager
                                                .get_last_accessed_room_timestamp(room_id.clone()) {
 Ok(v) => v,
 Err(err) => {
 error!("Database error : {err}.");
 continue;
 }
 };
                                            let now = Local::now().naive_utc();
                                            // Calculate the difference in minutes
                                            let duration_diff_minutes;
                                            match last_accessed_room_timestamp {
                                                Some(some) => {
                                                    duration_diff_minutes =
                                                        (now - some).num_minutes();
                                                }
                                                None => {
                                                    duration_diff_minutes = 0;
                                                }
                                            }

                                            info!(
                                                "Minutes since room : {} \nwas last accessed : {}",
                                                &room_id, duration_diff_minutes
                                            );
                                            // After an hour, we reinvite everyone. This is also meant to invite new respondants or remove old ones.
                                            let is_in_one_to_one;
                                            if duration_diff_minutes > 60 {
                                                db_manager
                                                    .set_room_in_one_to_one(room_id.clone(), false)
                                                    .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                                is_in_one_to_one = false;
                                            } else {
                                                is_in_one_to_one = match db_manager
                                                    .is_room_in_one_to_one(room_id.clone()) {
 Ok(v) => v,
 Err(err) => {
 error!("Database error : {err}");
 continue;
 }
 };
                                            }
                                            info!("Is in one to one mode : {}", is_in_one_to_one);
                                            // If the room is not in one to one mode, we need to update respondents. Otherwise, we do nothing
                                            if !is_in_one_to_one {

                                                rooms::update_room_invites(
                                                    &self.session,
                                                    &room_id,
                                                    invites,
                                                )
                                                .await
                                                .unwrap_or_else(|err| {
                                                    error!("Error updating room invites : {err}");
                                                });
                                            }
                                        }
                                        // The room does not exist in the database so we create one.
                                        None => {
                                            info!("This room id does not exist, creating it ...");
                                            message_prefix = String::from(help::NEW_ROOM_HINT);
                                            // If a macro is set, we send a newcomer welcome message
                                            // If we are reprocessing an error, make sure we do not send this macro, this would result in a non ending loop !!
                                            let macro_opt: Option<String>;
                                            if is_reprocessing_error {
                                                macro_opt = None;
                                            } else {
                                                macro_opt =
                                                    macros::get_newcomer_macro_text(call_center_id);
                                            }

                                            if let Some(my_macro) = macro_opt {
                                                let (my_macro, with_notice) =
                                                    deny_list::with_first_contact_notice(
                                                        my_macro,
                                                        notice_pending,
                                                    );
                                                if auto_sms::send(
                                                    AutoSms::Welcome,
                                                    call_center_id,
                                                    parsed_call_center_did,
                                                    parsed_caller_did,
                                                    my_macro,
                                                )
                                                .await
                                                    && with_notice
                                                {
                                                    deny_list::mark_notified(
                                                        call_center_id,
                                                        parsed_caller_did,
                                                    );
                                                    notice_pending = false;
                                                }
                                            }

                                            //let Some(current_session)=self.session.client.clone() else {panic!("Session from persistent session not referenced.");};

                                            let current_session = self.session.get_client().await;
                                            // Obfuscate the phone number in a reproducible way and use that to select a name from a list
                                            //let room_name = crypto::hash_phone(&String::from(parsed_caller_did.to_string()), &config.phone_hash_secret);
                                            // Message that are not actual SMS == 0 So we do not use an hashed name
                                            let room_name;
                                            if parsed_caller_did == 0 {
                                                room_name = "Message".to_string();
                                            } else {
                                                room_name =
                                                    names::hash_to_line(&crypto::hash_phone(
                                                        &String::from(
                                                            parsed_caller_did.to_string(),
                                                        ),
                                                        &config.phone_hash_secret,
                                                    ));
                                            }

                                            let room_id_option = rooms::create_room(
                                                room_name,
                                                current_session,
                                                invites,
                                            )
                                            .await; // Every specific request to matrix has it's own timeout so no timeout necessary here
                                            match room_id_option {
                                                Ok(room_id_success) => {
                                                    room_id = room_id_success;
                                                    new_room_created = true;
                                                    db_manager.add_room_id_to_did(&room_id,&call_center_id, &parsed_caller_did,&parsed_call_center_did).unwrap_or_else(|err| error!("Database error while saving the new room : {err}"));
                                                    info!("New room entered in database.");
                                                }
                                                Err(err) => {
                                                    error!("Error creating a room {}", err);
                                                    db_manager.change_processing_level_from_voip_ms_queue_item(queue_id,"Error creating a room".to_string(),err.to_string()).unwrap_or_else(|err| { error!("Database error : {err}"); });
                                                    continue;
                                                }
                                            }
                                            //self.session.client.clone()
                                        } // Here the room_id is not already existing so we create it
                                    }
                                }
                                Err(err) => {
                                    db_manager.change_processing_level_from_voip_ms_queue_item(queue_id,"Database error while fetching room_id from the room table.".to_string(),err.to_string()).unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    error!("Db error {}", err);
                                    continue;
                                }
                            }
                            info!("Room Id : {:?}", room_id);
                            // Now that we have a proper encrypted room, we verify the respondant are invited
                            //let Some(current_session)= self.session.client.clone() else {panic!("Session from persistent session not referenced.");};
                            //let current_session = self.session.get_client().await;
                            // No welcome message carried the opt-out notice: send it on its own
                            if notice_pending
                                && auto_sms::send(
                                    AutoSms::FirstContactNotice,
                                    call_center_id,
                                    parsed_call_center_did,
                                    parsed_caller_did,
                                    deny_list::first_contact_notice(),
                                )
                                .await
                            {
                                deny_list::mark_notified(call_center_id, parsed_caller_did);
                            }
                            let room_message = message_prefix + caller_message.as_str();
                            let result = rooms::send_message_to_room_using_id(
                                self.session.get_client().await,
                                room_message.as_str(),
                                room_id.as_str(),
                            )
                            .await;
                            match result {
                                Ok(_) => {
                                    // Here we know that the message was sent with success
                                    // We remove it from queue and update last accessed room timestamp
                                    info!("Message sent to room {}", room_id);
                                    // Respondents owe an answer; a brand new room also gets the "new conversation" ping
                                    attention::inbound(&room_id);
                                    if new_room_created {
                                        tokio::spawn(attention::announce_new_room(
                                            self.session.get_client().await,
                                            room_id.clone(),
                                            attention::Kind::Sms,
                                        ));
                                    }

                                    db_manager
                                        .update_last_accessed_room_timestamp(room_id.to_string())
                                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    db_manager
                                        .remove_from_voip_ms_queue_item(queue_id)
                                        .unwrap_or_else(|err| error!("Database error : {err}"));
                                    // Check if a Reprocessing item exist in queue, if so delete it
                                    if let Some(voip_ms_id) = msg.id {
                                        db_manager
                                            .remove_from_voip_ms_queue_item_from_voipms_id(
                                                voip_ms_id,
                                            )
                                            .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                    }
                                }
                                Err(err) => {
                                    error!("Message not sent\n{}", err);
                                    db_manager
                                        .change_processing_level_from_voip_ms_queue_item(
                                            queue_id,
                                            "Room does not exist server side".to_string(),
                                            err.to_string(),
                                        )
                                        .unwrap_or_else(|err| { error!("Database error : {err}"); });
                                }
                            }
                        }
                    }
                }
            }
        });
        if let Err(err) = listening_task.await {
            error!("SMS listening task failed : {err}");
        }
        return Err(anyhow!("This task should never end !!"));
    }
}
