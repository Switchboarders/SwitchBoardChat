**English** · [Français](fr/homeserver.md)

# Homeserver requirements (Synapse + MAS)

SwitchBoardChat needs its **own** Matrix homeserver: Synapse with Matrix Authentication Service (MAS).
Do not use a public server such as matrix.org (rate limits, and the bridge needs admin rights).
Install both with their official guides, then apply the settings below.

- Synapse: <https://element-hq.github.io/synapse/latest/setup/installation.html>
- MAS: <https://element-hq.github.io/matrix-authentication-service/setup/>
- Delegating Synapse's authentication to MAS: the `matrix_authentication_service` section of the
  [Synapse configuration manual](https://element-hq.github.io/synapse/latest/usage/configuration/config_documentation.html).

## Synapse `homeserver.yaml`

```yaml
# Authentication is MAS's job (Synapse 1.157+)
matrix_authentication_service:
  enabled: true
  endpoint: http://mas:8080/          # MAS on the internal Docker network
  secret: "<same value as MAS matrix.secret>"  # or secret_path: a file holding it

# Respondents are invited to every new room; accepting automatically makes them join at once,
# so the bridge's messages reach them as normal notifications. Local users only.
auto_accept_invites:
  enabled: true
  only_for_direct_messages: false
  only_from_local_users: true

# Respondents look each other up; nobody else should be on this server.
user_directory:
  search_all_users: false

# Optional safety net (Law 25): purges old events in EVERY room of the server.
# Keep max_lifetime at or above SMS_IDLE_RETENTION_DAYS. Only if everyone on the server agrees.
# retention:
#   enabled: true
#   default_policy: { min_lifetime: 1h, max_lifetime: 8d }
#   purge_jobs: [{ longest_max_lifetime: 8d, interval: 12h }]
```

Keep only one HTTP listener (port 8008, `x_forwarded: true`) behind your reverse proxy, and do not
publish 8008 on the host: proxy to it over the Docker network.

## MAS `config.yaml`

- **No open registration**: `account.password_registration_enabled: false` (create accounts with
  `mas-cli manage register-user`, or from the SwitchBoard interface). If you do enable it, require
  email verification.
- **Admin access for the bridge** (optional but recommended: lets it sign out its own stale devices,
  remove its rate limits and purge deleted rooms from Synapse, see [law25.md](law25.md)):
  1. under `clients:`, add a client with `client_id` (a ULID), `client_auth_method: client_secret_basic`
     and a random `client_secret` (`openssl rand -hex 32`);
  2. list that `client_id` under `policy.data.admin_clients`;
  3. make sure an `http.listeners` entry serves the `adminapi` resource (internal network only);
  4. make the bot user an admin: `mas-cli manage promote-admin <bot-localpart>`;
  5. put the id and secret in the bridge's `.env` as `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET`.

## Docker and network

- Pin image versions (e.g. `matrixdotorg/synapse:v1.162.0`), never `:latest`; update deliberately.
- Publish only your reverse proxy (80/443) and, if voip.ms is used, the bridge's port 3443. Synapse
  (8008), MAS (8080/8081) and databases stay on the internal Docker network.
- The reverse proxy serves `/.well-known/matrix/client` and `/.well-known/matrix/server`, and routes
  the MAS paths as described in the MAS documentation. For the web chat, see
  [reverse-proxy.md](reverse-proxy.md).

## The bot account

Create it in MAS (`mas-cli manage register-user`) with a long random password. Keep only the bridge's
own session: other sessions of the bot account that are not cross-signed make Element X refuse to send
messages to it.
