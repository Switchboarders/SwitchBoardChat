[English](../law25.md) · **Français**

# Loi 25 (Québec) — notes sur la protection des renseignements personnels pour wp-chat et switchboard-chat

> Pour publier des textes sur votre site Web (section de la politique de confidentialité, avis du
> clavardage, formulaires), utilisez le modèle à adapter du dépôt wp-chat :
> `docs/fr/law25-website-template.md`.

Document de travail destiné à la personne responsable de la protection des renseignements personnels.
Il décrit ce que fait le logiciel. Il ne constitue pas un avis juridique : faites réviser par un
juriste les avis et l’évaluation des facteurs relatifs à la vie privée (EFVP) avant le lancement.

## 1. Quelles données existent, où, et pour combien de temps

| Donnée | Où | Forme | Conservée jusqu’à |
|---|---|---|---|
| Texte des messages du clavardage Web | Navigateur ↔ mémoire de switchboard-chat ↔ salle Matrix | Jamais écrit sur disque ni dans les journaux par la passerelle. La salle Matrix est chiffrée de bout en bout. | Fin du clavardage ou délai d’inactivité (`WEB_IDLE_TIMEOUT_MIN`, 48 h = 2880 min par défaut, depuis la dernière interaction). La conversation est aussi gardée en mémoire (300 dernières lignes) pour qu’un rechargement puisse la restaurer. La passerelle retire tous les répondants, quitte la salle et l’oublie. |
| Identifiant du visiteur Web | Jeton de session dans le navigateur du visiteur (`sessionStorage`, ou `localStorage` si le site choisit « tout le navigateur »; jamais dans un témoin), son empreinte SHA-256 dans la mémoire de la passerelle | Aléatoire, sans lien avec une personne | Fin du clavardage / 48 h sans activité. Perdu au redémarrage de la passerelle. |
| Liste de blocage (Web, visiteurs abusifs) | Base de données, table `web_block` | Empreinte à clé de l’adresse IPv4 du visiteur ou de son réseau IPv6 /64, et une date de fin. Aucun motif, aucun contenu, aucune adresse lisible. | Jusqu’à l’expiration du blocage (1 jour par défaut, 30 jours au plus selon `WEB_BAN_MAX_DAYS`); les lignes expirées sont supprimées chaque minute |
| Marqueur de blocage (Web, seulement après un blocage) | Le navigateur du visiteur bloqué (`localStorage`, clé `wpchat_blocked`) | Un marqueur signé « bloqué jusqu’à » : deux dates et une signature, aucun identifiant | Jusqu’à la fin du blocage; le visiteur peut le supprimer en tout temps |
| Numéro d’appelant fictif (Web) | Base de données, table `room` | Numéro aléatoire, haché + chiffré | Supprimé avec la salle |
| Nom de la salle Web | Nom de la salle Matrix vu par les répondants | Un prénom + une lettre + 4 caractères tirés d’une empreinte à clé de l’adresse réseau du visiteur (IPv4, ou IPv6 /64). Irréversible sans `PHONE_HASH_SECRET`; l’adresse elle-même n’est jamais conservée | Supprimé avec la salle |
| Texte des SMS | voip.ms → passerelle → salle Matrix | Non conservé par la passerelle, sauf dans une file de reprise (plus bas). La salle Matrix est chiffrée. | Salle supprimée après `SMS_IDLE_RETENTION_DAYS` (7 par défaut) sans texto de l’appelant |
| Numéro de téléphone de l’appelant | Base de données, table `room` | Empreinte à clé (recherche) + copie chiffrée AES-256-GCM (nécessaire pour répondre) | Supprimé avec la salle |
| Liste de désabonnement (STOP) | Base de données, table `denied_did` | Empreinte à clé seulement, date, manière | Conservée volontairement, pour que le désabonnement soit respecté. Elle ne permet d’identifier personne sans `PHONE_HASH_SECRET`. |
| Registre de l’avis de premier contact, compteurs de SMS automatiques | Base de données | Empreinte à clé seulement | Registre de l’avis : jusqu’à l’effacement. Compteurs : 7 jours. |
| Files de reprise | Base de données | Textos entrants en attente d’un répondant : 24 h au maximum. Sortants : 1 h au maximum. Le mot de passe voip.ms n’y est jamais stocké. | Purgées automatiquement |
| Mot de passe voip.ms | Base de données | Chiffré AES-256-GCM | Jusqu’à son changement |
| Journaux | `data/logs`, 7 fichiers quotidiens | Aucun texte de message, aucun numéro de téléphone, aucun mot de passe ni jeton. Les appelants apparaissent sous la forme d’un préfixe d’empreinte de 8 caractères. | 7 jours |
| Historique Matrix | Synapse | Chiffré. Les appareils des répondants en gardent des copies. | Supprimé et purgé de Synapse quand la passerelle supprime la salle, si elle a un accès administrateur à Synapse (voir « Purger les salles de Synapse » plus bas); sinon `retention` de Synapse (8 jours dans l’exemple de configuration) |

## 2. Ce que le logiciel ne fait plus (par rapport au parcours SMS d’origine)

- Les numéros de téléphone, la liste de désabonnement et les compteurs ne sont plus stockés en clair.
- Les salles et les correspondances de numéros ne sont plus conservées indéfiniment.
- La file de reprise ne contient plus le mot de passe voip.ms et est purgée selon une minuterie.
- Le contenu des messages, les numéros de téléphone, les jetons d’accès et les URL voip.ms ne sont plus
  écrits dans les journaux.
- `.env` n’est plus copié dans l’image Docker. Le client PHP d’exemple ne contient plus de jeton d’API.

## 3. Droits de la personne

| Demande | Comment |
|---|---|
| Effacement (« oubliez-moi ») | Un répondant écrit `!oubli` (ou `!forget`) dans la salle, ou un administrateur appelle la commande d’API `erase_caller` avec le numéro de l’appelant. Supprime la salle, le numéro chiffré et les enregistrements. Le robot quitte la salle; **chaque répondant doit aussi supprimer la salle dans son propre client**. L’entrée de désabonnement (empreinte) demeure. |
| Mettre fin à un clavardage Web | Le visiteur appuie sur « Terminer », ou un répondant écrit `!fin` / `!end`. |
| Accès / rectification | La passerelle ne tient aucun profil : seule la conversation en cours existe. Répondez à une demande d’accès à partir de la salle Matrix tant qu’elle existe. |
| Se désabonner des SMS | L’appelant envoie STOP (ARRET). Le répondant peut écrire `!stop`. |

## 4. Avis à présenter

- **Module Web :** le texte de consentement et la phrase sur la conservation s’affichent avant le
  premier message (Réglages → WP Chat). Indiquez le nom de la personne responsable de la protection
  des renseignements personnels et l’URL de la politique de confidentialité, et gardez la phrase sur la
  conservation conforme à `WEB_IDLE_TIMEOUT_MIN`.
- **SMS :** le premier SMS que reçoit chaque appelant indique que les textos sont supprimés après
  `SMS_IDLE_RETENTION_DAYS` jours et comment se désabonner. Précisez dans la politique de
  confidentialité que voip.ms et les fournisseurs de services mobiles traitent les SMS selon leur
  propre durée de conservation, que nous ne contrôlons pas.
- **Répondants :** ils ne doivent ni copier, ni capturer, ni exporter les conversations, doivent
  supprimer la salle sur demande et garder leur appareil Matrix verrouillé. Inscrivez-le dans
  l’entente avec les bénévoles.

## 5. Risques résiduels (non réglés par le logiciel)

1. **Les SMS transitent par voip.ms et les fournisseurs mobiles** (non chiffrés de bout en bout, copies
   conservées par eux).
2. **Les appareils des répondants** gardent l’historique déchiffré jusqu’à ce qu’ils suppriment la salle
   ou que la conservation de Synapse le purge.
3. **Les jetons d’API** sont stockés chiffrés (AES-256-GCM avec `PHONE_ENC_KEY`) et retrouvés par leur
   empreinte. Quiconque détient à la fois la base de données et `PHONE_ENC_KEY` peut encore les lire :
   protégez le fichier de la base et le `.env` (permissions, disque chiffré, sauvegardes chiffrées).
4. **Les sauvegardes** de la base contiennent ce qui existe au moment de la copie (empreintes et numéros
   chiffrés, aucun texte de message). Chiffrez les sauvegardes et faites-les expirer. Sauvegardez
   `PHONE_ENC_KEY` et `PHONE_HASH_SECRET` *séparément* de la base. Perdre `PHONE_ENC_KEY` rend les
   conversations ouvertes illisibles; changer `PHONE_HASH_SECRET` fait oublier tout le monde à la liste
   de désabonnement.
5. **Les anciens fichiers journaux** écrits avant ce changement contiennent encore des numéros et du texte
   de message. Supprimez `data/logs/*` une fois après la mise à niveau.
6. **Les sessions du clavardage Web vivent en mémoire** : un redémarrage y met fin (les salles sont
   purgées au démarrage).
7. **La mémoire des limites de débit** garde les adresses IP jusqu’à une heure (en mémoire seulement).

### Purger les salles de Synapse

Quitter et oublier une salle ne la supprime pas de Synapse : ses événements chiffrés restent dans la base
de Synapse, et le magasin cryptographique de la passerelle garde les clés qui les déchiffrent. Quand la
passerelle supprime une salle (effacement, fin de la conservation, fin d’un clavardage Web), elle demande
donc aussi à Synapse de supprimer la salle et d’en purger l’historique
(`DELETE /_synapse/admin/v2/rooms/{room_id}` avec `purge: true`). Il faut un accès administrateur à Synapse :

- **Avec MAS** : renseignez `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET` (voir `src/mas_admin.rs`) et
  faites de l’utilisateur du robot un administrateur MAS (`mas-cli manage promote-admin <robot>`). Pour chaque
  purge, la passerelle crée une session personnelle MAS au nom du robot avec la portée d’administration
  Synapse, valide 2 minutes, et la révoque dès que la purge est terminée.
- **Sans MAS** : faites de l’utilisateur du robot un administrateur du serveur Synapse.

La purge a lieu `ROOM_PURGE_DELAY_SECS` (120 par défaut) après le retrait des membres, pour que les
clients des répondants voient d’abord le retrait; un redémarrage pendant ce délai laisse cette salle non purgée.

Sans l’un ou l’autre, les salles sont seulement quittées et oubliées, et la passerelle l’indique une fois
dans le journal. Les clés des salles purgées restent dans le magasin cryptographique de la passerelle, mais
sans les événements elles ne déchiffrent plus rien.

## 6. Procédure en cas d’incident (aperçu)

1. Contenir : arrêtez la passerelle (`docker compose stop`); renouveler `PHONE_ENC_KEY` ne suffit *pas*
   à lui seul; renouvelez le mot de passe voip.ms, les jetons d’API et le mot de passe du robot Matrix.
2. Évaluer : ce qui a été exposé (les empreintes et les numéros chiffrés présentent peu de risque sans
   les clés; le contenu des salles Matrix est chiffré de bout en bout; le contenu des SMS transite par
   voip.ms).
3. Déterminer s’il existe un risque de préjudice sérieux : si oui, avisez **promptement** la Commission
   d’accès à l’information et les personnes concernées.
4. Consigner chaque incident de confidentialité dans le registre des incidents (obligatoire même s’il n’y
   a pas d’avis), puis corriger la cause et mettre à jour ce document.

## 7. Liste de vérification de l’évaluation des facteurs relatifs à la vie privée

- [ ] Finalité et nécessité consignées (aide aux personnes en rétablissement, anonyme, en tête-à-tête).
- [ ] Inventaire des données ci-dessus révisé et confirmé en faisant fonctionner le système.
- [ ] Consentement et avis révisés par un juriste, en français d’abord.
- [ ] Responsable de la protection des renseignements personnels nommé et publié.
- [ ] Entente de confidentialité des répondants signée (dans Element : [confidentiality-agreement.md](confidentiality-agreement.md); définir `PRIVACY_OFFICER`, suivre le registre dans SwitchBoard).
- [ ] Lieu d’hébergement de la passerelle, de Synapse et de WordPress documenté (hors Québec = évaluation du transfert).
- [ ] Sauvegardes : où, chiffrées, pendant combien de temps.
- [ ] Registre des incidents créé.
- [ ] Date de révision annuelle fixée.
