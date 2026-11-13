**English** · [Français](fr/UPGRADE.md)

# Upgrading switchboard-chat to the Law 25 / web chat version

For an existing install that predates `PHONE_ENC_KEY` and the web chat. Works for Docker and for a
plain binary; replace `<install dir>` with the folder holding your `.env` and `<data dir>` with
`APP_DATA_DIR` (`./data` by default, `/app/data` in the Docker image).

Plan about 45 minutes. The SMS service is down for the 1–2 minutes of the restart.

## 0. What the upgrade does

1. **Law 25 for SMS** ([law25.md](law25.md)): caller numbers are hashed (lookups) and encrypted
   (replies), voip.ms passwords are encrypted, queues are purged, logs no longer hold message text
   or numbers, conversations expire after `SMS_IDLE_RETENTION_DAYS` (default 7).
2. **Web chat** ([web-chat.md](web-chat.md)), off until configured.
3. The database is **migrated automatically at the first start** (idempotent). The old version
   cannot read the migrated database, so a rollback means restoring your backup (section 7).

## 1. Know what must be backed up

Everything under `<data dir>` **plus** `<install dir>/.env`:

| Item | Why |
|---|---|
| `db/users.db` | call centers, respondents, rooms, opt-outs, queues |
| `db/<random folders>/` | the Matrix client's encrypted store (keys). Several folders may exist. |
| `session` (and `sessionbak`) | the bot's login. **It names one of the store folders**; the two belong together. |
| `web_server_keys/` | the self-signed certificate (regenerated if missing) |
| `.env` | secrets and settings |

> **Never restore, delete or move only part of this set.** If the `session` file survives but its
> store folder is replaced by an empty one, the bot's device appears "not cross-signed", the
> Matrix key upload is refused (HTTP 400) and **the web server never starts** (symptom: the
> container is "up" but the public address returns 502 and the log stops after
> `Starting sync loop`). Restore `db/` and `session` together from the same backup, or re-run the
> first-run login (readme, step 4).

Take the copy **while the bridge is stopped** (section 5) so the files are consistent, and keep it
somewhere that is not the same disk. It contains secrets: restrict its permissions.

## 2. Prepare the new version

**Docker.** Build the image on a machine with enough memory (a Rust build needs several GB), then
ship it, keeping the old image for rollback:

```bash
git pull && docker build -t sbchat:latest .
ssh <host> 'docker tag sbchat:latest sbchat:previous'
docker save sbchat:latest | gzip | ssh <host> 'gunzip | docker load'
```

Or build on the host with `docker compose build`. The image no longer contains `.env`: the file
is passed at run time (`env_file` in `docker-compose.yml`).

**Binary.** `cargo build --release` and keep the previous binary next to the new one.

## 3. New settings in `.env`

Generate the encryption key and **store a copy outside the server** (password manager):

```bash
echo "PHONE_ENC_KEY=$(openssl rand -hex 32)" >> .env
```

- Never change `PHONE_ENC_KEY` afterwards: open SMS conversations and stored passwords become unreadable.
- Never change `PHONE_HASH_SECRET`: the opt-out list would forget everyone.

Optional now or later (see `.env-example`): `SMS_IDLE_RETENTION_DAYS`, and for the web chat
`WEB_ALLOWED_ORIGINS`, `WEB_CALL_CENTER_ID`, `WEB_IDLE_TIMEOUT_MIN`, `WEB_MAX_SESSIONS`.

> **First-start retention purge.** Roughly a minute after the first start the bridge deletes every
> SMS room idle for more than `SMS_IDLE_RETENTION_DAYS` days: the bot posts an "expired" notice,
> leaves the room and forgets the caller mapping. On an old install this can be most of the rooms.
> That is the intended policy. To keep them for now, set `SMS_IDLE_RETENTION_DAYS` high (for example
> `3650`) for the first start and lower it later.

## 4. Reverse proxy (only if you will use the web chat)

Add the streaming rule from [reverse-proxy.md](reverse-proxy.md) to **every** virtual host that
serves the bridge. Validate and reload the proxy.

## 5. Switch over (downtime starts)

```bash
# Docker
cd <install dir>
docker compose stop sb-chat                       # stop the bridge first
cp -a <data dir> <backup location>/data-$(date +%F)   # consistent copy of db/, session, …
cp .env <backup location>/env-$(date +%F)
docker compose up -d --no-build --force-recreate sb-chat
docker logs -f sb-chat
```

`--no-build` runs the image you loaded; `--force-recreate` makes the container read the new `.env`.
For a binary: stop the service, copy the data directory, start the new binary.

Expected in the log within a minute:

- `Database upgraded for Law 25: phone numbers are now hashed or encrypted`
- `Is this device self verified ? is_cross_signed_by_owner() true`
- `HTTPS server listening on …`
- no `panicked`, no `PHONE_ENC_KEY must be set`

If the device shows `… false` or the web server line never appears, see the warning in section 1.

## 6. Verify

```bash
# database (expect: ok / 2 / 0)
sqlite3 <data dir>/db/users.db "pragma integrity_check; pragma user_version; select count(*) from room where did_enc is null;"
# voip.ms passwords are encrypted (expect enc:)
sqlite3 <data dir>/db/users.db "select substr(voip_ms_api_password,1,4) from call_center;"
```

- Send a text to the call center number: respondents see it; their reply reaches the phone.
- The first SMS a **new** number receives ends with the privacy line.
- The log has no tokens, passwords, phone numbers or message text:
  `docker logs sb-chat 2>&1 | grep -c -E "api_password|sb_chat_api_token|sbchatapikey"` → `0`.
- Web chat: follow section 4 of [web-chat.md](web-chat.md).

After a few days of normal use: delete the backup copies you do not need, and delete log files
written **before** the upgrade (they contain numbers and message text).

## 7. Rollback

```bash
docker compose stop sb-chat
# restore the data directory and .env from the backup taken in section 5 (all of it, together)
docker tag sbchat:previous sbchat:latest
docker compose up -d --no-build --force-recreate sb-chat
```

Data created after the backup is lost with the rollback.

## 8. After the upgrade

- [ ] Rotate any API token that was ever stored in a script or committed to a repository
      (`renew_api_token` in `add_new_call_center` / `update_call_center`).
- [ ] Tell respondents about `!oubli` / `!forget`, and `!fin` / `!end` (web chats); `!aide` lists them.
- [ ] Review [law25.md](law25.md) with whoever is responsible for privacy.
- [ ] Notifications: turn on Synapse `auto_accept_invites` and check each respondent's phone
      ([respondent-notifications.md](respondent-notifications.md)).
- [ ] Optional safety net: a retention policy on the Matrix homeserver ([homeserver.md](homeserver.md)
      has a sample). **It permanently deletes old events in every room of that server**: only enable
      it if everyone using the server agrees.

## 9. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Restart loop, log says `PHONE_ENC_KEY must be set` | key missing; also recreate the container after editing `.env` |
| `PHONE_ENC_KEY must be 64 hex characters` | regenerate with `openssl rand -hex 32` (before the first start with it) |
| `… cannot be decrypted (wrong PHONE_ENC_KEY?)` | a different key than the one used for the migration: put the right one back or restore the pre-upgrade backup |
| Opt-out list empty after upgrade | `PHONE_HASH_SECRET` changed: put the old value back |
| Public URL returns 502, container "up" | web server not started: bot device not cross-signed (section 1); or proxy cannot reach port 8088 |
| `docker load` out of space | `df -h`, `docker image prune` |
