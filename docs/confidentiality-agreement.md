# Volunteers' confidentiality agreement (signed in Element)

Version française : [fr/confidentiality-agreement.md](fr/confidentiality-agreement.md)

Volunteers sign the confidentiality agreement directly in Element, with no printer and no extra account. Under Quebec
law a signature only has to identify the person and show their consent, and an electronic signature is valid when the
link between the person and the document can be shown (Civil Code of Québec art. 2827; Act to establish a legal
framework for information technology s. 39). Have a lawyer review the **text** of the agreement.

## How a volunteer signs

1. In Element, open the room **"User room / Salle Utilisateur"** (the bridge's private room with them).
2. Write `!entente` (or `!agreement`) to read it. The bridge also sends it on its own, every day at noon, until they sign.
3. Write `!signer` followed by their first and last name, e.g. `!signer Marie Tremblay` (English: `!sign Marie Tremblay`).
4. The bridge answers with a **receipt**: name, account, device, date, version, the text's fingerprint and a reference.
   That message is their copy.

## What is kept as proof

One row per signature in the bridge database (`agreement_signature`), never changed afterwards:
Matrix account, typed name, Matrix server timestamp, the message's event ID, the device and whether the bridge had
verified it, the version label and the **SHA-256 fingerprint of the exact text shown** (French then English, with the
privacy officer filled in). Any change to the text changes the fingerprint, so everyone is asked to sign again.
Signatures stay when a volunteer leaves the schedule (the proof must outlive their volunteering).
Retention stated in the agreement: during volunteering and 3 years after; delete older rows by hand for now.

## Configuration (bridge `.env`)

| Variable | Purpose |
|---|---|
| `PRIVACY_OFFICER` | **Required** to turn the agreement on: how to reach the person in charge of the protection of personal information, e.g. `vie.privee@example.org` (a generic address keeps the person anonymous; the text already says who it is for). It is written into the text. |
| `AGREEMENT_VERSION` | Label shown to volunteers (default `2026-10-13`). Change it when you change the text. |
| `AGREEMENT_FR_FILE`, `AGREEMENT_EN_FILE` | Paths (inside the container) to your own text, replacing the built-in one (`src/agreement/entente.*.md`). Use `{RESPONSABLE}` where the privacy officer goes. |

## SwitchBoard

*SwitchBoard Chat → Entente de confidentialité des bénévoles* lists every volunteer of the account with their status
(signed / old version / to sign), typed name, date, device and version; it sends the agreement now to those who have not
signed and exports the register as CSV (with fingerprints and references, for an audit).
