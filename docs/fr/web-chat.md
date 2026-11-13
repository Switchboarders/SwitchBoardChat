[English](../web-chat.md) · **Français**

# Clavardage Web (pour le module WordPress wp-chat)

switchboard-chat était à l’origine une passerelle SMS ↔ Matrix. Le **clavardage Web** ajoute un
deuxième canal : un visiteur sur un site Web discute, en temps réel et en toute confidentialité, avec
les mêmes répondants que ceux qui répondent aux SMS, sans savoir qu’il s’agit d’une équipe. Côté
site Web, on utilise le module WordPress **wp-chat** (projet distinct); n’importe quelle autre
interface peut employer la même API HTTP (section 6).

Ce guide ne dépend ni de la façon ni de l’endroit où vous hébergez le tout. Il explique ce qu’il faut,
comment activer la fonction, comment elle se comporte et comment régler les problèmes courants. Les
détails réseau (TLS, proxys inverses) sont dans [reverse-proxy.md](reverse-proxy.md); la mise à
niveau d’une installation existante est dans [UPGRADE.md](UPGRADE.md); la protection des
renseignements personnels est dans [law25.md](law25.md).

## 1. Fonctionnement

```
 Navigateur du visiteur ── HTTPS ──▶ proxy inverse ──▶ switchboard-chat ──▶ Serveur Matrix ◀── répondants
   ▲  (JS du module)                  (port 443)       (HTTP, port 8088)    (salle chiffrée)   (Element, …)
   │
   └── le site Web ne fait que servir le module; il ne voit jamais un message
```

1. Le module appelle `POST /web/session`. Le serveur vérifie l’origine du site Web, crée une session
   en mémoire et retourne un **jeton de session** aléatoire, que le module garde en mémoire dans la page
   (aucun témoin).
2. Au premier message du visiteur, le serveur crée une **salle Matrix chiffrée de bout en bout**,
   nommée `Web <nom convivial>`, et y invite chaque **répondant actif** du centre d’appels configuré. Le nom vient
   d’une empreinte à clé de l’adresse réseau du visiteur (adresse IPv4, ou réseau IPv6 /64), comme le nom d’un
   appelant SMS vient de son numéro : le même réseau reçoit toujours le même nom, ce qui permet aux répondants de
   reconnaître un visiteur qui revient (y compris un visiteur abusif). Il ne peut pas être ramené à une adresse sans le
   `PHONE_HASH_SECRET` du serveur. Des visiteurs derrière une même adresse partagée (certains réseaux mobiles, une
   bibliothèque, un lieu de travail) reçoivent le même nom, et un visiteur qui change de réseau en reçoit un nouveau :
   un nom est un indice, pas une preuve.
3. Les messages du visiteur sont publiés dans cette salle. Les répondants répondent dans leur client
   Matrix habituel; la réponse est poussée vers le navigateur du visiteur par des **évènements
   envoyés par le serveur** (SSE).
4. Un rechargement, ou le passage à une autre page du site, conserve le clavardage : le module demande la
   conversation jusqu’ici (`GET /web/history`) et se reconnecte au flux, en reprenant après la dernière ligne affichée.
5. Le clavardage prend fin quand le visiteur y met fin, quand un répondant écrit `!fin`, ou après **48 heures
   sans aucune activité** (`WEB_IDLE_TIMEOUT_MIN`). Le serveur **retire alors tous les répondants de la salle**, la
   quitte, l’oublie (elle disparaît des applications des répondants) et supprime sa ligne dans la base de données.
   Un visiteur qui a ouvert le clavardage sans jamais écrire est retiré après `WEB_ABANDON_MIN` minutes sans connexion.

Ce que la passerelle **ne conserve jamais** pour les clavardages Web : le texte des messages (en
mémoire seulement, jamais journalisé), l’adresse IP du visiteur (gardée en mémoire pour les limites de
débit, une heure au plus), tout profil.

## 2. Prérequis

| Besoin | Détails |
|---|---|
| switchboard-chat | version avec le clavardage Web (ce dépôt, branche `main`) |
| Serveur Matrix | celui que la passerelle utilise déjà, avec les comptes des répondants |
| Répondants | au moins un répondant **actif** dans le centre d’appels utilisé pour le clavardage Web |
| HTTPS | le navigateur doit joindre le serveur en **HTTPS** (un site servi en HTTPS ne peut pas appeler du HTTP simple). Terminez le TLS dans un proxy inverse; voir [reverse-proxy.md](reverse-proxy.md) |
| Un nom public | p. ex. `chat.example.org`, pointant vers le proxy |
| L’origine du site Web | p. ex. `https://www.example.org` (protocole + hôte, sans chemin) |

La passerelle écoute sur **3443** (HTTPS, certificat autosigné, pour les essais) et **8088** (HTTP
simple, **destiné à un proxy inverse; ne l’exposez jamais sur Internet**).

## 3. Configuration

Définissez ces variables dans l’environnement de la passerelle (`.env`, environnement du conteneur ou
unité systemd). Le clavardage Web reste **désactivé** tant que les deux premières ne sont pas définies.

| Variable | Requise | Défaut | Signification |
|---|---|---|---|
| `WEB_ALLOWED_ORIGINS` | oui | – | Sites Web autorisés à utiliser le clavardage, séparés par des virgules, p. ex. `https://www.example.org,https://example.org`. Correspondance exacte : protocole, hôte et port, sans chemin ni barre oblique finale. Toute autre origine reçoit `403 origin_not_allowed`. |
| `WEB_CALL_CENTER_ID` | oui | – | Numéro du centre d’appels dont les répondants actifs sont invités (`id_call_center` dans la table `call_center`). Ses macros (`#nom`) sont utilisables dans les salles Web. |
| `WEB_IDLE_TIMEOUT_MIN` | non | `2880` | Minutes (48 h) après la dernière interaction avant que le clavardage soit terminé et supprimé. |
| `WEB_MAX_SESSIONS` | non | `200` | Clavardages ouverts en même temps; les visiteurs supplémentaires reçoivent `503 busy`. |
| `WEB_ABANDON_MIN` | non | `5` | Minutes après lesquelles une session dont le visiteur **n’a jamais écrit** et dont la connexion en direct est fermée est retirée. Les clavardages avec messages sont gardés jusqu’au délai d’inactivité, pour que le visiteur puisse revenir. |
| `WEB_SESSION_COOKIE` | non | `false` | Dépose aussi l’ancien témoin `wpchat`. Nécessaire seulement pour les modules antérieurs à 0.5.0 (section 9). |
| `WEB_SESSIONS_PER_IP_PER_HOUR` | non | `5` | Nouveaux clavardages permis par heure depuis une même adresse réseau (adresse IPv4 ou /64 IPv6). Réduisez-le contre les abus par rechargement de la page. |
| `WEB_BAN_DAYS` | non | `1` | Durée, en jours, d’un `!bloquer` quand le répondant n’en donne aucune |
| `WEB_BAN_MAX_DAYS` | non | `30` | Durée maximale, en jours, d’un blocage que peut fixer un répondant |
| `WEB_CHALLENGE` | non | `auto` | Épreuve de calcul avant le début d’un clavardage : `auto` (seulement après un blocage récent ou quand une même adresse ouvre plusieurs clavardages), `always`, `off` |
| `WEB_CHALLENGE_BASE_BITS`, `WEB_CHALLENGE_MAX_BITS` | non | `12`, `18` | Difficulté en bits (chaque bit double le travail) : 12 est instantané, 18 prend quelques secondes sur un ordinateur et davantage sur un téléphone |
| `WEB_CHALLENGE_WINDOW_HOURS` | non | `6` | Durée pendant laquelle un blocage continue d’augmenter la difficulté pour tout le monde |
| `WEB_ATTACK_STARTS_PER_MIN` | non | `30` | Requêtes de début de clavardage par minute (toutes adresses confondues) au-delà desquelles le mode sous attaque démarre |
| `WEB_ATTACK_HOLD_MIN` | non | `15` | Minutes pendant lesquelles le mode sous attaque dure après le dernier dépassement |
| `WEB_MAX_NEW_ROOMS_PER_MIN` | non | `10` | Nouvelles salles Matrix (et avis « nouveau clavardage ») permises par minute, au total |
| `WEB_NO_MESSAGE_TIMEOUT_MIN` | non | `10` | Minutes après lesquelles un clavardage où rien n’a jamais été écrit est fermé (2 sous attaque) |
| `PHONE_HASH_SECRET`, `PHONE_ENC_KEY` | oui (communs avec les SMS) | – | Utilisés par la base de données; voir `.env-example`. |

Le délai d’inactivité figure dans le texte de confidentialité montré aux visiteurs (une phrase dans les
réglages du module) : gardez les deux concordants.

Trouver le numéro du centre d’appels et ses répondants :

```sql
sqlite3 <dossier de données>/db/users.db "select id_call_center, call_center_did from call_center;"
sqlite3 <dossier de données>/db/users.db "select id_call_center, respondant_username, is_active from respondant;"
```

(Les répondants se gèrent avec la commande d’API `change_respondents`. Les répondants non rafraîchis
depuis un mois sont désactivés automatiquement, comme pour les SMS.)

## 4. Première activation

1. Rendez la passerelle accessible en HTTPS sous un nom public ([reverse-proxy.md](reverse-proxy.md)).
2. Définissez `WEB_ALLOWED_ORIGINS` et `WEB_CALL_CENTER_ID`; redémarrez la passerelle pour qu’elle les lise.
3. Vérifiez depuis n’importe quel poste :
   ```bash
   # origine autorisée → 200, un corps JSON avec un jeton, des en-têtes CORS
   curl -si -X POST https://chat.example.org/web/session -H 'Origin: https://www.example.org' | head -15
   # aucune origine ou une autre → 403 {"error":"origin_not_allowed"}
   curl -si -X POST https://chat.example.org/web/session | head -3
   ```
4. Vérifiez que **les réponses arrivent en direct** (l’étape que les proxys font le plus souvent échouer) :
   ```bash
   TOKEN=$(curl -s -X POST -H 'Origin: https://www.example.org' https://chat.example.org/web/session | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
   curl -sN -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/stream
   ```
   Une ligne `:` (un commentaire de maintien de connexion) doit apparaître **en 15 secondes environ**.
   Si rien n’apparaît, le proxy met les données en mémoire tampon : voir
   [reverse-proxy.md](reverse-proxy.md). Faites Ctrl+C, puis terminez la session d’essai :
   `curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/end`.
5. Installez et configurez le module sur le site Web (voir la documentation de wp-chat) et faites un
   vrai clavardage de bout en bout.

## 5. Expérience des répondants et commandes

Une nouvelle salle apparaît dans le client Matrix des répondants, nommée `Web …`. Le premier message
commence par un court rappel en français et en anglais. Les répondants sont avertis par un avis avec
**mention** quelques secondes après la création de la salle, puis rappelés après 1, 2 et 5 minutes, ensuite
chaque heure, tant que personne n’a tapé ni écrit (voir
[respondent-notifications.md](respondent-notifications.md), qui explique aussi le réglage Synapse
`auto_accept_invites` et comment vérifier l’enregistrement push de chaque téléphone). Dans une salle Web :

| Ce que le répondant écrit | Effet |
|---|---|
| du texte | envoyé au navigateur du visiteur |
| `#nomdemacro` | le texte de la macro du centre d’appels est envoyé au visiteur; une confirmation apparaît dans la salle |
| `#nomdemacro` portant l’étiquette `newcomerweb` | envoyé **automatiquement** au visiteur après son premier message (la salle reçoit une note le précisant). L’étiquette se choisit dans le formulaire de macro de SwitchBoard |
| `!ok` | salle privée : retire les autres répondants (comme pour les SMS); la passerelle répond dans la salle |
| `!fin` / `!end` | met fin au clavardage et le supprime (tous les répondants retirés, salle quittée et oubliée, visiteur informé) |
| `!oubli` / `!forget` | comme `!fin` pour un clavardage Web (effacement) |
| `!bloquer [jours]` / `!ban [jours]` | **visiteur abusif** : met fin au clavardage et refuse la connexion du visiteur pendant `jours` (défaut `WEB_BAN_DAYS`, au plus `WEB_BAN_MAX_DAYS`) |
| `!aide` / `!help` | liste les commandes et les macros (la réponse est publiée dans la salle : `!aide` en français, `!help` en anglais) |
| (personne de garde) | quand aucun répondant n’est actif, un nouveau clavardage est refusé avec `503 {"error":"no_respondent","message":"…"}`; `message` est la macro `no_respondent` du centre d’appels (définie dans SwitchBoard, aussi envoyée aux appelants SMS) et le module l’affiche |
| `!stop` | reçoit une note disant qu’elle ne s’applique pas (le visiteur n’a pas de numéro de téléphone) |
| tout autre `!mot` | reçoit « commande inconnue » et un renvoi vers `!aide` |

Quand un clavardage se termine, **les répondants devraient aussi supprimer la salle dans leur propre
client** : la passerelle quitte la salle, mais leurs copies restent sur leurs appareils jusqu’à ce
qu’ils les suppriment (et jusqu’à ce que la politique de conservation du serveur Matrix, s’il y en a
une, les purge).

### Visiteurs abusifs

Un visiteur abusif peut démarrer un nouveau clavardage à volonté, puisqu’un clavardage n’est qu’une session
jetable. La passerelle protège les répondants par couches :

1. **`!bloquer [jours]` (ou `!ban`) dans la salle.** N’importe quel répondant peut l’écrire. Le clavardage
   prend fin, **tous les autres clavardages ouverts depuis la même connexion aussi**, et les nouveaux
   clavardages depuis cette connexion sont refusés avec `403 blocked` pendant la durée choisie (1 jour par
   défaut, 30 jours au plus). Le visiteur voit un message poli (et le texte de contact défini dans le
   module, comme un autre numéro à appeler).
2. **À quoi s’applique un blocage :** l’adresse IPv4 du visiteur, ou le réseau /64 d’une adresse IPv6 (un
   visiteur contrôle les 64 derniers bits : bloquer une seule adresse serait inutile).
3. **Ce qui est stocké :** seulement une **empreinte à clé** de cette adresse (secret `PHONE_HASH_SECRET`) et
   la date de fin, dans la base de données, jusqu’à l’expiration du blocage; les blocages expirés sont
   supprimés chaque minute. Aucun motif, aucune conversation, aucune adresse lisible.
4. **Lever un blocage** (adresse partagée bloquée par erreur) : attendre son expiration, ou un administrateur
   de niveau de privilège 100 ou plus appelle la commande d’API `web_unban` avec
   `{"command":"web_unban","ip":"203.0.113.9"}` (ou `"ip":"all"`).
5. **Limites de débit qui agissent sans intervention :** `WEB_SESSIONS_PER_IP_PER_HOUR` nouveaux clavardages
   par heure et par adresse (5 par défaut, puis `429 rate_limited`), 10 messages par 30 secondes par
   clavardage, et `WEB_MAX_SESSIONS`.
6. **Un blocage qui suit le navigateur, pas le réseau.** Quand un répondant bloque un visiteur, le navigateur
   bloqué reçoit un **marqueur signé « bloqué jusqu’à »** (deux dates et une signature : aucun identifiant) dans
   un évènement `banned` de son flux en direct, et le module le garde dans le `localStorage` du navigateur
   (clé `wpchat_blocked`). Il envoie le marqueur dans un en-tête `X-WPChat-Block` quand il demande un nouveau
   clavardage, et le serveur répond `403 blocked`, quel que soit le VPN ou le réseau que le navigateur utilise
   maintenant. Rien n’est stocké dans le navigateur d’un visiteur qui n’a jamais été bloqué. Le marqueur prend
   fin avec le blocage; `web_unban` avec `"ip":"all"` révoque tous les marqueurs émis avant.
7. **Une épreuve de calcul quand il y a des problèmes.** Avant qu’un clavardage commence, le serveur peut
   répondre `428 challenge_required` avec une épreuve signée à usage unique (valide 3 minutes, liée à
   l’adresse qui la demande). Le module trouve un nonce tel que `SHA-256("<épreuve>:<nonce>")` commence par le
   nombre exigé de bits à zéro, puis recommence. Un visiteur y perd un instant; un script, ou quelqu’un qui
   recharge la page sans cesse, la paie à chaque tentative. En mode `auto` (par défaut), elle ne s’applique que
   si un blocage a eu lieu dans les `WEB_CHALLENGE_WINDOW_HOURS` dernières heures ou si la même adresse a déjà
   ouvert un clavardage dans la dernière heure, et la difficulté monte de 2 bits par blocage récent et par
   clavardage récent, jusqu’à `WEB_CHALLENGE_MAX_BITS`. Elle n’utilise aucun service tiers.

Limites de la méthode : une personne déterminée finit par contourner chaque couche : un nouveau VPN donne une
nouvelle adresse, et effacer les données du site dans le navigateur (ou utiliser une fenêtre privée ou un autre
navigateur) supprime le marqueur. Ensemble, ces couches font cesser le rechargement répété, les scripts et les
abus répétés depuis une même connexion, ce qui est le cas habituel, et le reste coûte réellement du temps, mais
aucune n’est une vérification d’identité. Une adresse partagée (école, bureau, fournisseur mobile) peut bloquer des
innocents pendant la durée du blocage, d’où la courte durée par défaut et `web_unban`.

### Sous attaque (déni de service)

Une attaque organisée (nombreuses adresses, une requête chacune) n’est pas arrêtée par les limites par adresse; la
passerelle surveille donc aussi le service dans son ensemble :

1. **Mode sous attaque.** Les requêtes de début de clavardage sont comptées au total. Au-delà de
   `WEB_ATTACK_STARTS_PER_MIN` en une minute, la passerelle passe en mode sous attaque pendant
   `WEB_ATTACK_HOLD_MIN` minutes après le dernier dépassement : l’épreuve de calcul est exigée de **tout le
   monde**, 4 bits plus difficile, et un clavardage où rien n’a été écrit est fermé après 2 minutes au lieu de
   `WEB_NO_MESSAGE_TIMEOUT_MIN`, de sorte que des robots ne peuvent pas occuper les places de `WEB_MAX_SESSIONS`.
   Il se termine de lui-même. La ligne de journal `Web chat: under attack mode ON` le signale. `WEB_CHALLENGE=off`
   désactive l’épreuve (et cette réponse).
2. **Plafond de salles.** Au plus `WEB_MAX_NEW_ROOMS_PER_MIN` nouvelles salles Matrix par minute, quel que soit le
   nombre de visiteurs; les autres reçoivent `503 busy` et peuvent réessayer. Les répondants ne sont donc jamais
   inondés d’avis de nouveau clavardage, et le serveur Matrix n’est pas inondé de créations de salles.
3. **Économe par conception.** Les routes Web refusent les corps de requête de plus de 8 Ko, un anneau de compteurs
   utilise une mémoire constante quelle que soit l’ampleur du déluge, et la liste de blocage, les répondants du
   centre d’appels et le nombre de blocages récents ne sont lus dans la base de données qu’au plus toutes les 10
   secondes : un déluge ne devient pas un déluge sur la base de données.

Ce que la passerelle ne peut pas faire : absorber un trafic qui sature la liaison réseau du serveur. Cela se règle
en amont : la protection réseau de l’hébergeur (vérifiez qu’elle est active), un pare-feu et des limites de débit
dans le proxy inverse. **Ne placez pas devant `/web/*` un CDN qui termine le HTTPS** : il lirait les messages des
visiteurs en clair, ce qui contredit la promesse de confidentialité (un CDN convient pour les pages statiques du
site Web). Décidez à l’avance comment les visiteurs vous joignent quand le clavardage Web est hors service : le
*Texte additionnel quand le clavardage est indisponible* du module leur indique un autre moyen d’obtenir de l’aide.

## 6. Référence de l’API HTTP

Toutes les routes sont sous `/web/`. Les appels inter-origines exigent l’origine du site Web dans
`WEB_ALLOWED_ORIGINS`; le serveur répond aux requêtes préalables CORS et envoie
`Access-Control-Allow-Origin: <origine>` avec `Access-Control-Allow-Credentials: true`
(méthodes `GET, POST`; en-tête `Content-Type`). Les requêtes doivent utiliser `credentials: include`.
Chaque requête doit porter un en-tête `Origin` correspondant.

**Le jeton de session :** aléatoire, 48 caractères, retourné dans le corps JSON de `POST /web/session`. Le
client le garde en mémoire et l’envoie à chaque appel suivant sous la forme `Authorization: Bearer <jeton>`;
aucun témoin n’est donc en cause, et les réglages de témoins tiers des navigateurs n’ont pas d’importance.
Seule son empreinte SHA-256 est conservée, en mémoire. `navigator.sendBeacon` ne peut pas définir d’en-têtes;
`POST /web/end` accepte donc aussi un corps JSON `{"token":"…"}`. *Ancien mode :* avec
`WEB_SESSION_COOKIE=true`, le serveur dépose aussi un témoin `wpchat` (`Path=/web; HttpOnly; Secure;
SameSite=None`) et l’accepte quand aucun en-tête n’est envoyé.

| Route | Corps | Succès | Erreurs |
|---|---|---|---|
| `POST /web/session` | – | `200 {"resumed":false,"token":"…"}`. `{"resumed":true,…}` si la requête porte déjà un jeton actif. | `403 origin_not_allowed`, `403 blocked` (adresse bloquée, ou un marqueur `X-WPChat-Block` valide a été envoyé), `428 challenge_required` (corps `{"error":"challenge_required","challenge":"…","bits":N}` : résolvez-la et répétez l’appel avec le corps `{"challenge":"…","nonce":"…"}`), `503 not_configured` (`WEB_CALL_CENTER_ID` absent ou invalide), `503 no_respondent`, `503 busy`, `429 rate_limited` |
| `POST /web/message` | `{"text":"…"}` (1 à 1000 caractères, espaces retirés aux extrémités) | `200 {}` | `401 no_session`, `400 empty`, `413 too_long`, `429 rate_limited`, `403 origin_not_allowed`, `503 no_respondent`, `503 busy` (plafond de salles atteint), `502 room_creation_failed` / `delivery_failed` |
| `GET /web/history` | – (jeton dans `Authorization`) | `200 {"messages":[{"seq":1,"from":"me"|"them","text":"…"}],"last":N}` la conversation gardée en mémoire, pour restaurer la fenêtre après un rechargement | `401 no_session`, `403 origin_not_allowed` |
| `GET /web/stream` | – | `text/event-stream` (à lire avec `fetch`, car `EventSource` ne peut pas définir l’en-tête). Évènements : `message` avec les données `{"text":"…"}` et une ligne `id:` (le numéro de ligne); envoyez `Last-Event-ID` pour reprendre après une ligne déjà affichée (sans lui, seules les lignes jamais livrées sont envoyées); `banned` avec les données `{"until":…,"marker":"…"}` (le visiteur a été bloqué; gardez le marqueur), suivi de `closed`; `closed` avec les données `{}` (clavardage terminé). Une ligne de commentaire `:` toutes les 15 s maintient la connexion. La réponse porte `X-Accel-Buffering: no`. | `401 no_session`, `403 origin_not_allowed` |
| `POST /web/end` | – (peut être vide) | `200 {}`; la session et la salle sont supprimées | `403 origin_not_allowed` |

Les erreurs sont en JSON : `{"error":"<code>"}`. Un message destiné à un visiteur dont le flux est
momentanément déconnecté est gardé en mémoire jusqu’à 60 secondes (20 au plus) et livré à la
reconnexion du flux (`EventSource` se reconnecte seul).

**Limites :** 10 messages par 30 secondes par session; `WEB_SESSIONS_PER_IP_PER_HOUR` (5 par défaut) nouvelles
sessions par heure par adresse cliente (la dernière entrée de `X-Forwarded-For`, sinon l’adresse de la
connexion, en adresse IPv4 ou en réseau /64 IPv6); `WEB_MAX_SESSIONS` clavardages ouverts. Une connexion
bloquée reçoit `403 blocked`.

## 7. Exploitation

- **En mémoire seulement.** Les sessions Web vivent en mémoire : redémarrer la passerelle met fin à tous
  les clavardages Web. Les salles Web restantes sont purgées environ 45 secondes après le démarrage.
- **Le nettoyage des sessions inactives** s’exécute chaque minute. Les salles des conversations SMS
  suivent `SMS_IDLE_RETENTION_DAYS` (voir [law25.md](law25.md)).
- **Les journaux** ne contiennent jamais de texte de message, de numéro de téléphone, de jeton ni de
  mot de passe.
- **Capacité.** Chaque clavardage ouvert coûte une connexion (SSE) et peu de mémoire. Vérifiez les
  limites de connexions du proxy et du système d’exploitation si vous attendez des centaines de
  visiteurs simultanés.
- **Limites de débit de Matrix.** La création de salles est limitée par le serveur Matrix. Configurez
  le client d’administration MAS facultatif (`MAS_ADMIN_*` dans `.env-example`) ou relevez les limites
  du compte du robot.

## 8. Dépannage

| Symptôme | Cause probable et correctif |
|---|---|
| `502`/`504` depuis l’adresse publique | le proxy ne joint pas le port 8088 (passerelle arrêtée ou en démarrage), ou la passerelle est bloquée au démarrage (voir le guide de mise à niveau, « appareil non signé par signature croisée ») |
| `403 origin_not_allowed` dans le navigateur | `WEB_ALLOWED_ORIGINS` n’est pas exactement l’origine du site (`https://www.example.org` ≠ `https://example.org`; ajoutez les deux). Redémarrez après modification. |
| Le visiteur peut écrire, mais les réponses n’apparaissent jamais | le proxy met le flux en mémoire tampon. Faites l’essai du flux en direct de la section 4; corrigez avec [reverse-proxy.md](reverse-proxy.md). |
| `503 no_respondent` | aucun répondant actif pour `WEB_CALL_CENTER_ID` |
| `503 not_configured` | `WEB_CALL_CENTER_ID` est absent ou n’existe pas |
| `429 rate_limited` pour tout le monde | la passerelle voit l’adresse du proxy pour chaque visiteur : le proxy doit transmettre `X-Forwarded-For` (voir reverse-proxy.md) |
| `401 no_session` sur les messages ou sur le flux | le jeton est absent ou expiré : le clavardage est terminé, le délai d’inactivité ou `WEB_ABANDON_MIN` s’est écoulé, la passerelle a redémarré, ou un module antérieur à 0.5.0 parle à un serveur dont `WEB_SESSION_COOKIE` est désactivé (section 9) |
| Réponses perdues après un redémarrage | normal : les sessions Web ne sont pas conservées |
| Salle créée mais personne d’invité | le centre d’appels n’a aucun répondant actif, ou leurs comptes Matrix sont introuvables |
| Le module affiche sans cesse « Vérification de votre connexion… » ou échoue après quelques essais | `428 challenge_required` se répète : l’adresse du visiteur change entre les deux appels (réseau mobile), ou le téléphone est très lent à la difficulté actuelle. Réduisez `WEB_CHALLENGE_MAX_BITS`, ou `WEB_CHALLENGE=off` |
| Un visiteur dit que le clavardage n’est « pas accessible depuis cette connexion » | `403 blocked` : un répondant a utilisé `!bloquer` (ou une adresse partagée a été bloquée). Le blocage expire de lui-même; levez-le avec la commande d’API `web_unban` |

## 9. Passer des sessions par témoin à celles par jeton (modules antérieurs à 0.5.0)

Les anciens modules (extension 0.4.0 et antérieures) reposent sur le témoin `wpchat` et sur `EventSource`. Le
module actuel utilise le jeton et se rabat sur les témoins quand un serveur ne retourne aucun jeton; l’ordre
n’a donc pas beaucoup d’importance :

1. Mettez à jour l’**extension** d’abord, puis la passerelle. Le nouveau module fonctionne avec les
   anciennes et les nouvelles passerelles.
2. Si certains sites Web utilisent encore une ancienne extension, définissez `WEB_SESSION_COOKIE=true` sur la
   passerelle jusqu’à leur mise à jour, puis retirez-le (plus aucun témoin n’est alors déposé).
3. L’ancien module ne met pas fin à un clavardage quand sa page est fermée; la passerelle y met fin après
   `WEB_ABANDON_MIN` minutes sans connexion en direct.
