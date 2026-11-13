**English** · [Français](fr/install.md)

# Installing with `scripts/install.sh`

`scripts/install.sh` installs switchboard-chat with Docker Compose from a checkout of this repository,
or updates the configuration of an existing installation. It can be run by hand or called by a larger
installer (Synapse + SwitchBoard + this bridge); everything below is a stable contract for that use.

## Before running it

- Docker with the Compose v2 plugin, `openssl`, `curl`; the user running the script can use Docker.
- A Synapse + MAS homeserver set up as in [homeserver.md](homeserver.md).
- The bot's Matrix account, created in MAS (`mas-cli manage register-user`), with a long random password.

## Usage

```sh
printf '%s\n' "$BOT_PASSWORD" > /run/bot-password    # or export SBCHAT_BOT_PASSWORD
scripts/install.sh --non-interactive \
  --homeserver-url https://matrix.example.org \
  --bot-user @sms:matrix.example.org \
  --bot-password-file /run/bot-password \
  --web-origins https://www.example.org --web-call-center 2 \
  --trusted-proxies 172.18.0.0/16 \
  --output-json /run/sbchat-install.json
```

| Option | Environment variable | Meaning |
|---|---|---|
| `--homeserver-url URL` | `SBCHAT_HOMESERVER_URL` | Synapse URL (required on first install) |
| `--bot-user @user:server` | `SBCHAT_BOT_USER` | the bot's Matrix id (required on first install) |
| `--bot-password-file FILE` | `SBCHAT_BOT_PASSWORD` (the value) | the bot's password; at least 12 characters. There is deliberately no `--bot-password`: a password on the command line is visible to every user of the machine |
| `--virtual-host NAME` | `SBCHAT_VIRTUAL_HOST` | host name in the self-signed certificate of port 3443 (default: the homeserver's host) |
| `--web-origins LIST` | `SBCHAT_WEB_ALLOWED_ORIGINS` | websites allowed to embed the web chat, comma-separated |
| `--web-call-center ID` | `SBCHAT_WEB_CALL_CENTER_ID` | call center that receives web chats |
| `--trusted-proxies LIST` | `SBCHAT_TRUSTED_PROXIES` | addresses/CIDRs of the reverse proxy ([reverse-proxy.md](reverse-proxy.md)) |
| `--http-bind ADDR:PORT` | `SBCHAT_HTTP_BIND` | plain HTTP listener for the reverse proxy (default `0.0.0.0:8088` inside the container, not published) |
| `--mas-admin-client-id ID` | `SBCHAT_MAS_ADMIN_CLIENT_ID` | optional MAS admin client ([homeserver.md](homeserver.md)) |
| `--mas-admin-client-secret-file FILE` | `SBCHAT_MAS_ADMIN_CLIENT_SECRET` (the value) | its secret |
| `--docker-network NAME` | `SBCHAT_DOCKER_NETWORK` | external Docker network shared with the reverse proxy (default `web`, created if missing) |
| `--no-start` | | write the configuration, build and create the admin token, but do not start |
| `--check` | | change nothing; check an existing installation (see below) |
| `--output-json FILE` | | write a summary for the calling installer (mode 0600) |
| `--non-interactive` | | never prompt (automatic when stdin is not a terminal) |
| `--ready-timeout SECONDS` | | how long to wait for the bridge to be ready (default 300) |

Values that would be misread in `.env` (spaces, quotes, `$`, `#`, backslashes, newlines) are refused.

## What it does

1. Creates `.env` from `.env-example` on first install (mode 0600). `PHONE_HASH_SECRET` and
   `PHONE_ENC_KEY` are generated with `openssl rand -hex 32` **once**: a re-run never replaces them
   (changing them would make stored numbers unreadable and the opt-out list forget everyone). The
   values given as options are written; the others are kept.
2. Creates the Docker network if needed and builds the image.
3. Gets the admin API token from the bridge (`switchboard-chat admin-token`, printed on stdout only)
   and saves it to `admin-api-token` (mode 0600) in the checkout. SwitchBoard uses this token to
   create call centers. To replace it: `docker compose run --rm sb-chat /app/switchboard-chat regenerate-admin-token`.
4. Starts the bridge (`docker compose up -d --force-recreate`) and waits until it logs
   "The client is ready".

Running it again is safe: it keeps the secrets, rewrites the given values and restarts the bridge.
The script never prints a secret.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | done (or `--check`: healthy) |
| 2 | bad or missing arguments |
| 3 | a dependency is missing (docker, compose, openssl, curl, Docker daemon access) |
| 4 | build or start failed, or the bridge was not ready in time |
| 5 | `--check` found a problem |

## `--output-json`

```json
{
  "status": "ok",
  "install_dir": "/srv/switchboard-chat",
  "env_file": "/srv/switchboard-chat/.env",
  "data_dir": "/srv/switchboard-chat/sb-chat",
  "admin_token_file": "/srv/switchboard-chat/admin-api-token",
  "container": "sb-chat",
  "ready": true,
  "webhook_url": "https://matrix.example.org:3443/voip_api",
  "webhook_note": "..."
}
```

`status` is `ok`, `configured` (`--no-start`), `start_failed` or `check_failed`. The token itself is
never in the JSON: read it from `admin_token_file`.

## `--check`

Checks, without changing anything: `.env` exists with mode 0600 and the required values set, the admin
token file exists, the container runs and reported ready since its last start, and port 3443 answers.
Prints one `PASS`/`FAIL` line per check on stderr, exits 0 or 5, and writes `--output-json` if given.
