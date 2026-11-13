**English** · [Français](fr/law25.md)

# Law 25 (Quebec) — privacy notes for wp-chat and switchboard-chat

> To publish texts on your website (privacy policy section, chat notice, forms), use the adaptable
> website template in the wp-chat repository: `docs/law25-website-template.md`.

Working document for the person responsible for the protection of personal information.
It describes what the software does. It is not legal advice: have a lawyer review the
notices and the privacy impact assessment (PIA) before launch.

## 1. What data exists, where, and for how long

| Data | Where | Form | Kept until |
|---|---|---|---|
| Web chat message text | Browser ↔ switchboard-chat memory ↔ Matrix room | Never written to disk or logs by the bridge. Matrix room is end-to-end encrypted. | Chat ends or idle timeout (`WEB_IDLE_TIMEOUT_MIN`, default 48 h = 2880 min, from the last interaction). The conversation is also kept in memory (last 300 lines) so a reload can restore it. Bridge removes every respondent, leaves and forgets the room. |
| Web visitor identifier | Session token in the visitor's browser (`sessionStorage`, or `localStorage` if the site chose "whole browser"; never in a cookie), SHA-256 of it in bridge memory | Random, no link to a person | Chat end / 48 h without activity. Lost on bridge restart. |
| Block list (web, abusive visitors) | Database `web_block` | Keyed hash of the visitor's IPv4 address or IPv6 /64 network, and an end date. No reason, no content, no readable address. | Until the block expires (default 1 day, at most 30 days by `WEB_BAN_MAX_DAYS`); expired rows are deleted every minute |
| Block marker (web, only after a block) | The blocked visitor's browser (`localStorage`, key `wpchat_blocked`) | A signed "blocked until" marker: two dates and a signature, no identifier | Until the block ends; the visitor can delete it any time |
| Pseudo caller number (web) | Database `room` | Random number, hashed + encrypted | Deleted with the room |
| Web room name | Matrix room name seen by respondents | A first name + letter + 4 characters derived from a keyed hash of the visitor's network address (IPv4, or IPv6 /64). Not reversible without `PHONE_HASH_SECRET`; the address itself is never stored | Deleted with the room |
| SMS message text | voip.ms → bridge → Matrix room | Not stored by the bridge except in a retry queue (below). Matrix room is encrypted. | Room deleted after `SMS_IDLE_RETENTION_DAYS` (default 7) without a text from the caller |
| Caller phone number | Database `room` | Keyed hash (lookup) + AES-256-GCM encrypted copy (needed to answer) | Deleted with the room |
| Opt-out list (STOP) | Database `denied_did` | Keyed hash only, date, how | Kept on purpose, so an opt-out is honored. It cannot identify anyone without `PHONE_HASH_SECRET`. |
| First-contact notice record, auto-SMS counters | Database | Keyed hash only | Notice record: until erased. Counters: 7 days. |
| Retry queues | Database | Inbound texts waiting for a respondent: 24 h max. Outbound: 1 h max. voip.ms password never stored there. | Purged automatically |
| voip.ms password | Database | AES-256-GCM encrypted | Until changed |
| Logs | `data/logs`, 7 daily files | No message text, no phone number, no password or token. Callers appear as an 8-character hash prefix. | 7 days |
| Matrix history | Synapse | Encrypted. Respondents' own devices keep copies. | Deleted and purged from Synapse when the bridge deletes the room, if it has Synapse admin access (see "Purging rooms from Synapse" below); otherwise Synapse `retention` (8 days in the sample config) |

## 2. What the software no longer does (compared with the original SMS path)

- Phone numbers, opt-out list and counters are no longer stored in clear.
- Rooms and number mappings are no longer kept forever.
- The retry queue no longer holds the voip.ms password and is purged on a timer.
- Message bodies, phone numbers, bearer tokens and voip.ms URLs are no longer written to logs.
- `.env` is no longer copied into the Docker image. The example PHP client no longer
  contains an API token.

## 3. Rights of the person

| Request | How |
|---|---|
| Erasure ("forget me") | A respondent types `!oubli` (or `!forget`) in the room, or an administrator calls the API command `erase_caller` with the caller's number. Deletes the room, the encrypted number and the records. The bot leaves the room; **each respondent must also delete the room in their own client**. The opt-out entry (hash) stays. |
| End a web chat | Visitor presses "End chat", or a respondent types `!fin` / `!end`. |
| Access / rectification | The bridge keeps no profile: only the live conversation exists. Answer an access request from the Matrix room while it exists. |
| Opt out of SMS | Caller texts STOP (ARRET). Respondent can type `!stop`. |

## 4. Notices to show

- **Web widget**: the consent text and retention sentence are shown before the first message
  (Settings → WP Chat). Fill in the privacy officer's name and the privacy-policy URL, and
  keep the retention sentence equal to `WEB_IDLE_TIMEOUT_MIN`.
- **SMS**: the first SMS every caller receives states that texts are deleted after
  `SMS_IDLE_RETENTION_DAYS` days and how to opt out. Say in the privacy policy that
  voip.ms and the mobile carriers handle SMS under their own retention, which we do not control.
- **Respondents**: they must not copy, screenshot or export conversations, must delete the
  room when asked, and must keep their Matrix device locked. Put this in the volunteer agreement.

## 5. Residual risks (not fixed by the software)

1. **SMS pass through voip.ms and the carriers** (not end-to-end encrypted, copies kept by them).
2. **Respondents' devices** hold decrypted history until they delete the room or Synapse retention purges it.
3. **API tokens** are stored encrypted (AES-256-GCM with `PHONE_ENC_KEY`) and looked up by hash.
   Anyone holding both the database and `PHONE_ENC_KEY` can still read them: protect the
   database file and the `.env` (permissions, encrypted disk, encrypted backups).
4. **Backups** of the database contain whatever exists at that moment (hashes and
   encrypted numbers, no message text). Encrypt backups and expire them. Keep
   `PHONE_ENC_KEY` and `PHONE_HASH_SECRET` backed up *separately* from the database. Losing
   `PHONE_ENC_KEY` makes open conversations unreadable; changing `PHONE_HASH_SECRET` makes
   the opt-out list forget everyone.
5. **Old log files** written before this change still contain numbers and message text.
   Delete `data/logs/*` once after upgrading.
6. **Web chat sessions live in memory**: a restart ends them (rooms are purged at start).
7. **Rate-limit memory** keeps IP addresses for up to an hour (in memory only).

### Purging rooms from Synapse

Leaving and forgetting a room does not delete it from Synapse: its encrypted events stay in the
Synapse database, and the bridge's crypto store keeps the keys that decrypt them. When the bridge
deletes a room (erasure, retention, end of a web chat) it therefore also asks Synapse to delete the
room and purge its history (`DELETE /_synapse/admin/v2/rooms/{room_id}` with `purge: true`). This needs
Synapse admin access:

- **With MAS**: set `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET` (see `src/mas_admin.rs`) and make
  the bot's user a MAS admin (`mas-cli manage promote-admin <bot>`). For each purge the bridge creates a
  MAS personal session acting as the bot with the Synapse admin scope, valid 2 minutes, and revokes it
  as soon as the purge is done.
- **Without MAS**: make the bot user a Synapse server admin.

The purge runs `ROOM_PURGE_DELAY_SECS` (default 120) after the members are removed, so that
respondents' clients see the removal first; a restart within that delay leaves that room unpurged.

Without either, rooms are only left and forgotten, and the bridge logs a warning once. The keys of
purged rooms stay in the bridge's crypto store, but without the events they decrypt nothing.

## 6. Incident procedure (outline)

1. Contain: stop the bridge (`docker compose stop`), rotate `PHONE_ENC_KEY` is *not* enough
   on its own; rotate the voip.ms password, API tokens, Matrix bot password.
2. Assess: what was exposed (hashes and encrypted numbers are low risk without the keys;
   Matrix room contents are end-to-end encrypted; SMS content passes through voip.ms).
3. Decide whether there is a risk of serious injury: if so, notify the Commission d'accès à
   l'information and the people concerned **promptly**.
4. Record every confidentiality incident in the incident register (required even when
   not notified), then fix the cause and update this document.

## 7. Privacy impact assessment checklist

- [ ] Purpose and necessity written down (help for people in recovery, anonymous, one-to-one).
- [ ] Data inventory above reviewed and confirmed by running the system.
- [ ] Consent and notices reviewed by a lawyer, in French first.
- [ ] Privacy officer named and published.
- [ ] Respondent confidentiality agreement signed (in Element: [confidentiality-agreement.md](confidentiality-agreement.md); set `PRIVACY_OFFICER`, follow the register in SwitchBoard).
- [ ] Hosting location of the bridge, Synapse and WordPress documented (outside Quebec = assessment of transfer).
- [ ] Backups: where, encrypted, how long.
- [ ] Incident register created.
- [ ] Yearly review date set.
