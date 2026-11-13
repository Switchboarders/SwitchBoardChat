use std::time::Duration;

use crate::DbManager;
use crate::PersistentSession;
use crate::rooms;
use crate::userroom::UserRoom;
use anyhow::anyhow;
use anyhow::bail;

use matrix_sdk::encryption::identities::Device;
use matrix_sdk::encryption::verification::Emoji;
use matrix_sdk::encryption::verification::SasState;
use matrix_sdk::encryption::verification::format_emojis;
use matrix_sdk::ruma::events::key::verification::VerificationMethod;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::room::message::OriginalSyncRoomMessageEvent;
use matrix_sdk::{
    Client,
    encryption::verification::{
        SasVerification,
        Verification,
        VerificationRequest,
        VerificationRequestState,
        // QrVerification,
    },
    ruma::{
        UserId,
        events::key::verification::{
            request::ToDeviceKeyVerificationRequestEvent, start::ToDeviceKeyVerificationStartEvent,
        },
    },
};
use tokio::time;
use tokio_stream::StreamExt;
use tracing::error;
use tracing::info;

/// Seconds we leave the other side, after it accepted our request, to start the emoji comparison itself.
/// VERIFY_START_DELAY_SECS overrides it (the end-to-end tests set 0 to force both sides to start at once).
fn start_sas_delay() -> Duration {
    Duration::from_secs(std::env::var("VERIFY_START_DELAY_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(3))
}

/// Tells the volunteer, once per verification, that it was interrupted and how to start again. A verification can be
/// cancelled without anybody doing anything wrong: for instance both sides starting the emoji comparison in the same
/// instant makes Matrix cancel both. Without this notice the volunteer's app would just stop.
#[derive(Clone)]
struct CancelNotice {
    client: Client,
    room_id: String,
    sent: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CancelNotice {
    async fn send(&self, reason: &str) {
        if self.sent.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        info!("Verification interrupted ({reason}), telling the volunteer to start again");
        let text = "⚠️ La vérification a été interrompue. Écrivez n'importe quoi ici pour recommencer, puis acceptez la nouvelle demande. / The verification was interrupted. Write anything here to start again, then accept the new request.";
        if let Err(err) = rooms::send_message_to_room_using_id(self.client.clone(), text, &self.room_id).await {
            error!("Could not tell the volunteer that the verification was interrupted: {err}");
        }
    }
}

pub struct UserVerificationHandler {
    client: Client,
}
pub struct UserDevicesInfo {
    devices_description: String,
    are_all_devices_verified: bool,
    some_of_the_device_are_verified: bool,
}

impl std::fmt::Debug for UserDevicesInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserDevicesInfo")
            .field("devices_description", &"<redacted>")
            .field("are_all_devices_verified", &self.are_all_devices_verified)
            .field("some_of_the_device_are_verified", &self.some_of_the_device_are_verified)
            .finish()
    }
}
impl UserVerificationHandler {
    // ================================
    // CONSTRUCTOR
    // ================================

    pub fn new(client: Client) -> Self {
        Self { client }
    }

    // ================================
    // PUBLIC API METHODS
    // ================================
    /// Deprecated
    pub async fn test_verification(&self) -> anyhow::Result<()> {
        // In case the current sms sender user has many sessions, we delete all the others and keep the current
        // Can happen when resetting the identity
        // Not supported with oauth
        //self.clean_own_devices().await.unwrap ();

        loop {
            let db = DbManager::new();
            let respondant_list_opt = match db.get_all_respondant_user_id(false) {
                Ok(list) => list,
                Err(err) => {
                    error!("Database error, retrying in 30 s : {err}");
                    time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            let respondant_list = match respondant_list_opt {
                Some(some) => some,
                None => {
                    tracing::debug!("No respondent in db yet");
                    time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            for user in respondant_list {
                //let user = "@alice:example.org";
                let user_id = &UserId::parse(&user)?;
                let devices = self
                    .client
                    .clone()
                    .encryption()
                    .get_user_devices(user_id)
                    .await?;
                let userdeviceinfo_unchecked =
                    Self::get_user_devices_info(user_id, &self.client).await;
                if let Ok(userdeviceinfo) = userdeviceinfo_unchecked {
                    tracing::debug!("Device info fetched");
                } else {
                    error!(
                        "Error while fetching user device info : {}",
                        userdeviceinfo_unchecked.unwrap_err()
                    );
                }

                for device in devices.devices() {
                    if !device.is_verified() {
                        info!("Verifying Device with id {}", device.device_id());
                        let client = self.client.clone();
                        let user_id = user_id.clone();
                        tokio::spawn(async move {
                            let handler = UserVerificationHandler::new(client);
                            let result =
                                handler.request_device_verification(&user_id, device).await;
                            tracing::debug!("Verification request for {} finished, ok: {}", &user_id, result.is_ok());
                        });
                        time::sleep(Duration::from_secs(160)).await;
                    }
                }
            }
            time::sleep(Duration::from_secs(30)).await;
        }
    }
    pub async fn send_verification_messages(&self, only_new_devices: bool) -> anyhow::Result<()> {
        let db = DbManager::new();
        let user_room = UserRoom::new(self.client.clone());
        let respondant_list_opt = db.get_all_respondant_user_id(false).map_err(|err| {
            error!("Database error : {err}");
            anyhow::anyhow!("Database error while listing respondents : {err}")
        })?;
        let respondant_list = match respondant_list_opt {
            Some(some) => some,
            None => {
                tracing::debug!("No respondent in database yet.");
                return Ok(());
            }
        };

        for user in respondant_list {
            // Does the user is valid and exist ?
            if !user_room.is_user_valid(&user).await {
                continue;
            }

            let parsed_userid = &UserId::parse(user.clone());
            let user_devices_info: UserDevicesInfo;
            if let Ok(parsed_userid_ok) = parsed_userid {
                let user_devices_info_unchecked =
                    Self::get_user_devices_info(parsed_userid_ok, &self.client).await;
                let Ok(user_devices_info_checked) = user_devices_info_unchecked else {
                    error!(
                        "Error while fetching user device info : {}",
                        user_devices_info_unchecked.unwrap_err()
                    );
                    continue;
                };
                user_devices_info = user_devices_info_checked;
            } else {
                error!("Error parsing user id : {:?}", parsed_userid.as_ref().err());
                continue;
            }

            if user_devices_info.are_all_devices_verified {
                continue;
            }

            let existing_room = match user_room.get_user_verification_room_if_exist(&user).await {
                Ok(existing) => existing,
                Err(err) => {
                    error!("Database error while looking for a verification room : {err}");
                    continue;
                }
            };
            let room_id = match existing_room {
                Some(some) => some,
                None => {
                    let room = match user_room.create_user_verification_room(&user).await {
                        Ok(ok) => {
                            let message_to_room_using_id_result = rooms::send_message_to_room_using_id(self.client.clone(), "Répondez à ce message pour commencer la vérification / Answer this message to start verification", &ok.clone())
                            .await;
                            if message_to_room_using_id_result.is_err() {
                                error!(
                                    "Error sending message to room : {}",
                                    message_to_room_using_id_result
                                        .err()
                                        .unwrap_or(anyhow!("No error message"))
                                );
                                continue;
                            }
                            ok
                        }
                        Err(err) => {
                            error!("Error : {err}");
                            continue;
                        }
                    };
                    room
                }
            };
            // This option is useful to sign up users more quickly and to avoid sending 3600 messages a day ;)
            if only_new_devices {
                tracing::debug!("Skipping {} (only_new_devices is true)", user);
                continue;
            }

            // Next here we send a message via the room to invite user to verify
            // we will then listen to the room and verify when the user send a message

            let mut message = "Voici l'état de vérification de vos appareils\nHere is your devices verification status \n".to_string();
            message.push_str(&user_devices_info.devices_description);
            message.push_str("\nPour vérifier votre appareil, veuillez répondre à ce message (vous pouvez écrire n'importe quoi). On vous demandera de confirmer des emojis. Veuillez confirmer que les emojis sont pareils.\nTo verify your devices, please answer this message (you can write down anything). You will be asked to confirm emojis. Please confirm that the emojis are the same.");
            if user_devices_info.some_of_the_device_are_verified {
                message.push_str("\n❗ Certains de vos appareils ne sont pas contre-signés. Soyez certain de bien contre-signer vos appareils entre eux ❗\n❗ Some of your devices are not cross-signed. Make sure all your devices are cross-signed together ❗\n");
                let get_account_management_url =
                    PersistentSession::get_account_management_url(&self.client).await;
                message.push_str(format!("Vous pouvez toujours gérer vos appareils à l'adresse suivante. Pour ne plus recevoir ce message, veuillez supprimer les appareils non vérifiés si vous n'y avez pas accès (ou co-signez les si vous y avez accès).\n{}\nYou can always manage your devices at the following address. To stop receiving this message, please delete the devices to which you no longer have access(or cross-sign them if you have access).\n{}",get_account_management_url,get_account_management_url).as_str());
            }
            info!("Sending a verification message to {}", user);

            if rooms::send_message_to_room_using_id(self.client.clone(), &message, &room_id.clone())
                .await
                .is_err()
            {
                error!("Could not send a verification message to {}", user);
            }
        }

        Ok(())
    }

    pub async fn verify_user_identity(&self, user_id: &UserId, user_room_id: &str) {
        let client = self.client.clone();
        // Our own identity must be trusted first, or everyone we verified before looks unverified again
        crate::crypto_health::ensure_own_identity_trusted(&client).await;
        // ... and every device of the bridge signed, or Element X blocks the person's messages to it
        crate::crypto_health::check_own_devices(&client).await;
        let useridentify = client.encryption().get_user_identity(user_id).await;
        let Ok(user) = useridentify else {
            error!("Error getting user id {}", useridentify.unwrap_err());
            return;
        };
        let user_devices_info_unchecked = Self::get_user_devices_info(user_id, &client).await;
        let Ok(user_devices_info) = user_devices_info_unchecked else {
            error!(
                "Error fetching user device info : {:?}",
                user_devices_info_unchecked.unwrap_err()
            );
            return;
        };
        if user_devices_info.are_all_devices_verified {
            info!("User {:?} already verified", user_id);
            let message = user_devices_info.devices_description
                + "Tous vos appareils sont vérifiés 👍 / All your devices are verified 👍";
            let message_result =
                rooms::send_message_to_room_using_id(client.clone(), &message, user_room_id).await;
            if message_result.is_err() {
                error!(
                    "Error while sending message : {}",
                    message_result.unwrap_err()
                );
            }
            return;
        }
        if user_devices_info.some_of_the_device_are_verified {
            // Do not stop here: the volunteer wrote because something is not verified, so also ask now.
            info!(
                "User {:?} partially verified, listing the devices and asking for a verification",
                user_id
            );
            for text in [
                user_devices_info.devices_description.as_str(),
                "Certains de vos appareils ne sont pas vérifiés ⚠️ / Some of your devices are unverified ⚠️\n\
                 Une demande de vérification vient d'être envoyée : acceptez-la sur l'appareil marqué d'un ❌ que vous voulez vérifier, puis comparez les émojis. \
                 Pour tous les vérifier d'un coup, vérifiez chaque session dans Element (Paramètres → Sessions → Vérifier la session) à l'aide d'un appareil déjà vérifié, puis écrivez de nouveau ici. / \
                 A verification request was just sent: accept it on the device marked ❌ that you want to verify, then compare the emoji. \
                 To verify them all at once, verify each session in Element (Settings → Sessions → Verify session) using a device that is already verified, then write here again.",
            ] {
                if let Err(err) = rooms::send_message_to_room_using_id(client.clone(), text, user_room_id).await {
                    error!("Error while sending message : {err}");
                }
            }
        }
        if let Some(user) = user {
            // Let's request the user to be verified.
            let verification_request = user.request_verification().await;
            let Ok(verification) = verification_request else {
                error!(
                    "Error while sending verification request : {}",
                    verification_request.unwrap_err()
                );
                return;
            };
            let verification_accepted = verification.accept().await;
            if verification_accepted.is_err() {
                error!(
                    "Error while accepting verification : {}",
                    verification_accepted.unwrap_err()
                );
                return;
            }
            let notice = CancelNotice {
                client: self.client.clone(),
                room_id: user_room_id.to_string(),
                sent: Default::default(),
            };
            Self::drive_request(self.client.clone(), verification, Some(notice)).await;
        }
    }

    /// Follows a verification request we sent until it is done.
    ///
    /// Both sides may start the emoji comparison at the same moment (Element X does it as soon as the volunteer
    /// presses "Start verification"). The protocol then keeps the start of the lexicographically smaller user,
    /// which is almost always the volunteer's (@a… < @bot), and the request switches to that new comparison.
    /// We must keep listening and answer it; stopping after our own start left the volunteer's phone spinning forever.
    /// To make the race rare, we also wait a few seconds after "ready" so the other side can start first.
    async fn drive_request(client: Client, verification: VerificationRequest, notice: Option<CancelNotice>) {
        let mut stream = verification.changes();
        let deadline = time::sleep(Duration::from_secs(600));
        tokio::pin!(deadline);
        loop {
            let state = tokio::select! {
                state = stream.next() => match state { Some(state) => state, None => break },
                _ = &mut deadline => {
                    info!("Verification with {} still unfinished after 10 minutes, no longer following it", verification.other_user_id());
                    break;
                }
            };
            match state {
                VerificationRequestState::Created { .. } | VerificationRequestState::Requested { .. } => (),
                VerificationRequestState::Ready { .. } => {
                    let request = verification.clone();
                    tokio::spawn(async move {
                        time::sleep(start_sas_delay()).await;
                        // Still only "ready": the other side did not start, so we do
                        if request.is_ready() {
                            match request.start_sas().await {
                                Ok(Some(_)) => info!("Started the emoji comparison with {}", request.other_user_id()),
                                Ok(None) => error!("Could not start the emoji comparison with {}", request.other_user_id()),
                                Err(err) => error!("Could not start the emoji comparison with {} : {err}", request.other_user_id()),
                            }
                        }
                    });
                }
                // Emitted for our own start and again when the other side's start replaces ours
                VerificationRequestState::Transitioned { verification: Verification::SasV1(sas) } => {
                    // A comparison replaced by the other side's never moves again: do not follow it forever
                    tokio::spawn(time::timeout(
                        Duration::from_secs(600),
                        Self::sas_verification_handler(client.clone(), sas, notice.clone()),
                    ));
                }
                VerificationRequestState::Transitioned { .. } => (),
                VerificationRequestState::Done => break,
                VerificationRequestState::Cancelled(cancel_info) => {
                    error!("Verification Request was canceled {:?}", cancel_info);
                    if let Some(notice) = &notice {
                        notice.send(cancel_info.reason()).await;
                    }
                    break;
                }
            }
        }
    }
    pub async fn request_device_verification(
        &self,
        user_id: &UserId,
        device: Device,
    ) -> anyhow::Result<()> {
        let device = self
            .client
            .encryption()
            .get_device(user_id, device.device_id())
            .await?;

        // We don't want to support showing a QR code, we only support SAS
        // verification
        let methods = vec![VerificationMethod::SasV1];

        if let Some(device) = device {
            let verification = device.request_verification_with_methods(methods).await?;
            let verification_accepted = verification.accept().await;
            if verification_accepted.is_err() {
                let err = verification_accepted.unwrap_err();
                error!(
                    "Verification failed, could not accept verification : {:?}",
                    err
                );
                bail!(
                    "Verification failed, could not accept verification : {}",
                    err
                );
            }
            Self::drive_request(self.client.clone(), verification, None).await;
        }
        Ok(())
    }
    // Not working right now...
    async fn request_verification_handler(client: Client, request: VerificationRequest) {
        tracing::debug!(
            "Accepting verification request from {}",
            request.other_user_id()
        );
        request
            .accept()
            .await
            .expect("Can't accept verification request");

        let mut stream = request.changes();

        while let Some(state) = stream.next().await {
            match state {
                VerificationRequestState::Created { .. }
                | VerificationRequestState::Requested { .. }
                | VerificationRequestState::Ready { .. } => (),
                VerificationRequestState::Transitioned { verification } => {
                    // We only support SAS verification.
                    if let Verification::SasV1(s) = verification {
                        tokio::spawn(Self::sas_verification_handler(client, s, None));
                        break;
                    }
                }
                VerificationRequestState::Done | VerificationRequestState::Cancelled(_) => break,
            }
        }
    }
    pub async fn sync(client: Client) -> matrix_sdk::Result<()> {
        info!("Listening for verification requests.");
        client.add_event_handler(
            |ev: ToDeviceKeyVerificationRequestEvent, client: Client| async move {
                info!("Verification request received from {:?}", &ev.sender);
                let request = client
                    .encryption()
                    .get_verification_request(&ev.sender, &ev.content.transaction_id)
                    .await
                    .expect("Request object wasn't created");

                tokio::spawn(Self::request_verification_handler(client, request));
            },
        );

        client.add_event_handler(
            |ev: OriginalSyncRoomMessageEvent, client: Client| async move {
                info!("Verification request received from {:?}", &ev.sender);

                if let MessageType::VerificationRequest(_) = &ev.content.msgtype {
                    let request = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.event_id)
                        .await
                        .expect("Request object wasn't created");

                    tokio::spawn(Self::request_verification_handler(client, request));
                }
            },
        );

        //client.sync(SyncSettings::new()).await?;

        Ok(())
    }
    // ================================
    // SAS VERIFICATION HANDLERS
    // ================================

    async fn sas_verification_handler(client: Client, sas: SasVerification, notice: Option<CancelNotice>) {
        info!(
            "Starting verification with {} {}",
            &sas.other_device().user_id(),
            &sas.other_device().device_id()
        );

        let device_info_unchecked =
            Self::get_user_devices_info(sas.other_device().user_id(), &client).await;
        if let Ok(device_info) = device_info_unchecked {
            tracing::debug!("Device info fetched");
        }
        let sas_accept = sas.accept().await;
        if sas_accept.is_err() {
            error!(
                "Error while accepting SAS request : {}",
                sas_accept.unwrap_err()
            );
            return;
        }
        let mut stream = sas.changes();

        while let Some(state) = stream.next().await {
            match state {
                SasState::KeysExchanged {
                    emojis,
                    decimals: _,
                } => {
                    tokio::spawn(Self::wait_for_confirmation(
                        sas.clone(),
                        emojis
                            .expect("We only support verifications using emojis")
                            .emojis,
                    ));
                }
                SasState::Done { .. } => {
                    let device = sas.other_device();

                    info!(
                        "Successfully verified device {} {} {:?}",
                        device.user_id(),
                        device.device_id(),
                        device.local_trust_state()
                    );
                    // A device that belongs to the person's cross-signing identity vouches for the whole
                    // identity: trust it, so ALL of the person's devices are verified, whichever one they used
                    // to answer (otherwise the volunteer would be asked to confirm again on each device).
                    let signed_by_owner = match client.encryption().get_device(device.user_id(), device.device_id()).await {
                        Ok(Some(d)) => d.is_cross_signed_by_owner(),
                        _ => false,
                    };
                    if signed_by_owner {
                        match client.encryption().get_user_identity(device.user_id()).await {
                            Ok(Some(identity)) if !identity.is_verified() => match identity.verify().await {
                                Ok(()) => info!("Verified the whole identity of {}", device.user_id()),
                                Err(err) => error!("Could not verify the identity of {} : {err}", device.user_id()),
                            },
                            _ => {}
                        }
                    }
                    let device_info_unchecked =
                        Self::get_user_devices_info(sas.other_device().user_id(), &client).await;
                    if let Ok(device_info) = device_info_unchecked {
                        tracing::debug!("Device info fetched");
                    }

                    break;
                }
                SasState::Cancelled(cancel_info) => {
                    tracing::debug!(
                        "The verification has been cancelled, reason: {}",
                        cancel_info.reason()
                    );
                    if let Some(notice) = &notice {
                        notice.send(cancel_info.reason()).await;
                    }
                    break;
                }
                SasState::Created { .. }
                | SasState::Started { .. }
                | SasState::Accepted { .. }
                | SasState::Confirmed => (),
            }
        }
    }

    async fn wait_for_confirmation(sas: SasVerification, emoji: [Emoji; 7]) {
        /*println!("\nDo the emojis match: \n{}", format_emojis(emoji));
        print!("Confirm with `yes` or cancel with `no`: ");
        std::io::stdout()
            .flush()
            .expect("We should be able to flush stdout");

        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("error: unable to read user input");


        match input.trim().to_lowercase().as_ref() {
            "yes" | "true" | "ok" => sas.confirm().await.unwrap (),
            _ => sas.cancel().await.unwrap (),
        }*/
        // Automatic confirmation. Security issue but needed to make it all work..
        let sas_confirm = sas.confirm().await;
        if sas_confirm.is_err() {
            error!("SAS own confirmation failed , {}", sas_confirm.unwrap_err());
        }
    }

    // ================================
    // DEVICE MANAGEMENT UTILITIES
    // ================================

    async fn clean_own_devices(&self) -> anyhow::Result<()> {
        if let Some(user_id) = self.client.user_id() {
            let devices = self
                .client
                .clone()
                .encryption()
                .get_user_devices(user_id)
                .await?;

            if let Some(current_device_id) = self.client.device_id() {
                for device in devices.devices() {
                    if device.device_id() != current_device_id {
                        match self
                            .client
                            .delete_devices(&[device.device_id().to_owned()], None)
                            .await
                        {
                            Ok(_) => {
                                info!("Unused device {} cleaned and deleted", device.device_id())
                            }
                            Err(e) => {
                                if let Some(_info) = e.as_uiaa_response() {
                                    // deprecated and does not work anyway
                                    /*let auth = Auth::new();
                                    let mut password = uiaa::Password::new(
                                        uiaa::UserIdentifier::UserIdOrLocalpart(
                                            auth.get_username().to_string(),
                                        ),
                                        auth.get_password().to_string(),
                                    );
                                    password.session = info.session.clone();
                                    let result = self
                                        .client
                                        .delete_devices(
                                            &[device.device_id().to_owned()],
                                            Some(uiaa::AuthData::Password(password)),
                                        )
                                        .await;*/
                                } else {
                                    error!(
                                        "Could not delete/clean own unused device {}, device_id {} Cleaning device is not supported while using matrix authentification system
                                        Please clean unused devices manually by logging directly to your matrix authentification server(eg. matrix.myserver.com/auth).",
                                        e,
                                        device.device_id()
                                    )
                                }
                            }
                        }
                    }
                }
                return Ok(());
            }
            return Err(anyhow!("Cant acceed current_device_id"));
        }
        Err(anyhow!("Cant acceed own devices"))
    }

    // ================================
    // DISPLAY/DEBUG UTILITIES
    // ================================

    async fn get_user_devices_info(
        user_id: &UserId,
        client: &Client,
    ) -> anyhow::Result<UserDevicesInfo> {
        let mut devices_description = String::new();
        tracing::debug!("Listing the devices of {user_id}");
        let mut are_all_devices_verified = true;
        let mut some_of_the_device_are_verified = false;

        let userdevices_unchecked = client.encryption().get_user_devices(user_id).await;
        let Ok(userdevices) = userdevices_unchecked else {
            let err = userdevices_unchecked.unwrap_err();
            error!("Error while checking user device : {}", err);
            bail!(err);
        };
        for device in userdevices.devices() {
            if device.device_id()
                == client
                    .device_id()
                    .expect("We should be logged in now and know our device id")
            {
                continue;
            }
            if device.is_verified() {
                some_of_the_device_are_verified = true;
            } else {
                are_all_devices_verified = false;
            }
            let line = format!(
                "   {:<10} {:<30} {:<} \n",
                device.device_id(),
                device.display_name().unwrap_or("-"),
                if device.is_verified() { "✅" } else { "❌" }
            );
            devices_description.push_str(&line);
        }
        Ok(UserDevicesInfo {
            devices_description,
            are_all_devices_verified,
            some_of_the_device_are_verified,
        })
    }
}
