//! Startup self-check of the bridge's encryption identity.
//!
//! Problem it catches: the bridge's local encryption store (`db/`) and the keys Synapse holds for the bridge's
//! device no longer match (typically after `session` and `db/` were restored from different moments).
//! Respondents' Element then cannot open an encrypted channel with the bridge: their messages vanish
//! (red in Element) and nothing is logged on the bridge's side. The check turns that into loud log lines.
//! Recovery steps: docs/encryption-recovery.md

use matrix_sdk::ruma::api::client::discovery::get_authorization_server_metadata::v1::{AccountManagementActionData, DeviceDeleteData};
use matrix_sdk::ruma::api::client::keys::get_keys;
use matrix_sdk::ruma::encryption::DeviceKeys;
use matrix_sdk::Client;
use std::collections::BTreeMap;
use tracing::{error, info, warn};

/// Returns true when everything is in order (so the caller knows whether to look again later).
pub async fn check(client: Client) -> bool {
    let (Some(user_id), Some(device_id)) = (client.user_id().map(|u| u.to_owned()), client.device_id().map(|d| d.to_owned())) else {
        return false;
    };
    // Refresh the own identity so the local view of cross-signing is not stale
    if let Err(err) = client.encryption().request_user_identity(&user_id).await {
        warn!("Crypto check: could not refresh our own identity: {err}");
    }

    let Some(local_key) = client.encryption().ed25519_key().await else {
        warn!("Crypto check: no local device key yet");
        return false;
    };
    let mut query = BTreeMap::new();
    query.insert(user_id.clone(), vec![device_id.clone()]);
    let mut request = get_keys::v3::Request::new();
    request.device_keys = query;
    let server_key = match client.send(request).await {
        Ok(response) => response
            .device_keys
            .get(&user_id)
            .and_then(|devices| devices.get(&device_id))
            .and_then(|raw| raw.deserialize_as_unchecked::<DeviceKeys>().ok())
            .and_then(|keys| {
                keys.keys
                    .iter()
                    .find(|(id, _)| id.to_string().starts_with("ed25519:"))
                    .map(|(_, key)| key.clone())
            }),
        Err(err) => {
            warn!("Crypto check: could not ask the server for our device keys: {err}");
            return false;
        }
    };
    let keys_ok = matches!(&server_key, Some(k) if *k == local_key);
    match server_key {
        Some(server_key) if server_key == local_key => {
            info!("Crypto check: OK, the device key held by the server matches the local encryption store ({device_id})");
        }
        Some(server_key) => {
            error!(
                "CRYPTO CHECK FAILED: the encryption store in db/ belongs to a different device key ({}…) than the one Synapse holds for {device_id} ({}…). Respondents' Element cannot open a secure channel with the bridge: their messages will vanish without an error. The `session` file and `db/` come from different moments. See docs/encryption-recovery.md",
                &local_key.chars().take(8).collect::<String>(),
                &server_key.chars().take(8).collect::<String>()
            );
        }
        None => warn!("Crypto check: the server holds no device keys for {device_id}"),
    }

    let complete = matches!(client.encryption().cross_signing_status().await, Some(status) if status.is_complete());
    if complete {
        info!("Crypto check: the bridge holds its cross-signing keys");
    } else {
        warn!(
            "Crypto check: the bridge does NOT hold all its cross-signing keys: it can only trust respondents' devices one at a time, and verifying a person will not cover their other devices. See docs/encryption-recovery.md"
        );
    }
    let trusted = ensure_own_identity_trusted(&client).await;
    if trusted {
        info!("Crypto check: our own identity is trusted");
    }
    let devices_ok = check_own_devices(&client).await;
    keys_ok && complete && trusted && devices_ok
}

/// Makes sure the bridge signs its own device and trusts its own identity when it holds the cross-signing keys.
/// Without that, every person the bridge verified before looks unverified (only devices verified one by one count).
/// Called at startup and before every verification. Returns true when the bridge's own identity is trusted.
pub async fn ensure_own_identity_trusted(client: &Client) -> bool {
    let Some(user_id) = client.user_id().map(|u| u.to_owned()) else {
        return false;
    };
    let complete = matches!(client.encryption().cross_signing_status().await, Some(status) if status.is_complete());
    let mut device_signed = false;
    if let Ok(Some(device)) = client.encryption().get_own_device().await {
        device_signed = device.is_cross_signed_by_owner();
        if !device_signed && complete {
            // Holding the cross-signing keys, the bridge signs its own device. The server checks the signature
            // against the self-signing key it holds, so success also proves our keys are the ones it knows.
            match device.verify().await {
                Ok(()) => {
                    info!("Crypto check: signed our own device with our self-signing key");
                    device_signed = true;
                }
                Err(err) => warn!("Crypto check: could not sign our own device: {err}"),
            }
        }
    }
    match client.encryption().get_user_identity(&user_id).await {
        Ok(Some(own)) if own.is_verified() => true,
        Ok(Some(own)) if complete && device_signed => match own.verify().await {
            Ok(()) => {
                info!("Crypto check: our own identity was not trusted locally; trusted it again (people verified before count again)");
                true
            }
            Err(err) => {
                error!("Crypto check: could not trust our own identity: {err}");
                false
            }
        },
        Ok(Some(_)) => {
            warn!("Crypto check: our own identity is NOT trusted and cannot be fixed automatically (cross-signing keys missing): people will look unverified. See docs/encryption-recovery.md");
            false
        }
        _ => false,
    }
}

/// Every device of the bridge must be cross-signed. Element X refuses to send a message to a person
/// you verified while they have a device that is not ("VoipSMS has unverified devices"): respondents
/// who verified the bridge could then not write to it until they press "Send anyway". Such a device
/// is typically left behind by an earlier login of the bridge. It is signed out through MAS when the
/// bridge has MAS admin access (it cannot delete its own devices itself), otherwise reported loudly.
/// The bridge never signs those devices itself: that would also trust a device made with a stolen password.
/// Returns true when no such device is left.
pub async fn check_own_devices(client: &Client) -> bool {
    let (Some(user_id), Some(own_device_id)) = (client.user_id().map(|u| u.to_owned()), client.device_id().map(|d| d.to_owned())) else {
        return false;
    };
    let devices = match client.encryption().get_user_devices(&user_id).await {
        Ok(devices) => devices,
        Err(err) => {
            warn!("Crypto check: could not list the bridge's devices: {err}");
            return false;
        }
    };
    let total = devices.devices().count();
    let unsigned: Vec<String> = devices
        .devices()
        .filter(|device| device.device_id() != own_device_id && !device.is_cross_signed_by_owner())
        .map(|device| device.device_id().to_string())
        .collect();
    if unsigned.is_empty() {
        info!("Crypto check: every device of the bridge is cross-signed ({total} device(s))");
        return true;
    }

    let mut remaining = unsigned.clone();
    match crate::mas_admin::MasAdmin::connect(client).await {
        Ok(Some(mas)) => match mas.find_user(user_id.localpart()).await {
            Ok(Some(mas_user)) => {
                for device_id in &unsigned {
                    match mas.end_device_session(&mas_user.id, device_id).await {
                        Ok(true) => {
                            info!("Crypto check: signed out the bridge's unverified device {device_id} (left by an earlier login)");
                            remaining.retain(|d| d != device_id);
                        }
                        Ok(false) => warn!("Crypto check: no active MAS session found for the bridge's device {device_id}"),
                        Err(err) => warn!("Crypto check: could not sign out the bridge's device {device_id}: {err}"),
                    }
                }
            }
            Ok(None) => warn!("Crypto check: the bridge's user {user_id} was not found in MAS"),
            Err(err) => warn!("Crypto check: could not look up the bridge's user in MAS: {err}"),
        },
        Ok(None) => {}
        Err(err) => warn!("Crypto check: MAS admin client not working: {err}"),
    }
    if remaining.is_empty() {
        return true;
    }

    let removal_links = match client.oauth().server_metadata().await {
        Ok(metadata) => remaining
            .iter()
            .filter_map(|device_id| {
                let device_id: matrix_sdk::ruma::OwnedDeviceId = device_id.as_str().into();
                metadata
                    .account_management_url_with_action(AccountManagementActionData::DeviceDelete(DeviceDeleteData::new(&device_id)))
                    .map(|url| format!("\n  {device_id}: {url}"))
            })
            .collect::<String>(),
        Err(_) => String::new(),
    };
    error!(
        "CRYPTO CHECK: the bridge {user_id} has {} device(s) that are not cross-signed: {}. Respondents who verified the bridge \
        cannot write to it from Element X (\"has unverified devices\") until they press \"Send anyway\". Sign these devices out: \
        open each link below while logged in to MAS as {user_id} (or set MAS_ADMIN_CLIENT_ID / MAS_ADMIN_CLIENT_SECRET so the \
        bridge does it by itself).{removal_links}",
        remaining.len(),
        remaining.join(", ")
    );
    false
}
