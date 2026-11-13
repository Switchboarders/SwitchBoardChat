use crate::fromqueuemanager::FromQueueManagerEntry;
use crate::toqueuemanager::ToQueueManagerEntry;
use crate::{crypto, web_server::QueryParams};

use chrono::NaiveDateTime;
use matrix_sdk::ruma::RoomId;
use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::{Connection, Result, params};
use serde::Deserialize;
use serde::Serialize;
use std::process::exit;
use std::{fs, path::PathBuf};

use chrono::DateTime;
use chrono::Utc;
use std::env;
use std::time::SystemTime;
use tracing::{debug, error, info};

pub struct DbManager {
    path: PathBuf,
}
pub struct CallCenter {
    pub id_call_center: i64,
    pub sb_chat_api_token: String,
    pub sb_chat_api_token_hashed: String,
    pub voip_ms_username: String,
    pub voip_ms_api_password: String,
    pub voip_ms_api_url: String,
    pub call_center_did: u64,
    pub privilege_level: u64,
}

impl std::fmt::Debug for CallCenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallCenter")
            .field("id_call_center", &self.id_call_center)
            .field("sb_chat_api_token", &"<redacted>")
            .field("sb_chat_api_token_hashed", &"<redacted>")
            .field("voip_ms_username", &"<redacted>")
            .field("voip_ms_api_password", &"<redacted>")
            .field("voip_ms_api_url", &self.voip_ms_api_url)
            .field("call_center_did", &"<redacted>")
            .field("privilege_level", &self.privilege_level)
            .finish()
    }
}
/// One signature of the confidentiality agreement (see src/agreement.rs)
#[derive(Serialize, Deserialize, Clone)]
pub struct AgreementSignature {
    pub respondant_username: String,
    pub typed_name: String,
    pub version: String,
    pub text_sha256: String,
    /// Unix seconds, from the Matrix server's timestamp of the signing message
    pub signed_at: i64,
    pub event_id: String,
    pub device_id: Option<String>,
    pub device_verified: bool,
}

impl std::fmt::Debug for AgreementSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgreementSignature")
            .field("respondant_username", &self.respondant_username)
            .field("typed_name", &"<redacted>")
            .field("version", &self.version)
            .field("text_sha256", &self.text_sha256)
            .field("signed_at", &self.signed_at)
            .field("event_id", &self.event_id)
            .field("device_id", &self.device_id)
            .field("device_verified", &self.device_verified)
            .finish()
    }
}
#[derive(Serialize, Deserialize, Debug)]
pub struct Macro {
    pub id_macro: Option<u64>,
    pub id_call_center: u64,
    pub macro_name: String,
    pub macro_text: String,
    pub macro_tags: Option<String>,
    pub macro_tags_config: Option<String>,
}
impl DbManager {
    // ========================================
    // INITIALIZATION AND SETUP METHODS
    // ========================================

    /// Creates a new DbManager instance with the database path
    pub fn new() -> Self {
        let data_dir = env::var("APP_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
        let db_folder = data_dir + "/db/";

        fs::create_dir_all(&db_folder).unwrap_or_else(|err| {
            error!("Error creating directory {db_folder} : {err}");
            exit(1);
        });
        // Owner only: the database holds encrypted numbers, queued texts and API secrets
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&db_folder, fs::Permissions::from_mode(0o700));
        }
        let path: PathBuf = (db_folder + "users.db").into();
        Self { path }
    }

    /// Logs the current database path
    pub fn log_db_path(&self) {
        info!("Db path : {:?}", self.path);
    }
    /// Seals every call center's API token and refreshes its hash (schema upgrade, idempotent)
    pub fn seal_call_center_tokens(&self) -> Result<()> {
        let db_manager = self;
        let existing_call_centers;

        match db_manager.get_existing_call_centers() {
            Ok(option) => match option {
                Some(call_centers) => existing_call_centers = call_centers,
                None => {
                    error!("No call center entered yet");
                    return Ok(());
                }
            },
            Err(err) => {
                error!("DB Error {}", err);
                return Ok(());
            }
        }

        // Tokens are looked up by their hash; the stored copy (returned to SwitchBoard when it asks
        // for an existing call center) is encrypted. Older databases kept it in clear: seal it.
        let conn = Connection::open(&self.path)?;
        for a_call_center in existing_call_centers {
            conn.execute(
                "UPDATE call_center SET sb_chat_api_token = ?1, sb_chat_api_token_hashed = ?2 WHERE id_call_center = ?3",
                params![
                    crypto::seal_secret(&a_call_center.sb_chat_api_token),
                    crypto::hash_token(&a_call_center.sb_chat_api_token),
                    a_call_center.id_call_center
                ],
            )?;
        }
        Ok(())
    }
    /// Adds a column to an existing table if it is not there yet (schema upgrade of
    /// databases created by an older version; `create table if not exists` won't do it).
    fn add_column_if_missing(
        conn: &Connection,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let exists = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<String>>>()?
            .iter()
            .any(|name| name == column);
        if !exists {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                (),
            )?;
            info!("Database upgraded: added column {table}.{column}");
        }
        Ok(())
    }

    /// Quebec Law 25 upgrade, safe to run at every start:
    /// - room.did_number becomes a keyed hash (lookup) and the real number is kept encrypted
    ///   in room.did_enc, only because replies need it while the conversation is active
    /// - denied_did and caller_notified keep only the hash; auto_sms_log (7 days of rate
    ///   limit counters) is simply emptied
    /// - voip.ms passwords are stored encrypted
    fn migrate_law25(conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(conn, "room", "did_enc", "blob")?;
        Self::add_column_if_missing(conn, "room", "channel", "text default 'sms'")?;
        let tx = conn.unchecked_transaction()?;

        let legacy_rooms: Vec<(String, i64)> = {
            let mut stmt =
                tx.prepare("SELECT id_room, did_number FROM room WHERE did_enc IS NULL")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_>>()?
        };
        for (id_room, did) in legacy_rooms {
            let did = did as u64;
            let channel = if crate::web_chat::is_web_did(did) { "web" } else { "sms" };
            tx.execute(
                "UPDATE room SET did_enc = ?1, channel = ?2, did_number = ?3 WHERE id_room = ?4",
                params![crypto::encrypt_did(did), channel, crypto::hash_did(did), id_room],
            )?;
        }

        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 2 {
            for (table, id_column) in [
                ("denied_did", "id_denied_did"),
                ("caller_notified", "id_caller_notified"),
            ] {
                let rows: Vec<(i64, i64)> = {
                    let mut stmt =
                        tx.prepare(&format!("SELECT {id_column}, did_number FROM {table}"))?;
                    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<_>>()?
                };
                for (id, did) in rows {
                    tx.execute(
                        &format!("UPDATE {table} SET did_number = ?1 WHERE {id_column} = ?2"),
                        params![crypto::hash_did(did as u64), id],
                    )?;
                }
            }
            tx.execute("DELETE FROM auto_sms_log", [])?;
            tx.execute_batch("PRAGMA user_version = 2")?;
            info!("Database upgraded for Law 25: phone numbers are now hashed or encrypted");
        }

        let passwords: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id_call_center, voip_ms_api_password FROM call_center
                 WHERE voip_ms_api_password IS NOT NULL AND voip_ms_api_password NOT LIKE 'enc:%'",
            )?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_>>()?
        };
        for (id, password) in passwords {
            tx.execute(
                "UPDATE call_center SET voip_ms_api_password = ?1 WHERE id_call_center = ?2",
                params![crypto::seal_secret(&password), id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Creates all required database tables if they don't exist
    pub fn create_db_if_not_exist(&self) -> Result<()> {
        info!("Db Path : {:?}", self.path);
        let conn = Connection::open(&self.path)?;

        conn.execute(
            "create table if not exists call_center (
             id_call_center integer primary key autoincrement,
             sb_chat_api_token text not null unique,
             sb_chat_api_token_hashed text not null,
             voip_ms_username text,
             voip_ms_api_password text,
             voip_ms_api_url text,
             call_center_did integer,
             privilege_level integer DEFAULT 0
         )",
            (),
        )?;
        conn.execute(
            "create table if not exists macros (
             id_macro integer primary key autoincrement,
             id_call_center integer,
             macro_name text not null,
             macro_text text not null,
             macro_tags text,
             macro_tags_config text
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists call_center_did (
             id_call_center_did integer primary key autoincrement,
             id_call_center integer not null,
             did_number integer not null unique
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists room (
             id_room text primary key,
             id_call_center integer not null,
             did_number integer not null,
             call_center_did integer,
             creation_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
             last_accessed_time timestamp,
             is_in_one_to_one INTEGER DEFAULT 0,
             unique(id_room,did_number)
         )",
            (),
        )?;
        conn.execute(
            "create table if not exists user_room (
             id_user_room integer primary key autoincrement,
             respondant_username text not null unique,
             id_room text not null unique
         )",
            (),
        )?;
        conn.execute(
            "create table if not exists respondant (
             id_respondant integer primary key autoincrement,
             id_call_center integer not null,
             respondant_username text not null,
             last_accessed_time timestamp,
	         is_active INTEGER DEFAULT 0
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists caller_did (
             id_caller_did integer primary key autoincrement,
             id_respondant integer not null,
             did_number integer not null
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists from_voipms_queue (
             id_from_voipms_queue integer primary key autoincrement,
             voipms_to text not null,
             voipms_from text not null,
             voipms_message text not null,
             voipms_id text not null unique,
             timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
             sbchatapikey text not null,
             processing_level text not null,
             error_log text
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists to_voipms_queue (
             id_to_voipms_queue integer primary key autoincrement,
             send_message_url text not null,
             timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
             processing_level text not null,
             error_log text
         )",
            (),
        )?;

        // Web chat block list: keyed hash of the visitor's network address, until a date. No reason,
        // no content, no readable address. Expired rows are purged.
        conn.execute(
            "create table if not exists web_block (
             ip_hash text primary key,
             blocked_until integer not null,
             created_at integer not null
         )",
            (),
        )?;

        // Small key/value state of the web chat (e.g. when browser block markers were last reset)
        conn.execute(
            "create table if not exists web_state (key text primary key, value integer not null)",
            (),
        )?;

        conn.execute(
            "create table if not exists denied_did (
             id_denied_did integer primary key autoincrement,
             id_call_center integer not null,
             did_number integer not null,
             denied_at integer,
             denied_by text,
             unique(id_call_center,did_number)
         )",
            (),
        )?;
        // Upgrade databases created before these columns existed
        Self::add_column_if_missing(&conn, "denied_did", "denied_at", "integer")?;
        // Queued texts are stored encrypted; these keyed hashes of the number let erasure find them
        Self::add_column_if_missing(&conn, "from_voipms_queue", "from_hash", "integer")?;
        Self::add_column_if_missing(&conn, "to_voipms_queue", "dst_hash", "integer")?;
        Self::add_column_if_missing(&conn, "denied_did", "denied_by", "text")?;
        // Call centers without voip.ms (web chat only) are identified by SwitchBoard's reference instead
        Self::add_column_if_missing(&conn, "call_center", "committee_ref", "text")?;
        // Websites linked to a call center by a pairing code (web chat), and the one-time codes
        conn.execute(
            "create table if not exists web_site (
             origin text primary key,
             id_call_center integer not null,
             linked_at integer not null
         )",
            (),
        )?;
        conn.execute(
            "create table if not exists web_pairing (
             code_hash text primary key,
             id_call_center integer not null,
             expires_at integer not null
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists caller_notified (
             id_caller_notified integer primary key autoincrement,
             id_call_center integer not null,
             did_number integer not null,
             notified_at integer not null,
             unique(id_call_center,did_number)
         )",
            (),
        )?;

        conn.execute(
            "create table if not exists auto_sms_log (
             id_auto_sms_log integer primary key autoincrement,
             id_call_center integer not null,
             did_number integer not null,
             kind text not null,
             sent_at integer not null
         )",
            (),
        )?;
        // Volunteers' signatures of the confidentiality agreement (src/agreement.rs). Proof of consent, so rows are
        // never changed; a new version of the text is signed again (new row).
        conn.execute(
            "create table if not exists agreement_signature (
             id_agreement_signature integer primary key autoincrement,
             respondant_username text not null,
             typed_name text not null,
             version text not null,
             text_sha256 text not null,
             signed_at integer not null,
             event_id text not null,
             device_id text,
             device_verified integer not null default 0,
             unique(respondant_username, text_sha256)
         )",
            (),
        )?;
        Self::migrate_law25(&conn)?;
        self.seal_call_center_tokens()?;
        // Create admin account
        tracing::info!("Creating admin account");
        self.create_or_update_call_center("ADMIN", "ADMIN", 1234561234, 101, false)
            .unwrap();
        Ok(())
    }

    // ========================================
    // CALL CENTER MANAGEMENT METHODS
    // ========================================

    /// Retrieves call center credentials by ID
    pub fn get_call_center_credentials(
        &self,
        call_center_id: u64,
    ) -> anyhow::Result<(String, String)> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT voip_ms_username, voip_ms_api_password FROM call_center WHERE id_call_center = (?1)")?;
        let mut rows = stmt.query(params![call_center_id])?;

        if let Ok(Some(row)) = rows.next() {
            let voip_ms_username: String = row.get(0)?;
            let voip_ms_api_password: String = crypto::open_secret(&row.get::<_, String>(1)?);
            return Ok(((voip_ms_username, voip_ms_api_password)));
        } else {
            return Err(anyhow::anyhow!(
                "Could not find call center {} in table call_center",
                call_center_id
            ));
        }
    }

    /// Gets call center ID by API token
    pub fn get_call_center_id_by_token(&self, token: String) -> Result<Option<(u64, u64)>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_call_center, privilege_level FROM call_center WHERE sb_chat_api_token_hashed = (?1)")?;
        let mut rows = stmt.query(params![crypto::hash_token(&token)])?;

        if let Ok(Some(row)) = rows.next() {
            let id_call_center: u64 = row.get(0)?;
            let privilege_level: u64 = row.get(1)?;
            return Ok(Some((id_call_center, privilege_level)));
        } else {
            return Ok(None);
        }
    }
    pub fn get_call_center_id_by_hashed_token(
        &self,
        hashed_token: String,
    ) -> Result<Option<(u64, u64)>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_call_center, privilege_level FROM call_center WHERE sb_chat_api_token_hashed = (?1)")?;
        let mut rows = stmt.query(params![hashed_token])?;

        if let Ok(Some(row)) = rows.next() {
            let id_call_center: u64 = row.get(0)?;
            let privilege_level: u64 = row.get(1)?;
            return Ok(Some((id_call_center, privilege_level)));
        } else {
            return Ok(None);
        }
    }
    pub fn delete_call_center_by_token(&self, token: String) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        let hashed = crypto::hash_token(&token);
        // its websites stop opening web chats, its unused pairing codes go too
        conn.execute(
            "DELETE FROM web_site WHERE id_call_center IN (SELECT id_call_center FROM call_center WHERE sb_chat_api_token_hashed = ?1)",
            params![hashed],
        )?;
        conn.execute(
            "DELETE FROM web_pairing WHERE id_call_center IN (SELECT id_call_center FROM call_center WHERE sb_chat_api_token_hashed = ?1)",
            params![hashed],
        )?;
        conn.execute("DELETE FROM call_center WHERE sb_chat_api_token_hashed = ?1", params![hashed])?;
        crate::web_sites::reload();
        Ok(())
    }
    /// Gets API token by call center ID
    pub fn get_call_center_token_by_id(&self, id: i64) -> Result<Option<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT sb_chat_api_token FROM call_center WHERE id_call_center = (?1)")?;
        let mut rows = stmt.query(params![id])?;

        if let Ok(Some(row)) = rows.next() {
            let token: String = crypto::open_secret(&row.get::<_, String>(0)?);
            return Ok(Some(token));
        } else {
            return Ok(None);
        }
    }

    /// Retrieves all existing call centers
    pub fn get_existing_call_centers(&self) -> Result<Option<Vec<CallCenter>>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_call_center,sb_chat_api_token,voip_ms_username,voip_ms_api_password,voip_ms_api_url,call_center_did,privilege_level FROM call_center")?;

        let mut rows = stmt.query(params![])?;
        let mut all_call_center: Vec<CallCenter> = Vec::new();

        while let Ok(Some(row)) = rows.next() {
            let id_call_center: i64 = row.get(0)?;
            let sb_chat_api_token: String = crypto::open_secret(&row.get::<_, String>(1)?);
            let sb_chat_api_token_hashed = crypto::hash_token(sb_chat_api_token.as_str());
            let voip_ms_username: String = row.get(2)?;
            let voip_ms_api_password: String = crypto::open_secret(&row.get::<_, String>(3)?);
            let voip_ms_api_url: String = row.get(4).unwrap_or("".to_string());
            let call_center_did: u64 = row.get(5)?;
            let privilege_level: u64 = row.get(6)?;

            all_call_center.push(CallCenter {
                id_call_center,
                sb_chat_api_token,
                sb_chat_api_token_hashed,
                voip_ms_username,
                voip_ms_api_password,
                voip_ms_api_url,
                call_center_did,
                privilege_level,
            });
        }

        if all_call_center.len() == 0 {
            return Ok(None);
        }
        Ok(Some(all_call_center))
    }
    fn get_call_center_by_voip_ms_username_and_call_center_did(
        &self,
        voip_ms_username: &str,
        call_center_did: u64,
    ) -> Result<Option<CallCenter>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_call_center,sb_chat_api_token,voip_ms_username,voip_ms_api_password,voip_ms_api_url,call_center_did,privilege_level FROM call_center WHERE voip_ms_username = (?1) AND call_center_did = (?2)")?;

        let mut rows = stmt.query(params![voip_ms_username, call_center_did])?;

        if let Ok(Some(row)) = rows.next() {
            let id_call_center: i64 = row.get(0)?;
            let sb_chat_api_token: String = crypto::open_secret(&row.get::<_, String>(1)?);
            let sb_chat_api_token_hashed = crypto::hash_token(sb_chat_api_token.as_str());
            let voip_ms_username: String = row.get(2)?;
            let voip_ms_api_password: String = crypto::open_secret(&row.get::<_, String>(3)?);
            let voip_ms_api_url: String = row.get(4).unwrap_or("".to_string());
            let call_center_did: u64 = row.get(5)?;
            let privilege_level: u64 = row.get(6)?;

            return Ok(Some(CallCenter {
                id_call_center,
                sb_chat_api_token,
                sb_chat_api_token_hashed,
                voip_ms_username,
                voip_ms_api_password,
                voip_ms_api_url,
                call_center_did,
                privilege_level,
            }));
        }

        Ok(None)
    }
    pub fn get_call_center_by_id(&self, call_center_id: u64) -> Result<Option<CallCenter>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_call_center,sb_chat_api_token,voip_ms_username,voip_ms_api_password,voip_ms_api_url,call_center_did,privilege_level FROM call_center WHERE id_call_center = (?1)")?;

        let mut rows = stmt.query(params![call_center_id])?;

        if let Ok(Some(row)) = rows.next() {
            let id_call_center: i64 = row.get(0)?;
            let sb_chat_api_token: String = crypto::open_secret(&row.get::<_, String>(1)?);
            let sb_chat_api_token_hashed = crypto::hash_token(sb_chat_api_token.as_str());
            let voip_ms_username: String = row.get(2)?;
            let voip_ms_api_password: String = crypto::open_secret(&row.get::<_, String>(3)?);
            let voip_ms_api_url: String = row.get(4).unwrap_or("".to_string());
            let call_center_did: u64 = row.get(5)?;
            let privilege_level: u64 = row.get(6)?;

            return Ok(Some(CallCenter {
                id_call_center,
                sb_chat_api_token,
                sb_chat_api_token_hashed,
                voip_ms_username,
                voip_ms_api_password,
                voip_ms_api_url,
                call_center_did,
                privilege_level,
            }));
        }

        Ok(None)
    }
    /// True when another call center already uses this number (0, "no number", is never taken)
    pub fn did_used_by_another_call_center(&self, did: u64, own_id: u64) -> Result<bool> {
        if did == 0 {
            return Ok(false);
        }
        let conn = Connection::open(&self.path)?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM call_center WHERE call_center_did = ?1 AND id_call_center != ?2",
            params![did, own_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn update_call_center_using_token(
        &self,
        api_token: &str,
        voip_ms_username: &str,
        voip_ms_api_password: &str,
        call_center_did: u64,
    ) -> anyhow::Result<bool> {
        let call_center_id_opt = self.get_call_center_id_by_token(api_token.to_string())?;
        match call_center_id_opt {
            Some(call_center_id) => {
                let call_center_id = call_center_id.0;
                let conn = Connection::open(&self.path)?;
                conn.execute(
                        "UPDATE call_center SET voip_ms_username = ?2 , voip_ms_api_password = ?3 , call_center_did = ?4  WHERE id_call_center = ?1",
                        params![call_center_id,voip_ms_username,crypto::seal_secret(voip_ms_api_password),call_center_did],
                    )?;
                return Ok(true);
            }
            None => {
                return Ok(false);
            }
        }
    }

    pub fn create_or_update_call_center(
        &self,
        voip_ms_username: &str,
        voip_ms_api_password: &str,
        call_center_did: u64,
        privilege_level: u64,
        regenerate_token: bool,
    ) -> anyhow::Result<(String, String)> {
        let mut note = String::new();
        let token = crypto::generate_token(128);
        // Test if the voip_ms_username already exist
        let call_center_opt = self.get_call_center_by_voip_ms_username_and_call_center_did(
            voip_ms_username,
            call_center_did,
        )?;

        match call_center_opt {
            // If voip_ms_username exist, verify that passwords match. If they don't, update the password, return token with note
            // We update every call center with same username regardless of call center did
            Some(call_center) => {
                if call_center.voip_ms_api_password != voip_ms_api_password {
                    tracing::info!("voip.ms password changed, updating it.");
                    let conn = Connection::open(&self.path)?;
                    conn.execute(
                        "UPDATE call_center SET voip_ms_api_password = ?1 WHERE voip_ms_username = ?2",
                        params![crypto::seal_secret(voip_ms_api_password),voip_ms_username],
                    )?;
                    note.push_str("voip.ms api password was updated in sb chat api db \n");
                }
                // Check if this specific call_center_did privilege needs to be updated
                if call_center.call_center_did == call_center_did
                    && call_center.privilege_level != privilege_level
                {
                    let conn = Connection::open(&self.path)?;
                    conn.execute(
                        "UPDATE call_center SET privilege_level = ?1 WHERE voip_ms_username = ?2 AND call_center_did = ?3",
                        params![privilege_level,voip_ms_username,call_center_did],
                    )?;
                    note.push_str("voip.ms privilege level was updated in sb chat api db \n");
                }

                // The call center could also be from another did but still have a username already in database
                // we create a new entry for this call center
                if call_center.call_center_did != call_center_did {
                    // We need to create a new entry
                    let hashed_token = crypto::hash_token(token.as_str());
                    let conn = Connection::open(&self.path)?;
                    conn.execute(
                    "INSERT INTO call_center (sb_chat_api_token,sb_chat_api_token_hashed,voip_ms_username,voip_ms_api_password,call_center_did,privilege_level) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![crypto::seal_secret(&token),hashed_token,voip_ms_username,crypto::seal_secret(voip_ms_api_password),call_center_did,privilege_level],
                )?;
                    return Ok((token, note));
                }
                // If the user instruct for token regeneration, do it. Then return current token.
                if regenerate_token {
                    let hashed_token = crypto::hash_token(token.as_str());
                    let conn = Connection::open(&self.path)?;
                    conn.execute(
                        "UPDATE call_center SET sb_chat_api_token = ?1 , sb_chat_api_token_hashed = ?2  WHERE voip_ms_username = ?3 AND call_center_did = ?4",
                        params![crypto::seal_secret(&token),hashed_token,voip_ms_username,call_center_did],
                    )?;
                    note.push_str("sb chat api token updated \n");
                    return Ok((token, note));
                } else {
                    // Fetch current token and simply return it
                    match self.get_call_center_token_by_id(call_center.id_call_center)? {
                        Some(token) => {
                            return Ok((token, note));
                        }
                        None => {
                            return Err(anyhow::anyhow!(
                                "Failed to retrieve call center token by id"
                            ));
                        }
                    }
                }
            }
            // The call center does not exist. We create it
            None => {
                let token_hashed = crypto::hash_token(&token);
                let conn = Connection::open(&self.path)?;
                conn.execute(
                    "INSERT INTO call_center (sb_chat_api_token,sb_chat_api_token_hashed,voip_ms_username,voip_ms_api_password,call_center_did,privilege_level) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![crypto::seal_secret(&token),token_hashed,voip_ms_username,crypto::seal_secret(voip_ms_api_password),call_center_did,privilege_level],
                )?;
                return Ok((token, note));
            }
        }
    }
    /// Creates (or finds again) a call center without voip.ms: web chat only. SwitchBoard identifies it with
    /// its own reference (one per committee), so asking twice gives the same call center.
    pub fn create_or_update_web_only_call_center(
        &self,
        committee_ref: &str,
        privilege_level: u64,
        regenerate_token: bool,
    ) -> anyhow::Result<(String, String)> {
        let conn = Connection::open(&self.path)?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id_call_center FROM call_center WHERE committee_ref = ?1",
                params![committee_ref],
                |r| r.get(0),
            )
            .ok();
        match existing {
            Some(id) => {
                let mut note = String::new();
                conn.execute(
                    "UPDATE call_center SET privilege_level = ?1 WHERE id_call_center = ?2",
                    params![privilege_level, id],
                )?;
                if regenerate_token {
                    let token = crypto::generate_token(128);
                    conn.execute(
                        "UPDATE call_center SET sb_chat_api_token = ?1, sb_chat_api_token_hashed = ?2 WHERE id_call_center = ?3",
                        params![crypto::seal_secret(&token), crypto::hash_token(&token), id],
                    )?;
                    note.push_str("sb chat api token updated \n");
                    return Ok((token, note));
                }
                let token = self
                    .get_call_center_token_by_id(id)?
                    .ok_or_else(|| anyhow::anyhow!("Failed to retrieve call center token by id"))?;
                Ok((token, note))
            }
            None => {
                let token = crypto::generate_token(128);
                conn.execute(
                    "INSERT INTO call_center (sb_chat_api_token,sb_chat_api_token_hashed,voip_ms_username,voip_ms_api_password,call_center_did,privilege_level,committee_ref) VALUES (?1,?2,'',?3,0,?4,?5)",
                    params![crypto::seal_secret(&token), crypto::hash_token(&token), crypto::seal_secret(""), privilege_level, committee_ref],
                )?;
                Ok((token, "call center created without voip.ms (web chat only)\n".to_string()))
            }
        }
    }

    // ========================================
    // WEBSITES (web chat pairing)
    // ========================================

    pub fn add_web_pairing(&self, id_call_center: u64, code_hash: &str, expires_at: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute("DELETE FROM web_pairing WHERE expires_at < ?1", params![chrono::Utc::now().timestamp()])?;
        conn.execute(
            "INSERT OR REPLACE INTO web_pairing (code_hash, id_call_center, expires_at) VALUES (?1, ?2, ?3)",
            params![code_hash, id_call_center, expires_at],
        )?;
        Ok(())
    }

    /// Uses a pairing code (once, before it expires): the website now sends its web chats to that call
    /// center. Returns the call center, or None for an unknown, used or expired code.
    pub fn redeem_web_pairing(&self, code_hash: &str, origin: &str, now: i64) -> Result<Option<u64>> {
        let mut conn = Connection::open(&self.path)?;
        let tx = conn.transaction()?;
        let id: Option<u64> = tx
            .query_row(
                "SELECT id_call_center FROM web_pairing WHERE code_hash = ?1 AND expires_at >= ?2",
                params![code_hash, now],
                |r| r.get(0),
            )
            .ok();
        tx.execute("DELETE FROM web_pairing WHERE code_hash = ?1", params![code_hash])?;
        if let Some(id) = id {
            tx.execute(
                "INSERT OR REPLACE INTO web_site (origin, id_call_center, linked_at) VALUES (?1, ?2, ?3)",
                params![origin, id, now],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Every linked website: (origin, call center)
    pub fn web_sites(&self) -> Result<Vec<(String, u64, i64)>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT origin, id_call_center, linked_at FROM web_site ORDER BY origin")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    pub fn remove_web_site(&self, id_call_center: u64, origin: &str) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let n = conn.execute(
            "DELETE FROM web_site WHERE origin = ?1 AND id_call_center = ?2",
            params![origin, id_call_center],
        )?;
        Ok(n > 0)
    }

    // ========================================
    // ROOM MANAGEMENT METHODS
    // ========================================

    /// Fetches subscriber info from a Matrix room ID
    pub fn fetch_subscriber_info_from_room_id(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<(u64, u64, u64)>> {
        let room_id = room_id.as_str();
        info!("room_id {}", room_id);
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id_call_center, did_enc, call_center_did FROM room WHERE id_room = (?1)",
        )?;
        let mut rows = stmt.query(params![room_id])?;

        if let Ok(Some(row)) = rows.next() {
            let call_center_id: u64 = row.get(0)?;
            let did_enc: Vec<u8> = row.get(1)?;
            let did = crypto::decrypt_did(&did_enc).ok_or_else(|| {
                rusqlite::Error::InvalidParameterName(
                    "room caller number cannot be decrypted (wrong PHONE_ENC_KEY?)".to_string(),
                )
            })?;
            let call_center_did: u64 = row.get(2)?;
            return Ok(Some((call_center_id, did, call_center_did)));
        } else {
            return Ok(None);
        }
    }
    // Get a respondent own room. Each respondent have their room
    pub fn get_user_room_respondant_username(&self, room_id: &RoomId) -> Result<Option<String>> {
        let room_id = room_id.as_str();
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT respondant_username FROM user_room WHERE id_room = (?1)")?;
        let mut rows = stmt.query(params![room_id])?;

        if let Ok(Some(row)) = rows.next() {
            let respondant_username: String = row.get(0)?;
            return Ok(Some((respondant_username)));
        } else {
            return Ok(None);
        }
    }
    /// Call centers a respondent belongs to (usually one)
    pub fn get_call_centers_of_respondant(&self, respondant_username: &str) -> Result<Vec<u64>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT id_call_center FROM respondant WHERE respondant_username = ?1 ORDER BY id_call_center",
        )?;
        let ids = stmt.query_map(params![respondant_username], |row| row.get(0))?;
        ids.collect()
    }
    pub fn clean_user_rooms(&self) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute("DELETE FROM user_room", [])?;
        Ok(())
    }
    /// Fetches room ID from DID information
    pub fn fetch_room_id_from_did(
        &self,
        id_call_center: u64,
        did: u64,
        call_center_did: u64,
    ) -> Result<Option<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id_room FROM room WHERE did_number = (?1) AND id_call_center = (?2) AND call_center_did = (?3)",
        )?;
        let mut rows = stmt.query(params![crypto::hash_did(did), id_call_center, call_center_did])?;

        if let Ok(Some(row)) = rows.next() {
            let room_id: String = row.get(0)?;
            return Ok(Some(room_id));
        } else {
            return Ok(None);
        }
    }

    /// Associates a room ID with DID information
    pub fn add_room_id_to_did(
        &self,
        room_id: &String,
        call_center_id: &u64,
        did: &u64,
        call_center_did: &u64,
    ) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        // The number is stored hashed (lookup) and encrypted (needed to answer by SMS)
        let channel = if crate::web_chat::is_web_did(*did) { "web" } else { "sms" };
        conn.execute(
            "INSERT INTO room (id_room,id_call_center,did_number,did_enc,channel,call_center_did) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                room_id,
                call_center_id,
                crypto::hash_did(*did),
                crypto::encrypt_did(*did),
                channel,
                call_center_did
            ],
        )?;
        Ok(())
    }

    // ---- web chat block list (abusive visitors)

    /// Blocks a network address (hashed) until `until` (unix seconds); extends an existing block.
    pub fn add_web_block(&self, ip_hash: &str, until: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "INSERT INTO web_block (ip_hash, blocked_until, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(ip_hash) DO UPDATE SET blocked_until = MAX(blocked_until, excluded.blocked_until)",
            params![ip_hash, until, Utc::now().timestamp()],
        )?;
        Ok(())
    }

    /// True if the address is blocked at `now` (unix seconds)
    pub fn is_web_blocked(&self, ip_hash: &str, now: i64) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT 1 FROM web_block WHERE ip_hash = ?1 AND blocked_until > ?2")?;
        Ok(stmt.query(params![ip_hash, now])?.next()?.is_some())
    }

    /// Number of blocks created at or after `since` (unix seconds): how much abuse there has been lately
    pub fn count_recent_web_blocks(&self, since: i64) -> Result<i64> {
        let conn = Connection::open(&self.path)?;
        conn.query_row("SELECT count(*) FROM web_block WHERE created_at >= ?1", params![since], |r| r.get(0))
    }

    pub fn get_web_state(&self, key: &str) -> Result<Option<i64>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT value FROM web_state WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    pub fn set_web_state(&self, key: &str, value: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "INSERT INTO web_state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Hashes of every block still running at `now` (the web chat keeps a short-lived copy in memory)
    pub fn list_active_web_block_hashes(&self, now: i64) -> Result<Vec<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT ip_hash FROM web_block WHERE blocked_until > ?1")?;
        let rows = stmt.query_map(params![now], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    /// Removes one block (true if it existed)
    pub fn remove_web_block(&self, ip_hash: &str) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        Ok(conn.execute("DELETE FROM web_block WHERE ip_hash = ?1", params![ip_hash])? > 0)
    }

    /// Removes every block; returns how many
    pub fn clear_web_blocks(&self) -> Result<usize> {
        let conn = Connection::open(&self.path)?;
        Ok(conn.execute("DELETE FROM web_block", [])?)
    }

    /// Drops expired blocks; returns how many
    pub fn purge_expired_web_blocks(&self, now: i64) -> Result<usize> {
        let conn = Connection::open(&self.path)?;
        Ok(conn.execute("DELETE FROM web_block WHERE blocked_until <= ?1", params![now])?)
    }

    /// Deletes the room row (when a chat ends or its retention period is over)
    pub fn delete_room(&self, room_id: &str) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute("DELETE FROM room WHERE id_room = ?1", params![room_id])?;
        Ok(())
    }

    /// Room ids of a channel: "web" or "sms"
    pub fn fetch_room_ids_by_channel(&self, channel: &str) -> Result<Vec<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT id_room FROM room WHERE channel = (?1)")?;
        let rows = stmt.query_map(params![channel], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    /// SMS rooms with no activity from the caller for more than `days` days
    pub fn fetch_idle_sms_room_ids(&self, days: u64) -> Result<Vec<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id_room FROM room WHERE channel = 'sms'
             AND COALESCE(last_accessed_time, creation_time) < datetime('now', ?1)",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    /// Right to erasure: removes every trace of a caller except the opt-out (the hash in
    /// denied_did is kept so a caller who asked for no more messages is not texted again).
    /// Returns the Matrix room ids that must also be left and forgotten.
    pub fn erase_caller(&self, id_call_center: u64, did: u64) -> Result<Vec<String>> {
        let conn = Connection::open(&self.path)?;
        let hash = crypto::hash_did(did);
        let room_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id_room FROM room WHERE id_call_center = ?1 AND did_number = ?2",
            )?;
            stmt.query_map(params![id_call_center, hash], |row| row.get(0))?
                .collect::<Result<_>>()?
        };
        conn.execute(
            "DELETE FROM room WHERE id_call_center = ?1 AND did_number = ?2",
            params![id_call_center, hash],
        )?;
        conn.execute(
            "DELETE FROM caller_notified WHERE id_call_center = ?1 AND did_number = ?2",
            params![id_call_center, hash],
        )?;
        conn.execute(
            "DELETE FROM auto_sms_log WHERE id_call_center = ?1 AND did_number = ?2",
            params![id_call_center, hash],
        )?;
        // Encrypted rows are found by their keyed hash, rows from older versions by the clear number
        conn.execute(
            "DELETE FROM from_voipms_queue WHERE from_hash = ?1 OR voipms_from = ?2",
            params![hash, did.to_string()],
        )?;
        conn.execute(
            "DELETE FROM to_voipms_queue WHERE dst_hash = ?1 OR send_message_url LIKE ?2",
            params![hash, format!("%&dst={did}&%")],
        )?;
        Ok(room_ids)
    }

    /// Drops queued SMS data older than `max_age_secs`, successful or not. The queues only
    /// exist to retry; they must not become a message archive.
    pub fn purge_old_queue_items(&self, from_max_age_secs: i64, to_max_age_secs: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        let removed_from = conn.execute(
            "DELETE FROM from_voipms_queue WHERE timestamp < datetime('now', ?1)",
            params![format!("-{from_max_age_secs} seconds")],
        )?;
        let removed_to = conn.execute(
            "DELETE FROM to_voipms_queue WHERE timestamp < datetime('now', ?1)",
            params![format!("-{to_max_age_secs} seconds")],
        )?;
        if removed_from + removed_to > 0 {
            info!("Purged {removed_from} inbound and {removed_to} outbound expired queue items");
        }
        Ok(())
    }

    /// voip.ms password for an account name (to complete a queued URL stored without it)
    pub fn get_voip_ms_password_by_username(&self, username: &str) -> Result<Option<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT voip_ms_api_password FROM call_center WHERE voip_ms_username = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![username])?;
        match rows.next()? {
            Some(row) => Ok(Some(crypto::open_secret(&row.get::<_, String>(0)?))),
            None => Ok(None),
        }
    }

    /// Updates the last accessed timestamp for a room
    pub fn update_last_accessed_room_timestamp(&self, id_room: String) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        let now: DateTime<Utc> = SystemTime::now().into();

        conn.execute(
            "UPDATE room SET last_accessed_time = DATETIME('now') WHERE id_room = ?1",
            params![id_room],
        )?;

        info!("Updated room last accessed time (utc) : {}", now);
        Ok(())
    }
    pub fn get_last_accessed_room_timestamp(
        &self,
        id_room: String,
    ) -> anyhow::Result<Option<NaiveDateTime>> {
        let conn = Connection::open(&self.path)?;
        //let now: DateTime<Utc> = SystemTime::now().into();
        //let formatted_time = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let mut stmt = conn.prepare("SELECT last_accessed_time FROM room WHERE id_room = ?1")?;
        let mut rows = stmt.query(params![id_room])?;
        if let Some(row) = rows.next()? {
            let last_accessed_time: String = match row.get(0) {
                Ok(ok) => ok,
                Err(_err) => {
                    return Ok(None);
                }
            };
            let timestamp =
                NaiveDateTime::parse_from_str(last_accessed_time.as_str(), "%Y-%m-%d %H:%M:%S")?;

            return Ok(Some(timestamp));
        }
        Err(anyhow::anyhow!("Room id not found in rooms table"))
    }
    pub fn is_room_in_one_to_one(&self, id_room: String) -> anyhow::Result<bool> {
        let conn = Connection::open(&self.path)?;
        //let now: DateTime<Utc> = SystemTime::now().into();
        //let formatted_time = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let mut stmt = conn.prepare("SELECT is_in_one_to_one FROM room WHERE id_room = ?1")?;
        let mut rows = stmt.query(params![id_room])?;
        if let Some(row) = rows.next()? {
            let is_in_one_to_one: i32 = row.get(0)?;
            return Ok(is_in_one_to_one == 1);
        }
        Err(anyhow::anyhow!("Room id not found in rooms table"))
    }
    pub fn set_room_in_one_to_one(
        &self,
        id_room: String,
        is_in_one_to_one: bool,
    ) -> anyhow::Result<()> {
        let is_in_one_to_one_int;
        if is_in_one_to_one {
            is_in_one_to_one_int = 1;
        } else {
            is_in_one_to_one_int = 0;
        }
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "UPDATE room SET is_in_one_to_one = ?1 WHERE id_room = ?2",
            params![is_in_one_to_one_int, id_room],
        )?;
        //info!("Updated room last accessed time (utc) : {}", now);
        Ok(())
    }
    // ========================================
    // USER ROOMS MANAGEMENT METHODS
    // ========================================

    /// Get a room_id by respondant user name
    pub fn get_user_room_id_by_username(&self, username: String) -> anyhow::Result<Option<String>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_room FROM user_room WHERE respondant_username = (?1)")?;
        let mut rows = stmt.query(params![username])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row.get(0)?));
        }
        Ok(None)
    }
    pub fn set_user_room_username_room_id(
        &self,
        username: String,
        room_id: String,
    ) -> anyhow::Result<()> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("INSERT INTO user_room (respondant_username,id_room) values (?1,?2)")?;
        stmt.execute(params![username, room_id])?;

        Ok(())
    }
    // ========================================
    // RESPONDENT MANAGEMENT METHODS
    // ========================================

    /// Gets all unique respondent user IDs.
    pub fn get_all_respondant_user_id(
        &self,
        only_active: bool,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let conn = Connection::open(&self.path)?;
        self.clean_old_respondants()?;
        let mut stmt;

        if (only_active) {
            stmt = conn.prepare(
                "SELECT DISTINCT respondant_username FROM respondant WHERE is_active = 1",
            )?;
        } else {
            stmt = conn.prepare("SELECT DISTINCT respondant_username FROM respondant")?;
        }
        let mut rows = stmt.query(params![])?;

        let mut repondants: Vec<String> = Vec::new();

        while let Ok(Some(row)) = rows.next() {
            let respondant_username: String = row.get(0)?;
            repondants.push(respondant_username);
        }

        if repondants.len() == 0 {
            return Ok(None);
        }
        Ok(Some(repondants))
    }

    pub fn set_call_center_respondant(
        &self,
        id_call_center: u64,
        respondents: Vec<String>,
    ) -> Result<()> {
        self.clean_old_respondants()?;
        let conn = Connection::open(&self.path)?;
        // Make users not in respondents Vec inactive for this call center
        let placeholders = respondents
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let delete_inactive_sql = format!(
            "UPDATE respondant SET is_active = 0 WHERE id_call_center = ?1 AND respondant_username NOT IN({})",
            placeholders
        );
        // Prepare parameters: new value + excluded values
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&id_call_center];
        for val in &respondents {
            params_vec.push(val);
        }
        conn.execute(&delete_inactive_sql, params_vec.as_slice())?;
        // Sqlite to verify if a respondent exist
        let mut respondent_exist =  conn.prepare("SELECT respondant_username FROM respondant WHERE id_call_center = (?1) AND respondant_username = (?2)")?;
        // Request to update a respondent if it exist
        let mut stmt = conn.prepare(
            "UPDATE respondant SET last_accessed_time = DATETIME('now'),is_active = 1 WHERE id_call_center = (?1) AND respondant_username = (?2)",
        )?;
        // We verify each repondant exist, if so, we update it. Otherwise, we insert a new entry
        for respondant in respondents {
            let respondent_name_request =
                respondent_exist.query(params![id_call_center, respondant]);
            if let Ok(mut respondent_names) = respondent_name_request {
                if let Ok(Some(respondent_name)) = respondent_names.next() {
                    // Respondent exist
                    stmt.execute(params![id_call_center, respondant])?;
                } else {
                    // Respondent does not exist, we create it
                    conn.execute(
                    "INSERT INTO respondant (id_call_center,respondant_username,last_accessed_time,is_active) VALUES (?1,?2,DATETIME('now'),1)",
                     params![id_call_center,respondant],
                    )?;
                }
            }
        }
        Ok(())
    }
    /// Gets respondents for a specific call center
    pub fn get_call_center_respondant(
        &self,
        id_call_center: u64,
        only_active: bool,
    ) -> Result<Option<Vec<String>>> {
        let conn = Connection::open(&self.path)?;
        let only_active_sql;
        if only_active {
            only_active_sql = " AND is_active = 1";
        } else {
            only_active_sql = "";
        }
        let mut stmt = conn.prepare(
            (String::from(
                "SELECT respondant_username FROM respondant WHERE id_call_center = (?1)",
            ) + only_active_sql)
                .as_str(),
        )?;
        let mut rows = stmt.query(params![id_call_center])?;
        let mut repondants: Vec<String> = Vec::new();

        while let Ok(Some(row)) = rows.next() {
            let respondant_username: String = row.get(0)?;
            repondants.push(respondant_username);
        }

        if repondants.len() == 0 {
            return Ok(None);
        }
        Ok(Some(repondants))
    }

    /// Records a signature of the confidentiality agreement. Returns false when this person already signed this exact text.
    pub fn add_agreement_signature(&self, s: &AgreementSignature) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO agreement_signature
             (respondant_username, typed_name, version, text_sha256, signed_at, event_id, device_id, device_verified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![s.respondant_username, s.typed_name, s.version, s.text_sha256, s.signed_at, s.event_id, s.device_id, s.device_verified],
        )?;
        Ok(inserted == 1)
    }

    /// The most recent signature of a person (any version), if any.
    pub fn get_latest_agreement_signature(&self, respondant_username: &str) -> Result<Option<AgreementSignature>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT respondant_username, typed_name, version, text_sha256, signed_at, event_id, device_id, device_verified
             FROM agreement_signature WHERE respondant_username = ?1 ORDER BY signed_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![respondant_username])?;
        Ok(match rows.next()? {
            Some(row) => Some(AgreementSignature {
                respondant_username: row.get(0)?,
                typed_name: row.get(1)?,
                version: row.get(2)?,
                text_sha256: row.get(3)?,
                signed_at: row.get(4)?,
                event_id: row.get(5)?,
                device_id: row.get(6)?,
                device_verified: row.get(7)?,
            }),
            None => None,
        })
    }

    /// Whether a person signed this exact text.
    pub fn has_signed_agreement(&self, respondant_username: &str, text_sha256: &str) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM agreement_signature WHERE respondant_username = ?1 AND text_sha256 = ?2",
            params![respondant_username, text_sha256],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn clean_old_respondants(&self) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        // Delete entries older than 1 month
        let deleted = conn.execute(
            "DELETE FROM respondant WHERE last_accessed_time < datetime('now', '-1 month')",
            [],
        )?;
        Ok(())
    }
    // ========================================
    // INCOMING QUEUE MANAGEMENT METHODS (FROM VOIP.MS)
    // ========================================

    /// Adds a new item to the incoming VoIP.ms queue
    pub fn add_from_voip_ms_queue_item(
        &self,
        query_params: QueryParams,
        processing_level: String,
    ) -> Result<i64> {
        self.add_from_voip_ms_queue_item_since(query_params, processing_level, None)
    }

    /// Queues an inbound text; `first_arrival` keeps the age of a text that is being retried.
    pub fn add_from_voip_ms_queue_item_since(
        &self,
        query_params: QueryParams,
        processing_level: String,
        first_arrival: Option<String>,
    ) -> Result<i64> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("INSERT INTO from_voipms_queue (voipms_to,voipms_from,voipms_message,voipms_id,sbchatapikey,processing_level,timestamp,from_hash) values (?1,?2,?3,?4,?5,?6,COALESCE(?7,CURRENT_TIMESTAMP),?8)")?;

        // The caller's number and the text are confidential (Law 25): encrypted at rest
        let from_hash = query_params.from.as_deref().and_then(|f| f.trim().parse::<u64>().ok()).map(crypto::hash_did);
        stmt.execute(params![
            query_params.to,
            query_params.from.as_deref().map(crypto::seal_secret),
            query_params.message.as_deref().map(crypto::seal_secret),
            query_params.id,
            query_params.sbchatapikey,
            processing_level,
            first_arrival,
            from_hash
        ])?;

        let last_id = conn.last_insert_rowid();
        debug!("Last inserted row ID: {}", last_id);
        Ok(last_id)
    }

    /// Updates processing level for an incoming queue item
    pub fn change_processing_level_from_voip_ms_queue_item(
        &self,
        id_from_voipms_queue: i64,
        processing_level: String,
        error_log: String,
    ) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
        "UPDATE from_voipms_queue SET processing_level = ?1, error_log = ?2 WHERE id_from_voipms_queue = ?3",
        params![processing_level, error_log, id_from_voipms_queue],
        )?;

        info!("Updated queue processing level to {processing_level}.");
        Ok(())
    }

    /// Checks if an incoming queue item exists
    pub fn exist_from_voip_ms_queue_item(&self, id_from_voipms_queue: i64) -> Result<(bool)> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT * FROM from_voipms_queue WHERE id_from_voipms_queue = ?")?;
        let rows = stmt.query(params![id_from_voipms_queue])?;
        let row_count = rows.count();

        info!("row_count{:?}", row_count);
        if let Ok(row_count) = row_count {
            if row_count > 0 {
                return Ok(true);
            } else {
                return Ok(false);
            }
        }
        panic!("Could not count rows");
    }

    /// Removes an item from the incoming queue by ID
    pub fn remove_from_voip_ms_queue_item(&self, id_from_voipms_queue: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "DELETE FROM from_voipms_queue WHERE id_from_voipms_queue = ?",
            params![id_from_voipms_queue],
        )?;
        info!("Item removed from incoming queue");
        Ok(())
    }

    /// Removes an item from the incoming queue by VoIP.ms ID
    pub fn remove_from_voip_ms_queue_item_from_voipms_id(&self, voip_ms_id: String) -> Result<()> {
        self.take_from_voip_ms_queue_item_from_voipms_id(voip_ms_id).map(|_| ())
    }

    /// Removes the queued copy of an inbound text and returns when it FIRST arrived, so a retried text keeps its
    /// age (the 24 h purge must count from the first arrival, not from the last retry).
    pub fn take_from_voip_ms_queue_item_from_voipms_id(&self, voip_ms_id: String) -> Result<Option<String>> {
        let conn = Connection::open(&self.path)?;
        let first: Option<String> = conn
            .query_row(
                "SELECT MIN(timestamp) FROM from_voipms_queue WHERE voipms_id = ?1",
                params![voip_ms_id],
                |row| row.get(0),
            )
            .unwrap_or(None);
        conn.execute(
            "DELETE FROM from_voipms_queue WHERE voipms_id = ?",
            params![voip_ms_id],
        )?;
        debug!("Item removed from incoming queue using voip_ms ID.");
        Ok(first)
    }

    /// Retrieves all incoming queue items
    pub fn get_from_voip_ms_queue_content(&self) -> Result<Option<Vec<FromQueueManagerEntry>>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_from_voipms_queue,voipms_to,voipms_from,voipms_message,voipms_id,timestamp,sbchatapikey,processing_level,error_log FROM from_voipms_queue")?;
        let mut rows = stmt.query(params![])?;
        let mut from_queue_content: Vec<FromQueueManagerEntry> = Vec::new();

        while let Ok(Some(row)) = rows.next() {
            let id_from_voipms_queue: u64 = row.get(0)?;
            let voipms_to: String = row.get(1)?;
            let voipms_from: String = crypto::open_secret(&row.get::<_, String>(2)?);
            let voipms_message: String = crypto::open_secret(&row.get::<_, String>(3)?);
            let voipms_id: String = row.get(4)?;
            let timestamp: String = row.get(5)?;
            let sbchatapikey: String = row.get(6)?;
            let processing_level: String = row.get(7)?;
            let error_log: String = row.get(8).unwrap_or("".to_string());

            let queryparams = QueryParams {
                to: Some(voipms_to),
                from: Some(voipms_from),
                message: Some(voipms_message),
                id: Some(voipms_id),
                timestamp: Some(timestamp),
                sbchatapikey: Some(sbchatapikey),
                is_reprocessing_error: Some(true),
            }; // Is reprocessing error is flagged to true here so we know this has already been processed.

            from_queue_content.push(FromQueueManagerEntry {
                id: id_from_voipms_queue,
                queryparams,
                processing_level,
                error_log,
            });
        }

        if from_queue_content.len() == 0 {
            return Ok(None);
        }
        Ok(Some(from_queue_content))
    }

    // ========================================
    // OUTGOING QUEUE MANAGEMENT METHODS (TO VOIP.MS)
    // ========================================

    /// Adds a new item to the outgoing VoIP.ms queue
    pub fn add_to_voip_ms_queue_item(
        &self,
        send_message_url: String,
        processing_level: String,
    ) -> Result<i64> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "INSERT INTO to_voipms_queue (send_message_url,processing_level,dst_hash) values (?1,?2,?3)",
        )?;
        // The URL holds the caller's number and the text: encrypted at rest
        stmt.execute(params![crypto::seal_secret(&send_message_url), processing_level, dst_hash_of(&send_message_url)])?;

        let last_id = conn.last_insert_rowid();
        info!("Last inserted row ID: {}", last_id);
        Ok(last_id)
    }

    /// Updates processing level for an outgoing queue item
    pub fn change_processing_level_to_voip_ms_queue_item(
        &self,
        id_to_voipms_queue: i64,
        processing_level: String,
        error_log: String,
    ) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
        "UPDATE to_voipms_queue SET processing_level = ?1, error_log = ?2 WHERE id_to_voipms_queue = ?3",
        params![processing_level, error_log, id_to_voipms_queue],
        )?;

        info!("Updated queue processing level to {processing_level}.");
        Ok(())
    }

    /// Removes an item from the outgoing queue
    pub fn remove_to_voip_ms_queue_item(&self, id_to_voipms_queue: i64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "DELETE FROM to_voipms_queue WHERE id_to_voipms_queue = ?",
            params![id_to_voipms_queue],
        )?;
        info!("Item removed from outcoming queue");
        Ok(())
    }

    /// Retrieves all outgoing queue items
    pub fn get_to_voip_ms_queue_content(&self) -> Result<Option<Vec<ToQueueManagerEntry>>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_to_voipms_queue,send_message_url,timestamp,processing_level,error_log FROM to_voipms_queue")?;
        let mut rows = stmt.query(params![])?;
        let mut to_queue_content: Vec<ToQueueManagerEntry> = Vec::new();

        while let Ok(Some(row)) = rows.next() {
            let id_to_voipms_queue: u64 = row.get(0)?;
            let send_message_url: String = crypto::open_secret(&row.get::<_, String>(1)?);
            let timestamp: String = row.get(2)?;
            let processing_level: String = row.get(3)?;
            let error_log: String = row.get(4).unwrap_or(String::from(""));

            to_queue_content.push(ToQueueManagerEntry {
                id_to_voipms_queue,
                send_message_url,
                timestamp,
                processing_level,
                error_log,
            });
        }

        if to_queue_content.len() == 0 {
            return Ok(None);
        }
        Ok(Some(to_queue_content))
    }
    // ========================================
    // MACROS
    // ========================================
    pub fn add_or_replace_macro(&self, my_macro: Macro) -> anyhow::Result<(Option<u64>,String)> {
        let conn = Connection::open(&self.path)?;
        let id_macro = my_macro.id_macro;
        let is_delete = id_macro.is_some() && my_macro.macro_name.is_empty();

        // Only one macro per (call center, tag) is allowed for non-empty tags (eg. "newcomer",
        // "no_respondent") since callers like macros::get_newcomer_macro_text pick the first
        // match; an empty tag is the default for ordinary #macroname macros, of which there
        // can be many, so it is exempt from this rule.
        if !is_delete {
            // Macro names must be unique per call center, since callers (eg. the #macroname
            // room command, list_macros) look macros up by name and would otherwise get an
            // arbitrary one of several matches.
            let mut stmt = conn.prepare(
                "SELECT id_macro FROM macros WHERE id_call_center = ?1 AND macro_name = ?2 AND id_macro IS NOT ?3",
            )?;
            let mut rows = stmt.query(params![&my_macro.id_call_center, &my_macro.macro_name, id_macro])?;
            if let Some(row) = rows.next()? {
                let conflicting_id_macro: u64 = row.get(0)?;
                return Err(anyhow::anyhow!(
                    "Cannot save macro '{}': a macro with this name already exists for call center {} (id_macro={conflicting_id_macro}). Macro names must be unique per call center.",
                    my_macro.macro_name, my_macro.id_call_center
                ));
            }

            if let Some(tags) = &my_macro.macro_tags {
                if !tags.is_empty() {
                    let mut stmt = conn.prepare(
                        "SELECT id_macro, macro_name FROM macros WHERE id_call_center = ?1 AND macro_tags = ?2 AND id_macro IS NOT ?3",
                    )?;
                    let mut rows = stmt.query(params![&my_macro.id_call_center, tags, id_macro])?;
                    if let Some(row) = rows.next()? {
                        let conflicting_id_macro: u64 = row.get(0)?;
                        let conflicting_macro_name: String = row.get(1)?;
                        return Err(anyhow::anyhow!(
                            "Cannot save macro '{}': tag '{tags}' is already used by macro '{conflicting_macro_name}' (id_macro={conflicting_id_macro}) for call center {}. Only one macro per tag is allowed per call center.",
                            my_macro.macro_name, my_macro.id_call_center
                        ));
                    }
                }
            }
        }

        if id_macro.is_some() {
            // If the macro name is empty, that's a delete command
            if &my_macro.macro_name == "" {
                conn.execute("DELETE FROM macros WHERE id_macro = ?1", params![id_macro])?;
                return Ok((None,format!("Deleted id_macro={:?}",id_macro.unwrap())));
            } else {
                // Already have an existing macro, update it
                conn.execute(
                "UPDATE macros SET id_call_center = ?1, macro_name = ?2,macro_text = ?3,macro_tags = ?4,macro_tags_config = ?5 WHERE id_macro = ?6",
                params![&my_macro.id_call_center,&my_macro.macro_name,&my_macro.macro_text,&my_macro.macro_tags,&my_macro.macro_tags_config,id_macro],
            )?;
                return Ok((None,format!("Updated id_macro={:?}",id_macro.unwrap())));
            }
        } else {
            // No macro yet, create it
            conn.execute(
                "INSERT INTO macros (id_call_center,macro_name,macro_text,macro_tags,macro_tags_config) values (?1,?2,?3,?4,?5)",
                params![&my_macro.id_call_center,&my_macro.macro_name,&my_macro.macro_text,&my_macro.macro_tags,&my_macro.macro_tags_config],
            )?;
            let new_id_macro: u64 = conn.last_insert_rowid() as u64;
            return Ok((Some(new_id_macro),format!("Created new_id_macro={:?}",new_id_macro)));
        }
    }
    // Currently existing tags are newcomer, newcomerweb, no_respondent, help and "" (empty)
    pub fn get_macro_by_tag(&self, id_call_center: &u64, tag: String) -> Result<Option<Macro>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_macro,id_call_center,macro_name,macro_text,macro_tags,macro_tags_config FROM macros WHERE id_call_center = (?1) AND macro_tags = ?2")?;
        // Exact match: "newcomer" must not find a "newcomerweb" macro
        let mut rows = stmt.query(params![id_call_center, tag])?;

        if let Ok(Some(row)) = rows.next() {
            let id_macro: u64 = row.get(0)?;
            let id_call_center: u64 = row.get(1)?;
            let macro_name: String = row.get(2)?;
            let macro_text: String = row.get(3)?;
            let macro_tags: Option<String> = row.get(4)?;
            let macro_tags_config: Option<String> = row.get(5)?;
            // The macro exist
            return Ok(Some(Macro {
                id_macro: Some(id_macro),
                id_call_center,
                macro_name,
                macro_text,
                macro_tags: macro_tags,
                macro_tags_config: macro_tags_config,
            }));
        } else {
            // No macro with this name for this call center but no db error here
            return Ok(None);
        }
    }

    pub fn get_all_macros_by_call_center_id(&self, id_call_center: &u64) -> Result<Vec<Macro>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare("SELECT id_macro,id_call_center,macro_name,macro_text,macro_tags,macro_tags_config FROM macros WHERE id_call_center = (?1)")?;
        let macro_iter = stmt.query_map(params![id_call_center], |row| {
            Ok(Macro {
                id_macro: row.get(0)?,
                id_call_center: row.get(1)?,
                macro_name: row.get(2)?,
                macro_text: row.get(3)?,
                macro_tags: row.get(4)?,
                macro_tags_config: row.get(5)?,
            })
        })?;
        macro_iter.collect()
    }

    pub fn get_macro_by_call_center_id(
        &self,
        macro_name: &String,
        id_call_center: &u64,
    ) -> Result<Option<Macro>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt =
            conn.prepare("SELECT id_macro,id_call_center,macro_name,macro_text,macro_tags,macro_tags_config FROM macros WHERE id_call_center = (?1) AND macro_name = (?2)")?;
        let mut rows = stmt.query(params![id_call_center, macro_name])?;

        if let Ok(Some(row)) = rows.next() {
            let id_macro: u64 = row.get(0)?;
            let id_call_center: u64 = row.get(1)?;
            let macro_name: String = row.get(2)?;
            let macro_text: String = row.get(3)?;
            let macro_tags: Option<String> = row.get(4)?;
            let macro_tags_config: Option<String> = row.get(5)?;
            // The macro exist
            return Ok(Some(Macro {
                id_macro: Some(id_macro),
                id_call_center,
                macro_name,
                macro_text,
                macro_tags: macro_tags,
                macro_tags_config: macro_tags_config,
            }));
        } else {
            // No macro with this name for this call center but no db error here
            return Ok(None);
        }
    }
    // Denied list management (callers who texted STOP)

    /// Adds a caller to the call center's deny list, recording when and how (kept as
    /// proof of the opt-out). Returns true if newly added, false if already denied.
    pub fn deny_did(&self, id_call_center: u64, did_number: u64, denied_by: &str) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO denied_did (id_call_center,did_number,denied_at,denied_by) values (?1,?2,?3,?4)",
            params![id_call_center, crypto::hash_did(did_number), Utc::now().timestamp(), denied_by],
        )?;
        let who = crypto::short_id(did_number);
        if inserted > 0 {
            info!("Caller {who} for call center {id_call_center}, now denied");
        } else {
            info!("Caller {who} for call center {id_call_center}, was already denied");
        }
        Ok(inserted > 0)
    }

    /// Returns true if the caller is on the call center's deny list.
    pub fn is_did_denied(&self, id_call_center: u64, did_number: u64) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT 1 FROM denied_did WHERE id_call_center = ?1 AND did_number = ?2",
        )?;
        let mut rows = stmt.query(params![id_call_center, crypto::hash_did(did_number)])?;
        Ok(rows.next()?.is_some())
    }

    /// Removes a caller from the call center's deny list.
    /// Returns true if the caller was on the list.
    pub fn did_undeny(&self, id_call_center: u64, did_number: u64) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let deleted = conn.execute(
            "DELETE FROM denied_did WHERE id_call_center = ?1 AND did_number = ?2",
            params![id_call_center, crypto::hash_did(did_number)],
        )?;
        if deleted > 0 {
            info!("Caller {} for call center {id_call_center}, was undenied.", crypto::short_id(did_number));
        }
        Ok(deleted > 0)
    }

    /// Returns true if the caller is denied by the call center that owns `call_center_did`.
    /// Used where only the phone numbers are known (outgoing queue retries).
    pub fn is_did_denied_by_call_center_did(
        &self,
        call_center_did: u64,
        did_number: u64,
    ) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT 1 FROM denied_did d WHERE d.did_number = ?2 AND (
                 EXISTS (SELECT 1 FROM call_center c
                         WHERE c.id_call_center = d.id_call_center AND c.call_center_did = ?1)
                 OR EXISTS (SELECT 1 FROM call_center_did cd
                            WHERE cd.id_call_center = d.id_call_center AND cd.did_number = ?1))",
        )?;
        let mut rows = stmt.query(params![call_center_did, crypto::hash_did(did_number)])?;
        Ok(rows.next()?.is_some())
    }

    /// True if the caller already received the first-contact opt-out notice
    pub fn is_caller_notified(&self, id_call_center: u64, did_number: u64) -> Result<bool> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT 1 FROM caller_notified WHERE id_call_center = ?1 AND did_number = ?2",
        )?;
        let mut rows = stmt.query(params![id_call_center, crypto::hash_did(did_number)])?;
        Ok(rows.next()?.is_some())
    }

    /// Records that the caller received the first-contact opt-out notice
    pub fn mark_caller_notified(&self, id_call_center: u64, did_number: u64) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute(
            "INSERT OR IGNORE INTO caller_notified (id_call_center,did_number,notified_at) values (?1,?2,?3)",
            params![id_call_center, crypto::hash_did(did_number), Utc::now().timestamp()],
        )?;
        Ok(())
    }

    // Automatic SMS log (rate limiting of messages the app sends on its own)

    /// Counts automatic SMS of `kind` sent to the caller since `since` (unix seconds)
    pub fn count_auto_sms_since(
        &self,
        id_call_center: u64,
        did_number: u64,
        kind: &str,
        since: i64,
    ) -> Result<u32> {
        let conn = Connection::open(&self.path)?;
        conn.query_row(
            "SELECT count(*) FROM auto_sms_log
             WHERE id_call_center = ?1 AND did_number = ?2 AND kind = ?3 AND sent_at >= ?4",
            params![id_call_center, crypto::hash_did(did_number), kind, since],
            |row| row.get(0),
        )
    }

    /// Records an automatic SMS and drops entries older than a week
    pub fn log_auto_sms(&self, id_call_center: u64, did_number: u64, kind: &str) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO auto_sms_log (id_call_center,did_number,kind,sent_at) values (?1,?2,?3,?4)",
            params![id_call_center, crypto::hash_did(did_number), kind, now],
        )?;
        conn.execute(
            "DELETE FROM auto_sms_log WHERE sent_at < ?1",
            params![now - 7 * 24 * 3600],
        )?;
        Ok(())
    }
}

/// Keyed hash of the destination number of a queued voip.ms sendSMS URL (for erasure)
fn dst_hash_of(send_message_url: &str) -> Option<i64> {
    let url = url::Url::parse(send_message_url).ok()?;
    let dst = url.query_pairs().find(|(k, _)| k == "dst")?.1.into_owned();
    dst.trim().parse::<u64>().ok().map(crypto::hash_did)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|c| c.unwrap())
            .collect()
    }

    fn temp_db(name: &str) -> (DbManager, std::path::PathBuf) {
        crypto::init_test_env();
        let dir = std::env::temp_dir().join(format!("sbchat-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.db");
        let _ = fs::remove_file(&path);
        let db = DbManager { path: path.clone() };
        db.create_db_if_not_exist().unwrap();
        (db, path)
    }

    /// A pairing code links a website to its call center once, before it expires
    #[test]
    fn test_web_pairing_is_used_once_and_expires() {
        let (db, path) = temp_db("pairing");
        let now = 1_800_000_000;
        db.add_web_pairing(7, "h1", now + 60).unwrap();
        assert_eq!(db.redeem_web_pairing("h1", "https://a.example", now).unwrap(), Some(7));
        assert_eq!(db.redeem_web_pairing("h1", "https://b.example", now).unwrap(), None, "used once");
        db.add_web_pairing(7, "h2", now - 1).unwrap();
        assert_eq!(db.redeem_web_pairing("h2", "https://c.example", now).unwrap(), None, "expired");
        let sites: Vec<_> = db.web_sites().unwrap().into_iter().map(|(o, id, _)| (o, id)).collect();
        assert_eq!(sites, vec![("https://a.example".to_string(), 7)]);
        assert!(db.remove_web_site(7, "https://a.example").unwrap());
        assert!(!db.remove_web_site(7, "https://a.example").unwrap());
        let _ = fs::remove_file(&path);
    }

    /// Committees without voip.ms: one call center per SwitchBoard reference, found again on the next request
    #[test]
    fn test_web_only_call_centers_are_one_per_committee() {
        let (db, path) = temp_db("webonly");
        let (t1, _) = db.create_or_update_web_only_call_center("sb.example:12", 0, false).unwrap();
        let (t1b, _) = db.create_or_update_web_only_call_center("sb.example:12", 0, false).unwrap();
        assert_eq!(t1, t1b, "same committee, same call center and token");
        let (t2, _) = db.create_or_update_web_only_call_center("sb.example:13", 0, false).unwrap();
        assert_ne!(t1, t2, "another committee gets its own call center");
        let (t1c, _) = db.create_or_update_web_only_call_center("sb.example:12", 0, true).unwrap();
        assert_ne!(t1, t1c, "a renewal gives a new token");
        let id = db.get_call_center_id_by_token(t1c).unwrap().unwrap().0;
        let (user, _) = db.get_call_center_credentials(id).unwrap();
        assert!(user.is_empty(), "no voip.ms account");
        let _ = fs::remove_file(&path);
    }

    /// A text retried while nobody is on duty keeps its first arrival time, so the 24 h purge still applies.
    #[test]
    fn test_retried_inbound_text_keeps_its_age() {
        crypto::init_test_env();
        let dir = std::env::temp_dir().join(format!("sbchat-queue-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.db");
        let _ = fs::remove_file(&path);
        let db = DbManager { path: path.clone() };
        db.create_db_if_not_exist().unwrap();
        let text = |id: &str| crate::web_server::QueryParams {
            to: Some("5145550100".into()),
            from: Some("5145550199".into()),
            message: Some("x".into()),
            id: Some(id.into()),
            timestamp: None,
            sbchatapikey: Some("k".into()),
            is_reprocessing_error: None,
        };
        // a text that arrived 30 hours ago and is still waiting
        db.add_from_voip_ms_queue_item(text("v1"), "No Respondent".into()).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute("UPDATE from_voipms_queue SET timestamp = datetime('now','-30 hours') WHERE voipms_id='v1'", ())
            .unwrap();
        // the retry: remove the old copy, queue it again
        let first = db.take_from_voip_ms_queue_item_from_voipms_id("v1".into()).unwrap();
        assert!(first.is_some());
        db.add_from_voip_ms_queue_item_since(text("v1"), "Processing".into(), first).unwrap();
        // a fresh text
        db.add_from_voip_ms_queue_item(text("v2"), "No Respondent".into()).unwrap();
        db.purge_old_queue_items(24 * 3600, 3600).unwrap();
        let left: Vec<String> = {
            let conn = Connection::open(&path).unwrap();
            let mut stmt = conn.prepare("SELECT voipms_id FROM from_voipms_queue").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(left, vec!["v2".to_string()], "the 30 h old text must be purged even after a retry");
        let _ = fs::remove_file(&path);
    }

    /// A database from before the opt-out date existed must be upgraded at startup,
    /// keep its data, and survive being upgraded again on the next start.
    #[test]
    fn test_upgrade_old_denied_did_table() {
        crypto::init_test_env();
        let dir = std::env::temp_dir().join(format!("sbchat-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.db");
        let _ = fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "create table denied_did (
                 id_denied_did integer primary key autoincrement,
                 id_call_center integer not null,
                 did_number integer not null,
                 unique(id_call_center,did_number))",
                (),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO denied_did (id_call_center,did_number) values (1,5551234567)",
                (),
            )
            .unwrap();
        }
        let db_manager = DbManager { path: path.clone() };
        db_manager.create_db_if_not_exist().unwrap();
        db_manager.create_db_if_not_exist().unwrap();

        let conn = Connection::open(&path).unwrap();
        let cols = columns(&conn, "denied_did");
        assert!(cols.contains(&"denied_at".to_string()));
        assert!(cols.contains(&"denied_by".to_string()));
        assert!(db_manager.is_did_denied(1, 5551234567).unwrap());
        assert!(db_manager.deny_did(1, 5559999999, "test").unwrap());
        assert!(!db_manager.deny_did(1, 5559999999, "test").unwrap());
        assert!(db_manager.is_did_denied_by_call_center_did(0, 5559999999).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Law 25 upgrade: old rows keep working but no clear number or password stays in the
    /// database, and running the upgrade again does not hash twice.
    #[test]
    fn test_law25_migration() {
        crypto::init_test_env();
        let dir = std::env::temp_dir().join(format!("sbchat-law25-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.db");
        let _ = fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "create table room (id_room text primary key, id_call_center integer not null,
                    did_number integer not null, call_center_did integer,
                    creation_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
                    last_accessed_time timestamp, is_in_one_to_one INTEGER DEFAULT 0,
                    unique(id_room,did_number));
                 insert into room (id_room,id_call_center,did_number,call_center_did)
                    values ('!old:s',7,4185551234,5145550142);
                 create table denied_did (id_denied_did integer primary key autoincrement,
                    id_call_center integer not null, did_number integer not null,
                    denied_at integer, denied_by text, unique(id_call_center,did_number));
                 insert into denied_did (id_call_center,did_number) values (7,4185559999);
                 create table call_center (id_call_center integer primary key autoincrement,
                    sb_chat_api_token text not null unique, sb_chat_api_token_hashed text not null,
                    voip_ms_username text, voip_ms_api_password text, voip_ms_api_url text,
                    call_center_did integer, privilege_level integer DEFAULT 0);
                 insert into call_center (sb_chat_api_token,sb_chat_api_token_hashed,voip_ms_username,voip_ms_api_password,call_center_did)
                    values ('tok-legacy-plaintext','h','user@x.ca','s3cret',5145550142);",
            )
            .unwrap();
        }
        let db = DbManager { path: path.clone() };
        db.create_db_if_not_exist().unwrap();
        db.create_db_if_not_exist().unwrap(); // idempotent

        // Lookups and replies still work
        assert_eq!(
            db.fetch_room_id_from_did(7, 4185551234, 5145550142).unwrap().as_deref(),
            Some("!old:s")
        );
        let room = RoomId::parse("!old:s").unwrap();
        assert_eq!(
            db.fetch_subscriber_info_from_room_id(&room).unwrap(),
            Some((7, 4185551234, 5145550142))
        );
        assert!(db.is_did_denied(7, 4185559999).unwrap());
        assert_eq!(db.get_call_center_credentials(1).unwrap().1, "s3cret");

        // Nothing readable is left in the file
        let conn = Connection::open(&path).unwrap();
        let stored_did: i64 = conn
            .query_row("select did_number from room", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored_did, 4185551234);
        let denied: i64 = conn
            .query_row("select did_number from denied_did", [], |r| r.get(0))
            .unwrap();
        assert_ne!(denied, 4185559999);
        let pw: String = conn
            .query_row("select voip_ms_api_password from call_center", [], |r| r.get(0))
            .unwrap();
        assert!(pw.starts_with("enc:") && !pw.contains("s3cret"));
        let raw = fs::read(&path).unwrap();
        let contains = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes());
        assert!(!contains("4185551234") && !contains("4185559999") && !contains("s3cret"));
        // API tokens: sealed in the file, still found by the token itself
        assert!(!contains("tok-legacy-plaintext"));
        assert_eq!(db.get_call_center_id_by_token("tok-legacy-plaintext".into()).unwrap().map(|r| r.0), Some(1));
        assert_eq!(db.get_call_center_token_by_id(1).unwrap().as_deref(), Some("tok-legacy-plaintext"));
        assert!(db.get_call_center_id_by_token("wrong".into()).unwrap().is_none());

        // Retention and erasure
        db.add_room_id_to_did(&"!new:s".to_string(), &7, &4185550000, &5145550142).unwrap();
        conn.execute("update room set creation_time = datetime('now','-10 days') where id_room = '!old:s'", []).unwrap();
        assert_eq!(db.fetch_idle_sms_room_ids(7).unwrap(), vec!["!old:s".to_string()]);
        // Queued texts of that caller: encrypted at rest, readable back, removed by the erasure
        db.add_from_voip_ms_queue_item_since(
            QueryParams {
                to: Some("5145550142".into()),
                from: Some("4185550000".into()),
                message: Some("queued-secret-text".into()),
                id: Some("q-erase".into()),
                timestamp: None,
                sbchatapikey: Some("k".into()),
                is_reprocessing_error: None,
            },
            "Processing".into(),
            None,
        )
        .unwrap();
        db.add_to_voip_ms_queue_item(
            "https://voip.ms/api/v1/rest.php?api_username=u&method=sendSMS&did=5145550142&dst=4185550000&message=queued-reply-text".into(),
            "Processing".into(),
        )
        .unwrap();
        let raw = fs::read(&path).unwrap();
        let contains = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes());
        assert!(!contains("queued-secret-text") && !contains("queued-reply-text") && !contains("4185550000"));
        let from_q = db.get_from_voip_ms_queue_content().unwrap().unwrap();
        assert_eq!(from_q[0].queryparams.message.as_deref(), Some("queued-secret-text"));
        assert_eq!(from_q[0].queryparams.from.as_deref(), Some("4185550000"));
        assert!(db.get_to_voip_ms_queue_content().unwrap().unwrap()[0].send_message_url.contains("queued-reply-text"));
        let erased = db.erase_caller(7, 4185550000).unwrap();
        assert!(db.get_from_voip_ms_queue_content().unwrap().is_none());
        assert!(db.get_to_voip_ms_queue_content().unwrap().is_none());
        assert_eq!(erased, vec!["!new:s".to_string()]);
        assert!(db.fetch_room_id_from_did(7, 4185550000, 5145550142).unwrap().is_none());
        drop(conn);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Manual check on a COPY of a real database:
    /// SBCHAT_DB_COPY=/path/to/copy.db cargo test migrate_copy_of_real_database -- --ignored --nocapture
    #[test]
    #[ignore]
    fn migrate_copy_of_real_database() {
        dotenvy::dotenv().ok();
        let path = PathBuf::from(env::var("SBCHAT_DB_COPY").expect("SBCHAT_DB_COPY"));
        let db = DbManager { path: path.clone() };
        db.create_db_if_not_exist().unwrap();
        db.create_db_if_not_exist().unwrap();
        let conn = Connection::open(&path).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        println!("rooms: {}", count("select count(*) from room"));
        assert_eq!(count("select count(*) from room where did_enc is null"), 0);
        assert_eq!(count("select count(*) from room where did_number > 100000000000 and did_number < 100000000000000000"), 0);
        println!("denied: {}", count("select count(*) from denied_did"));
        assert_eq!(
            count("select count(*) from call_center where voip_ms_api_password not like 'enc:%'"),
            0
        );
        // every room still resolves to its caller number
        let ids = db.fetch_room_ids_by_channel("sms").unwrap();
        for id in &ids {
            let room = RoomId::parse(id.as_str()).unwrap();
            assert!(db.fetch_subscriber_info_from_room_id(&room).unwrap().is_some());
        }
        println!("sms rooms decryptable: {}", ids.len());
        // credentials still readable
        assert!(db.get_call_center_credentials(1).is_ok());
    }

    /// The web chat block list stores only hashes, honors expiry, and can be lifted.
    #[test]
    fn test_web_block_list() {
        let dir = std::env::temp_dir().join(format!("sbchat-block-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.db");
        let _ = fs::remove_file(&path);
        crypto::init_test_env();
        let db = DbManager { path: path.clone() };
        db.create_db_if_not_exist().unwrap();
        let now = 1_000_000;
        assert!(!db.is_web_blocked("aaaa", now).unwrap());
        db.add_web_block("aaaa", now + 3600).unwrap();
        assert!(db.is_web_blocked("aaaa", now).unwrap());
        assert!(!db.is_web_blocked("aaaa", now + 3600).unwrap()); // expired exactly at the end
        assert!(!db.is_web_blocked("bbbb", now).unwrap());
        // a shorter block never shortens an existing one
        db.add_web_block("aaaa", now + 60).unwrap();
        assert!(db.is_web_blocked("aaaa", now + 3000).unwrap());
        // purge and lift
        db.add_web_block("old", now - 10).unwrap();
        assert_eq!(db.purge_expired_web_blocks(now).unwrap(), 1);
        assert!(db.remove_web_block("aaaa").unwrap());
        assert!(!db.remove_web_block("aaaa").unwrap());
        db.add_web_block("c", now + 5).unwrap();
        db.add_web_block("d", now + 5).unwrap();
        assert_eq!(db.clear_web_blocks().unwrap(), 2);
        assert!(db.list_active_web_block_hashes(now).unwrap().is_empty());
        db.add_web_block("live", now + 100).unwrap();
        db.add_web_block("dead", now - 100).unwrap();
        assert_eq!(db.list_active_web_block_hashes(now).unwrap(), vec!["live".to_string()]);
        db.clear_web_blocks().unwrap();
        // abuse pressure and small state
        db.add_web_block("x", now + 5).unwrap();
        assert!(db.count_recent_web_blocks(0).unwrap() >= 1);
        assert_eq!(db.count_recent_web_blocks(i64::MAX).unwrap(), 0);
        assert_eq!(db.get_web_state("markers_valid_after").unwrap(), None);
        db.set_web_state("markers_valid_after", 42).unwrap();
        db.set_web_state("markers_valid_after", 43).unwrap();
        assert_eq!(db.get_web_state("markers_valid_after").unwrap(), Some(43));
        fs::remove_dir_all(&dir).unwrap();
    }
}
