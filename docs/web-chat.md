**English** · [Français](fr/web-chat.md)

# Web chat (for the wp-chat WordPress plugin)

switchboard-chat started as an SMS ↔ Matrix bridge. The **web chat** adds a second channel: a visitor
on a website talks, in real time and in confidence, with the same respondents who answer the SMS,
without knowing it is a team. The website side is the **wp-chat** WordPress plugin (separate
project); any other front end can use the same HTTP API (section 6).

This guide is independent of how and where you host things. It covers what you need, how to turn
the feature on, how it behaves, and how to fix common problems. Network details (TLS, reverse
proxies) are in [reverse-proxy.md](reverse-proxy.md); upgrading an existing install is in
[UPGRADE.md](UPGRADE.md); privacy is in [law25.md](law25.md).

## 1. How it works

```
 Visitor's browser ── HTTPS ──▶ reverse proxy ──▶ switchboard-chat ──▶ Matrix homeserver ◀── respondents
   ▲  (widget JS)                  (port 443)       (HTTP, port 8088)     (encrypted room)    (Element, …)
   │
   └── the website only serves the widget; it never sees a message
```

1. The widget calls `POST /web/session`. The server checks the website's origin, creates an
   in-memory session and returns a random **session token**, which the widget keeps in the page's
   memory (no cookie).
2. On the visitor's first message the server creates an **end-to-end encrypted Matrix room** named
   `Web <friendly name>` and invites every **active respondent** of the configured call center. The name comes
   from a keyed hash of the visitor's network address (IPv4 address, or IPv6 /64 network), like an SMS caller's
   name comes from their number: the same network always gets the same name, so respondents recognise a visitor
   who comes back (an abusive one included). It cannot be turned back into an address without the server's
   `PHONE_HASH_SECRET`. Visitors behind the same shared address (some mobile networks, a library, a workplace)
   get the same name, and a visitor who changes network gets a new one: a name is a hint, not proof.
3. The visitor's messages are posted into that room. Respondents answer in their normal Matrix
   client; the reply is pushed to the visitor's browser over **server-sent events** (SSE).
4. A reload, or a move to another page of the website, keeps the chat: the widget asks for the conversation so
   far (`GET /web/history`) and reconnects to the stream, resuming after the last line it showed.
5. The chat ends when the visitor ends it, when a respondent types `!fin`, or after **48 hours without any
   activity** (`WEB_IDLE_TIMEOUT_MIN`). The server then **removes every respondent from the room**, leaves and
   forgets it (so it vanishes from the respondents' apps) and deletes its database row. A visitor who opened the
   chat but never wrote is dropped after `WEB_ABANDON_MIN` minutes without a connection.

What is **never** stored by the bridge for web chats: message text (kept in memory only, the last 300 lines of a
session so a reload can restore them; never logged, never on disk),
the visitor's IP (kept in memory for rate limits, at most one hour), any profile.

## 2. Requirements

| Need | Details |
|---|---|
| switchboard-chat | version with the web chat (this repository, `main`) |
| Matrix homeserver | the one already used by the bridge, with respondents' accounts |
| Respondents | at least one **active** respondent on the call center used for web chat |
| HTTPS | the browser must reach the server over **HTTPS** (a website served over HTTPS cannot call plain HTTP). Terminate TLS in a reverse proxy; see [reverse-proxy.md](reverse-proxy.md) |
| A public name | e.g. `chat.example.org`, pointing at the proxy |
| The website's origin | e.g. `https://www.example.org` (scheme + host, no path) |

The bridge listens on **3443** (HTTPS, self-signed certificate, for tests) and **8088** (plain HTTP,
**meant for a reverse proxy; never expose it to the Internet**).

## 3. Configuration

Set these in the bridge's environment (`.env`, container environment, or systemd unit). The web
chat stays **off** until the first two are set.

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `WEB_ALLOWED_ORIGINS` | yes | – | Websites allowed to use the chat, comma separated, e.g. `https://www.example.org,https://example.org`. Exact match: scheme, host and port, no path, no trailing slash. Any other origin gets `403 origin_not_allowed`. |
| `WEB_CALL_CENTER_ID` | yes | – | Numeric id of the call center whose active respondents are invited (`id_call_center` in the `call_center` table). Its macros (`#name`) are usable in web rooms. |
| `WEB_IDLE_TIMEOUT_MIN` | no | `2880` | Minutes (48 h) after the last interaction before a chat is ended and deleted. |
| `WEB_MAX_SESSIONS` | no | `200` | Open chats at the same time; more visitors get `503 busy`. |
| `WEB_ABANDON_MIN` | no | `5` | Minutes after which a session whose visitor **never wrote** and whose live connection has closed is dropped. Chats with messages are kept until the idle timeout, so the visitor can come back. |
| `WEB_SESSION_COOKIE` | no | `false` | Also set the legacy `wpchat` cookie. Only needed by widgets older than 0.5.0 (section 9). |
| `WEB_SESSIONS_PER_IP_PER_HOUR` | no | `5` | New chats allowed per hour from one network address (IPv4 address or IPv6 /64). Lower it against refresh abuse. |
| `WEB_BAN_DAYS` | no | `1` | Duration, in days, of a `!ban` when the respondent gives none |
| `WEB_BAN_MAX_DAYS` | no | `30` | Longest ban, in days, a respondent can set |
| `WEB_CHALLENGE` | no | `auto` | Proof-of-work challenge before a chat starts: `auto` (only after a recent ban or when one address opens several chats), `always`, `off` |
| `WEB_CHALLENGE_BASE_BITS`, `WEB_CHALLENGE_MAX_BITS` | no | `12`, `18` | Difficulty in bits (each bit doubles the work): 12 is instant, 18 takes a couple of seconds on a computer and several on a phone |
| `WEB_CHALLENGE_WINDOW_HOURS` | no | `6` | How long a ban keeps raising the difficulty for everyone |
| `WEB_ATTACK_STARTS_PER_MIN` | no | `30` | Chat-start requests per minute (all addresses together) above which the under-attack mode starts |
| `WEB_ATTACK_HOLD_MIN` | no | `15` | Minutes the under-attack mode lasts after the last excess |
| `WEB_MAX_NEW_ROOMS_PER_MIN` | no | `10` | New Matrix rooms (and new-chat pings) allowed per minute, in total |
| `WEB_NO_MESSAGE_TIMEOUT_MIN` | no | `10` | Minutes after which a chat in which nothing was ever written is closed (2 under attack) |
| `PHONE_HASH_SECRET`, `PHONE_ENC_KEY` | yes (shared with SMS) | – | Used by the database; see `.env-example`. |

The idle timeout appears in the privacy text shown to visitors (a sentence in the plugin
settings): keep both in agreement.

Find the call center id and its respondents:

```sql
sqlite3 <data dir>/db/users.db "select id_call_center, call_center_did from call_center;"
sqlite3 <data dir>/db/users.db "select id_call_center, respondant_username, is_active from respondant;"
```

(Respondents are managed with the `change_respondents` API command. Respondents not refreshed
for a month are deactivated automatically, as for SMS.)

## 4. Turning it on (first time)

1. Make the bridge reachable over HTTPS on a public name ([reverse-proxy.md](reverse-proxy.md)).
2. Set `WEB_ALLOWED_ORIGINS` and `WEB_CALL_CENTER_ID`; restart the bridge so it reads them.
3. Check from any machine:
   ```bash
   # allowed origin → 200, a JSON body with a token, CORS headers
   curl -si -X POST https://chat.example.org/web/session -H 'Origin: https://www.example.org' | head -15
   # no/other origin → 403 {"error":"origin_not_allowed"}
   curl -si -X POST https://chat.example.org/web/session | head -3
   ```
4. Check that **replies stream live** (the step most often broken by proxies):
   ```bash
   TOKEN=$(curl -s -X POST -H 'Origin: https://www.example.org' https://chat.example.org/web/session | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
   curl -sN -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/stream
   ```
   A line `:` (a keep-alive comment) must appear **within about 15 seconds**. If nothing appears,
   the proxy is buffering: see [reverse-proxy.md](reverse-proxy.md). Press Ctrl+C, then end the
   test session: `curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/end`.
5. Install and configure the plugin on the website (see the wp-chat documentation) and run a
   real chat end to end.

### Linking a website from SwitchBoard (no server settings)

A committee can link its own website without anyone editing `.env` or restarting the bridge:

1. In SwitchBoard, the committee's *SwitchBoard Chat* page, *Link a website*: SwitchBoard asks the bridge for a
   one-time code (`create_web_pairing`, valid 30 minutes) and shows one string,
   `https://chat.example.org#K7QM-3XRA-PW2D`. The address is `WEB_PUBLIC_URL` when it is set (the https address
   visitors' browsers use, behind the reverse proxy), otherwise SwitchBoard's own address of the bridge.
2. In WordPress, *Settings > WP Chat > Link this website to a committee*: paste the string, *Link*. The plugin's
   server sends `POST /web/pair {"code": "...", "origin": "https://www.example.org"}`; the bridge records the
   website for that committee (codes are hashed, used once, 10 attempts per address and hour).
3. From then on that website is allowed (CORS) like the ones in `WEB_ALLOWED_ORIGINS`, and **every new chat it
   opens goes to its own committee**: many committees can share one bridge, each with its own website(s).
   `WEB_CALL_CENTER_ID` is only for the websites listed in `.env`.

SwitchBoard lists the linked websites and removes them (`list_web_sites`, `remove_web_site`); a removed website
is refused at once. Deleting a call center unlinks its websites.

A committee that only uses the web chat needs no voip.ms account: SwitchBoard requests its call center without
voip.ms (`add_new_call_center` with `committee_ref` instead of the voip.ms fields), and the bridge never calls
voip.ms for it.

## 5. Respondent experience and commands

A new room appears in the respondents' Matrix client, named `Web …`. The first message starts
with a short hint in French and English. Respondents are alerted by a **mention** ping a few seconds
after the room is created, and reminded after 1, 2 and 5 minutes then hourly while nobody has typed or
written (see [respondent-notifications.md](respondent-notifications.md), which also explains the
Synapse `auto_accept_invites` setting and how to check each phone's push registration). In a web room:

| What the respondent types | Effect |
|---|---|
| plain text | sent to the visitor's browser |
| `#macroname` | the call center's macro text is sent to the visitor; a confirmation appears in the room |
| `#macroname` with the tag `newcomerweb` | sent **automatically** to the visitor after their first message (the room gets a note saying so). Set the tag in the SwitchBoard macro form |
| `!ok` | private room: removes the other respondents (as for SMS); the bridge answers in the room |
| `!fin` / `!end` | ends the chat and deletes it (every respondent removed, room left and forgotten, visitor told) |
| `!oubli` / `!forget` | same as `!fin` for a web chat (erasure) |
| `!ban [days]` / `!bloquer [days]` | **abusive visitor**: ends the chat and refuses the visitor's connection for `days` (default `WEB_BAN_DAYS`, at most `WEB_BAN_MAX_DAYS`) |
| `!aide` / `!help` | lists commands and macros (the answer is posted in the room: `!aide` in French, `!help` in English) |
| (nobody on duty) | when no respondent is active, a new chat is refused with `503 {"error":"no_respondent","message":"…"}`; `message` is the call center's `no_respondent` macro (set in SwitchBoard, also sent to SMS callers) and the widget shows it |
| `!stop` | answered with a note that it does not apply (the visitor has no phone number) |
| any other `!word` | answered with "unknown command" and a pointer to `!help` |

When a chat ends the bridge **removes every respondent from the room**, then leaves and forgets it, so the
room disappears from their room list (Element shows a short "removed from the room" state at most). The
Matrix server's admin "delete room" API is not used because the bridge account is not a server admin. Copies
already synchronized to a respondent's device can survive until they clear them (and until the homeserver's
retention policy, if any, purges them).

### Abusive visitors

A visitor who is abusive can start a new chat at will, because a chat is only a disposable session.
The bridge protects the respondents in layers:

1. **`!ban [days]` (or `!bloquer`) in the room.** Any respondent can type it. The chat ends, **every other
   open chat from the same connection ends too**, and new chats from that connection are refused with
   `403 blocked` for the chosen time (default 1 day, at most 30 days). The visitor sees a polite
   message (and the contact text set in the plugin, such as another number to call).
2. **What a block applies to:** the visitor's IPv4 address, or the /64 network of an IPv6 address
   (a visitor controls the last 64 bits, so blocking one address would be pointless).
3. **What is stored:** only a **keyed hash** of that address (secret `PHONE_HASH_SECRET`) and the end
   date, in the database, until the block expires; expired blocks are deleted every minute. No reason, no
   conversation, no readable address.
4. **Lifting a block** (a shared address blocked by mistake): wait for it to expire, or an administrator
   with privilege level 100 or more calls the API command `web_unban` with `{"command":"web_unban","ip":"203.0.113.9"}`
   (or `"ip":"all"`).
5. **Rate limits that work without anyone acting:** `WEB_SESSIONS_PER_IP_PER_HOUR` new chats per hour per
   address (default 5, then `429 rate_limited`), 10 messages per 30 seconds per chat, and `WEB_MAX_SESSIONS`.
6. **A block that follows the browser, not the network.** When a respondent bans a visitor, the banned
   browser receives a signed **"blocked until" marker** (two dates and a signature: no identifier) in a
   `banned` event on its live stream, and the widget keeps it in the browser's `localStorage` (key
   `wpchat_blocked`). It sends the marker in an `X-WPChat-Block` header when it asks for a new chat, and the
   server answers `403 blocked`, whatever VPN or network the browser now uses. Nothing is stored in the browser
   of a visitor who was never blocked. The marker ends with the ban; `web_unban` with `"ip":"all"` revokes
   every marker issued before it.
7. **A proof of work when there is trouble.** Before a chat starts the server can answer
   `428 challenge_required` with a signed, single-use challenge (valid 3 minutes, tied to the requesting
   address). The widget finds a nonce such that `SHA-256("<challenge>:<nonce>")` begins with the required number
   of zero bits and retries. One visitor loses a moment; a script, or someone refreshing again and again, pays
   for every attempt. In `auto` mode (default) it applies only when a ban happened in the last
   `WEB_CHALLENGE_WINDOW_HOURS` hours or when the same address already opened a chat in the last hour, and the
   difficulty rises by 2 bits for each recent ban and each recent chat, up to `WEB_CHALLENGE_MAX_BITS`. It uses
   no third-party service.

Limits of the method: a determined person gets around each layer eventually: a new VPN gets a new address, and
clearing the browser's site data (or using a private window or another browser) discards the marker. Together the layers make
casual refreshing, scripts and repeated abuse from one connection stop, which is the usual case, and they make
the rest cost real time, but none of them is an identity check. A shared address (school, office, mobile carrier) can block innocent
people for the duration, hence the short default and `web_unban`.

### Under attack (denial of service)

An organised attack (many addresses, one request each) is not stopped by per-address limits, so the bridge also
watches the service as a whole:

1. **Under-attack mode.** Chat-start requests are counted in total. Above `WEB_ATTACK_STARTS_PER_MIN` in a minute
   the bridge switches to under-attack mode for `WEB_ATTACK_HOLD_MIN` minutes after the last excess: the proof-of-work
   challenge is required from **everyone**, 4 bits harder, and a chat in which nothing was written is closed after 2
   minutes instead of `WEB_NO_MESSAGE_TIMEOUT_MIN`, so bots cannot hold the `WEB_MAX_SESSIONS` slots. It ends by itself.
   The log line `Web chat: under attack mode ON` marks it. `WEB_CHALLENGE=off` disables the challenge (and this response).
2. **Room cap.** At most `WEB_MAX_NEW_ROOMS_PER_MIN` new Matrix rooms per minute, whatever the number of visitors; the
   others get `503 busy` and can try again. Respondents are therefore never flooded with new-chat pings, and the
   Matrix server is not flooded with room creations.
3. **Cheap by design.** Web routes refuse request bodies above 8 KB, a counter ring uses constant memory however large
   the flood, and the block list, the call center's respondents and the recent-ban count are read from the database
   at most every 10 seconds, so a flood does not become a database flood.

What the bridge cannot do: it cannot absorb traffic that saturates the server's network link. That must be handled
upstream: the hosting provider's network protection (check that it is active), a firewall, and rate limits in the
reverse proxy. **Do not put a CDN that terminates HTTPS in front of `/web/*`**: it would read the visitors' messages
in clear, which contradicts the confidentiality promise (a CDN is fine for the website's static pages). Decide in
advance how visitors reach you when the web chat is down: the plugin's *Extra text when the chat is unavailable*
shows them another way to get help.

## 6. HTTP API reference

All routes live under `/web/`. Cross-origin calls need the website's origin in
`WEB_ALLOWED_ORIGINS`; the server answers CORS preflights and sends
`Access-Control-Allow-Origin: <origin>` with `Access-Control-Allow-Credentials: true`
(methods `GET, POST`; header `Content-Type`). Requests must use `credentials: include`.
Every request needs a matching `Origin` header.

**The session token:** random, 48 characters, returned in the JSON body of `POST /web/session`. The
client keeps it in memory and sends it on every later call as `Authorization: Bearer <token>`, so no
cookie is involved and browsers' third-party cookie settings do not matter. Only its SHA-256 is kept,
in memory. `navigator.sendBeacon` cannot set headers, so `POST /web/end` also accepts a JSON body
`{"token":"…"}`. *Legacy:* with `WEB_SESSION_COOKIE=true` the server also sets a `wpchat` cookie
(`Path=/web; HttpOnly; Secure; SameSite=None`) and accepts it when no header is sent.

| Route | Body | Success | Errors |
|---|---|---|---|
| `POST /web/session` | – | `200 {"resumed":false,"token":"…"}`. `{"resumed":true,…}` if the request already carries a live token. | `403 origin_not_allowed`, `403 blocked` (address blocked, or a valid `X-WPChat-Block` marker was sent), `428 challenge_required` (body `{"error":"challenge_required","challenge":"…","bits":N}`: solve it and repeat the call with the body `{"challenge":"…","nonce":"…"}`), `503 not_configured` (missing/invalid `WEB_CALL_CENTER_ID`), `503 no_respondent`, `503 busy`, `429 rate_limited` |
| `POST /web/message` | `{"text":"…"}` (1–1000 characters, trimmed) | `200 {}` | `401 no_session`, `400 empty`, `413 too_long`, `429 rate_limited`, `403 origin_not_allowed`, `503 no_respondent`, `503 busy` (room cap reached), `502 room_creation_failed` / `delivery_failed` |
| `GET /web/history` | – (token in `Authorization`) | `200 {"messages":[{"seq":1,"from":"me"|"them","text":"…"}],"last":N}` the in-memory conversation, to restore a window after a reload | `401 no_session`, `403 origin_not_allowed` |
| `GET /web/stream` | – | `text/event-stream` (read it with `fetch`, since `EventSource` cannot set the header). Events: `message` with data `{"text":"…"}` and an `id:` line (the line number); send `Last-Event-ID` to resume after a line the browser already shows (without it, only lines never delivered are sent); `banned` with data `{"until":…,"marker":"…"}` (the visitor was blocked; keep the marker), followed by `closed`; `closed` with data `{}` (chat ended). A comment line `:` every 15 s keeps the connection alive. The response carries `X-Accel-Buffering: no`. | `401 no_session`, `403 origin_not_allowed` |
| `POST /web/end` | – (may be empty) | `200 {}`; session and room are removed | `403 origin_not_allowed` |

Errors are JSON: `{"error":"<code>"}`. A message for a visitor whose stream is momentarily
disconnected is kept in memory for up to 60 seconds (at most 20) and delivered when the stream
reconnects (`EventSource` reconnects by itself).

**Limits:** 10 messages per 30 seconds per session; `WEB_SESSIONS_PER_IP_PER_HOUR` (default 5) new sessions
per hour per client address (the last `X-Forwarded-For` entry, otherwise the connection's address, as an
IPv4 address or an IPv6 /64 network); `WEB_MAX_SESSIONS` open chats. A blocked connection gets `403 blocked`.

## 7. Operations

- **Memory only.** Web sessions live in memory: restarting the bridge ends every web chat. Leftover
  web rooms are purged about 45 seconds after start.
- **Idle cleanup** runs every minute. Rooms of SMS conversations follow `SMS_IDLE_RETENTION_DAYS`
  (see [law25.md](law25.md)).
- **Logs** never contain message text, phone numbers, tokens or passwords.
- **Capacity.** Each open chat costs one connection (SSE) and little memory. Check the proxy's and
  the operating system's connection limits when expecting hundreds of simultaneous visitors.
- **Matrix rate limits.** Creating rooms is rate limited by the homeserver. Configure the optional
  MAS admin client (`MAS_ADMIN_*` in `.env-example`) or raise the limits for the bot account.

## 8. Troubleshooting

| Symptom | Likely cause and fix |
|---|---|
| `502`/`504` from the public URL | the proxy cannot reach port 8088 (bridge down or still starting), or the bridge is stuck at startup (see the upgrade guide, "device not cross-signed") |
| `403 origin_not_allowed` in the browser | `WEB_ALLOWED_ORIGINS` does not exactly equal the site's origin (`https://www.example.org` ≠ `https://example.org`; add both). Restart after editing. |
| Visitor can send, replies never appear | the proxy buffers the stream. Run the live stream test in section 4; fix with [reverse-proxy.md](reverse-proxy.md). |
| `503 no_respondent` | no active respondent for `WEB_CALL_CENTER_ID` |
| `503 not_configured` | `WEB_CALL_CENTER_ID` is missing or does not exist |
| `429 rate_limited` for everyone | the bridge sees the proxy's address for every visitor: the proxy must forward `X-Forwarded-For` (see reverse-proxy.md) |
| `401 no_session` on messages or on the stream | the token is missing or expired: the chat ended, the idle timeout or `WEB_ABANDON_MIN` passed, the bridge restarted, or a widget older than 0.5.0 talks to a server with `WEB_SESSION_COOKIE` off (section 9) |
| Replies lost after a restart | expected: web sessions are not persisted |
| Room created but nobody invited | the call center has no active respondent, or their Matrix accounts could not be found |
| The widget keeps saying "Checking your connection…" or fails after a few tries | `428 challenge_required` is repeated: the visitor's address changes between the two calls (mobile network), or the phone is very slow at the current difficulty. Lower `WEB_CHALLENGE_MAX_BITS`, or set `WEB_CHALLENGE=off` |
| A visitor says the chat is "not available from this connection" | `403 blocked`: a respondent used `!ban` (or a shared address was blocked). It expires by itself; lift it with the `web_unban` API command |

## 9. Upgrading from cookie sessions (widgets older than 0.5.0)

Older widgets (plugin 0.4.0 and earlier) rely on the `wpchat` cookie and on `EventSource`. The
current widget uses the token and falls back to cookies when a server returns no token, so the order
does not matter much:

1. Update the **plugin** first, then the bridge. The new widget works with old and new bridges.
2. If some websites still run an older plugin, set `WEB_SESSION_COOKIE=true` on the bridge until they are
   updated, then remove it (no cookie is then set at all).
3. Widgets older than 0.8.0 end the chat when their page is closed; they do not restore it after a reload.
