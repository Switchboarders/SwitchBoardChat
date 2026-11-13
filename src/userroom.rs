use crate::{db_manager::DbManager, rooms};
use anyhow::anyhow;
use matrix_sdk::{Client, ruma::UserId};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Whether a respondent account exists, remembered for an hour: the verification loop runs every 30 s and must not
/// ask the homeserver about every respondent each time (an account that does not exist cost ~100 lookups an hour).
static VALIDITY: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const VALIDITY_TTL: Duration = Duration::from_secs(3600);
pub struct UserRoom {
    client: Client,
}
impl UserRoom {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
    pub async fn send_message_to_user_room(
        &self,
        message: &String,
        user_id: &String,
    ) -> anyhow::Result<()> {
        let room_id = match self.get_user_verification_room_if_exist(user_id).await? {
            Some(some) => some,
            None => match self.create_user_verification_room(user_id).await {
                Ok(ok) => ok,
                Err(err) => {
                    error!("Error creating room while sending a message to {user_id} : {err}");
                    return Err(anyhow!(
                        "Error creating room while sending a message to {user_id} : {err}"
                    ));
                }
            },
        };
        if rooms::send_message_to_room_using_id(self.client.clone(), &message, &room_id.clone())
            .await
            .is_err()
        {
            error!("Could not send message to the user room of {user_id}");
        }
        Ok(())
    }

    pub async fn is_user_valid(&self, user_id: &String) -> bool {
        if let Some((at, valid)) = VALIDITY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(user_id) {
            if at.elapsed() < VALIDITY_TTL {
                return *valid;
            }
        }
        let valid = self.check_user_valid(user_id).await;
        if !valid {
            warn!(
                "Respondent {user_id} is listed in a call center but is not a valid account on this server: remove or fix it in SwitchBoard (checked again in an hour)"
            );
        }
        VALIDITY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user_id.clone(), (Instant::now(), valid));
        valid
    }

    async fn check_user_valid(&self, user_id: &String) -> bool {
        match UserId::parse(user_id.clone()) {
            Ok(user_id) => {
                if !rooms::do_user_is_from_homeserver(&self.client.clone(), &user_id).await {
                    warn!(
                        "User {} has invalid homeserver name or is from another matrix server",
                        user_id
                    );
                    return false;
                }

                if !rooms::do_user_exist(&self.client.clone(), &user_id).await {
                    warn!("User {} does not exist", user_id.clone());
                    return false;
                } else {
                    return true;
                }
            }
            Err(err) => {
                warn!("Parsing error for user {} : {} ", user_id.clone(), err);
                return false;
            }
        }
    }
    pub async fn get_user_verification_room_if_exist(
        &self,
        user_id: &String,
    ) -> anyhow::Result<Option<String>> {
        let db = DbManager::new();
        // Check if verification room already exist for this user
        let room_id = db.get_user_room_id_by_username(user_id.clone())?;
        if let Some(room_id) = &room_id {
            info!("Verification room pre-existant : {}", room_id);
        }
        Ok(room_id)
    }
    pub async fn create_user_verification_room(&self, user_id: &String) -> anyhow::Result<String> {
        let db = DbManager::new();
        info!(
            "No verification room for this user, creating one for {}",
            user_id.clone()
        );
        let result = rooms::create_room(
            "User room / Salle Utilisateur".to_string(),
            self.client.clone(),
            vec![user_id.clone()],
        )
        .await; // TODO this unwrap can fail easily

        let room_id = match result {
            Ok(ok) => {
                info!("Room created. Room id : {}", ok);
                ok
            }

            Err(err) => {
                error!("{}", err);
                return Err(anyhow!(err));
            }
        };
        db.set_user_room_username_room_id(user_id.clone(), room_id.clone())
            .unwrap_or_else(|err| { error!("Database error : {err}"); });
        return Ok(room_id);
    }
}
