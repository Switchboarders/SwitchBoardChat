use crate::auth::Auth;
use crate::persistent_session::PersistentSession;
use crate::rooms;
use crate::rooms::user_id::OwnedUserId;

use anyhow::anyhow;
use chrono::{Local, Timelike};
use matrix_sdk::Room;
use matrix_sdk::RoomMemberships;
use matrix_sdk::ruma::api::client::profile::get_profile;
use matrix_sdk::{
    Client, Result,
    ruma::{
        RoomId, TransactionId, UserId,
        api::client::room::create_room::v3::Request as CreateRoomRequest,
        events::{
            InitialStateEvent,
            room::{
                encryption::RoomEncryptionEventContent, member::MembershipState,
                message::RoomMessageEventContent,
            },
        },
        serde::Raw,
        user_id,
    },
};
use serde_json::{json, value::to_raw_value};
use tokio::time::Duration;
use tokio::time::timeout;

use tracing::{error, info, warn};

// ================================
// ROOM CREATION AND CONFIGURATION
// ================================

pub async fn create_room(
    room_name: String,
    client: Client,
    invites: Vec<String>,
) -> anyhow::Result<String> {
    // Create a private room with a name and invite power-level preset
    let mut request = CreateRoomRequest::new();

    request.name = Some(room_name.into());

    let mut validated_invites: Vec<String> = Vec::new();
    for i in invites {
        let user_id = <&UserId>::try_from(i.as_str())?;

        // You can then try to create a DM or fetch their profile
        let request = get_profile::v3::Request::new(user_id.into());

        match client.send(request).await {
            Ok(_response) => {
                //info!("Display name: {:?}", response.get);
                //println!("Avatar URL: {:?}", response.avatar_url);
                validated_invites.push(i);
            }
            Err(err) => {
                warn!("Failed to fetch profile for {} : {:?}", i, err);
            }
        }
    }

    let invites: Result<Vec<OwnedUserId>, _> = validated_invites
        .into_iter()
        .map(|aninvite| <OwnedUserId>::try_from(aninvite.as_str()))
        .collect();
    let invites = invites?;
    if invites.len() == 0 {
        return Err(anyhow!("No valid invites"));
    }
    request.invite = invites.clone();
    request.preset =
        Some(matrix_sdk::ruma::api::client::room::create_room::v3::RoomPreset::PrivateChat);
    // Encryption and power levels are part of the creation request itself, so the room is
    // encrypted from its very first event. Enabling them in separate requests afterwards
    // could fail (e.g. Synapse rate limiting with 429), leaving an unencrypted room with
    // the respondents already invited, and a second room created on the next retry.
    request.initial_state = vec![
        InitialStateEvent::with_empty_state_key(
            RoomEncryptionEventContent::with_recommended_defaults(),
        )
        .to_raw_any(),
    ];
    // Allow everyone (power level 0) to rename the room. The whole "events" map is
    // given because the server replaces it rather than merging it with its defaults.
    // Only the bot (100) invites: a respondent must not be able to bring someone else into a
    // caller's conversation (that person would read it and could text the caller).
    request.power_level_content_override = Some(Raw::from_json(to_raw_value(&json!({
        "invite": 100,
        "events": {
            "m.room.name": 0,
            "m.room.power_levels": 100,
            "m.room.history_visibility": 100,
            "m.room.canonical_alias": 50,
            "m.room.avatar": 50,
            "m.room.tombstone": 100,
            "m.room.server_acl": 100,
            "m.room.encryption": 100
        }
    }))?));

    info!("Asking matrix server for room creation",);
    // Generous timeout: matrix-sdk waits and retries by itself when rate limited (429),
    // and giving up while a creation is still in flight could leave an orphan room.
    let room;
    match timeout(Duration::from_secs(120), client.create_room(request)).await {
        Ok(aroom) => {
            room = aroom?;
        }
        Err(_) => {
            error!("Timed out when creating room.");
            return Err(anyhow!("Timed out when creating room."));
        }
    }
    let room_id = room.room_id();
    info!("Room created with encryption at {}.", get_formatted_time());

    info!("Created room {}", room_id);
    info!("Encryption {:?}", client.encryption().tracked_users().await);
    /*
    // Verify encryption state for all invites
    for user_id in invites {
        let devices = client
            .encryption()
            .get_user_devices(<&UserId>::try_from(user_id.as_str())?)
            .await?;
        println!("Device {:?}", devices);
    }*/

    Ok(room_id.to_string())
}
pub async fn do_user_exist(client: &Client, user_id: &UserId) -> bool {
    // You can then try to create a DM or fetch their profile
    let request = get_profile::v3::Request::new(user_id.into());

    match client.send(request).await {
        Ok(_response) => {
            return true;
        }
        Err(_err) => {
            return false;
        }
    }
}
pub async fn do_user_is_from_homeserver(client: &Client, user_id: &UserId) -> bool {
    let homeserver = client.homeserver();
    let servername = homeserver.host_str().unwrap_or("error");
    let colon_index;
    match user_id.as_str().find(":") {
        Some(index) => colon_index = index,
        None => {
            return false;
        }
    }
    let user_id_servername = &user_id.as_str()[colon_index + 1..];

    return servername == user_id_servername;
}

// ================================
// ROOM MEMBERSHIP MANAGEMENT
// ================================
/// Remove all room participants except for the staying participant and the caller
pub async fn remove_other_room_participants(
    session: &PersistentSession,
    room_id: &RoomId,
    staying_participant: OwnedUserId,
) -> Result<()> {
    info!(
        "Removing every participants from room {} except {}",
        room_id, staying_participant
    );
    let auth = Auth::new();
    let auth_user = auth.get_username();
    let client = session.get_client().await;
    let room = client.get_room(&room_id).ok_or_else(|| {
        matrix_sdk::Error::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Room not found",
        ))
    })?;
    // Note, the system may try to kick already kicked room members. This is ok.
    let members = room.members(RoomMemberships::all()).await;
    match members {
        Ok(memberlist) => {
            for member in memberlist {
                if member.user_id() == &auth_user {
                    continue;
                }
                if member.user_id() != staying_participant {
                    info!(
                        "Removing (kicking gently) {} from room {} (one-to-one room command initiated)",
                        member.user_id(),
                        room_id
                    );
                    match room.kick_user(member.user_id(),Some("Discussion seul à seul initiée / One-to-one room command initiated.")).await{
                        Ok(_) => {},
                        Err(err) => {error!("Could not kick user {}",err);}
                    }
                }
            }
        }
        Err(err) => {
            error!("Error while trying to find room members : {}", err);
        }
    }
    //kick_user(
    Ok(())
}

// Need rework, unused for now
pub async fn update_room_invites(
    session: &PersistentSession,
    room_id: &String,
    invites: Vec<String>,
) -> Result<()> {
    info!("room_id{}", room_id);
    //let room_id = <&RoomId>::try_from(room_id as &str).unwrap ();
    let room_id = RoomId::parse(room_id)?;
    let client = session.get_client().await;
    //session.sync_client_once(&client).await;
    //client.sync_once(SyncSettings::default()).await?;

    // let my_duration: Duration = tokio::time::Duration::from_millis(10000);
    //let mut handles = Vec::new();
    //uninvited =
    let room = client.get_room(&room_id).ok_or_else(|| {
        matrix_sdk::Error::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Room not found",
        ))
    })?;
    // Note, if something strange happen when inviting members, check the definition of this method
    // We remove the invites from this list to get in the final, the list of the now uninvited members of the room to remove them
    //let mut room_invites = room.members_no_sync(RoomMemberships::all()).await.unwrap ();
    let mut room_invites;
    match room.members_no_sync(RoomMemberships::all()).await {
        Ok(invites) => room_invites = invites,
        Err(err) => {
            error!("Error while fetching room members ! {err}");
            return Err(err);
        }
    }

    // Start removing our own user so it does not get kicked out of the room 😉
    if let Some(system_user_id) = client.user_id() {
        room_invites.retain(|x: &matrix_sdk::room::RoomMember| x.user_id() != system_user_id);
    }

    for user_id_str in invites {
        let user_id = UserId::parse(user_id_str)?;
        if !rooms::do_user_exist(&client, &user_id).await {
            warn!("User {} does not exist", user_id);
            continue;
        }
        tracing::debug!("Inviting user: {}", user_id);
        let member_with_no_sync = room
            .get_member_no_sync(&user_id)
            .await
            .unwrap_or_else(|err| {
                error!("Error while getting room member : {err}");
                None
            });
        if let Some(member) = member_with_no_sync {
            tracing::debug!("User {} membership {}", user_id, member.membership());
            match member.membership() {
                MembershipState::Invite => {
                    tracing::debug!("{} already invited", user_id);
                    room_invites.retain(|x: &matrix_sdk::room::RoomMember| x.user_id() != &user_id);
                    continue;
                }
                MembershipState::Join => {
                    tracing::debug!("{} already in the room", user_id);
                    room_invites.retain(|x: &matrix_sdk::room::RoomMember| x.user_id() != &user_id);
                    continue;
                }
                _ => {}
            }
        }
        //handles.push(timeout(my_duration,room.invite_user_by_id(user_id.clone().as_ref())));
        match timeout(
            Duration::from_millis(10000),
            room.invite_user_by_id(&user_id),
        )
        .await
        {
            //Ok(_) => {println!("✓ Successfully invited {}", user_id);},
            //Err(err) =>{println!("{} timed out, err : {}", user_id,err)}
            Ok(Ok(_)) => {
                tracing::debug!("Invited {}", user_id);
                room_invites.retain(|x| x.user_id() != &user_id);
            }
            Ok(Err(e)) => tracing::warn!("Error inviting {}: {:?}", user_id, e),
            Err(_) => tracing::warn!("Timeout inviting {}", user_id),
        };
        //tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        room.sync_members().await.unwrap_or_else(|err| {
            error!("{err}");
        });
        tracing::debug!(
            "Total count of member invited : {}, Are member synced {:?}",
            room.invited_members_count(),
            room.are_members_synced()
        );

        //println!("(After sync)Total count of member invited : {}, Are member synced {:?}",room.invited_members_count(),room.are_members_synced());
    }
    for uninvited_member in room_invites {
        info!(
            "Removing (kicking gently) {} from room {} (call center respondent changed)",
            uninvited_member.user_id(),
            room_id
        );
        match room
            .kick_user(
                uninvited_member.user_id(),
                Some("Changement de répondants / Respondent changed."),
            )
            .await
        {
            Ok(_) => {}
            Err(err) => {
                error!("Could not kick user {}", err);
            }
        }
    }
    Ok(())
}

// ================================
// MESSAGING FUNCTIONS
// ================================

pub async fn send_message_to_room_using_id(
    client: Client,
    message: &str,
    room_id: &str,
) -> anyhow::Result<()> {
    //println!("Rooms : {:?}",self.client.rooms());
    let room_id = <&RoomId>::try_from(room_id)?;
    // TODO Unwrap here mean that the room in the db does not exist on client
    let room: Room;
    match client.get_room(room_id) {
        Some(aroom) => {
            room = aroom;
        }
        None => {
            error!("Message not sent, room id {} does not exist", room_id);
            return Err(anyhow!(
                "Message not sent, room id {} does not exist",
                room_id
            ));
        }
    }

    let content = RoomMessageEventContent::text_plain(message);
    let txn_id = TransactionId::new();
    // send our message to the room we found the "!party" command in
    match timeout(
        Duration::from_millis(10000),
        room.send(content).with_transaction_id(txn_id),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Err:{}", err));
        }
        Err(_) => {
            return Err(anyhow!(
                "Timed out when sending a message to roomid {}",
                room_id
            ));
        }
    }

    Ok(())
}

// ================================
// UTILITY FUNCTIONS
// ================================

pub fn get_formatted_time() -> String {
    // Get the current local time
    let now = Local::now();
    // Extract hour, minute, and second components
    let hour = now.hour();
    let minute = now.minute();
    let second = now.second();
    let ms = now.nanosecond() / 10000000;
    // Print the formatted time
    format!("{:02}:{:02}:{:02}::{:02}", hour, minute, second, ms)
}
