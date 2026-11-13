**English** · [Français](fr/security.md)

# Security notes and threat model

What the bridge protects, what it trusts, and how to deploy it safely. Privacy obligations
(retention, erasure, notices) are in [law25.md](law25.md).

## Assets

- **Conversations**: SMS and web chat text. Never written to disk or logs by the bridge; carried in
  end-to-end encrypted Matrix rooms. Queued texts waiting for delivery are encrypted at rest.
- **Callers' phone numbers**: stored encrypted (AES-256-GCM, `PHONE_ENC_KEY`) and as keyed hashes
  (`PHONE_HASH_SECRET`); logs show only an 8-character hash prefix.
- **Secrets**: the bot's Matrix password and session, its crypto store, the voip.ms API password
  (encrypted), call-center API tokens (encrypted, looked up by hash), `PHONE_ENC_KEY`,
  `PHONE_HASH_SECRET`, the MAS admin client secret. All live in `.env` and the data directory.

## Entry points and who may use them

| Entry point | Caller | Authentication | Limits |
|---|---|---|---|
| `GET /voip_api` (port 3443, TLS) | voip.ms SMS webhook | `sbchatapikey` in the URL (hash of the call center's API token) | Key checked before queuing; 20 wrong keys in 10 min from one address → refused for 10 min; 30 s timeout |
| `POST /sbchat_api` | SwitchBoard admin interface | `Authorization: Bearer <call-center token>`; per-command privilege levels | Token checked before the body is read; 64 KiB body; 20 wrong tokens → refused; 300 s timeout |
| `/web/*` | Visitors' browsers (chat widget) | Session token (48 random characters, cookie or header); `Origin` must be in `WEB_ALLOWED_ORIGINS` | 8 KiB bodies, message length, per-IP sessions per hour, proof of work under attack, bans, 4 streams per session |
| Matrix rooms | Respondents | Matrix account, membership of the call center's respondent list | Only the bot may invite (power level 100) |

All listeners: at most 256 requests handled at once, request headers within 15 s.

## Trust boundaries

- **Port 8088 (plain HTTP) is for a reverse proxy only.** Never publish it. `X-Forwarded-For` is
  believed only on this listener and only from addresses in `TRUSTED_PROXIES`; bind it to a private
  address with `HTTP_BIND`. See [reverse-proxy.md](reverse-proxy.md).
- **Port 3443** uses a self-signed certificate by default (voip.ms accepts it). Put a real
  certificate in front if anything else calls it.
- **voip.ms webhook key**: anyone who learns a call center's `/voip_api` URL can inject texts that
  look like they come from any number. Treat that URL as a secret; regenerating the call center's API
  token changes it.
- **Homeserver**: the bridge trusts its own Synapse and MAS. Use your own homeserver, not a public one.
  With MAS admin access, the bridge can create short-lived admin sessions for its own user (to purge
  deleted rooms and sign out its stale devices); keep the MAS admin client secret as safe as the
  database.

## Deployment checklist

- Generate every secret with `openssl rand -hex 32` (the bridge refuses to start with the
  placeholders from `.env-example` or with short secrets). `.env` mode 0600.
- Run the provided Docker image: the bridge runs as the unprivileged `sb-chat` user with `umask 077`;
  data, session and crypto store are readable by that user only.
- Publish only port 3443 (if voip.ms is used) and your reverse proxy; keep 8088, Synapse's and MAS's
  internal ports off the internet.
- Pin image versions (no `:latest`) for Synapse, MAS and databases; update them regularly.
- Back up `.env` and the data directory separately, encrypted, with an expiry.
- Give the bot Synapse admin access (see [law25.md](law25.md), "Purging rooms from Synapse") so
  erased conversations are purged from Synapse too.

## Known limits

- SMS are not end-to-end encrypted between the caller and voip.ms.
- Respondents' devices keep decrypted history until the room is deleted on their side.
- Anyone holding both the database and `.env` can decrypt stored numbers and tokens.
