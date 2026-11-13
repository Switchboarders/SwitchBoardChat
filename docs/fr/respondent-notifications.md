[English](../respondent-notifications.md) · **Français**

# Avertir les répondants des conversations nouvelles et en attente

Une nouvelle conversation (clavardage Web ou SMS) est une salle que la passerelle crée et à laquelle elle
**invite** les répondants actifs. Une invitation seule ne produit souvent **aucune notification** : cela
dépend de l’enregistrement des notifications poussées (push) de chaque téléphone et des réglages du client,
et un répondant qui n’a pas joint la salle n’est pas averti des messages qui suivent. Ce guide explique ce
que la passerelle fait à ce sujet, le réglage de serveur à activer, et comment vérifier l’appareil de
chaque répondant.

## 1. Ce que fait la passerelle

1. **Avis de nouvelle conversation.** Après avoir créé une salle, la passerelle attend (jusqu’à 20 s) que
   les répondants invités la joignent, puis publie un court message bilingue qui **mentionne** chaque
   répondant qui l’a jointe : `🔔 Nouveau clavardage Web en attente. / New web chat waiting.` (ou
   « conversation SMS »). Une mention est une *mise en évidence* : elle notifie avec un son, même si la
   salle est réglée sur « mentions seulement ». Le message **énumère aussi les répondants par leur nom** sur
   une deuxième ligne (`👥 Alice, Bob, …`, nom d’affichage ou `@nom`; pastilles cliquables dans Element, 15
   noms au plus, puis `+N`), ce qui rend la notification plus facile à repérer sur un écran verrouillé.
2. **Rappels.** Une conversation est **en attente** dès l’arrivée d’un message d’un visiteur ou d’un appelant
   jusqu’à ce qu’un répondant **tape ou écrive** dans la salle. Tant qu’elle attend, la passerelle publie
   `⏰ Personne n’a répondu depuis 1 minute. / Nobody has answered for 1 minute.` après **1 minute,
   2 minutes et 5 minutes**, puis **chaque heure** (1 h, 2 h, 3 h …, 24 rappels horaires au plus par
   défaut). Chaque rappel mentionne les répondants qui ont joint la salle. **Aucun avis ni rappel n’est publié
   pendant qu’un répondant tape dans la salle** (il reste dû et est publié plus tard si personne ne répond).
3. **Cela s’arrête** dès qu’un répondant tape ou envoie un message dans la salle (une commande comme `!ok`
   compte), quand le clavardage est terminé ou supprimé, ou quand la durée de conservation retire la salle.
   *Ouvrir simplement la salle ne compte pas* : un répondant doit taper ou écrire quelque chose.
4. **Les messages de suite dans une conversation déjà prise en charge** ne relancent pas les rappels rapides : dès
   qu’un répondant a tapé ou écrit dans la salle, un nouveau message de l’appelant ou du visiteur ne reçoit qu’un rappel
   discret (`⏰ Le dernier message attend une réponse depuis 10 minutes. / The last message has been waiting for an
   answer for 10 minutes.`) après `ATTENTION_FOLLOWUP_MIN` minutes (10 par défaut) sans réponse, puis chaque heure.
   Auparavant, un échange en direct avertissait tout le monde dès qu’une réponse prenait plus d’une minute.
5. Rien n’est publié tant qu’**il n’y a que le robot** dans la salle (il n’y a personne à avertir); le
   premier avis est tout de même publié, pour qu’il soit là quand un répondant accepte l’invitation.

Les messages ne contiennent **aucun renseignement personnel** : aucun texte de visiteur, numéro ni nom.

## 2. Réglage du serveur : faire joindre les salles automatiquement

Les notifications de messages n’existent que pour une salle que le répondant a **jointe**. Avec Synapse
(version 1.109 ou plus récente), laissez les utilisateurs locaux accepter automatiquement les invitations.
Dans `homeserver.yaml` :

```yaml
auto_accept_invites:
    enabled: true
    only_for_direct_messages: false
    only_from_local_users: true
```

Redémarrez Synapse. Effets à connaître : tout utilisateur **local** peut ajouter un autre utilisateur local
à une salle sans lui demander; la liste des salles des répondants se remplit de chaque nouvelle conversation
(ils les quittent ou les suppriment comme avant). Si vous utilisez un autre serveur Matrix, employez son
équivalent, ou faites accepter les invitations rapidement par les répondants (l’avis et les rappels ne sont
alors visibles qu’après l’acceptation).

## 3. Réglages de la passerelle (`.env`)

| Variable | Défaut | Signification |
|---|---|---|
| `ATTENTION_ENABLED` | `true` | `false` désactive l’avis et les rappels |
| `ATTENTION_JOIN_WAIT_SECS` | `20` | durée d’attente, avant le premier avis, pour que les répondants joignent une nouvelle salle |
| `ATTENTION_HOURLY_MAX` | `24` | nombre de rappels horaires après ceux de 1, 2 et 5 minutes (`0` = aucun) |

Redémarrez la passerelle après les avoir modifiés.

## 4. Vérifier l’appareil de chaque répondant

La passerelle et le serveur ne peuvent avertir qu’un appareil **enregistré pour les notifications poussées**.
Demandez à chaque répondant de vérifier :

**Element X sur Android**
- Installez-le depuis le **Google Play** (il s’enregistre alors auprès du service de notifications de
  Google), **ou**, sur un téléphone sans services Google (Huawei, Android sans Google) ou avec une version de
  F-Droid / GitHub, installez un fournisseur **UnifiedPush** comme **ntfy** et choisissez-le dans Element X :
  Paramètres → Notifications → *Fournisseur de notifications poussées*.
- Paramètres → Notifications : *Activer les notifications sur cet appareil* activé; le réglage par défaut des
  **discussions de groupe** devrait être *Tous les messages* (ou au moins *Mentions et mots-clés*; les
  messages de la passerelle sont des mentions); *Invitations aux salons* activé.
- Paramètres Android → Applications → Element X : notifications autorisées; batterie **Sans restriction**;
  le mode Ne pas déranger ne doit pas le bloquer.

**Element X sur iOS :** Réglages → Notifications → activées pour cet appareil et pour les discussions de
groupe; les modes de concentration (Focus) d’iOS doivent autoriser Element X.

**Element Desktop (Linux, Windows, macOS) :** Paramètres → Notifications → *Activer les notifications de
bureau* et *Activer les notifications sonores*; le système d’exploitation doit autoriser les notifications
pour Element (sous Linux, un démon de notifications doit être en marche); gardez Element en marche (zone de
notification), car l’application de bureau n’avertit que pendant qu’elle fonctionne. Un client de bureau est
un bon complément, pas un remplacement du téléphone.

**Vérifier depuis le serveur** (administrateur). Un téléphone n’apparaît dans la table `pushers` que s’il
s’est enregistré pour les notifications poussées :

```sql
-- Synapse avec SQLite ou PostgreSQL
select user_name, app_id, device_display_name from pushers order by user_name;
```

Identifiants d’application typiques : Element (ancienne application Android) `im.vector.app.android`,
Element X sur iOS `io.element.elementx.ios.prod`; Element X sur Android enregistre son propre identifiant
(il commence par `io.element.android`). Un répondant dont l’appareil Element X n’a **aucune ligne** n’est pas
enregistré : corrigez d’abord cet appareil (ci-dessus). Les enregistrements se font au démarrage de
l’application; rouvrez-la donc après avoir changé de fournisseur de notifications.

## 5. Essai

1. Démarrez un clavardage Web (ou envoyez un texto au numéro SMS) pendant que les téléphones des
   répondants sont verrouillés.
2. Quelques secondes après qu’ils ont joint la salle, l’avis arrive (avec un son).
3. Ne faites rien : les rappels suivent après 1, 2 et 5 minutes. Tapez un caractère dans la salle : ils
   s’arrêtent.

## 6. Dépannage

| Symptôme | Cause / correctif |
|---|---|
| Aucune notification sur un téléphone | l’appareil n’est pas enregistré pour les notifications poussées (section 4) : fournisseur, autorisations, réglages de batterie; vérifiez la table `pushers` |
| Avis publié, mais pas de son / rien sur un téléphone | canal de notifications de cette application, Focus / Ne pas déranger; réglage d’Element X pour les discussions de groupe; le téléphone n’est pas enregistré |
| L’avis n’apparaît qu’après que le répondant a accepté l’invitation | `auto_accept_invites` est désactivé ou non appliqué (section 2), ou le répondant n’est pas un utilisateur *local* |
| Les rappels continuent alors que quelqu’un s’en occupe | personne n’a tapé ni écrit dans la salle; l’ouvrir ne suffit pas. Tapez, ou envoyez un message comme `!ok` |
| Trop de rappels / salles SMS sans réponse | réduisez `ATTENTION_HOURLY_MAX` (ou `0`), ou `ATTENTION_ENABLED=false` |
| Le bureau n’affiche aucune notification | l’application est fermée, les notifications de bureau sont désactivées, ou aucun démon de notifications (Linux) |
