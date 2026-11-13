**English** · [Français](readme.fr.md)

# SwitchBoardChat — Installation Guide

This README explains how to deploy **SwitchBoardChat** on a VPS (recommended) or prepare local builds for remote deployment.

> **Quick install:** `scripts/install.sh` does steps 3 to 5 below in one go (after cloning), also non-interactively: see [docs/install.md](docs/install.md).

> **Important:** You need your **own Matrix Synapse server** with **Matrix Authentication Service (MAS)**. Modern Matrix clients require MAS for end‑to‑end encryption. DO NOT use `matrix.org` — its rate limits will cause crashes.

---

## 🚀 VPS Installation (Recommended)

### 1. **Create a password‑based Matrix account (via MAS)**
You must create a local user on your Matrix homeserver.

Inside your MAS Docker container, create the user:
```bash
docker compose exec mas mas-cli manage register-user
```
- `mas` = container name
- `mas-cli manage register-user` = command run inside container

Mark the user as **admin** and choose a strong password.
This account will act as **sbchat** (internal bot user).

Make sure that you have only one active session for your user. Delete unwanted sessions via MAS. A common bug is having unverified sessions which prevent messages from being delivered.
Save:
- Full username (e.g. `@sbchat:your-homeserver.com`)
- Password

For the Synapse + MAS settings the bridge needs, see [docs/homeserver.md](docs/homeserver.md).

---

## 2. **Clone the Repository**
```bash
git clone https://github.com/Switchboarders/SwitchBoardChat.git
```
*Repository is private — you may need access rights.*

---

## 3. **Configure the `.env` File**
At the project root, rename the example file:
```bash
mv .env-example .env
```
Edit the `.env` file:

### Required values
| Variable | Description |
|---------|-------------|
| `PHONE_HASH_SECRET` | Any random string. Used to hash phone numbers into names. |
| `HOMESERVER_URL` | Your Matrix homeserver URL. |
| `MATRIX_USERNAME` | The MAS account you just created. |
| `MATRIX_PASSWORD` | Password of this MAS account. |
| `VIRTUAL_HOST` | Your domain or subdomain name. Use localhost if everything on the same server. Note that some web hoster block 8448 port. You can use a reverse proxy, sbchat listen internally on the 8088 port|
### Optional values
The `.env-example` file documents every variable. Besides the required ones:

| Variable | Description |
|---------|-------------|
| `MUTE_VOIPMS_REQUEST` | `true` queues SMS but does not send them (testing). |
| `APP_DATA_DIR` | Folder for database, logs, Matrix session and certificates. Default `./data`. |
| `MAS_ADMIN_*` | Optional MAS admin client, lets the bot remove its own rate limits. |
| `PHONE_ENC_KEY` | **Required.** 64 hex characters. Encrypts caller numbers (open conversations) and voip.ms passwords. |
| `SMS_IDLE_RETENTION_DAYS` | Days of caller silence before an SMS conversation is deleted. Default 7. |
| `ATTENTION_ENABLED`, `ATTENTION_JOIN_WAIT_SECS`, `ATTENTION_HOURLY_MAX`, `ATTENTION_FOLLOWUP_MIN` | Ping and reminders for new / unanswered conversations (default on); follow-ups in an answered conversation are reminded after 10 minutes only. See [docs/respondent-notifications.md](docs/respondent-notifications.md). |
| `WEB_ALLOWED_ORIGINS` | Websites allowed to use the web chat widget (comma separated, e.g. `https://myorg.ca`). Web chat is off while empty. |
| `WEB_CALL_CENTER_ID` | Call center whose respondents answer web chats. Web chat is off while empty. |
| `WEB_IDLE_TIMEOUT_MIN` | Minutes after the last interaction before a web chat is deleted. Default 2880 (48 h). |
| `WEB_MAX_SESSIONS` | Simultaneous web chats allowed. Default 200. |
| `WEB_ABANDON_MIN` | Minutes after which a session whose visitor never wrote and whose connection has closed is dropped. Default 5. |
| `WEB_CHALLENGE`, `WEB_CHALLENGE_BASE_BITS`, `WEB_CHALLENGE_MAX_BITS`, `WEB_CHALLENGE_WINDOW_HOURS` | Proof-of-work challenge before a chat starts (`auto` by default: only after abuse). See [docs/web-chat.md](docs/web-chat.md). |
| `WEB_ATTACK_STARTS_PER_MIN`, `WEB_ATTACK_HOLD_MIN`, `WEB_MAX_NEW_ROOMS_PER_MIN`, `WEB_NO_MESSAGE_TIMEOUT_MIN` | Denial-of-service protection: under-attack mode, room cap, silent chats. See [docs/web-chat.md](docs/web-chat.md). |
| `WEB_SESSION_COOKIE` | `true` also sets the legacy cookie (only for wp-chat widgets older than 0.5.0). Default `false`. |

### Privacy (Quebec Law 25)
Caller phone numbers are never stored in clear: the database keeps a keyed hash (`PHONE_HASH_SECRET`) and, only while a conversation is open, an AES-256-GCM encrypted copy (`PHONE_ENC_KEY`, 64 hex characters, `openssl rand -hex 32`). **Back both secrets up apart from the database**: without `PHONE_ENC_KEY` open conversations can't be read, and changing `PHONE_HASH_SECRET` makes the opt-out list forget everyone. Voip.ms passwords are stored encrypted too. SMS rooms are deleted after `SMS_IDLE_RETENTION_DAYS` days (default 7) without a text from the caller; retry queues are purged after 24 h (inbound) and 1 h (outbound); logs hold no message text, number or password. A respondent can type `!oubli` (or `!forget`) in a room, or an admin can call the API command `erase_caller`, to erase a caller. The first start after upgrading migrates the database automatically: **stop the bridge and copy the whole data directory first** (`db/` and `session` must always be restored together) and delete the old `data/logs/*`. Details: [docs/UPGRADE.md](docs/UPGRADE.md) and [docs/law25.md](docs/law25.md). Synapse settings (retention, invitations): [docs/homeserver.md](docs/homeserver.md).

### Documentation
See the [docs/](docs/README.md) folder: [web chat](docs/web-chat.md), [reverse proxy (nginx, Caddy, Apache, Traefik)](docs/reverse-proxy.md), [upgrading](docs/UPGRADE.md), [Law 25 privacy notes](docs/law25.md).

### How the web chat works
A visitor opens the chat bubble of the wp-chat WordPress plugin. The visitor's browser talks directly to this server (`/web/*` routes); the WordPress server never sees the messages. The server gives the browser a random session token (kept in the page's memory, no cookie) and creates an encrypted Matrix room with the call center's respondents when the first message is sent. Respondents answer in their usual Matrix client; their text goes back to the browser, not by SMS. Messages are never written to the database or logs. The chat ends, and the room is left and its database row deleted, when the visitor ends it, when a respondent types `!fin` (or `!end`), or after the idle timeout. Web chats live in memory only, so a server restart ends them. Put a reverse proxy with a real certificate in front (port 8088) for production, because the website itself is served over HTTPS.

If you want to change the generated names list:
```
src/nameslist/names.csv
```
Use many entries to avoid confusion.

---

## 4. **Start the Service (First Time Bootstrap)**
```bash
docker compose up --build
```
You will see a link appear — click it.
It will redirect you to MAS authentication.

⚠️ **Make sure you are logged into MAS with the user you created earlier!**
If you authenticate with the wrong user, bootstrap will fail.

When bootstrap completes, the program exits cleanly.

Then start in background:
```bash
docker compose up -d
```
To watch logs live, omit `-d`.

---
## 5. **You will need your admin api key**
Print it (it is stored encrypted, so it cannot be read from the database):
```bash
docker compose run --rm sb-chat /app/switchboard-chat admin-token
```
(`regenerate-admin-token` replaces it.) `scripts/install.sh` saves it to `admin-api-token` (mode 0600) for you.
Use it in SwitchBoard to create the call centers' own keys.
⚠️ Do not use that token for a call center: it has no DID. Keep it secret.

# 🖥️ Local Build & Remote Deployment (Optional)
These commands help when building locally and sending the image to a remote VPS.

## 1. Clone the repo locally
```bash
git clone https://github.com/Switchboarders/SwitchBoardChat.git
```

## 2. Build the Docker image
```bash
docker compose build
```

## 3. Export the image
```bash
docker save -o sb-chat.tar sbchat:latest
```

## 4. Transfer image to VPS
```bash
rsync -avz -e ssh sb-chat.tar vps:~/
```
Replace `vps` with `user@yourserver.com` if needed.

## 5. Import the image on the VPS
```bash
docker load -i sb-chat.tar
```

Copy your `docker-compose.yml` and `.env` to the VPS project directory.

---


# 🗃️ Old Instructions (Legacy)
These remain for reference but are no longer recommended.

### Build old-style image
```bash
docker build . -t switchboard-chat
```

### Run
```bash
docker run --rm -t -i \
    --restart=unless-stopped \
    -v sb-chat:/app/data \
    -p 3443:3443 \
    switchboard-chat
```

### Run and auto‑restart
```bash
docker run \
    --restart=unless-stopped \
    -v sb-chat:/app/data \
    -p 3443:3443 \
    switchboard-chat
```

### Share local data directory
```bash
docker run --rm \
    -v $(pwd)/data:/app/data:ro \
    switchboard-chat
```

### Save / Load images
```bash
docker save -o sb-chat.tar switchboard-chat
docker load -i sb-chat.tar
```

### Transfer via SSH
```bash
rsync -avz -e ssh sb-chat.tar vps:~/
```

---

## ⚠️ Warning About Session / Matrix SDK Files
Copying session or Matrix SDK files between machines can break E2EE.
You may see warnings like:
```
WARN matrix_sdk::encryption: ... One time key already exists ...
```
If this happens:
👉 **Delete your session file.**
Messages will start sending normally again.

> **Careful:** the `session` file names one of the store folders under `db/`; they belong together.
> Never delete only one of them without following the first-run procedure (step 4): see
> [docs/UPGRADE.md](docs/UPGRADE.md), section 1.

---

📌 **You're now ready to run SwitchBoardChat on your server!**


---

## Security and license

- Report vulnerabilities privately: see [SECURITY.md](SECURITY.md). Threat model and hardening notes:
  [docs/security.md](docs/security.md). Privacy (Quebec Law 25): [docs/law25.md](docs/law25.md).
- **Source-available, non-commercial.** SwitchBoardChat is licensed under the
  [PolyForm Noncommercial License 1.0.0](LICENSE): you may use, study and modify it for any
  noncommercial purpose (helplines, charities, public bodies, personal use). Commercial use needs a
  separate license from the copyright holder. It is not an OSI "open source" license.
