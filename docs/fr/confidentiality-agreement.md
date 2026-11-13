# Entente de confidentialité des bénévoles (signée dans Element)

English version: [../confidentiality-agreement.md](../confidentiality-agreement.md)

Les bénévoles signent l'entente de confidentialité directement dans Element, sans imprimante ni compte de plus. En
droit québécois, une signature doit seulement identifier la personne et montrer son consentement, et une signature
électronique est valide quand on peut établir le lien entre la personne et le document (Code civil du Québec,
art. 2827; Loi concernant le cadre juridique des technologies de l'information, art. 39). Faites relire le **texte** de
l'entente par un juriste.

## Comment un bénévole signe

1. Dans Element, ouvrir la salle **« User room / Salle Utilisateur »** (la salle privée de la passerelle avec lui).
2. Écrire `!entente` pour la lire. La passerelle l'envoie aussi d'elle-même chaque jour à midi tant qu'elle n'est pas signée.
3. Écrire `!signer` suivi de son prénom et de son nom, par exemple `!signer Marie Tremblay`.
4. La passerelle répond par un **reçu** : nom, compte, appareil, date, version, empreinte du texte et référence. Ce
   message est sa copie.

## Ce qui est gardé comme preuve

Une ligne par signature dans la base de la passerelle (`agreement_signature`), jamais modifiée ensuite : compte Matrix,
nom tapé, heure du serveur Matrix, identifiant du message, appareil et s'il était vérifié par la passerelle, version et
**empreinte SHA-256 du texte exact affiché** (français puis anglais, avec la personne responsable inscrite). Toute
modification du texte change l'empreinte : tout le monde est alors invité à signer de nouveau. Les signatures restent
quand un bénévole quitte l'horaire (la preuve doit survivre à son bénévolat). Conservation prévue dans l'entente : pendant
le bénévolat et 3 ans après; pour l'instant, supprimer les lignes plus anciennes à la main.

## Configuration (`.env` de la passerelle)

| Variable | Rôle |
|---|---|
| `PRIVACY_OFFICER` | **Obligatoire** pour activer l'entente : comment joindre la personne responsable de la protection des renseignements personnels, par ex. `vie.privee@exemple.org` (une adresse générique garde la personne anonyme; le texte précise déjà à qui elle s'adresse). Inscrit dans le texte. |
| `AGREEMENT_VERSION` | Étiquette montrée aux bénévoles (défaut `2026-10-13`). À changer quand le texte change. |
| `AGREEMENT_FR_FILE`, `AGREEMENT_EN_FILE` | Chemins (dans le conteneur) de votre propre texte, qui remplace celui intégré (`src/agreement/entente.*.md`). Mettre `{RESPONSABLE}` à l'endroit de la personne responsable. |

## SwitchBoard

*SwitchBoard Chat → Entente de confidentialité des bénévoles* liste les bénévoles du compte avec leur statut (signée /
ancienne version / à signer), le nom tapé, la date, l'appareil et la version; elle envoie l'entente tout de suite à ceux
qui ne l'ont pas signée et exporte le registre en CSV (avec empreintes et références, pour une vérification).
