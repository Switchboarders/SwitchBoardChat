[English](../homeserver.md) · **Français**

# Exigences pour le serveur Matrix (Synapse + MAS)

SwitchBoardChat a besoin de son **propre** serveur Matrix : Synapse avec Matrix Authentication Service
(MAS). N’utilisez pas un serveur public comme matrix.org (limites de débit, et la passerelle a besoin de
droits d’administration). Installez les deux avec leurs guides officiels, puis appliquez les réglages
ci-dessous.

- Synapse : <https://element-hq.github.io/synapse/latest/setup/installation.html>
- MAS : <https://element-hq.github.io/matrix-authentication-service/setup/>
- Déléguer l’authentification de Synapse à MAS : la section `matrix_authentication_service` du
  [manuel de configuration de Synapse](https://element-hq.github.io/synapse/latest/usage/configuration/config_documentation.html).

## `homeserver.yaml` de Synapse

```yaml
# L’authentification relève de MAS (Synapse 1.157+)
matrix_authentication_service:
  enabled: true
  endpoint: http://mas:8080/          # MAS sur le réseau Docker interne
  secret: "<même valeur que matrix.secret de MAS>"  # ou secret_path : un fichier qui le contient

# Les répondants sont invités à chaque nouvelle salle; accepter automatiquement les y fait entrer tout
# de suite, et les messages de la passerelle leur arrivent comme des notifications normales.
# Utilisateurs locaux seulement.
auto_accept_invites:
  enabled: true
  only_for_direct_messages: false
  only_from_local_users: true

# Les répondants se trouvent entre eux; personne d’autre ne devrait être sur ce serveur.
user_directory:
  search_all_users: false

# Filet de sécurité facultatif (Loi 25) : purge les vieux évènements de TOUTES les salles du serveur.
# Gardez max_lifetime au moins égal à SMS_IDLE_RETENTION_DAYS. Seulement si tous les utilisateurs du
# serveur sont d’accord.
# retention:
#   enabled: true
#   default_policy: { min_lifetime: 1h, max_lifetime: 8d }
#   purge_jobs: [{ longest_max_lifetime: 8d, interval: 12h }]
```

Ne gardez qu’un écouteur HTTP (port 8008, `x_forwarded: true`) derrière votre mandataire inverse, et ne
publiez pas 8008 sur l’hôte : passez par le réseau Docker.

## `config.yaml` de MAS

- **Pas d’inscription ouverte** : `account.password_registration_enabled: false` (créez les comptes avec
  `mas-cli manage register-user`, ou depuis l’interface SwitchBoard). Si vous l’activez, exigez la
  vérification du courriel.
- **Accès d’administration pour la passerelle** (facultatif mais recommandé : elle peut déconnecter ses
  anciens appareils, retirer ses limites de débit et purger de Synapse les salles supprimées, voir
  [law25.md](law25.md)) :
  1. sous `clients:`, ajoutez un client avec `client_id` (un ULID),
     `client_auth_method: client_secret_basic` et un `client_secret` aléatoire (`openssl rand -hex 32`);
  2. inscrivez ce `client_id` sous `policy.data.admin_clients`;
  3. vérifiez qu’une entrée de `http.listeners` sert la ressource `adminapi` (réseau interne seulement);
  4. faites de l’utilisateur du robot un administrateur : `mas-cli manage promote-admin <robot>`;
  5. mettez l’identifiant et le secret dans le `.env` de la passerelle : `MAS_ADMIN_CLIENT_ID` /
     `MAS_ADMIN_CLIENT_SECRET`.

## Docker et réseau

- Fixez les versions des images (p. ex. `matrixdotorg/synapse:v1.162.0`), jamais `:latest`; mettez à
  jour délibérément.
- Ne publiez que votre mandataire inverse (80/443) et, si voip.ms est utilisé, le port 3443 de la
  passerelle. Synapse (8008), MAS (8080/8081) et les bases de données restent sur le réseau Docker interne.
- Le mandataire inverse sert `/.well-known/matrix/client` et `/.well-known/matrix/server`, et achemine les
  chemins de MAS comme le décrit la documentation de MAS. Pour le clavardage Web, voir
  [reverse-proxy.md](reverse-proxy.md).

## Le compte du robot

Créez-le dans MAS (`mas-cli manage register-user`) avec un long mot de passe aléatoire. Ne gardez que la
session de la passerelle : d’autres sessions du compte du robot qui ne sont pas signées croisées font
refuser par Element X l’envoi de messages au robot.
