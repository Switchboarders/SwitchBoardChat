[English](../encryption-recovery.md) · **Français**

# Récupération du chiffrement : quand les messages des répondants disparaissent

Les répondants sont des bénévoles. Chaque fois qu’une réponse disparaît, ou qu’on leur demande de vérifier ou de
réinitialiser quelque chose, certains perdent patience, et certains abandonnent. Cette page existe pour que
l’exploitant corrige la cause une fois, discrètement, et ne demande jamais à un répondant de réinitialiser son
identité à cause d’une panne de la passerelle.

## 1. Règles

1. **Ne jamais demander à un répondant de réinitialiser son identité de chiffrement, de se déconnecter de tous ses
   appareils ou de tout revérifier à cause d’un problème de la passerelle.** La faute est du côté de la passerelle
   tant qu’on n’a pas prouvé le contraire (section 3).
2. **`session` et `db/` forment un tout.** Les sauvegarder ensemble, du même moment, passerelle arrêtée
   (`scripts/backup-data.sh`). Les restaurer ensemble, depuis un seul dossier de sauvegarde
   (`scripts/restore-data.sh`). Jamais l’un sans l’autre, jamais des fichiers de deux sauvegardes, jamais copier un
   magasin pendant que la passerelle tourne.
3. **Faire une sauvegarde avant chaque mise à jour et chaque modification des données**, et garder au moins les trois dernières.
4. **Ne pas supprimer ni réinitialiser la signature croisée** (l’« identité » de la passerelle) pour régler un problème
   de messages. Chaque répondant devrait approuver et refaire confiance, et quelqu’un capable de provoquer une
   réinitialisation pourrait y glisser sa propre session.
5. **Après chaque redémarrage, lire le journal** : `Crypto check: OK` et `HTTPS server listening` doivent tous deux
   apparaître (la vérification s’exécute une minute après le démarrage). « client is ready » seul ne prouve rien.
6. **Ne modifier la base de données de Synapse qu’avec `scripts/reset-bridge-keys.sh`** (qui sauvegarde d’abord), et
   seulement quand `crypto-doctor show` prouve que le magasin local de la passerelle est le bon.
7. **Consigner chaque incident** (date, symptôme, cause, correctif) et en transmettre la version courte aux
   répondants (voir `respondent-troubleshooting.md`) : les gens à qui on explique ce qui s’est passé restent.
8. **Tester comme un répondant après chaque correctif** : un nouveau clavardage depuis le site, avec réponse depuis un
   téléphone **et** un ordinateur.

## 2. Symptômes

| Ce que voit le répondant | Ce que cela signifie |
|---|---|
| Sa réponse affiche une marque rouge dans Element, le visiteur ne la reçoit jamais, rien dans le journal de la passerelle | La passerelle ne peut pas ouvrir de canal sécurisé avec son appareil (section 3) |
| La salle affiche « ⚠️ Le message de … n’a PAS été reçu » | La passerelle a reçu le message mais ne peut pas le déchiffrer : l’appareil de l’expéditeur n’a pas partagé sa clé (section 4) |
| Un seul de ses appareils fonctionne, ou la salle de vérification vérifie un appareil à la fois | La passerelle a perdu ses clés de signature croisée, ou ne fait pas confiance à sa propre identité (section 5) |
| Le clavardage du site dit « problème de connexion » | La passerelle ne sert pas le clavardage Web (chercher `HTTPS server listening` dans le journal) |

## 3. Les clés de la passerelle ne correspondent pas à celles de Synapse (le cas grave)

Le journal dit `CRYPTO CHECK FAILED` après le démarrage, ou `scripts/check-bridge-keys.sh` affiche des clés invalides,
ou la console d’Element montre `Error creating Olm session … InvalidSignature` pour l’appareil de la passerelle.

Cause : le magasin de chiffrement de `db/` a une autre clé d’appareil que celle que Synapse détient pour l’appareil de
la passerelle (par exemple `session` et `db/` ont été restaurés de moments différents).

**Correctif qui conserve la confiance de tout le monde : connecter la passerelle comme NOUVEL appareil et y copier son
identité de signature croisée existante.** Ne pas simplement remettre le bon magasin : l’Element des répondants
(surtout Element X sur téléphone) garde pour de bon en cache les anciennes clés d’un identifiant d’appareil et ne les
rafraîchit jamais; les appareils existants resteraient incapables de joindre la passerelle. Un nouvel identifiant
d’appareil n’a aucun cache périmé nulle part. (Les tests de bout en bout rejouent cet incident et vérifient les deux cas.)

1. Arrêter la passerelle. Sauvegarder (`scripts/backup-data.sh`).
2. Trouver un magasin qui contient encore les clés de signature croisée (`tools/crypto-doctor`) :
   ```
   crypto-doctor show sb-chat/session sb-chat/db/* sb-chat/*/db/*
   ```
   Chercher `cross-signing: master=true self-signing=true user-signing=true` et une `master key` égale à celle que
   Synapse détient pour le compte de la passerelle. Tout magasin du même compte convient; le magasin actuel l’a en général.
3. Garder l’ancienne session de côté et laisser la passerelle se reconnecter :
   ```
   mv sb-chat/session sb-chat/session.old
   docker compose up -d sb-chat        # se connecte comme nouvel appareil; le journal dit « NOT resetting it »
   docker compose stop sb-chat
   ```
   (Une nouvelle connexion ne réinitialise jamais une identité existante. Seul `BRIDGE_RESET_CROSS_SIGNING=true` le fait.)
4. Copier l’identité dans le nouveau magasin (le dossier du nouveau magasin est `db_name` dans le nouveau fichier `session`) :
   ```
   crypto-doctor import-identity sb-chat/session.old sb-chat/db/<ancien-magasin> sb-chat/session sb-chat/db/<nouveau-magasin>
   ```
5. Démarrer la passerelle. Après une minute, le journal doit dire `Crypto check: OK`, `the bridge holds its cross-signing keys`
   et `signed our own device`. `scripts/check-bridge-keys.sh @passerelle:… IDNOUVELAPPAREIL` doit afficher `valid=50 invalid=0`.
6. Tester comme un répondant (règle 8). Personne n’a rien à revérifier.

Les anciens identifiants d’appareil peuvent être retirés plus tard depuis la page du compte. Ce n’est que si aucun
magasin ne contient plus les clés de signature croisée qu’il faut réinitialiser celle de la passerelle
(`BRIDGE_RESET_CROSS_SIGNING=true`); c’est le seul cas où l’on demande aux gens de refaire confiance à la passerelle.

`scripts/reset-bridge-keys.sh` (sauvegarde et supprime les clés périmées de l’appareil de la passerelle sur Synapse,
redémarre Synapse) ne sert que dans le cas rare où le bon magasin est déjà en place et où seul Synapse détient des clés périmées.

## 4. « Message NON reçu » dans une salle

Les clés de la passerelle sont bonnes, mais l’appareil de l’expéditeur a chiffré avec une clé de salle qu’il n’a jamais
partagée avec la passerelle (courant juste après une réparation : les sessions créées avant ne sont jamais partagées de
nouveau). L’avertissement nomme l’appareil et la session de clé dans le journal. Correctif : dans Element Bureau, écrire
`/discardsession` dans la salle et renvoyer. Sur Element X (téléphones), ouvrir un nouveau clavardage (une nouvelle
salle a une nouvelle clé); si cela échoue encore, se déconnecter puis se reconnecter une fois sur ce téléphone.

## 5. Un seul appareil est approuvé à la fois

La passerelle ne fait confiance à tous les appareils d’une personne d’un coup que si elle détient ses propres clés de
signature croisée. `Crypto check: the bridge does NOT hold all its cross-signing keys` dans le journal signifie qu’elles
manquent dans le magasin : les rétablir avec `crypto-doctor import-identity` (section 3, étape 3). Personne n’a à
revérifier.

La passerelle doit aussi **faire confiance à sa propre identité** : une personne ne compte comme vérifiée que si l’identité de la
passerelle est approuvée et a signé la sienne. Les clés copiées par `crypto-doctor` arrivent non approuvées; la passerelle
répare cela d’elle-même au démarrage et avant chaque vérification (journal : `Crypto check: our own identity is trusted` ou
`… trusted it again`). Si le journal dit `our own identity is NOT trusted`, les clés de signature croisée manquent (voir plus haut).

## 6. Ce que la passerelle fait maintenant d’elle-même

- Avise le bénévole dans sa salle utilisateur quand une vérification a été interrompue (annulée, ou les deux côtés ont
  lancé la comparaison d’émojis au même instant, ce qui fait annuler les deux par Matrix) et qu’il suffit d’écrire de
  nouveau pour recommencer.
- Publie un avertissement bilingue dans la salle quand elle ne peut pas déchiffrer un message, avec la marche à suivre.
- Journalise l’appareil expéditeur et la session de clé de chaque tel message (jamais le contenu).
- Vérifie son identité une minute après chaque démarrage (lignes `Crypto check: …`) et journalise
  `CRYPTO CHECK FAILED` avec un renvoi à cette page quand le magasin et Synapse divergent.
- Démarre le clavardage Web même si son appareil apparaît « non signé »; l’ancien comportement d’attente exige
  `BRIDGE_WAIT_FOR_CROSS_SIGNING=true`.

## 7. Les tests qui prouvent que les répondants ne rencontreront pas ces problèmes

- **Avant chaque déploiement :** `scripts/predeploy-check.sh` (environ 25 minutes; `quick` pour une version plus courte).
  Il lance les tests unitaires, construit l’image et lance `tests/e2e` : un Synapse jetable dans Docker, des
  répondants simulés avec un vrai chiffrement de bout en bout (plusieurs appareils par personne, connexions neuves), le
  vrai module du site (`chat.js`) et un visiteur. Il couvre : un premier message et une réponse, deux appareils d’une
  même personne, le rechargement et le changement de page, toutes les commandes, la fin d’un clavardage (la salle
  disparaît pour chaque répondant), la saisie qui supprime les rappels, un redémarrage de la passerelle, le
  changement d’appareil de la passerelle (personne n’a rien à faire), une seule confirmation qui vérifie tous les
  appareils d’une personne, et un rejeu de cet incident avec la récupération ci-dessus. Voir `tests/e2e/README.md`.
- **Après chaque déploiement, et de temps en temps (cron) :** `scripts/prod-smoke.sh https://chat.exemple.org https://www.exemple.org vps sb-chat synapse @passerelle:matrix.exemple.org`
  vérifie les points d’accès en service, les lignes de démarrage du journal et les clés de la passerelle sur Synapse.
  Rien n’est créé sauf une session Web qui est terminée aussitôt.
- Une vérification côté répondant que personne ne peut automatiser : après une réparation, faire répondre à un
  clavardage d’essai un bénévole sur téléphone et un autre sur ordinateur.

## 8. Journal des incidents

| Date | Symptôme | Cause | Correctif |
|---|---|---|---|
| 2026-10-08 | Element d’un répondant ne joignait pas la passerelle; réponses perdues | Magasin et `session` restaurés de moments différents le 2026-10-06 | Bon magasin et clés de signature croisée remis en place, clés périmées réinitialisées sur Synapse |
| 2026-10-08 | Element X (téléphone) échouait encore : journal `Olm event doesn't contain a ciphertext for our key` | Le téléphone avait gardé en cache les anciennes clés de l’appareil de la passerelle et ne rafraîchit jamais les clés d’un appareil déjà connu | La passerelle s’est connectée comme nouvel appareil; clés de signature croisée copiées avec `crypto-doctor import-identity`; la passerelle a signé son nouvel appareil. Aucune réinitialisation d’identité |
| 2026-10-12 | Une bénévole sur Element X (iPhone) appuyait sur « Commencer la vérification » et cela tournait sans fin, à chaque essai | Element X lance lui-même la comparaison d’émojis pendant que la passerelle le fait aussi; le protocole garde celle de la bénévole (identifiant plus petit que celui de la passerelle), mais la passerelle avait cessé d’écouter après avoir lancé la sienne et n’y répondait jamais | La passerelle continue de suivre la demande et répond à la comparaison retenue; elle attend aussi 3 s (`VERIFY_START_DELAY_SECS`) pour laisser le téléphone commencer. Le test `elementx` rejoue la situation |
