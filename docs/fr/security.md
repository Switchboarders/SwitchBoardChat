[English](../security.md) · **Français**

# Notes de sécurité et modèle de menace

Ce que la passerelle protège, ce à quoi elle fait confiance, et comment la déployer de façon sûre. Les
obligations de protection des renseignements personnels (conservation, effacement, avis) sont dans
[law25.md](law25.md).

## Actifs

- **Conversations** : texte des SMS et du clavardage Web. Jamais écrit sur disque ni dans les journaux par
  la passerelle; transporté dans des salles Matrix chiffrées de bout en bout. Les textes en attente de
  livraison sont chiffrés au repos.
- **Numéros de téléphone des appelants** : stockés chiffrés (AES-256-GCM, `PHONE_ENC_KEY`) et sous forme
  d’empreintes à clé (`PHONE_HASH_SECRET`); les journaux n’en montrent qu’un préfixe de 8 caractères.
- **Secrets** : mot de passe Matrix du robot et sa session, son magasin cryptographique, le mot de passe
  d’API voip.ms (chiffré), les jetons d’API des centres d’appel (chiffrés, retrouvés par empreinte),
  `PHONE_ENC_KEY`, `PHONE_HASH_SECRET`, le secret du client d’administration MAS. Tous sont dans `.env` et
  le dossier de données.

## Points d’entrée et qui peut les utiliser

| Point d’entrée | Appelant | Authentification | Limites |
|---|---|---|---|
| `GET /voip_api` (port 3443, TLS) | Webhook SMS de voip.ms | `sbchatapikey` dans l’URL (empreinte du jeton d’API du centre d’appel) | Clé vérifiée avant la mise en file; 20 mauvaises clés en 10 min d’une même adresse → refusée 10 min; délai de 30 s |
| `POST /sbchat_api` | Interface d’administration SwitchBoard | `Authorization: Bearer <jeton du centre d’appel>`; niveaux de privilège par commande | Jeton vérifié avant la lecture du corps; corps de 64 Kio; 20 mauvais jetons → refusée; délai de 300 s |
| `/web/*` | Navigateurs des visiteurs (widget) | Jeton de session (48 caractères aléatoires, témoin ou en-tête); `Origin` doit figurer dans `WEB_ALLOWED_ORIGINS` | Corps de 8 Kio, longueur des messages, sessions par IP par heure, preuve de travail en cas d’attaque, bannissements, 4 flux par session |
| Salles Matrix | Répondants | Compte Matrix, présence dans la liste des répondants du centre d’appel | Seul le robot peut inviter (niveau de pouvoir 100) |

Tous les ports : au plus 256 requêtes traitées à la fois, en-têtes reçus en moins de 15 s.

## Frontières de confiance

- **Le port 8088 (HTTP en clair) est réservé à un mandataire inverse.** Ne le publiez jamais.
  `X-Forwarded-For` n’est cru que sur ce port et seulement depuis les adresses de `TRUSTED_PROXIES`;
  liez-le à une adresse privée avec `HTTP_BIND`. Voir [reverse-proxy.md](reverse-proxy.md).
- **Le port 3443** utilise par défaut un certificat autosigné (voip.ms l’accepte). Placez un vrai certificat
  devant s’il est appelé par autre chose.
- **Clé du webhook voip.ms** : quiconque connaît l’URL `/voip_api` d’un centre d’appel peut injecter des
  textes qui semblent venir de n’importe quel numéro. Traitez cette URL comme un secret; régénérer le jeton
  d’API du centre d’appel la change.
- **Serveur Matrix** : la passerelle fait confiance à son propre Synapse et à son MAS. Utilisez votre propre
  serveur, pas un serveur public. Avec l’accès d’administration MAS, la passerelle peut créer des sessions
  d’administration de courte durée pour son propre utilisateur (pour purger les salles supprimées et
  déconnecter ses anciens appareils); protégez le secret du client d’administration MAS autant que la base
  de données.

## Liste de vérification du déploiement

- Générez chaque secret avec `openssl rand -hex 32` (la passerelle refuse de démarrer avec les valeurs
  d’exemple de `.env-example` ou avec des secrets trop courts). `.env` en mode 0600.
- Utilisez l’image Docker fournie : la passerelle tourne sous l’utilisateur non privilégié `sb-chat` avec
  `umask 077`; données, session et magasin cryptographique ne sont lisibles que par cet utilisateur.
- Ne publiez que le port 3443 (si voip.ms est utilisé) et votre mandataire inverse; gardez 8088 et les ports
  internes de Synapse et de MAS hors d’Internet.
- Fixez les versions des images (pas de `:latest`) pour Synapse, MAS et les bases de données; mettez-les à
  jour régulièrement.
- Sauvegardez `.env` et le dossier de données séparément, chiffrés, avec une date d’expiration.
- Donnez au robot l’accès d’administration Synapse (voir [law25.md](law25.md), « Purger les salles de
  Synapse ») pour que les conversations effacées soient aussi purgées de Synapse.

## Limites connues

- Les SMS ne sont pas chiffrés de bout en bout entre l’appelant et voip.ms.
- Les appareils des répondants gardent l’historique déchiffré jusqu’à ce que la salle soit supprimée de
  leur côté.
- Quiconque détient à la fois la base de données et le `.env` peut déchiffrer les numéros et jetons stockés.
