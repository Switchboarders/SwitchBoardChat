[English](../UPGRADE.md) · **Français**

# Mise à niveau de switchboard-chat vers la version Loi 25 / clavardage Web

Pour une installation existante qui précède `PHONE_ENC_KEY` et le clavardage Web. Valable pour Docker
et pour un binaire ordinaire; remplacez `<dossier d’installation>` par le dossier qui contient votre
`.env` et `<dossier de données>` par `APP_DATA_DIR` (`./data` par défaut, `/app/data` dans l’image
Docker).

Prévoyez environ 45 minutes. Le service SMS est interrompu pendant les 1 à 2 minutes du redémarrage.

## 0. Ce que fait la mise à niveau

1. **Loi 25 pour les SMS** ([law25.md](law25.md)) : les numéros des appelants sont hachés (recherches) et
   chiffrés (réponses), les mots de passe voip.ms sont chiffrés, les files d’attente sont purgées, les
   journaux ne contiennent plus de texte de message ni de numéro, et les conversations expirent après
   `SMS_IDLE_RETENTION_DAYS` (7 par défaut).
2. **Clavardage Web** ([web-chat.md](web-chat.md)), désactivé tant qu’il n’est pas configuré.
3. La base de données est **migrée automatiquement au premier démarrage** (idempotent). L’ancienne
   version ne peut pas lire la base migrée : un retour en arrière passe donc par la restauration de
   votre copie de sauvegarde (section 7).

## 1. Savoir quoi sauvegarder

Tout le contenu de `<dossier de données>` **plus** `<dossier d’installation>/.env` :

| Élément | Pourquoi |
|---|---|
| `db/users.db` | centres d’appels, répondants, salles, désabonnements, files d’attente |
| `db/<dossiers aléatoires>/` | le magasin chiffré du client Matrix (clés). Plusieurs dossiers peuvent exister. |
| `session` (et `sessionbak`) | l’ouverture de session du robot. **Elle désigne l’un des dossiers du magasin**; les deux vont ensemble. |
| `web_server_keys/` | le certificat autosigné (régénéré s’il manque) |
| `.env` | secrets et réglages |

> **Ne restaurez, ne supprimez et ne déplacez jamais seulement une partie de cet ensemble.** Si le
> fichier `session` subsiste alors que son dossier de magasin est remplacé par un dossier vide,
> l’appareil du robot apparaît « non signé par signature croisée », le téléversement des clés Matrix
> est refusé (HTTP 400) et **le serveur Web ne démarre jamais** (symptôme : le conteneur est « en
> marche », mais l’adresse publique répond 502 et le journal s’arrête après `Starting sync loop`).
> Restaurez `db/` et `session` ensemble à partir de la même sauvegarde, ou refaites la première
> ouverture de session (readme, étape 4).

Faites la copie **pendant que la passerelle est arrêtée** (section 5) pour que les fichiers soient
cohérents, et gardez-la ailleurs que sur le même disque. Elle contient des secrets : limitez ses
permissions.

## 2. Préparer la nouvelle version

**Docker.** Construisez l’image sur une machine disposant d’assez de mémoire (une compilation Rust
demande plusieurs Go), puis expédiez-la en gardant l’ancienne image pour un retour en arrière :

```bash
git pull && docker build -t sbchat:latest .
ssh <hôte> 'docker tag sbchat:latest sbchat:previous'
docker save sbchat:latest | gzip | ssh <hôte> 'gunzip | docker load'
```

Ou construisez sur l’hôte avec `docker compose build`. L’image ne contient plus `.env` : le fichier est
fourni à l’exécution (`env_file` dans `docker-compose.yml`).

**Binaire.** `cargo build --release`, et gardez l’ancien binaire à côté du nouveau.

## 3. Nouveaux réglages dans `.env`

Générez la clé de chiffrement et **conservez-en une copie hors du serveur** (gestionnaire de mots de
passe) :

```bash
echo "PHONE_ENC_KEY=$(openssl rand -hex 32)" >> .env
```

- Ne changez jamais `PHONE_ENC_KEY` par la suite : les conversations SMS ouvertes et les mots de passe
  stockés deviendraient illisibles.
- Ne changez jamais `PHONE_HASH_SECRET` : la liste de désabonnement oublierait tout le monde.

Facultatifs maintenant ou plus tard (voir `.env-example`) : `SMS_IDLE_RETENTION_DAYS` et, pour le
clavardage Web, `WEB_ALLOWED_ORIGINS`, `WEB_CALL_CENTER_ID`, `WEB_IDLE_TIMEOUT_MIN`, `WEB_MAX_SESSIONS`.

> **Purge de conservation au premier démarrage.** Environ une minute après le premier démarrage, la
> passerelle supprime toutes les salles SMS inactives depuis plus de `SMS_IDLE_RETENTION_DAYS` jours :
> le robot publie un avis d’« expiration », quitte la salle et oublie la correspondance avec l’appelant.
> Sur une vieille installation, cela peut représenter la plupart des salles. C’est la politique voulue.
> Pour les garder pour l’instant, donnez à `SMS_IDLE_RETENTION_DAYS` une valeur élevée (par exemple
> `3650`) au premier démarrage, puis abaissez-la plus tard.

## 4. Proxy inverse (seulement si vous utiliserez le clavardage Web)

Ajoutez la règle de diffusion de [reverse-proxy.md](reverse-proxy.md) à **chaque** hôte virtuel qui sert
la passerelle. Validez puis rechargez le proxy.

## 5. Basculer (le temps d’arrêt commence)

```bash
# Docker
cd <dossier d’installation>
docker compose stop sb-chat                       # arrêter d’abord la passerelle
cp -a <dossier de données> <lieu de sauvegarde>/data-$(date +%F)   # copie cohérente de db/, session, …
cp .env <lieu de sauvegarde>/env-$(date +%F)
docker compose up -d --no-build --force-recreate sb-chat
docker logs -f sb-chat
```

`--no-build` exécute l’image que vous avez chargée; `--force-recreate` fait lire le nouveau `.env` au
conteneur. Pour un binaire : arrêtez le service, copiez le dossier de données, démarrez le nouveau
binaire.

Dans le journal, en moins d’une minute, vous devriez voir :

- `Database upgraded for Law 25: phone numbers are now hashed or encrypted`
- `Is this device self verified ? is_cross_signed_by_owner() true`
- `HTTPS server listening on …`
- aucun `panicked`, aucun `PHONE_ENC_KEY must be set`

Si l’appareil affiche `… false` ou que la ligne du serveur Web n’apparaît jamais, voyez
l’avertissement de la section 1.

## 6. Vérifier

```bash
# base de données (attendu : ok / 2 / 0)
sqlite3 <dossier de données>/db/users.db "pragma integrity_check; pragma user_version; select count(*) from room where did_enc is null;"
# les mots de passe voip.ms sont chiffrés (attendu : enc:)
sqlite3 <dossier de données>/db/users.db "select substr(voip_ms_api_password,1,4) from call_center;"
```

- Envoyez un texto au numéro du centre d’appels : les répondants le voient; leur réponse parvient au
  téléphone.
- Le premier SMS qu’un **nouveau** numéro reçoit se termine par la ligne de confidentialité.
- Le journal ne contient ni jeton, ni mot de passe, ni numéro de téléphone, ni texte de message :
  `docker logs sb-chat 2>&1 | grep -c -E "api_password|sb_chat_api_token|sbchatapikey"` → `0`.
- Clavardage Web : suivez la section 4 de [web-chat.md](web-chat.md).

Après quelques jours d’utilisation normale : supprimez les copies de sauvegarde dont vous n’avez pas
besoin, ainsi que les fichiers journaux écrits **avant** la mise à niveau (ils contiennent des numéros
et du texte de message).

## 7. Retour en arrière

```bash
docker compose stop sb-chat
# restaurez le dossier de données et .env à partir de la sauvegarde de la section 5 (le tout, ensemble)
docker tag sbchat:previous sbchat:latest
docker compose up -d --no-build --force-recreate sb-chat
```

Les données créées après la sauvegarde sont perdues avec le retour en arrière.

## 8. Après la mise à niveau

- [ ] Renouvelez tout jeton d’API qui a déjà été placé dans un script ou versé dans un dépôt
      (`renew_api_token` dans `add_new_call_center` / `update_call_center`).
- [ ] Informez les répondants de `!oubli` / `!forget` et de `!fin` / `!end` (clavardages Web); `!aide`
      les liste.
- [ ] Passez en revue [law25.md](law25.md) avec la personne responsable de la protection des
      renseignements personnels.
- [ ] Notifications : activez `auto_accept_invites` dans Synapse et vérifiez le téléphone de chaque
      répondant ([respondent-notifications.md](respondent-notifications.md)).
- [ ] Filet de sécurité facultatif : une politique de conservation sur le serveur Matrix
      ([homeserver.md](homeserver.md) en contient un exemple). **Elle supprime
      définitivement les vieux évènements de toutes les salles de ce serveur** : ne l’activez que si
      tous ceux qui utilisent le serveur sont d’accord.

## 9. Dépannage

| Symptôme | Cause / correctif |
|---|---|
| Boucle de redémarrage, le journal indique `PHONE_ENC_KEY must be set` | clé absente; recréez aussi le conteneur après avoir modifié `.env` |
| `PHONE_ENC_KEY must be 64 hex characters` | régénérez avec `openssl rand -hex 32` (avant le premier démarrage avec cette clé) |
| `… cannot be decrypted (wrong PHONE_ENC_KEY?)` | une autre clé que celle utilisée pour la migration : remettez la bonne ou restaurez la sauvegarde d’avant la mise à niveau |
| Liste de désabonnement vide après la mise à niveau | `PHONE_HASH_SECRET` a changé : remettez l’ancienne valeur |
| L’adresse publique répond 502, conteneur « en marche » | le serveur Web n’a pas démarré : appareil du robot non signé par signature croisée (section 1); ou le proxy ne joint pas le port 8088 |
| `docker load` manque d’espace | `df -h`, `docker image prune` |
