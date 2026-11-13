# switchboard-chat: notes for Claude

SMS (voip.ms) and web chat ⇄ Matrix bridge. Respondents answer callers from Element / Element X.

## Build and test
- `cargo build`, `cargo test` (unit tests; `test_upgrade_old_denied_did_table` writes a temp DB in /tmp).
- End-to-end suite (throwaway Synapse in Docker, no MAS, nothing touches production): `scripts/predeploy-check.sh [quick]`,
  or `python3 tests/e2e/run.py bringup <scenario>...` after `docker build -t sbchat:e2e .`. Most scenarios
  need `bringup` first, and some need state from earlier ones (`restart` needs `roundtrip`). `--keep` leaves the
  containers up; `docker compose -f tests/e2e/docker-compose.yml down -v` afterwards.
- `docker compose logs --since` takes UTC only with a trailing `Z`.

## Deploy (your server)
- Push `main`, then `./scripts/deploy-remote.sh` (snapshot of users.db, `git pull`, rebuild, waits for
  "The client is ready", checks the SMS webhook). Settings in `scripts/deploy.env` (copy
  `scripts/deploy.env.example`). SMS arriving during the ~1 min swap are lost (voip.ms does not retry).
- `./scripts/restart-remote.sh` recreates the bot so it re-reads .env (`docker compose restart` does not).
- The SMS webhook on port 3443 has a self-signed certificate by default: use `curl -k`. Without a key it answers 403.
- MAS's `config.yaml` holds secrets: only read or edit it when the user explicitly asks.
- With MAS, the bot uses a MAS admin client (`MAS_ADMIN_CLIENT_ID` / `_SECRET`) to sign out its own stale devices
  (under MAS a client cannot delete its devices: Synapse answers 404) and, through short-lived personal sessions,
  to purge deleted rooms from Synapse (`src/room_purge.rs`). MAS grants that client `urn:mas:admin` but refuses
  `urn:synapse:admin:*` to it directly.
- Production-specific notes (hosts, paths, accounts) go in `CLAUDE.local.md`, which is git-ignored.

## Gotchas learned the hard way
- A cross-signing reset must be approved in MAS while logged in as the bot account itself; approving from another
  account resets that other account and the bot just times out.
- Element X blocks messages to a verified user who has a device that is not cross-signed ("has unverified devices"):
  every device of the bot must stay cross-signed (`crypto_health::check_own_devices`). When checking by hand,
  verify the signature cryptographically: on 2026-11-05 a stale bot device carried a cross-signature with the
  right key id that did not verify, and only a real verification showed it.
- voip.ms refuses SMS over 160 characters (`sms_toolong`): `VoipMs::send_sms` splits longer ones; automatic texts
  must fit in one SMS (tested in `deny_list`).
- Message bodies are confidential (Quebec Law 25): never log them.
