# End-to-end tests / Tests de bout en bout

**English** — Proves, before every deploy, that respondents will not have to deal with technical problems.
Everything runs in Docker on your machine: a throwaway Synapse, the bridge image under test, simulated
respondents (`respondent-sim`: real Matrix clients with real end-to-end encryption, one process per device)
and the real website widget (`chat.js` from the wp_chat plugin, in a fake DOM). Nothing touches production.

**Français** — Prouve, avant chaque déploiement, que les répondants n’auront pas à gérer de problèmes techniques.
Tout s’exécute dans Docker sur votre machine : un Synapse jetable, l’image de la passerelle à tester, des
répondants simulés (`respondent-sim` : de vrais clients Matrix avec un vrai chiffrement de bout en bout, un
processus par appareil) et le vrai module du site (`chat.js` du module wp_chat, dans un faux DOM). Rien ne touche la production.

## Run it / Lancer

```
scripts/predeploy-check.sh          # everything, ~25 min / tout, ~25 min
scripts/predeploy-check.sh quick    # short scenarios only / scénarios courts seulement
# by hand / à la main
docker build -t sbchat:e2e .
(cd tools/crypto-doctor && cargo build) && (cd tests/e2e/respondent-sim && cargo build)
python3 tests/e2e/run.py                       # all
python3 tests/e2e/run.py bringup roundtrip     # some scenarios (bringup first)
python3 tests/e2e/run.py --keep                # leave the containers up / laisser les conteneurs
```

Needs Docker, Python 3, Node 18+, Rust. The widget scenario needs the plugin repository
(`WPCHAT_REPO=/path/to/wp_chat`, default `../wp_chat` next to this repository, else `~/wp_chat`). Ports used: 18008 (Synapse) and 18088 (bridge), local only.

## Scenarios / Scénarios

| # | What it proves / Ce que cela prouve |
|---|---|
| bringup | a brand new bridge starts, holds its cross-signing keys, the startup crypto check passes |
| roundtrip | visitor → two respondents (fresh devices) read it, the welcome macro (`newcomerweb`) arrives, the ping is posted, a reply reaches the visitor, no undecryptable message |
| second_device | the same person on a phone and a computer, both can read and answer |
| reload | history, resume after the last line (`Last-Event-ID`), no duplicate, a line written between two pages is delivered |
| widget | the real `chat.js`: consent, start, reply, welcome, page change restores the chat, no duplicate, end erases the saved session |
| commands | `!help`, `!aide`, unknown command, `#macro`, `!stop`, `!ok` (others removed) always answer visibly |
| end | `!fin` and the visitor's end both remove the room for every respondent |
| typing | a chat where someone types gets no reminder; the quiet one does (takes ~90 s) |
| restart | the bridge restarts: chats keep working, crypto check OK |
| stray_device | a device of the bridge is left unsigned (an earlier login): the startup check names it (in Element X it blocks volunteers' messages: "has unverified devices"); once it is signed out the check is clean |
| verification | one emoji confirmation verifies a person; the bridge then says ALL their devices are verified |
| elementx | a volunteer's phone (Element X) starts the emoji comparison itself; the bridge answers it and the verification completes (incident of 2026-10-12) |
| interrupted | a volunteer cancels a verification halfway: the bridge tells her how to restart, and writing again completes it |
| agreement | a volunteer reads (`!entente`) and signs (`!signer Name`) the confidentiality agreement in her user room, gets a receipt; SwitchBoard lists who signed and "send now" reaches only those who have not |
| shortnames | SwitchBoard sends respondents as `@carol` (no homeserver): the bridge completes them and they still get the chats |
| newdevice | the bridge logs in as a new device (no identity reset, cross-signing copied with `crypto-doctor`): old devices, other respondents and a brand new device all keep working |
| incident | replay of the 6 October incident: store and Synapse out of step → loud log, key check fails, a respondent is told their message was NOT received, then the runbook (`docs/encryption-recovery.md`) fixes it for old and new devices |

A scenario that cannot start (Docker, ports) is reported as a failure, never skipped silently.
Add a scenario: write a `sc_…` function in `run.py` using `Sim` (respondent device), `Visitor` (website) and
`R.check(name, condition, detail)`, and list it in `SCENARIOS`.
