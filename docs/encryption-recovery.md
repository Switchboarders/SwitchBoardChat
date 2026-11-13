**English** · [Français](fr/encryption-recovery.md)

# Encryption recovery: when respondents' messages vanish

Respondents are volunteers. Each time a reply disappears, or they are asked to verify or reset something, some of
them lose patience, and some quit. This page exists so that the operator fixes the cause once, quietly, and never
asks a respondent to reset their identity because of a bridge-side fault.

## 1. Rules

1. **Never ask a respondent to reset their encryption identity, sign out of every device, or re-verify everything
   because of a bridge problem.** The fault is on the bridge side until proven otherwise (section 3).
2. **`session` and `db/` are one unit.** Back them up together, from the same moment, with the bridge stopped
   (`scripts/backup-data.sh`). Restore them together, from one backup folder (`scripts/restore-data.sh`). Never
   restore one without the other, never mix files from two backups, never copy a store while the bridge runs.
3. **Take a backup before every update and every data change**, and keep at least the last three.
4. **Do not delete or reset cross-signing** (the bridge's "identity") to cure a message problem. Every respondent
   would have to approve and re-trust, and an attacker who can trigger a reset could slip in their own session.
5. **After every restart, read the log**: `Crypto check: OK` and `HTTPS server listening` must both appear
   (the check runs a minute after start). "client is ready" alone proves nothing.
6. **Do not edit Synapse's database** except with `scripts/reset-bridge-keys.sh` (it backs up first), and only
   once `crypto-doctor show` proves the bridge's local store is the right one.
7. **Write down every incident** (date, symptom, cause, fix) and share the short version with the respondents
   (see `respondent-troubleshooting.md`): people who are told what happened stay.
8. **Test as a respondent after every fix**: a new chat from the website, answered from a phone **and** a computer.

## 2. Symptoms

| What the respondent sees | What it means |
|---|---|
| Their reply shows a red mark in Element and the visitor never gets it, nothing in the bridge log | The bridge cannot open a secure channel with their device (section 3) |
| The room shows "⚠️ The message from … was NOT received" | The bridge received the message but cannot decrypt it: the sender's device did not share its key (section 4) |
| Only one of their devices works, or the verification room verifies one device at a time | The bridge lost its cross-signing keys, or does not trust its own identity (section 5) |
| The website chat says "connection problem" | The bridge is not serving the web chat (check the log for `HTTPS server listening`) |

## 3. The bridge's keys do not match Synapse's (the serious case)

The log says `CRYPTO CHECK FAILED` after start, or `scripts/check-bridge-keys.sh` prints invalid keys, or Element's
console shows `Error creating Olm session … InvalidSignature` for the bridge's device.

Cause: the encryption store in `db/` has a different device key than the one Synapse holds for the bridge's device
(for example `session` and `db/` were restored from different moments).

**Fix that keeps everyone's trust: log the bridge in as a NEW device and copy its existing cross-signing identity.**
Do not just put the right store back: respondents' Element (especially Element X on phones) caches the old keys of a
device ID for good and never refreshes them, so existing devices would stay unable to reach the bridge. A new device
ID has no stale cache anywhere. (The end-to-end tests replay this incident and check both ways.)

1. Stop the bridge. Back up (`scripts/backup-data.sh`).
2. Find a store that still holds the cross-signing keys (`tools/crypto-doctor`):
   ```
   crypto-doctor show sb-chat/session sb-chat/db/* sb-chat/*/db/*
   ```
   Look for `cross-signing: master=true self-signing=true user-signing=true` and a `master key` equal to the one
   Synapse holds for the bridge's account. Any store of the same account will do; the live one usually has it.
3. Keep the old session aside and let the bridge log in again:
   ```
   mv sb-chat/session sb-chat/session.old
   docker compose up -d sb-chat        # logs in as a new device; the log says "NOT resetting it"
   docker compose stop sb-chat
   ```
   (A fresh login never resets an existing identity. Only `BRIDGE_RESET_CROSS_SIGNING=true` does.)
4. Copy the identity into the new store (the new store folder is `db_name` in the new `session` file):
   ```
   crypto-doctor import-identity sb-chat/session.old sb-chat/db/<old-store> sb-chat/session sb-chat/db/<new-store>
   ```
5. Start the bridge. After a minute the log must say `Crypto check: OK`, `the bridge holds its cross-signing keys` and
   `signed our own device`. `scripts/check-bridge-keys.sh @bridge:… NEWDEVICEID` must print `valid=50 invalid=0`.
6. Test as a respondent (rule 8). Nobody has to re-verify anything.

Old device IDs can be removed later from the account page. Only if no store holds the cross-signing keys any more
is a reset of the bridge's cross-signing needed (`BRIDGE_RESET_CROSS_SIGNING=true`); that is the one case where people
are asked to re-trust the bridge.

`scripts/reset-bridge-keys.sh` (back up and delete the bridge device's stale keys on Synapse, restart Synapse) is only
for the rare case where the right store is already in place and only Synapse holds stale keys.

## 4. "Message NOT received" in a room

The bridge's keys are fine, but that sender's device encrypted with a room key it never shared with the bridge
(common right after a repair: sessions created before it are never re-shared). The warning names the device and
key session in the log. Fix: in Element Desktop type `/discardsession` in the room and send again. On Element X
(phones) start a new chat (a new room has a new key); if it still fails, sign out and in once on that phone.

## 5. Only one device is trusted at a time

The bridge trusts a person's devices all at once only if it holds its own cross-signing keys. `Crypto check: the
bridge does NOT hold all its cross-signing keys` in the log means they are missing from the store: restore them
with `crypto-doctor import-identity` (section 3, step 4). Nobody has to re-verify.

The bridge must also **trust its own identity**: a person counts as verified only if the bridge's identity is trusted
and has signed theirs. Keys copied in by `crypto-doctor` arrive untrusted; the bridge repairs that by itself at startup
and before every verification (log: `Crypto check: our own identity is trusted` or `… trusted it again`). If the log says
`our own identity is NOT trusted`, the cross-signing keys are missing (see above).

## 6. What the bridge now does by itself

- Posts a bilingual warning in the room when it cannot decrypt a message, with what to do.
- Tells a volunteer in their user room when a verification was interrupted (cancelled, or both sides started the emoji
  comparison in the same instant, which makes Matrix cancel both) and that writing again restarts it.
- Logs the sender device and key session of every such message (never the content).
- Checks its identity a minute after each start (`Crypto check: …` lines) and logs `CRYPTO CHECK FAILED` with a
  pointer to this page when the store and Synapse disagree.
- Starts the web chat even if its device shows "not cross-signed"; the old waiting behaviour needs
  `BRIDGE_WAIT_FOR_CROSS_SIGNING=true`.

## 7. Tests that prove respondents will not meet these problems

- **Before every deploy:** `scripts/predeploy-check.sh` (about 25 minutes; `quick` for a shorter run). It runs the unit
  tests, builds the image and runs `tests/e2e`: a throwaway Synapse in Docker, simulated respondents with real
  end-to-end encryption (several devices per person, fresh sign-ins), the real website widget (`chat.js`) and a
  visitor. It covers: a first message and reply, two devices of one person, reload and page change, every command,
  ending a chat (the room disappears for every respondent), typing suppressing reminders, a bridge restart, the
  bridge changing device (nobody does anything), one confirmation verifying all of a person's devices, and a replay
  of this incident with the recovery above. See `tests/e2e/README.md`.
- **After every deploy, and from time to time (cron):** `scripts/prod-smoke.sh https://chat.example.org https://www.example.org vps sb-chat synapse @bridge:matrix.example.org`
  checks the live endpoints, the startup lines in the log and the bridge's keys on Synapse. Nothing is created
  except one web session that is ended at once.
- A respondent-side check nobody can automate: after a repair, have one volunteer on a phone and one on a computer
  answer a test chat.

## 8. Incident log

| Date | Symptom | Cause | Fix |
|---|---|---|---|
| 2026-10-08 | One respondent's Element could not reach the bridge; replies lost | Store and `session` restored from different moments on 2026-10-06 | Right store and cross-signing keys put back, stale keys reset on Synapse |
| 2026-10-08 | Element X (phone) still failed: log `Olm event doesn't contain a ciphertext for our key` | The phone had cached the bridge device's old keys and never refreshes the keys of a known device ID | The bridge logged in as a new device; cross-signing keys copied in with `crypto-doctor import-identity`; the bridge signed its new device. No identity reset |
| 2026-10-12 | A volunteer on Element X (iPhone) pressed "Start verification" and it spun forever, again at every try | Element X starts the emoji comparison itself while the bridge does too; the protocol keeps the volunteer's start (smaller user id than the bridge's), but the bridge had stopped listening after its own start and never answered | The bridge keeps following the request and answers whichever comparison wins; it also waits 3 s (`VERIFY_START_DELAY_SECS`) so the phone can start first. Test `elementx` replays the race |
