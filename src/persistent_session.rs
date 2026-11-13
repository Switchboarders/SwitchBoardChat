use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::auth::Auth;
use crate::callbacks::on_room_message;
use crate::callbacks::on_stripped_state_member;
use crate::db_manager::DbManager;
use anyhow::{Result, anyhow};

use matrix_sdk::ruma::api::client::session::login::v3::Response;
use matrix_sdk::{
    Client, Error, LoopCtrl, authentication::matrix::MatrixSession, config::SyncSettings,
    ruma::api::client::filter::FilterDefinition,
};
use matrix_sdk::{
    encryption::CrossSigningResetAuthType,
    ruma::{OwnedUserId, api::client::uiaa},
};
use rand::{Rng, distributions::Alphanumeric, thread_rng};
use serde::{Deserialize, Serialize};
use std::{
    env,
    process::exit,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::fs;
use tracing::{debug, error, info, warn};

// ========================================
// DATA STRUCTURES
// ========================================

/// Main persistent session manager for Matrix client
#[derive(Clone)]
pub struct PersistentSession {
    pub client: Option<Client>,
    initial_sync_token: Option<String>,
    session_file: Option<PathBuf>,
    login_response: Option<Response>,
}

impl std::fmt::Debug for PersistentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentSession")
            .field("client", &"<redacted>")
            .field("initial_sync_token", &"<redacted>")
            .field("session_file", &self.session_file)
            .field("login_response", &"<redacted>")
            .finish()
    }
}

/// The data needed to re-build a client
#[derive(Serialize, Deserialize)]
struct ClientSession {
    /// The URL of the homeserver of the user
    homeserver: String,
    /// The path of the database
    db_name: PathBuf,
    /// The passphrase of the database
    passphrase: String,
}

impl std::fmt::Debug for ClientSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSession")
            .field("homeserver", &self.homeserver)
            .field("db_name", &self.db_name)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

/// The full session to persist
#[derive(Serialize, Deserialize)]
struct FullSession {
    /// The data to re-build the client
    client_session: ClientSession,
    /// The Matrix user session
    user_session: MatrixSession,
    /// The latest sync token
    /// It is only needed to persist it when using `Client::sync_once()` and we
    /// want to make our syncs faster by not receiving all the initial sync again
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_token: Option<String>,
}

impl std::fmt::Debug for FullSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullSession")
            .field("client_session", &self.client_session)
            .field("user_session", &"<redacted>")
            .field("sync_token", &"<redacted>")
            .finish()
    }
}

/// Custom event handler context for room messages
#[derive(Debug, Clone)]
pub struct MyRoomMessageContext {
    pub session: PersistentSession,
}

impl PersistentSession {
    // ========================================
    // INITIALIZATION METHODS
    // ========================================

    /// Creates a new PersistentSession instance and initializes the session
    pub async fn new() -> Self {
        let mut instance = PersistentSession {
            client: None,
            initial_sync_token: None,
            session_file: None,
            login_response: None,
        };
        match instance.init_session().await {
            Ok(ok) => ok,
            Err(err) => {
                panic!("{err}");
            }
        }
        instance
    }

    /// Gets the application data directory from environment variable or default
    pub fn get_app_dir(&self) -> String {
        let app_dir = env::var("APP_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
        app_dir
    }

    /// Path of the saved session file (APP_DATA_DIR/session)
    fn session_file_path() -> PathBuf {
        PathBuf::from(env::var("APP_DATA_DIR").unwrap_or_else(|_| "./data".to_string()))
            .join("session")
    }

    /// Initializes the Matrix session by either restoring or creating a new one
    pub async fn init_session(&mut self) -> anyhow::Result<()> {
        let session_file: PathBuf = Self::session_file_path();

        info!("Session file location : {:?}", session_file);

        let (client, sync_token) = if session_file.exists() {
            match self.restore_session(&session_file).await {
                Ok(result) => result,
                Err(err) => {
                    error!(
                        "Failed to restore session from {session_file:?}: {err}. The file is likely empty or corrupt; logging in fresh instead."
                    );
                    (self.login(&session_file).await?, None)
                }
            }
        } else {
            // To force bootstrap, the session files must be deleted
            (self.login(&session_file).await?, None)
        };

        self.client = Some(client);
        self.initial_sync_token = sync_token;
        self.session_file = Some(session_file.clone());
        Ok(())
    }

    // ========================================
    // CLIENT MANAGEMENT METHODS
    // ========================================

    /// Gets the Matrix client instance
    pub async fn get_client(&self) -> Client {
        let client = self.client.clone().unwrap_or_else(|| {
            error!("Could not get Matrix client !");
            exit(1);
        });
        client
    }

    /// Builds a new Matrix client with random database and passphrase
    async fn build_client(&self) -> anyhow::Result<(Client, ClientSession)> {
        let mut rng = thread_rng();

        // Generating a subfolder for the database is not mandatory, but it is useful if
        // you allow several clients to run at the same time. Each one must have a
        // separate database, which is a different folder with the SQLite store.
        let db_subfolder: String = (&mut rng)
            .sample_iter(Alphanumeric)
            .take(7)
            .map(char::from)
            .collect();
        let db_name: PathBuf = db_subfolder.into();

        // Generate a random passphrase.
        let passphrase: String = (&mut rng)
            .sample_iter(Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();

        // We create a loop here so the user can retry if an error happens.
        loop {
            let auth = Auth::new();
            let homeserver: String = auth.get_homeserver_url().to_string();
            let sql_store_path = PathBuf::from(
                self.get_app_dir()
                    + "/db/"
                    + db_name
                        .to_str()
                        .expect("Database name conversion to str failed !"),
            );

            match Client::builder()
                .homeserver_url(&homeserver)
                // We use the SQLite store, which is enabled by default. This is the crucial part to
                // persist the encryption setup.
                // Note that other store backends are available and you can even implement your own.
                .sqlite_store(sql_store_path, Some(&passphrase))
                .build()
                .await
            {
                Ok(client) => {
                    return Ok((
                        client,
                        ClientSession {
                            homeserver,
                            db_name,
                            passphrase,
                        },
                    ));
                }
                Err(error) => match &error {
                    matrix_sdk::ClientBuildError::AutoDiscovery(_)
                    | matrix_sdk::ClientBuildError::Url(_)
                    | matrix_sdk::ClientBuildError::Http(_) => {
                        error!("Error checking the homeserver: {error}");
                        error!("Please try again\n");
                    }
                    _ => {
                        // Forward other errors, it's unlikely we can retry with a different outcome.
                        return Err(error.into());
                    }
                },
            }
        }
    }

    // ========================================
    // AUTHENTICATION METHODS
    // ========================================

    /// Performs login with a new device
    async fn login(&mut self, session_file: &Path) -> anyhow::Result<Client> {
        info!("No previous session found, logging in…");

        let (client, client_session) = self.build_client().await?;
        let matrix_auth = client.matrix_auth();
        let login_auth = Auth::new();

        match matrix_auth
            .login_username(login_auth.get_username(), &login_auth.get_password())
            .initial_device_display_name("persist-session client")
            .await
        {
            Ok(response) => {
                self.login_response = Some(response);
                info!(
                    "Response User ID  {}",
                    self.login_response.clone().unwrap().user_id // Safe to unwrap since we made it into an option on the previous line
                );
                info!("Logged in as {}", login_auth.get_username());
            }
            Err(error) => {
                // Never log the password
                error!("Error logging in as {}: {error}", login_auth.get_username());
                panic!("Could not log in as the bot: wrong MATRIX_USERNAME / MATRIX_PASSWORD, or the homeserver cannot be reached (see the error above)");
            }
        }

        // Persist the session to reuse it later.
        // TODO This is not very secure, for simplicity. If the system provides a way of
        // storing secrets securely, it should be used instead.
        // Note that we could also build the user session from the login response.
        let user_session = matrix_auth
            .session()
            .expect("A logged-in client should have a session");
        let serialized_session = serde_json::to_string(&FullSession {
            client_session,
            user_session,
            sync_token: None,
        })?;
        write_private(session_file, &serialized_session).await?;

        info!("Session persisted in {}", session_file.to_string_lossy());

        Ok(client)
    }

    /// Restores a previous session from the session file
    async fn restore_session(
        &self,
        session_file: &Path,
    ) -> anyhow::Result<(Client, Option<String>)> {
        info!(
            "Previous session found in '{}'",
            session_file.to_string_lossy()
        );

        // The session was serialized as JSON in a file.
        let serialized_session = fs::read_to_string(session_file).await?;
        let FullSession {
            client_session,
            user_session,
            sync_token,
        } = serde_json::from_str(&serialized_session)?;

        // A saved session always wins over .env, so changing MATRIX_USERNAME alone silently
        // keeps running as the old account. Stop and say exactly what to do instead.
        let configured_user = Auth::new().get_username();
        let saved_user = user_session.meta.user_id.as_str();
        let same_user = if configured_user.starts_with('@') {
            configured_user == saved_user
        } else {
            user_session.meta.user_id.localpart() == configured_user
        };
        if !same_user {
            error!(
                "
==================== ACTION REQUIRED: account changed ====================
MATRIX_USERNAME in .env is  : {configured_user}
but the saved session is for: {saved_user}
(saved session file: {session})

To run as {configured_user}:
  1. Delete or rename {session}
     (e.g. rename it to {session}_{saved_localpart} to switch back later)
  2. Restart the program; it will log in as {configured_user}.

To keep running as {saved_user} instead, set MATRIX_USERNAME and
MATRIX_PASSWORD in .env back to that account.
==========================================================================",
                session = session_file.to_string_lossy(),
                saved_localpart = user_session.meta.user_id.localpart(),
            );
            exit(1);
        }

        let sql_store_path = PathBuf::from(
            self.get_app_dir()
                + "/db/"
                + client_session.db_name.to_str().unwrap_or_else(|| {
                    error!(
                        "Error parsing client session database name {:?}",
                        client_session.db_name
                    );
                    exit(1);
                }),
        );

        // Build the client with the previous settings from the session.
        let client = Client::builder()
            .homeserver_url(client_session.homeserver)
            .sqlite_store(sql_store_path, Some(&client_session.passphrase))
            .build()
            .await?;

        info!("Restoring session for {}…", user_session.meta.user_id);

        // Restore the Matrix user session.
        client.restore_session(user_session).await?;

        Ok((client, sync_token))
    }

    // ========================================
    // ENCRYPTION METHODS
    // ========================================

    /// Prints detailed encryption information for debugging
    pub async fn print_encryption_info(&self) {
        let client = self.get_client().await;

        // Never log device keys or the device record here
        let cross_signing_status = client.encryption().cross_signing_status().await;
        tracing::debug!("cross_signing_status = {:?}", cross_signing_status);
        tracing::debug!(
            "can_homeserver_push_encrypted_event_to_device : {:?}",
            client.can_homeserver_push_encrypted_event_to_device().await
        );
    }

    /// Identifies the account whose encryption identity is being reset, e.g.
    /// "@bot:example.org (device ABCDEF) on https://matrix.example.org/"
    fn account_summary(client: &Client, user_id: &OwnedUserId) -> String {
        let device = client
            .device_id()
            .map(|device_id| device_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!("{user_id} (device {device}) on {}", client.homeserver())
    }

    /// Bootstraps cross-signing identity for encryption
    async fn bootstrap(client: Client, user_id: OwnedUserId, password: String) -> Result<()> {
        let account = Self::account_summary(&client, &user_id);
        warn!(
            "
============ ENCRYPTION IDENTITY RESET ============
Account being reset: {account}
(MATRIX_USERNAME in .env, saved in {session})

A reset replaces this account's cross-signing keys: its other sessions
(phones, Element, ...) will have to be verified again.
Wrong account? Stop the program, fix MATRIX_USERNAME in .env, rename
{session}, then restart.
===================================================",
            session = Self::session_file_path().to_string_lossy(),
        );

        if let Some(handle) = client.encryption().reset_cross_signing().await? {
            debug!("Cross-signing reset handle obtained: {handle:?}");
            info!("Cross-signing reset auth type: {:?}", handle.auth_type());

            match handle.auth_type() {
                CrossSigningResetAuthType::Uiaa(uiaa) => {
                    let mut password = uiaa::Password::new(user_id.into(), password);
                    password.session = uiaa.session.clone();
                    info!("Submitting password authentication to reset the identity of {account}");
                    match handle
                        .auth(Some(uiaa::AuthData::Password(password)))
                        .await
                    {
                        Ok(()) => info!("Encryption identity of {account} was reset"),
                        Err(err) => {
                            error!("Password authentication to reset {account} failed: {err}");
                            return Err(err.into());
                        }
                    }
                }
                CrossSigningResetAuthType::OAuth(oauth) => {
                    let account_management_url = Self::get_account_management_url(&client).await;

                    // matrix-sdk polls every 500ms and gives up after 2 minutes.
                    let deadline = chrono::Local::now() + chrono::Duration::minutes(2);
                    warn!(
                        "
============ ACTION REQUIRED: approve the encryption reset ============
Account being reset: {account}
Nothing changes until you approve it below.

  1. Open this page and check which account your browser is logged in as:
       {account_management_url}
     If it is NOT {user_id}, log out and log back in as {user_id}
     (password: MATRIX_PASSWORD in .env). Approving from another
     account only resets THAT account, and this bot will time out.
  2. Open this link and approve the reset:
       {approval_url}
  3. Wait here. You have until {deadline}. On success the program
     stops by itself; just start it again.
========================================================================",
                        approval_url = oauth.approval_url,
                        deadline = deadline.format("%H:%M:%S"),
                    );
                    match handle.auth(None).await {
                        Ok(()) => info!("OAuth approval for cross-signing reset detected"),
                        Err(err) => {
                            error!(
                                "
============ Encryption reset NOT approved (timed out: {err}) ============
No approval for {user_id} arrived within 2 minutes (account: {account}).
Most likely the link was approved while logged in as another account.
  1. Log in as {user_id} at {account_management_url}
  2. Restart the program and approve the new link it prints.
==========================================================================="
                            );
                            return Err(err.into());
                        }
                    }
                    let db_manager = DbManager::new();
                    db_manager.clean_user_rooms().unwrap_or_else(|err| { error!("Database error : {err}"); });
                    info!("Cross signing has ended.");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    info!(
                        "Terminating program gracefully. This should be working now. Please restart."
                    );
                    exit(0);
                }
            }
        } else {
            info!("reset_cross_signing() returned no handle; no cross-signing reset was needed.");
        }

        Ok(())
    }
    pub async fn get_account_management_url(client: &Client) -> String {
        use matrix_sdk::ruma::api::client::discovery::get_authorization_server_metadata::v1::AccountManagementActionData;

        let metadata = client.oauth().server_metadata().await.expect("Could not fetch account management url. Are you sure your server implements Matrix Authentification Service ?");

        match metadata.account_management_url_with_action(AccountManagementActionData::Profile) {
            Some(url) => url.to_string(),
            None => "account_management_url not available".to_string(),
        }
    }
    // ========================================
    // SYNCHRONIZATION METHODS
    // ========================================

    /// Performs a single sync operation with the Matrix server
    pub async fn sync_client_once(&self) -> anyhow::Result<()> {
        let filter = FilterDefinition::with_lazy_loading();
        let sync_settings: SyncSettings = SyncSettings::default().filter(filter.into());

        match self
            .get_client()
            .await
            .sync_once(sync_settings.clone())
            .await
        {
            Ok(response) => {
                let _sync_settings = sync_settings.token(response.next_batch.clone());
                if let Some(session_file) = self.session_file.clone() {
                    let session_file = session_file.clone();
                    self.persist_sync_token(&session_file, response.next_batch)
                        .await?;
                } else {
                    error!("Error while accessing session file path !");
                }
            }
            Err(error) => {
                error!("An error occurred during initial sync: {error}");
                error!("Trying again…");
            }
        }
        Ok(())
    }

    /// Sets up the client to listen to new messages and starts the sync loop
    pub async fn sync(&self) -> anyhow::Result<()> {
        info!("Launching a first sync to ignore past messages…");

        // Enable room members lazy-loading, it will speed up the initial sync a lot
        // with accounts in lots of rooms.
        // See <https://spec.matrix.org/v1.6/client-server-api/#lazy-loading-room-members>.
        let filter = FilterDefinition::with_lazy_loading();

        let mut sync_settings: SyncSettings = SyncSettings::default()
            .filter(filter.into())
            // Without this, the first tick of the ongoing sync loop below still waits
            // the full 30s default long-poll timeout even though nothing new has to
            // arrive, since the server already has nothing to report right after the
            // initial sync. That's what was delaying the bootstrap engage check (and
            // the very first message-processing tick) by up to 30 extra seconds.
            .ignore_timeout_on_first_sync(true);

        // We restore the sync where we left.
        // This is not necessary when not using `sync_once`. The other sync methods get
        // the sync token from the store.
        if let Some(sync_token) = &self.initial_sync_token {
            sync_settings = sync_settings.token(sync_token);
        }

        // Let's ignore messages before the program was launched.
        // This is a loop in case the initial sync is longer than our timeout. The
        // server should cache the response and it will ultimately take less time to
        // receive.
        let client = self.client.clone().unwrap_or_else(|| {
            error!("self.client is None");
            exit(1);
        });
        let session_file: PathBuf = self.session_file.clone().unwrap_or_else(|| {
            error!("Unable to fetch session file path");
            exit(1);
        });

        info!("Starting sync loop");
        loop {
            match client.sync_once(sync_settings.clone()).await {
                Ok(response) => {
                    // This is the last time we need to provide this token, the sync method after
                    // will handle it on its own.
                    sync_settings = sync_settings.token(response.next_batch.clone());
                    let session_file = session_file.clone();
                    self.persist_sync_token(&session_file, response.next_batch)
                        .await?;
                    break;
                }
                Err(error) => {
                    error!("An error occurred during initial sync: {error}");
                    error!("Trying again…");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }

        info!("The client is ready! Listening to new messages…");

        client.add_event_handler_context(MyRoomMessageContext {
            session: self.clone(),
        });

        // Now that we've synced, let's attach handlers for incoming events
        client.add_event_handler(on_room_message);
        client.add_event_handler(crate::callbacks::on_undecryptable);
        client.add_event_handler(crate::attention::on_typing);
        client.add_event_handler(on_stripped_state_member);
        // Does nothing right now. I wonder why .....
        //UserVerificationHandler::sync(client.clone()).await.unwrap (); // event handler for any verification request from other users

        // For bootstrapping
        let client_ref = &client;
        let asked = AtomicBool::new(false);
        let asked_ref = &asked;

        // This loops until we kill the program or an error happens.
        client
            .sync_with_result_callback(sync_settings, move |sync_result| {
                // This will engage bootstrapping process. For self.login_response to be called, the login function
                // must be called too! The easiest way to do that is delete the session files and the db. The
                // system will be forced to call the login method for file recreation.
                match self.login_response.clone() {
                    Some(aresponse) => {
                        let asked = asked_ref;
                        // Runs on every sync tick once logged in (login_response is never
                        // cleared). Only log while bootstrap hasn't been engaged yet, so
                        // this doesn't repeat forever once it's done its job.
                        if !asked.load(Ordering::SeqCst) {
                            debug!("Login response present, checking whether bootstrap is needed");
                        }
                        let user_id = aresponse.user_id;
                        let client = &client_ref;
                        let user_id = &user_id;
                        let auth = Auth::new();

                        // Wait for sync to be done then ask the user to bootstrap.
                        if !asked.load(Ordering::SeqCst) {
                            info!("Engage bootstrap for {}", user_id);
                            let client = (*client).clone();
                            let user_id = (*user_id).clone().to_owned();
                            let password = auth.get_password().to_owned();
                            tokio::spawn(async move {
                                // A fresh login must never reset an identity that already exists on the server:
                                // that would force every respondent to re-trust the bridge. The keys of the
                                // existing identity are copied into the new store instead (docs/encryption-recovery.md).
                                let force_reset = matches!(
                                    std::env::var("BRIDGE_RESET_CROSS_SIGNING").unwrap_or_default().to_lowercase().as_str(),
                                    "true" | "1" | "yes" | "on"
                                );
                                if !force_reset {
                                    // Give the first sync a moment, then look at what the server already holds
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                    if let Ok(Some(_)) = client.encryption().request_user_identity(&user_id).await {
                                        warn!(
                                            "The server already holds a cross-signing identity for {user_id}: NOT resetting it. Stop the bridge and copy the identity into this new store with `crypto-doctor import-identity` (docs/encryption-recovery.md). Set BRIDGE_RESET_CROSS_SIGNING=true to reset it on purpose."
                                        );
                                        return;
                                    }
                                }
                                if let Err(err) = Self::bootstrap(client, user_id, password).await
                                {
                                    error!("Bootstrap (cross-signing reset) task failed: {err:#}");
                                }
                            });
                        }

                        asked.store(true, Ordering::SeqCst);
                    }
                    None => {}
                }

                // Persistence
                let cloned_session_file = session_file.clone();
                async move {
                    // A sync iteration can fail transiently (network blip, timeout,
                    // momentary homeserver issue). Retrying here instead of
                    // propagating keeps the sync loop (and the whole process, since
                    // main.rs's try_join! tears everything down on any single task
                    // error) alive. A short sleep guards against hot-looping if the
                    // failure turns out to not be transient after all.
                    let response = match sync_result {
                        Ok(response) => response,
                        Err(error) => {
                            error!("Sync iteration failed, retrying: {error}");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            return Ok(LoopCtrl::Continue);
                        }
                    };

                    // We persist the token each time to be able to restore our session
                    self.persist_sync_token(&cloned_session_file, response.next_batch)
                        .await
                        .map_err(|err| Error::UnknownError(err.into()))?;

                    Ok(LoopCtrl::Continue)
                }
            })
            .await?;

        Err(anyhow!("Unreachable ?????"))
    }

    // ========================================
    // PERSISTENCE METHODS
    // ========================================

    /// Persists the sync token for a future session
    /// Note that this is needed only when using `sync_once`. Other sync methods get
    /// the sync token from the store.
    async fn persist_sync_token(
        &self,
        session_file: &PathBuf,
        sync_token: String,
    ) -> anyhow::Result<()> {
        let serialized_session = fs::read_to_string(&session_file).await?;
        let mut full_session: FullSession = serde_json::from_str(&serialized_session)?;

        full_session.sync_token = Some(sync_token);
        let serialized_session = serde_json::to_string(&full_session)?;
        write_private(&session_file, &serialized_session).await?;

        Ok(())
    }
}

/// Writes a file only its owner can read (0600): the session holds the access token and the
/// crypto store passphrase. An existing file with looser rights is tightened too.
async fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600)).await?;
    file.write_all(contents.as_bytes()).await?;
    file.flush().await
}

#[cfg(test)]
mod private_file_tests {
    #[tokio::test]
    async fn session_file_is_owner_only_even_if_it_existed_before() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sbchat-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_private(&path, "new").await.unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
