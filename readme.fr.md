[English](readme.md) · **Français**

# SwitchBoardChat — Guide d’installation

Ce fichier explique comment déployer **SwitchBoardChat** sur un VPS (recommandé) ou préparer des
constructions locales pour un déploiement à distance.

> **Installation rapide :** `scripts/install.sh` fait les étapes 3 à 5 ci-dessous d’un coup (après le clonage), aussi sans interaction : voir [docs/fr/install.md](docs/fr/install.md).

> **Important :** il vous faut **votre propre serveur Matrix Synapse** avec **Matrix Authentication
> Service (MAS)**. Les clients Matrix modernes exigent MAS pour le chiffrement de bout en bout.
> N’utilisez PAS `matrix.org` : ses limites de débit provoqueraient des pannes.

---

## 🚀 Installation sur un VPS (recommandée)

### 1. **Créer un compte Matrix à mot de passe (via MAS)**
Vous devez créer un utilisateur local sur votre serveur Matrix.

Dans votre conteneur Docker MAS, créez l’utilisateur :
```bash
docker compose exec mas mas-cli manage register-user
```
- `mas` = nom du conteneur
- `mas-cli manage register-user` = commande exécutée dans le conteneur

Marquez l’utilisateur comme **administrateur** et choisissez un mot de passe robuste.
Ce compte jouera le rôle de **sbchat** (utilisateur robot interne).

Assurez-vous que votre utilisateur n’a qu’une seule session active. Supprimez les sessions indésirables
dans MAS. Un bogue courant est d’avoir des sessions non vérifiées, qui empêchent la livraison des
messages.
Notez :
- Le nom d’utilisateur complet (p. ex. `@sbchat:your-homeserver.com`)
- Le mot de passe

Pour les réglages de Synapse + MAS dont la passerelle a besoin, voir [docs/fr/homeserver.md](docs/fr/homeserver.md).

---

## 2. **Cloner le dépôt**
```bash
git clone https://github.com/Switchboarders/SwitchBoardChat.git
```
*Le dépôt est privé — vous pourriez avoir besoin de droits d’accès.*

---

## 3. **Configurer le fichier `.env`**
À la racine du projet, renommez le fichier d’exemple :
```bash
mv .env-example .env
```
Modifiez le fichier `.env` :

### Valeurs obligatoires
| Variable | Description |
|---------|-------------|
| `PHONE_HASH_SECRET` | Une chaîne aléatoire quelconque. Sert à hacher les numéros de téléphone en noms. |
| `HOMESERVER_URL` | L’adresse de votre serveur Matrix. |
| `MATRIX_USERNAME` | Le compte MAS que vous venez de créer. |
| `MATRIX_PASSWORD` | Le mot de passe de ce compte MAS. |
| `VIRTUAL_HOST` | Votre nom de domaine ou de sous-domaine. Utilisez localhost si tout est sur le même serveur. Notez que certains hébergeurs bloquent le port 8448. Vous pouvez utiliser un proxy inverse : sbchat écoute à l’interne sur le port 8088. |
### Valeurs facultatives
Le fichier `.env-example` documente chaque variable. En plus des variables obligatoires :

| Variable | Description |
|---------|-------------|
| `MUTE_VOIPMS_REQUEST` | `true` met les SMS en file d’attente sans les envoyer (essais). |
| `APP_DATA_DIR` | Dossier de la base de données, des journaux, de la session Matrix et des certificats. Défaut : `./data`. |
| `MAS_ADMIN_*` | Client d’administration MAS facultatif; permet au robot de retirer ses propres limites de débit. |
| `PHONE_ENC_KEY` | **Obligatoire.** 64 caractères hexadécimaux. Chiffre les numéros des appelants (conversations ouvertes) et les mots de passe voip.ms. |
| `SMS_IDLE_RETENTION_DAYS` | Jours de silence de l’appelant avant la suppression d’une conversation SMS. Défaut : 7. |
| `ATTENTION_ENABLED`, `ATTENTION_JOIN_WAIT_SECS`, `ATTENTION_HOURLY_MAX`, `ATTENTION_FOLLOWUP_MIN` | Avis et rappels pour les conversations nouvelles ou sans réponse (activés par défaut); les messages de suite d’une conversation prise en charge ne sont rappelés qu’après 10 minutes. Voir [docs/fr/respondent-notifications.md](docs/fr/respondent-notifications.md). |
| `WEB_ALLOWED_ORIGINS` | Sites Web autorisés à utiliser le module de clavardage Web (séparés par des virgules, p. ex. `https://myorg.ca`). Le clavardage Web est désactivé tant que cette variable est vide. |
| `WEB_CALL_CENTER_ID` | Centre d’appels dont les répondants répondent aux clavardages Web. Le clavardage Web est désactivé tant que cette variable est vide. |
| `WEB_IDLE_TIMEOUT_MIN` | Minutes après la dernière interaction avant la suppression d’un clavardage Web. Défaut : 2880 (48 h). |
| `WEB_MAX_SESSIONS` | Nombre de clavardages Web simultanés permis. Défaut : 200. |
| `WEB_ABANDON_MIN` | Minutes après lesquelles une session dont le visiteur n’a jamais écrit et dont la connexion est fermée est retirée. Défaut : 5. |
| `WEB_CHALLENGE`, `WEB_CHALLENGE_BASE_BITS`, `WEB_CHALLENGE_MAX_BITS`, `WEB_CHALLENGE_WINDOW_HOURS` | Épreuve de calcul avant le début d’un clavardage (`auto` par défaut : seulement après des abus). Voir [docs/fr/web-chat.md](docs/fr/web-chat.md). |
| `WEB_ATTACK_STARTS_PER_MIN`, `WEB_ATTACK_HOLD_MIN`, `WEB_MAX_NEW_ROOMS_PER_MIN`, `WEB_NO_MESSAGE_TIMEOUT_MIN` | Protection contre le déni de service : mode sous attaque, plafond de salles, clavardages silencieux. Voir [docs/fr/web-chat.md](docs/fr/web-chat.md). |
| `WEB_SESSION_COOKIE` | `true` dépose aussi l’ancien témoin (seulement pour les modules wp-chat antérieurs à 0.5.0). Défaut : `false`. |

### Confidentialité (Loi 25 du Québec)
Les numéros de téléphone des appelants ne sont jamais stockés en clair : la base de données conserve une empreinte à clé (`PHONE_HASH_SECRET`) et, seulement pendant qu’une conversation est ouverte, une copie chiffrée AES-256-GCM (`PHONE_ENC_KEY`, 64 caractères hexadécimaux, `openssl rand -hex 32`). **Sauvegardez ces deux secrets séparément de la base de données** : sans `PHONE_ENC_KEY`, les conversations ouvertes sont illisibles, et changer `PHONE_HASH_SECRET` fait oublier tout le monde à la liste de désabonnement. Les mots de passe voip.ms sont eux aussi stockés chiffrés. Les salles SMS sont supprimées après `SMS_IDLE_RETENTION_DAYS` jours (7 par défaut) sans texto de l’appelant; les files de reprise sont purgées après 24 h (entrantes) et 1 h (sortantes); les journaux ne contiennent aucun texte de message, numéro ni mot de passe. Un répondant peut écrire `!oubli` (ou `!forget`) dans une salle, ou un administrateur peut appeler la commande d’API `erase_caller`, pour effacer un appelant. Le premier démarrage après la mise à niveau migre automatiquement la base de données : **arrêtez la passerelle et copiez d’abord tout le dossier de données** (`db/` et `session` doivent toujours être restaurés ensemble) et supprimez les anciens `data/logs/*`. Détails : [docs/fr/UPGRADE.md](docs/fr/UPGRADE.md) et [docs/fr/law25.md](docs/fr/law25.md). Réglages de Synapse (conservation, invitations) : [docs/fr/homeserver.md](docs/fr/homeserver.md).

### Documentation
Voir le dossier [docs/fr/](docs/fr/README.md) : [clavardage Web](docs/fr/web-chat.md), [proxy inverse (nginx, Caddy, Apache, Traefik)](docs/fr/reverse-proxy.md), [mise à niveau](docs/fr/UPGRADE.md), [notes sur la Loi 25](docs/fr/law25.md).

### Fonctionnement du clavardage Web
Un visiteur ouvre la bulle de clavardage du module WordPress wp-chat. Le navigateur du visiteur parle directement à ce serveur (routes `/web/*`); le serveur WordPress ne voit jamais les messages. Le serveur remet au navigateur un jeton de session aléatoire (gardé en mémoire dans la page, sans témoin) et crée une salle Matrix chiffrée avec les répondants du centre d’appels lors de l’envoi du premier message. Les répondants répondent dans leur client Matrix habituel; leur texte retourne au navigateur, pas par SMS. Les messages ne sont jamais écrits dans la base de données ni dans les journaux. Le clavardage prend fin, la salle est quittée et sa ligne de base de données supprimée, quand le visiteur y met fin, quand un répondant écrit `!fin` (ou `!end`), ou après le délai d’inactivité. Les clavardages Web vivent en mémoire seulement : un redémarrage du serveur y met fin. En production, placez devant un proxy inverse avec un vrai certificat (port 8088), parce que le site Web lui-même est servi en HTTPS.

Si vous voulez changer la liste des noms générés :
```
src/nameslist/names.csv
```
Utilisez beaucoup d’entrées pour éviter la confusion.

---

## 4. **Démarrer le service (première initialisation)**
```bash
docker compose up --build
```
Un lien apparaîtra — cliquez dessus.
Il vous redirigera vers l’authentification MAS.

⚠️ **Assurez-vous d’être connecté à MAS avec l’utilisateur que vous avez créé plus tôt!**
Si vous vous authentifiez avec le mauvais utilisateur, l’initialisation échouera.

Quand l’initialisation est terminée, le programme se ferme proprement.

Démarrez ensuite en arrière-plan :
```bash
docker compose up -d
```
Pour suivre les journaux en direct, omettez `-d`.

---
## 5. **Vous aurez besoin de votre clé d’API d’administration**
Affichez-la (elle est stockée chiffrée, on ne peut donc pas la lire dans la base de données) :
```bash
docker compose run --rm sb-chat /app/switchboard-chat admin-token
```
(`regenerate-admin-token` la remplace.) `scripts/install.sh` l’enregistre pour vous dans `admin-api-token` (mode 0600).
Utilisez-la dans SwitchBoard pour créer les clés propres à chaque centre d’appel.
⚠️ N’utilisez pas ce jeton pour un centre d’appel : aucun numéro (DID) ne lui est associé. Gardez-le secret.

# 🖥️ Construction locale et déploiement à distance (facultatif)
Ces commandes servent à construire localement et à envoyer l’image vers un VPS distant.

## 1. Cloner le dépôt localement
```bash
git clone https://github.com/Switchboarders/SwitchBoardChat.git
```

## 2. Construire l’image Docker
```bash
docker compose build
```

## 3. Exporter l’image
```bash
docker save -o sb-chat.tar sbchat:latest
```

## 4. Transférer l’image vers le VPS
```bash
rsync -avz -e ssh sb-chat.tar vps:~/
```
Remplacez `vps` par `utilisateur@votreserveur.com` au besoin.

## 5. Importer l’image sur le VPS
```bash
docker load -i sb-chat.tar
```

Copiez votre `docker-compose.yml` et votre `.env` dans le dossier du projet sur le VPS.

---


# 🗃️ Anciennes instructions (héritage)
Elles sont conservées pour référence, mais ne sont plus recommandées.

### Construire l’image à l’ancienne
```bash
docker build . -t switchboard-chat
```

### Exécuter
```bash
docker run --rm -t -i \
    --restart=unless-stopped \
    -v sb-chat:/app/data \
    -p 3443:3443 \
    switchboard-chat
```

### Exécuter avec redémarrage automatique
```bash
docker run \
    --restart=unless-stopped \
    -v sb-chat:/app/data \
    -p 3443:3443 \
    switchboard-chat
```

### Partager un dossier de données local
```bash
docker run --rm \
    -v $(pwd)/data:/app/data:ro \
    switchboard-chat
```

### Enregistrer / charger des images
```bash
docker save -o sb-chat.tar switchboard-chat
docker load -i sb-chat.tar
```

### Transférer par SSH
```bash
rsync -avz -e ssh sb-chat.tar vps:~/
```

---

## ⚠️ Avertissement au sujet des fichiers de session / du SDK Matrix
Copier les fichiers de session ou du SDK Matrix d’une machine à l’autre peut briser le chiffrement de
bout en bout. Vous pourriez voir des avertissements comme :
```
WARN matrix_sdk::encryption: ... One time key already exists ...
```
Si cela se produit :
👉 **Supprimez votre fichier de session.**
L’envoi des messages reprendra normalement.

> **Attention :** le fichier `session` désigne l’un des dossiers du magasin sous `db/`; ils vont
> ensemble. Ne supprimez jamais seulement l’un des deux sans suivre la procédure de première
> initialisation (étape 4) : voir [docs/fr/UPGRADE.md](docs/fr/UPGRADE.md), section 1.

---

📌 **Vous êtes maintenant prêt à exécuter SwitchBoardChat sur votre serveur!**

---

## Sécurité et licence

- Signalez les vulnérabilités en privé : voir [SECURITY.md](SECURITY.md). Modèle de menace et notes de
  durcissement : [docs/fr/security.md](docs/fr/security.md). Vie privée (Loi 25) :
  [docs/fr/law25.md](docs/fr/law25.md).
- **Code source disponible, usage non commercial.** SwitchBoardChat est distribué sous la
  [licence PolyForm Noncommercial 1.0.0](LICENSE) (texte officiel en anglais) : vous pouvez l’utiliser,
  l’étudier et le modifier à toute fin non commerciale (lignes d’écoute, organismes de bienfaisance,
  organismes publics, usage personnel). Un usage commercial demande une licence distincte du titulaire
  des droits. Ce n’est pas une licence « open source » au sens de l’OSI.
