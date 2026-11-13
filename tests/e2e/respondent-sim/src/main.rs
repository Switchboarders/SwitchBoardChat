#![recursion_limit = "512"]
//! respondent-sim: one scriptable Matrix device acting like a volunteer's Element, for the end-to-end tests.
//!
//! Usage: respondent-sim <homeserver> <user-id-localpart> <password> <store-dir> <device-name> [--cross-sign] [--no-auto-verify] [--start-sas]
//!
//! Events are printed on stdout, one JSON object per line:
//!   {"event":"ready","device":"…"}                  logged in and syncing
//!   {"event":"joined","room":"!…"}                  accepted an invitation
//!   {"event":"message","room":"!…","sender":"@…","body":"…"}   a message it could read
//!   {"event":"utd","room":"!…","sender":"@…"}       a message it could NOT decrypt
//!   {"event":"removed","room":"!…"}                 it was kicked / left / the room was closed for it
//!   {"event":"typing","room":"!…","users":["@…"]}
//!   {"event":"verification","state":"…"}            bridge-initiated verification progress
//!   {"event":"sent","room":"!…","ok":true|false,"error":"…"}   result of a "send" command
//!   {"event":"rooms","rooms":[{"room":"!…","state":"joined","name":"…"}]}
//! Commands are read from stdin, one JSON object per line:
//!   {"cmd":"send","room":"!…","text":"…"}  {"cmd":"typing","room":"!…","on":true}
//!   {"cmd":"discard","room":"!…"}  {"cmd":"rooms"}  {"cmd":"quit"}
//!   {"cmd":"cancel_sas","on":true}  the volunteer presses "Cancel" at the emoji step
use matrix_sdk::{
    config::SyncSettings,
    encryption::verification::{SasState, Verification, VerificationRequest, VerificationRequestState},
    ruma::{
        events::{
            key::verification::request::ToDeviceKeyVerificationRequestEvent,
            room::{
                encrypted::OriginalSyncRoomEncryptedEvent,
                member::{MembershipState, StrippedRoomMemberEvent, SyncRoomMemberEvent},
                message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
            },
            typing::SyncTypingEvent,
        },
        OwnedRoomId, RoomId,
    },
    Client, Room, RoomState,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};

/// When set, the volunteer presses "Cancel" instead of confirming the emoji (cmd "cancel_sas")
static CANCEL_SAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn emit(v: Value) {
    println!("{v}");
}

async fn handle_request(request: VerificationRequest, start_sas: bool) {
    if let Err(e) = request.accept().await {
        emit(json!({"event":"verification","state":format!("accept failed: {e}")}));
        return;
    }
    emit(json!({"event":"verification","state":"accepted"}));
    let mut stream = request.changes();
    if start_sas {
        // Element X: pressing "Start verification" sends "ready" then starts the emoji comparison itself
        match request.start_sas().await {
            Ok(Some(_)) => emit(json!({"event":"verification","state":"started by us"})),
            other => emit(json!({"event":"verification","state":format!("start failed: {:?}", other.err())})),
        }
    }
    while let Some(state) = stream.next().await {
        match state {
            VerificationRequestState::Transitioned { verification } => {
                if let Verification::SasV1(sas) = verification {
                    tokio::spawn(async move {
                        let _ = sas.accept().await;
                        let mut changes = sas.changes();
                        while let Some(s) = changes.next().await {
                            match s {
                                SasState::KeysExchanged { .. } if CANCEL_SAS.load(std::sync::atomic::Ordering::SeqCst) => {
                                    let _ = sas.cancel().await;
                                    emit(json!({"event":"verification","state":"cancelled by the volunteer"}));
                                }
                                SasState::KeysExchanged { .. } => {
                                    // A volunteer compares the emoji and says they match
                                    let _ = sas.confirm().await;
                                    emit(json!({"event":"verification","state":"emoji confirmed"}));
                                }
                                SasState::Done { .. } => {
                                    emit(json!({"event":"verification","state":"done"}));
                                    break;
                                }
                                SasState::Cancelled(info) => {
                                    emit(json!({"event":"verification","state":format!("cancelled: {}", info.reason())}));
                                    break;
                                }
                                _ => {}
                            }
                        }
                    });
                }
            }
            VerificationRequestState::Done => {
                emit(json!({"event":"verification","state":"done"}));
                break;
            }
            VerificationRequestState::Cancelled(info) => {
                emit(json!({"event":"verification","state":format!("cancelled: {} (by us: {}, code {})", info.reason(), info.cancelled_by_us(), info.cancel_code())}));
                break;
            }
            VerificationRequestState::Ready { .. } => emit(json!({"event":"verification","state":"ready"})),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).with_ansi(false).init();
    }
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: respondent-sim <homeserver> <localpart> <password> <store-dir> <device-name> [--cross-sign]");
        std::process::exit(2);
    }
    let (homeserver, user, password, store, name) = (&args[1], &args[2], &args[3], &args[4], &args[5]);
    let cross_sign = args.iter().any(|a| a == "--cross-sign");
    // A person answers a verification prompt on ONE of their devices; the others just show it
    let auto_verify = Arc::new(std::sync::atomic::AtomicBool::new(!args.iter().any(|a| a == "--no-auto-verify")));
    // --start-sas: behave like Element X, which starts the emoji comparison itself after accepting
    let start_sas = args.iter().any(|a| a == "--start-sas");

    let client = Client::builder()
        .homeserver_url(homeserver)
        .sqlite_store(store, None)
        .build()
        .await?;
    client
        .matrix_auth()
        .login_username(user, password)
        .initial_device_display_name(name)
        .await?;

    // Invitations are accepted at once (Synapse's auto-accept does the same for real respondents)
    client.add_event_handler(|ev: StrippedRoomMemberEvent, room: Room, client: Client| async move {
        if Some(ev.state_key.as_ref()) == client.user_id() && room.state() == RoomState::Invited {
            let mut tries = 0;
            while room.join().await.is_err() && tries < 5 {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            emit(json!({"event":"joined","room":room.room_id().to_string()}));
        }
    });
    let auto_a = auto_verify.clone();
    client.add_event_handler(move |ev: OriginalSyncRoomMessageEvent, room: Room, client: Client| {
        let auto_verify = auto_a.clone();
        async move {
        if Some(ev.sender.as_ref()) == client.user_id() {
            return;
        }
        match &ev.content.msgtype {
            MessageType::VerificationRequest(_) => {
                if !auto_verify.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                if let Some(request) = client.encryption().get_verification_request(&ev.sender, &ev.event_id).await {
                    emit(json!({"event":"verification","state":"request in room"}));
                    tokio::spawn(handle_request(request, start_sas));
                }
            }
            other => {
                emit(json!({"event":"message","room":room.room_id().to_string(),"sender":ev.sender.to_string(),"body":other.body()}));
            }
        }
        }
    });
    let auto_b = auto_verify.clone();
    client.add_event_handler(move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
        let auto_verify = auto_b.clone();
        async move {
        if !auto_verify.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some(request) = client.encryption().get_verification_request(&ev.sender, &ev.content.transaction_id).await {
            emit(json!({"event":"verification","state":"request to device"}));
            tokio::spawn(handle_request(request, start_sas));
        }
        }
    });
    client.add_event_handler(|ev: OriginalSyncRoomEncryptedEvent, room: Room, client: Client| async move {
        if Some(ev.sender.as_ref()) != client.user_id() {
            emit(json!({"event":"utd","room":room.room_id().to_string(),"sender":ev.sender.to_string()}));
        }
    });
    client.add_event_handler(|ev: SyncRoomMemberEvent, room: Room, client: Client| async move {
        if Some(ev.state_key().as_ref()) == client.user_id() {
            let gone = matches!(ev.membership(), MembershipState::Leave | MembershipState::Ban);
            if gone {
                emit(json!({"event":"removed","room":room.room_id().to_string()}));
            }
        }
    });
    client.add_event_handler(|ev: SyncTypingEvent, room: Room, client: Client| async move {
        let others: Vec<String> = ev
            .content
            .user_ids
            .iter()
            .filter(|u| Some(u.as_ref()) != client.user_id())
            .map(|u| u.to_string())
            .collect();
        if !others.is_empty() {
            emit(json!({"event":"typing","room":room.room_id().to_string(),"users":others}));
        }
    });

    let sync_client = client.clone();
    tokio::spawn(async move {
        let _ = sync_client.sync(SyncSettings::default()).await;
    });
    // One sync round so the device keys are known before anything is sent
    client.sync_once(SyncSettings::default().timeout(std::time::Duration::from_secs(1))).await.ok();

    if cross_sign {
        if let Err(e) = client.encryption().bootstrap_cross_signing_if_needed(None).await {
            eprintln!("cross-signing bootstrap failed: {e}");
        }
    }
    emit(json!({"event":"ready","device":client.device_id().map(|d| d.to_string()).unwrap_or_default()}));

    let client = Arc::new(client);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(cmd) = serde_json::from_str::<Value>(&line) else { continue };
        let room_of = |c: &Value| -> Option<Room> {
            let id: OwnedRoomId = RoomId::parse(c["room"].as_str()?).ok()?;
            client.get_room(&id)
        };
        match cmd["cmd"].as_str() {
            Some("send") => {
                let text = cmd["text"].as_str().unwrap_or("").to_string();
                match room_of(&cmd) {
                    Some(room) => {
                        let result = room.send(RoomMessageEventContent::text_plain(text)).await;
                        emit(json!({"event":"sent","room":room.room_id().to_string(),"ok":result.is_ok(),
                                    "error":result.err().map(|e| e.to_string())}));
                    }
                    None => emit(json!({"event":"sent","ok":false,"error":"unknown room"})),
                }
            }
            Some("typing") => {
                if let Some(room) = room_of(&cmd) {
                    let _ = room.typing_notice(cmd["on"].as_bool().unwrap_or(true)).await;
                }
            }
            Some("discard") => {
                // Element's /discardsession
                if let Some(room) = room_of(&cmd) {
                    let _ = room.discard_room_key().await;
                }
            }
            Some("rooms") => {
                let mut list = vec![];
                for room in client.rooms() {
                    list.push(json!({"room":room.room_id().to_string(),"state":format!("{:?}", room.state()),
                                     "name":room.name().unwrap_or_default()}));
                }
                emit(json!({"event":"rooms","rooms":list}));
            }
            Some("recovery_enable") => {
                // The volunteer saves a recovery key (Element asks for it when signing in on a new device)
                match client.encryption().recovery().enable().await {
                    Ok(key) => emit(json!({"event":"recovery_key","key":key})),
                    Err(e) => emit(json!({"event":"recovery_key","error":e.to_string()})),
                }
            }
            Some("recover") => {
                let key = cmd["key"].as_str().unwrap_or("").to_string();
                let result = client.encryption().recovery().recover(&key).await;
                emit(json!({"event":"recovered","ok":result.is_ok(),"error":result.err().map(|e| e.to_string())}));
            }
            Some("sign_self") => {
                // After entering the recovery key, the new device signs itself with the account's cross-signing keys
                let result = match client.encryption().get_own_device().await {
                    Ok(Some(device)) => device.verify().await.map_err(|e| e.to_string()),
                    other => Err(format!("no own device: {other:?}")),
                };
                emit(json!({"event":"signed","ok":result.is_ok(),"error":result.err()}));
            }
            Some("verify") => {
                // The volunteer opens the bridge's user room and presses "Verify"
                let who = cmd["user"].as_str().unwrap_or("");
                if let Ok(user_id) = matrix_sdk::ruma::UserId::parse(who) {
                    match client.encryption().get_user_identity(&user_id).await {
                        Ok(Some(identity)) => match identity.request_verification().await {
                            Ok(request) => {
                                emit(json!({"event":"verification","state":"requested by us"}));
                                tokio::spawn(handle_request(request, false));
                            }
                            Err(e) => emit(json!({"event":"verification","state":format!("request failed: {e}")})),
                        },
                        other => emit(json!({"event":"verification","state":format!("no identity: {other:?}")})),
                    }
                }
            }
            Some("cancel_sas") => {
                CANCEL_SAS.store(cmd["on"].as_bool().unwrap_or(true), std::sync::atomic::Ordering::SeqCst);
            }
            Some("auto_verify") => {
                auto_verify.store(cmd["on"].as_bool().unwrap_or(true), std::sync::atomic::Ordering::SeqCst);
            }
            Some("quit") => break,
            _ => {}
        }
    }
    Ok(())
}
