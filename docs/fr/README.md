[English](../README.md) · **Français**

# Documentation de switchboard-chat

| Document | Pour |
|---|---|
| [web-chat.md](web-chat.md) | le canal de clavardage Web : fonctionnement, configuration, commandes des répondants, API HTTP, dépannage |
| [encryption-recovery.md](encryption-recovery.md) | règles et marche à suivre quand les messages des répondants disparaissent (clés de la passerelle désynchronisées de Synapse), avec les outils `scripts/` et `tools/crypto-doctor` |
| [respondent-troubleshooting.md](respondent-troubleshooting.md) | une page en langage simple pour les répondants quand une réponse n’arrive pas (à transmettre à vos bénévoles) |
| [respondent-notifications.md](respondent-notifications.md) | s’assurer que les répondants sont avertis des conversations nouvelles et en attente (avis, rappels, acceptation automatique Synapse, liste de vérification des appareils) |
| [reverse-proxy.md](reverse-proxy.md) | placer la passerelle derrière nginx, Caddy, Apache ou Traefik (TLS, diffusion SSE, adresse IP du client) |
| [UPGRADE.md](UPGRADE.md) | mettre à niveau une installation existante (sauvegarde, migration, vérification, retour en arrière) |
| [confidentiality-agreement.md](confidentiality-agreement.md) | les bénévoles signent l'entente de confidentialité dans Element (`!entente`, `!signer`); preuve conservée; registre dans SwitchBoard |
| [law25.md](law25.md) | protection des renseignements personnels (Loi 25 du Québec) : ce qui est stocké et où, conservation, droits, aperçu des incidents, liste de vérification |
| [../../readme.fr.md](../../readme.fr.md) | installation et passerelle SMS |
| [../../.env-example](../../.env-example) | tous les réglages, expliqués (commentaires bilingues) |

Le côté site Web est le module WordPress **wp-chat** (son propre dépôt et sa propre documentation).
