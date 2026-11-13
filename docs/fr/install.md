[English](../install.md) · **Français**

# Installer avec `scripts/install.sh`

`scripts/install.sh` installe switchboard-chat avec Docker Compose à partir d’une copie de ce dépôt, ou met
à jour la configuration d’une installation existante. On peut le lancer à la main ou depuis un installateur
plus large (Synapse + SwitchBoard + cette passerelle); tout ce qui suit est un contrat stable pour cet usage.

## Avant de le lancer

- Docker avec le greffon Compose v2, `openssl`, `curl`; l’utilisateur qui lance le script peut utiliser Docker.
- Un serveur Synapse + MAS configuré comme dans [homeserver.md](homeserver.md).
- Le compte Matrix du robot, créé dans MAS (`mas-cli manage register-user`), avec un long mot de passe aléatoire.

## Utilisation

```sh
printf '%s\n' "$BOT_PASSWORD" > /run/bot-password    # ou export SBCHAT_BOT_PASSWORD
scripts/install.sh --non-interactive \
  --homeserver-url https://matrix.example.org \
  --bot-user @sms:matrix.example.org \
  --bot-password-file /run/bot-password \
  --web-origins https://www.example.org --web-call-center 2 \
  --trusted-proxies 172.18.0.0/16 \
  --output-json /run/sbchat-install.json
```

| Option | Variable d’environnement | Sens |
|---|---|---|
| `--homeserver-url URL` | `SBCHAT_HOMESERVER_URL` | URL de Synapse (obligatoire à la première installation) |
| `--bot-user @user:serveur` | `SBCHAT_BOT_USER` | identifiant Matrix du robot (obligatoire à la première installation) |
| `--bot-password-file FICHIER` | `SBCHAT_BOT_PASSWORD` (la valeur) | mot de passe du robot, au moins 12 caractères. Il n’y a volontairement pas de `--bot-password` : un mot de passe sur la ligne de commande est visible par tous les utilisateurs de la machine |
| `--virtual-host NOM` | `SBCHAT_VIRTUAL_HOST` | nom d’hôte du certificat autosigné du port 3443 (par défaut : l’hôte du serveur Matrix) |
| `--web-origins LISTE` | `SBCHAT_WEB_ALLOWED_ORIGINS` | sites autorisés à intégrer le clavardage Web, séparés par des virgules |
| `--web-call-center ID` | `SBCHAT_WEB_CALL_CENTER_ID` | centre d’appel qui reçoit les clavardages Web |
| `--trusted-proxies LISTE` | `SBCHAT_TRUSTED_PROXIES` | adresses/CIDR du mandataire inverse ([reverse-proxy.md](reverse-proxy.md)) |
| `--http-bind ADR:PORT` | `SBCHAT_HTTP_BIND` | écouteur HTTP en clair pour le mandataire inverse (`0.0.0.0:8088` dans le conteneur par défaut, non publié) |
| `--mas-admin-client-id ID` | `SBCHAT_MAS_ADMIN_CLIENT_ID` | client d’administration MAS facultatif ([homeserver.md](homeserver.md)) |
| `--mas-admin-client-secret-file FICHIER` | `SBCHAT_MAS_ADMIN_CLIENT_SECRET` (la valeur) | son secret |
| `--docker-network NOM` | `SBCHAT_DOCKER_NETWORK` | réseau Docker externe partagé avec le mandataire inverse (`web` par défaut, créé au besoin) |
| `--no-start` | | écrit la configuration, construit l’image et crée le jeton d’administration, sans démarrer |
| `--check` | | ne change rien; vérifie une installation existante (voir plus bas) |
| `--output-json FICHIER` | | écrit un résumé pour l’installateur appelant (mode 0600) |
| `--non-interactive` | | ne pose jamais de question (automatique quand l’entrée standard n’est pas un terminal) |
| `--ready-timeout SECONDES` | | délai d’attente pour que la passerelle soit prête (300 par défaut) |

Les valeurs qui seraient mal lues dans `.env` (espaces, guillemets, `$`, `#`, barres obliques inverses,
sauts de ligne) sont refusées.

## Ce qu’il fait

1. Crée `.env` à partir de `.env-example` à la première installation (mode 0600). `PHONE_HASH_SECRET` et
   `PHONE_ENC_KEY` sont générés avec `openssl rand -hex 32` **une seule fois** : une nouvelle exécution ne les
   remplace jamais (les changer rendrait les numéros stockés illisibles et ferait oublier tout le monde à la
   liste de désabonnement). Les valeurs données en options sont écrites; les autres sont conservées.
2. Crée le réseau Docker au besoin et construit l’image.
3. Obtient le jeton d’API d’administration de la passerelle (`switchboard-chat admin-token`, affiché sur la
   sortie standard seulement) et l’enregistre dans `admin-api-token` (mode 0600) dans la copie du dépôt.
   SwitchBoard utilise ce jeton pour créer les centres d’appel. Pour le remplacer :
   `docker compose run --rm sb-chat /app/switchboard-chat regenerate-admin-token`.
4. Démarre la passerelle (`docker compose up -d --force-recreate`) et attend qu’elle écrive
   « The client is ready » dans son journal.

Le relancer est sans danger : il garde les secrets, réécrit les valeurs données et redémarre la passerelle.
Le script n’affiche jamais de secret.

## Codes de sortie

| Code | Sens |
|---|---|
| 0 | terminé (ou `--check` : en bon état) |
| 2 | arguments invalides ou manquants |
| 3 | dépendance manquante (docker, compose, openssl, curl, accès au démon Docker) |
| 4 | échec de la construction ou du démarrage, ou passerelle pas prête à temps |
| 5 | `--check` a trouvé un problème |

## `--output-json`

Même format qu’en anglais (voir [../install.md](../install.md)). `status` vaut `ok`, `configured`
(`--no-start`), `start_failed` ou `check_failed`. Le jeton n’est jamais dans le JSON : lisez-le dans
`admin_token_file`.

## `--check`

Vérifie, sans rien changer : `.env` existe en mode 0600 avec les valeurs requises, le fichier du jeton
d’administration existe, le conteneur tourne et s’est déclaré prêt depuis son dernier démarrage, et le port
3443 répond. Écrit une ligne `PASS`/`FAIL` par vérification sur la sortie d’erreur, sort avec 0 ou 5, et
écrit `--output-json` s’il est donné.
