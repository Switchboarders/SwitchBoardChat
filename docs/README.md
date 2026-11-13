**English** · [Français](fr/README.md)

# switchboard-chat documentation

| Document | For |
|---|---|
| [web-chat.md](web-chat.md) | the web chat channel: how it works, configuration, respondent commands, HTTP API, troubleshooting |
| [encryption-recovery.md](encryption-recovery.md) | rules and runbook when respondents' messages vanish (bridge keys out of step with Synapse), with the `scripts/` and `tools/crypto-doctor` helpers |
| [respondent-troubleshooting.md](respondent-troubleshooting.md) | a plain-language page for respondents when a reply does not arrive (copy it to your volunteers) |
| [respondent-notifications.md](respondent-notifications.md) | making sure respondents are notified of new and waiting conversations (ping, reminders, Synapse auto-accept, device checklist) |
| [reverse-proxy.md](reverse-proxy.md) | putting the bridge behind nginx, Caddy, Apache or Traefik (TLS, SSE streaming, client IP) |
| [UPGRADE.md](UPGRADE.md) | upgrading an existing install (backup, migration, verification, rollback) |
| [confidentiality-agreement.md](confidentiality-agreement.md) | volunteers sign the confidentiality agreement in Element (`!entente`, `!signer`); proof kept; SwitchBoard register |
| [law25.md](law25.md) | privacy (Quebec Law 25): what is stored where, retention, rights, incident outline, checklist |
| [../readme.md](../readme.md) | installation and the SMS bridge |
| [../.env-example](../.env-example) | every setting, explained |

The website side is the **wp-chat** WordPress plugin (its own repository and documentation).
