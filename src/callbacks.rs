use crate::auth::Auth;
use crate::auto_sms::{self, AutoSms};
use crate::db_manager::DbManager;
use crate::deny_list;
use crate::help;
use crate::room_commands::{self, RoomCommand};
use crate::web_chat;
use crate::verification::UserVerificationHandler;
use crate::voip_ms::VoipMs;
use crate::{macros, rooms};

use crate::persistent_session::MyRoomMessageContext;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::event_handler::Ctx;
use matrix_sdk::{
    Client, Room, RoomState,
    ruma::events::room::{
        encrypted::OriginalSyncRoomEncryptedEvent,
        member::StrippedRoomMemberEvent,
        message::{MessageType, OriginalSyncRoomMessageEvent},
    },
};
use tokio::time;
use tokio::time::Duration;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use tracing::{error, info, warn};

/// Last time each (room, sender) was told their message could not be read, so a burst gives one notice
static UNREADABLE_NOTICED: LazyLock<Mutex<HashMap<(String, String), Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A message the bridge cannot decrypt (the sender's device did not share its keys with the bridge, or
/// the sender's session is unverified). Without this the message vanishes without a trace: the
/// respondent sees it sent, the visitor never gets it.
pub async fn on_undecryptable(event: OriginalSyncRoomEncryptedEvent, room: Room, client: Client) {
    if Some(event.sender.as_ref()) == client.user_id() || room.state() != RoomState::Joined {
        return;
    }
    // Which device and megolm session failed (identifiers only, never content)
    let (device, session) = match &event.content.scheme {
        matrix_sdk::ruma::events::room::encrypted::EncryptedEventScheme::MegolmV1AesSha2(c) => {
            (c.device_id.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "?".into()), c.session_id.clone())
        }
        _ => (String::from("?"), String::from("?")),
    };
    warn!(
        "Message from {} could not be decrypted (room {}, sender device {device}, session {session})",
        event.sender,
        room.room_id()
    );
    {
        let mut seen = UNREADABLE_NOTICED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (room.room_id().to_string(), event.sender.to_string());
        if seen.get(&key).is_some_and(|t| t.elapsed() < Duration::from_secs(120)) {
            return;
        }
        seen.insert(key, Instant::now());
    }
    let notice = format!(
        "⚠️ Le message de {sender} n’a PAS été reçu : la passerelle ne peut pas le déchiffrer (session ou appareil non vérifié, clés manquantes). Le visiteur ne l’a pas vu. Dans Element, tapez /discardsession dans cette salle puis renvoyez le message (sinon déconnectez-vous et reconnectez-vous). / ⚠️ The message from {sender} was NOT received: the bridge cannot decrypt it (missing keys). The visitor did not see it. In Element type /discardsession in this room, then send the message again (otherwise sign out and back in).",
        sender = event.sender
    );
    reply(&client, room.room_id().as_str(), &notice).await;
}
pub async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    context: Ctx<MyRoomMessageContext>,
    encryption_info: Option<EncryptionInfo>,
) {
    let client = context.session.get_client().await;
    // First, we need to unpack the message: We only want messages from rooms we are
    // still in and that are regular text messages - ignoring everything else.
    info!("Received room message from {}", event.sender);
    let room_id = room.room_id().to_string();
    info!("Room Id: {:?}", room_id);
    if room.state() != RoomState::Joined {
        return;
    }
    let auth = Auth::new();
    if event.sender.as_str() == auth.get_username() {
        return;
    }
    // A respondent wrote in the room: someone is on it (stops the reminders)
    crate::attention::respondent_active(&room_id);
    // Test if the message is from a verification room. If so, process to user identity verification
    let db_manager = DbManager::new();
    match db_manager
        .get_user_room_respondant_username(&room.room_id())
        .unwrap_or_else(|err| { error!("Database Error : {err}"); None }) {
        Some(respondant_username) => {
            // The confidentiality agreement is read and signed here too (src/agreement.rs)
            if let MessageType::Text(text) = &event.content.msgtype {
                if let Some(command) = crate::agreement::parse(&text.body) {
                    let ts = i64::from(event.origin_server_ts.as_secs());
                    tokio::spawn(crate::agreement::handle(
                        client.clone(),
                        room_id.clone(),
                        event.sender.to_string(),
                        command,
                        event.event_id.to_string(),
                        ts,
                        encryption_info,
                    ));
                    return;
                }
                // Other commands come before the device check, and always get an answer
                if let Some(command) = room_commands::parse(&text.body) {
                    info!("Command in a personal room: {command:?}");
                    let answer = match command {
                        RoomCommand::Help { french } => {
                            help::get_user_room_help_text(&respondant_username, french)
                        }
                        RoomCommand::Unknown(word) => room_commands::unknown(&word),
                        _ => help::CONVERSATION_ONLY.to_string(),
                    };
                    reply(&client, &room_id, &answer).await;
                    return;
                }
            }
            // This is a verification room, process and exit method
            info!("Received verification request message.");
            let verificator = UserVerificationHandler::new(context.session.get_client().await);
            tokio::spawn(async move {
                verificator
                    .verify_user_identity(&event.sender, &room_id)
                    .await;
                time::sleep(Duration::from_secs(3600)).await;
            });

            return;
        }
        None => {
            info!("Not a verification room");
        }
    }
    // We fetch the caller call_center_id, caller_did, call_center_did from room id in the db
    let Some(answer) = db_manager
        .fetch_subscriber_info_from_room_id(room.room_id())
        .unwrap_or_else(|err| { error!("Database Error : {err}"); None })
    else {
        error!(
            "Could not find the room_id in the sbchat database, may be a private message sent to this client user."
        );
        return;
        //panic!("Could not find the room_id in the sbchat database.TODO create a new room");
    };
    // Note: last_accessed_time is intentionally NOT updated here. It must only be
    // extended by an inbound SMS from the caller (see matrix.rs), not by a
    // respondent's own room message — otherwise a respondent could keep a room
    // locked in one-to-one mode indefinitely just by talking, with no way for an
    // admin to reclaim it in real time.
    let (call_center_id, caller_did, call_center_did) = answer;
    info!("call_center_id {}, caller {}", call_center_id, crate::crypto::short_id(caller_did));
    //voip_ms.send_sms(call_center_id, call_center_did, caller_did, message)
    let MessageType::Text(text_content) = event.content.msgtype else {
        return;
    };
    // Message bodies are confidential (Law 25): never log them
    info!("Received message from : {}", event.sender);
    info!("Room ID : {}", room.room_id());
    let is_web = web_chat::is_web_did(caller_did);
    // Commands first, for web chats and SMS alike: every one gets a visible answer in the room
    if let Some(command) = room_commands::parse(&text_content.body) {
        run_command(
            command,
            is_web,
            &client,
            &room,
            &context,
            &db_manager,
            call_center_id,
            caller_did,
            call_center_did,
            event.sender.clone(),
        )
        .await;
        return;
    }
    // Web chat visitors (pseudo caller number) are answered in the browser, never by SMS
    if is_web {
        let trimmed = text_content.body.trim().to_string();
        if trimmed.starts_with('#') {
            let macro_name = macros::format_macro_name(text_content.body.clone());
            let notice = match db_manager.get_macro_by_call_center_id(&macro_name, &call_center_id) {
                Ok(Some(my_macro)) => {
                    web_chat::deliver_to_room(room.room_id().as_str(), my_macro.macro_text.clone());
                    format!("Macro {macro_name} envoyé avec succès. \n {}", my_macro.macro_text)
                }
                Ok(None) => format!("Macro {macro_name} n'existe pas."),
                Err(err) => {
                    error!("DB error {}", err);
                    return;
                }
            };
            reply(&client, room.room_id().as_str(), &notice).await;
        } else if !web_chat::deliver_to_room(room.room_id().as_str(), text_content.body) {
            warn!("Web chat message for a room without a live session");
        }
        return;
    }
    // Callers who texted STOP must not receive anything (replies or macros); tell the room why.
    let body = text_content.body.trim_start_matches(" ");
    let denied = match db_manager.is_did_denied(call_center_id, caller_did) {
        Ok(denied) => denied,
        Err(err) => {
            error!("Database Error : {err}");
            reply(&client, room.room_id().as_str(), "Message non envoyé (erreur interne), réessayez. / Message not sent (internal error), please retry.").await;
            return;
        }
    };
    if denied {
        info!("Caller {} for call center {call_center_id} is on the deny list, SMS not sent.", crate::crypto::short_id(caller_did));
        reply(&client, room.room_id().as_str(), deny_list::ROOM_NOTICE_NOT_SENT).await;
        return;
    }
    // If the message is a macro, fetch the macro and send it
    if body.starts_with("#") {
        let macro_name = macros::format_macro_name(text_content.body);

        info!(
            "macro name : {}, id_call_center {}",
            macro_name, call_center_id
        );
        let my_macro_result = db_manager.get_macro_by_call_center_id(&macro_name, &call_center_id);
        match my_macro_result {
            Ok(Some(my_macro)) => {
                // Message the macro via sms and put it in the conversation if successful
                let voip_ms = VoipMs::new();
                let voip_ms_result = voip_ms
                    .send_sms(call_center_id, call_center_did, caller_did, my_macro.macro_text.clone())
                    .await;
                match voip_ms_result {
                    Ok(_) => {
                        reply(
                            &client,
                            room.room_id().as_str(),
                            &format!("Macro {macro_name} envoyé avec succès. \n {}", my_macro.macro_text),
                        )
                        .await;
                    }
                    Err(err) => {
                        error!("Error while sending sms macro via voip.ms : {:?}", err);
                        reply(
                            &client,
                            room.room_id().as_str(),
                            &format!("Macro {macro_name} non envoyé. \nProblème de connexion avec voip.ms."),
                        )
                        .await;
                    }
                }
            }
            Ok(None) => {
                warn!("Macro {macro_name} does not exist.");
                reply(&client, room.room_id().as_str(), &format!("Macro {macro_name} n'existe pas.")).await;
            }
            Err(err) => {
                error!("DB error {}", err);
            }
        }
    } else {
        // This is a message to send back via sms
        let voip_ms = VoipMs::new();
        let voip_ms_result = voip_ms
            .send_sms(call_center_id, call_center_did, caller_did, text_content.body)
            .await;
        if let Err(err) = voip_ms_result {
            error!("Error while sending sms via voip.ms : {:?}", err);
        }
    }

    /*let content = RoomMessageEventContent::text_plain("🎉🎊🥳 let's PARTY!! 🥳🎊🎉");
    println!("sending");
    room.send(content).await.unwrap ();
    // here comes the actual "logic": when the bot see's a `!party` in the message,
    // it responds
    if text_content.body.contains("!party") {
        let content = RoomMessageEventContent::text_plain("🎉🎊🥳 let's PARTY!! 🥳🎊🎉");

        println!("sending");

        // send our message to the room we found the "!party" command in
        room.send(content).await.unwrap ();

        println!("message sent");
    }*/
}
/// Posts a text in the room (errors are only logged)
async fn reply(client: &Client, room_id: &str, text: &str) {
    if let Err(err) = rooms::send_message_to_room_using_id(client.clone(), text, room_id).await {
        error!("Reply not sent to room : {err}");
    }
}

/// Runs a respondent's command and answers in the room
#[allow(clippy::too_many_arguments)]
async fn run_command(
    command: RoomCommand,
    is_web: bool,
    client: &Client,
    room: &Room,
    context: &Ctx<MyRoomMessageContext>,
    db_manager: &DbManager,
    call_center_id: u64,
    caller_did: u64,
    call_center_did: u64,
    sender: matrix_sdk::ruma::OwnedUserId,
) {
    let room_id = room.room_id().to_string();
    info!("Room command: {command:?}");
    match command {
        RoomCommand::Ok => {
            // Mark the room as one-to-one, so respondents are not re-invited, and remove the others
            db_manager
                .set_room_in_one_to_one(room_id.clone(), true)
                .unwrap_or_else(|err| { error!("Database Error : {err}"); });
            let result = rooms::remove_other_room_participants(&context.session, &room.room_id(), sender).await;
            match result {
                Ok(_) => reply(client, &room_id, room_commands::PRIVATE_ROOM_DONE).await,
                Err(err) => {
                    error!("Could not remove other participants : {err}");
                    reply(client, &room_id, room_commands::PRIVATE_ROOM_FAILED).await;
                }
            }
        }
        RoomCommand::Help { french } => {
            // !aide answers in French, !help in English
            let help_text = help::get_help_text(call_center_id, french);
            reply(client, &room_id, &help_text).await;
        }
        RoomCommand::Stop => {
            if is_web {
                reply(client, &room_id, room_commands::STOP_NOT_FOR_WEB).await;
                return;
            }
            // A respondent opts the caller out, e.g. the caller asked in their own words
            let denied_by = format!("respondent: {sender}");
            let newly_denied = match db_manager.deny_did(call_center_id, caller_did, &denied_by) {
                Ok(newly_denied) => newly_denied,
                Err(err) => {
                    error!("Database Error : {err}");
                    return;
                }
            };
            let notice = if newly_denied {
                auto_sms::send(
                    AutoSms::DenyListConfirmation,
                    call_center_id,
                    call_center_did,
                    caller_did,
                    deny_list::RESPONDENT_OPT_OUT_REPLY.to_string(),
                )
                .await;
                deny_list::ROOM_NOTICE_RESPONDENT_OPT_OUT
            } else {
                deny_list::ROOM_NOTICE_ALREADY_OPTED_OUT
            };
            reply(client, &room_id, notice).await;
        }
        RoomCommand::Forget => {
            // Right to erasure: delete the conversation and every record of this caller
            if is_web {
                web_chat::end_session_for_room(client, &room_id).await;
            } else if let Err(err) = crate::retention::erase_caller(client, call_center_id, caller_did).await {
                error!("Erasure failed : {err}");
                reply(client, &room_id, "L'effacement a échoué. / The erasure failed.").await;
            }
        }
        RoomCommand::End => {
            if is_web {
                web_chat::end_session_for_room(client, &room_id).await;
            } else {
                reply(client, &room_id, room_commands::WEB_ONLY).await;
            }
        }
        RoomCommand::Ban(days) => {
            if is_web {
                // Abusive visitor: block their connection (and end every chat from it)
                web_chat::ban_for_room(client, &room_id, days).await;
            } else {
                reply(client, &room_id, room_commands::WEB_ONLY).await;
            }
        }
        RoomCommand::Unknown(word) => {
            reply(client, &room_id, &room_commands::unknown(&word)).await;
        }
    }
}

// Callbacks SHOULD be stand-alone functions

// Whenever we see a new stripped room member event, we've asked our client to
// call this function. So what exactly are we doing then?
pub async fn on_stripped_state_member(
    _room_member: StrippedRoomMemberEvent,
    _client: Client,
    room: Room,
) {
    // The bot never auto-joins: it creates its rooms and invites the respondents itself.
    info!("Invite seen for room {}, not auto-joining", room.room_id());
}
// This fn is called whenever we see a new room message event. You notice that
// the difference between this and the other function that we've given to the
// handler lies only in their input parameters. However, that is enough for the
// rust-sdk to figure out which one to call and only do so, when the parameters
// are available.
