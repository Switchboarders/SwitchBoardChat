// Server administration through Matrix Authentication Service (MAS).
//
// With MAS, the bot's own login (a password "compatibility" session) can never be a
// server admin. Admin calls instead use a dedicated MAS OAuth client (client
// credentials grant) whose token is accepted both by the MAS admin API (checking
// that the bot user is an admin, signing out its stale devices) and by the Synapse
// admin API (removing its rate limits).
//
// Optional .env settings:
//   MAS_ADMIN_CLIENT_ID      client id declared in MAS config `clients:`
//   MAS_ADMIN_CLIENT_SECRET  its client_secret (client_auth_method: client_secret_basic)
//   MAS_ADMIN_API_URL        defaults to <MAS issuer>api/admin/v1/
// The client must also be listed in MAS config `policy.data.admin_clients`, and the
// `adminapi` resource must be enabled on a MAS http listener.
use anyhow::{Result, anyhow};
use matrix_sdk::Client;
use matrix_sdk::reqwest;
use serde_json::{Value, json};
use std::env;
use tracing::{info, warn};
use url::Url;

/// MAS admin API (look up users, sign out devices). MAS grants it to `admin_clients`.
const MAS_SCOPE: &str = "urn:mas:admin";
/// Synapse admin API (rate limits). Asked for separately: MAS may refuse it to a client
/// ("requires admin privileges") while granting the MAS scope, and one refused scope fails the whole request.
const SYNAPSE_SCOPE: &str = "urn:synapse:admin:*";

pub struct MasAdmin {
    http: reqwest::Client,
    token: String,
    admin_api: Url,
    homeserver: Url,
}

pub struct MasUser {
    /// MAS user id (a ULID, not the Matrix ID)
    pub id: String,
    pub admin: bool,
}

fn configured_client() -> Option<(String, String)> {
    let id = env::var("MAS_ADMIN_CLIENT_ID").ok().filter(|v| !v.is_empty())?;
    let secret = env::var("MAS_ADMIN_CLIENT_SECRET").ok().filter(|v| !v.is_empty())?;
    Some((id, secret))
}

/// Explains how to give the bot admin access, shown when it is missing or broken
const SETUP_HELP: &str = "To enable it: in MAS config.yaml, add a client under `clients:` \
(client_id: a ULID, client_auth_method: client_secret_basic, client_secret: ...), list that \
client_id under `policy.data.admin_clients`, make sure a MAS http listener has the `adminapi` \
resource, restart MAS, then set MAS_ADMIN_CLIENT_ID and MAS_ADMIN_CLIENT_SECRET in .env.";

impl MasAdmin {
    /// Gets an admin token. Ok(None) when MAS_ADMIN_CLIENT_ID/SECRET are not configured.
    /// Tokens are short lived: connect again for each operation.
    pub async fn connect(client: &Client) -> Result<Option<Self>> {
        Self::connect_with_scope(client, MAS_SCOPE).await
    }

    /// Same, with a token for the Synapse admin API
    pub async fn connect_synapse(client: &Client) -> Result<Option<Self>> {
        Self::connect_with_scope(client, SYNAPSE_SCOPE).await
    }

    async fn connect_with_scope(client: &Client, scope: &str) -> Result<Option<Self>> {
        let Some((client_id, client_secret)) = configured_client() else {
            return Ok(None);
        };
        let metadata = client
            .oauth()
            .server_metadata()
            .await
            .map_err(|err| anyhow!("Could not fetch MAS server metadata: {err}"))?;
        let admin_api = match env::var("MAS_ADMIN_API_URL") {
            Ok(url) if !url.is_empty() => Url::parse(&url)?,
            _ => metadata.issuer.join("api/admin/v1/")?,
        };
        let admin_api = if admin_api.path().ends_with('/') {
            admin_api
        } else {
            Url::parse(&format!("{admin_api}/"))?
        };

        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("scope", scope)
            .finish();
        let response = http
            .post(metadata.token_endpoint.clone())
            .basic_auth(&client_id, Some(&client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;
        let status = response.status();
        let json: Value = serde_json::from_str(&response.text().await?)
            .map_err(|err| anyhow!("MAS token endpoint returned invalid JSON ({status}): {err}"))?;
        let Some(token) = json["access_token"].as_str() else {
            return Err(anyhow!(
                "MAS refused the admin client credentials ({status}): {} {}",
                json["error"],
                json["error_description"]
            ));
        };
        Ok(Some(Self {
            http,
            token: token.to_string(),
            admin_api,
            homeserver: client.homeserver(),
        }))
    }

    async fn request(&self, method: reqwest::Method, url: Url, body: Option<Value>) -> Result<(u16, Value)> {
        let mut request = self.http.request(method, url.clone()).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let text = response.text().await?;
        let json = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok((status, json))
    }

    fn parse_user(json: &Value) -> Result<MasUser> {
        let attributes = &json["data"]["attributes"];
        let Some(id) = json["data"]["id"].as_str() else {
            return Err(anyhow!("Unexpected MAS answer: {json}"));
        };
        Ok(MasUser {
            id: id.to_string(),
            admin: attributes["admin"].as_bool().unwrap_or(false),
        })
    }

    /// Looks up a MAS user by username (localpart)
    pub async fn find_user(&self, username: &str) -> Result<Option<MasUser>> {
        let url = self.admin_api.join(&format!("users/by-username/{username}"))?;
        match self.request(reqwest::Method::GET, url, None).await? {
            (200, json) => Ok(Some(Self::parse_user(&json)?)),
            (404, _) => Ok(None),
            (status, json) => Err(anyhow!("MAS admin API error {status}: {json}")),
        }
    }

    /// Signs out one device of a user: ends the MAS session behind it, which removes the
    /// device from Synapse. Returns false when no active session has that device id.
    /// (With MAS, a client cannot delete its own devices: Synapse answers 404.)
    pub async fn end_device_session(&self, mas_user_id: &str, device_id: &str) -> Result<bool> {
        // The bot logs in with a password, so its devices are "compatibility" sessions
        let mut url = self.admin_api.join("compat-sessions")?;
        url.query_pairs_mut()
            .append_pair("filter[user]", mas_user_id)
            .append_pair("filter[status]", "active")
            .append_pair("page[first]", "100");
        let (status, json) = self.request(reqwest::Method::GET, url, None).await?;
        if status != 200 {
            return Err(anyhow!("MAS admin API error {status}: {json}"));
        }
        let session_id = json["data"].as_array().and_then(|sessions| {
            sessions
                .iter()
                .find(|session| session["attributes"]["device_id"].as_str() == Some(device_id))
                .and_then(|session| session["id"].as_str())
        });
        let Some(session_id) = session_id else {
            return Ok(false);
        };
        let url = self.admin_api.join(&format!("compat-sessions/{session_id}/finish"))?;
        match self.request(reqwest::Method::POST, url, None).await? {
            (status, _) if (200..300).contains(&status) => Ok(true),
            (status, json) => Err(anyhow!("MAS admin API error {status}: {json}")),
        }
    }

    /// Creates a personal session acting as `mas_user_id` with the given scopes, its token expiring
    /// after `expires_in` seconds. Returns (session id, access token). Used for one-off Synapse admin
    /// calls that MAS refuses to the admin client itself (see room_purge).
    pub async fn create_personal_session(
        &self,
        mas_user_id: &str,
        name: &str,
        scope: &str,
        expires_in: u32,
    ) -> Result<(String, String)> {
        let url = self.admin_api.join("personal-sessions")?;
        let body = json!({
            "actor_user_id": mas_user_id,
            "human_name": name,
            "scope": scope,
            "expires_in": expires_in,
        });
        match self.request(reqwest::Method::POST, url, Some(body)).await? {
            (200 | 201, json) => {
                let id = json["data"]["id"].as_str();
                let token = json["data"]["attributes"]["access_token"].as_str();
                match (id, token) {
                    (Some(id), Some(token)) => Ok((id.to_string(), token.to_string())),
                    _ => Err(anyhow!("MAS did not return a personal session token")),
                }
            }
            // The body never holds the token on errors
            (status, json) => Err(anyhow!("MAS admin API error {status}: {}", json["errors"])),
        }
    }

    /// Revokes a personal session (its token stops working at once)
    pub async fn revoke_personal_session(&self, session_id: &str) -> Result<()> {
        let url = self.admin_api.join(&format!("personal-sessions/{session_id}/revoke"))?;
        match self.request(reqwest::Method::POST, url, None).await? {
            (status, _) if (200..300).contains(&status) => Ok(()),
            (status, json) => Err(anyhow!("MAS admin API error {status}: {}", json["errors"])),
        }
    }

    /// Exempts a Matrix user from all Synapse rate limits (room creation included)
    pub async fn remove_rate_limits(&self, user_id: &str) -> Result<()> {
        let url = self
            .homeserver
            .join(&format!("_synapse/admin/v1/users/{user_id}/override_ratelimit"))?;
        let body = json!({ "messages_per_second": 0, "burst_count": 0 });
        match self.request(reqwest::Method::POST, url, Some(body)).await? {
            (200, _) => Ok(()),
            (status, json) => Err(anyhow!("Synapse admin API error {status}: {json}")),
        }
    }
}

/// Startup check: warns when the bot has no admin access or its user isn't an admin,
/// and removes the bot's Synapse rate limits when it can.
pub async fn startup_check(client: Client) {
    let Some(bot_user_id) = client.user_id().map(|id| id.to_owned()) else {
        warn!("Admin check skipped: the bot is not logged in.");
        return;
    };
    let admin = match MasAdmin::connect(&client).await {
        Ok(Some(admin)) => admin,
        Ok(None) => {
            warn!(
                "No MAS admin client configured (MAS_ADMIN_CLIENT_ID / MAS_ADMIN_CLIENT_SECRET): \
                the bot keeps Synapse's rate limits (rooms may be created slowly when busy). \
                {SETUP_HELP}"
            );
            return;
        }
        Err(err) => {
            warn!("MAS admin client not working: {err}. {SETUP_HELP}");
            return;
        }
    };

    match admin.find_user(bot_user_id.localpart()).await {
        Ok(Some(user)) if user.admin => info!("Bot user {bot_user_id} is a server admin."),
        Ok(Some(_)) => warn!(
            "Bot user {bot_user_id} is NOT a server admin in MAS. Make it one with: \
            docker exec -it mas mas-cli manage promote-admin {}",
            bot_user_id.localpart()
        ),
        Ok(None) => warn!("Bot user {bot_user_id} was not found in MAS."),
        Err(err) => warn!("Could not check whether {bot_user_id} is an admin: {err}"),
    }

    // The override is stored by Synapse: once removed, the limits stay removed even if this fails later
    match MasAdmin::connect_synapse(&client).await {
        Ok(Some(synapse)) => match synapse.remove_rate_limits(bot_user_id.as_str()).await {
            Ok(()) => info!("Synapse rate limits removed for {bot_user_id}."),
            Err(err) => warn!("Could not remove Synapse rate limits for {bot_user_id}: {err}"),
        },
        Ok(None) => {}
        Err(err) => info!(
            "The MAS client has no Synapse admin access ({err}), so the bot's Synapse rate limits are left as \
            they are. That is fine if they were already removed once (Synapse keeps the override)."
        ),
    }
}
