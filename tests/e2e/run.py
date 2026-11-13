#!/usr/bin/env python3
"""End-to-end tests of the web chat bridge against a throwaway Synapse (nothing here touches production).

    tests/e2e/run.py [scenario ...]      (no argument = every scenario)

What it does: starts Synapse + the bridge image in Docker, registers simulated respondents (the
`respondent-sim` crate: real Matrix clients with real end-to-end encryption), drives a visitor through the
same HTTP protocol as the website widget, and checks what each side sees. See tests/e2e/README.md.
"""
import json
import sys
import time

import lib
from lib import (BRIDGE, COMPOSE, DOCTOR, REPO, SERVER, WORK, Report, Sim, Visitor, bridge_logs, bridge_running, compose,
                 in_synapse_image, register, seed_call_center, sh, start_synapse, synapse_config)

R = Report()
ALICE = f"@alice:{SERVER}"
BOB = f"@bob:{SERVER}"
WELCOME = "Bienvenue sur le clavardage, un bénévole va vous répondre."
MERCI = "Merci de votre message, nous sommes là."
NOBODY = "Personne n est disponible pour le moment. Appelez le 555-0199."
MACROS = [("accueil-web", WELCOME, "newcomerweb"), ("merci", MERCI, "")]


def start_bridge(timeout=240):
    """Starts the bridge and waits until the web server answers. A brand new bot resets its identity once and exits."""
    compose("up", "-d", "--no-build", "bridge")
    deadline = time.time() + timeout
    restarts = 0
    while time.time() < deadline:
        logs = bridge_logs()
        running = bridge_running()
        done = any(t in logs for t in ("was reset", "no cross-signing reset was needed", "task failed", "NOT resetting"))
        resetting = "ENCRYPTION IDENTITY RESET" in logs and not done
        if running and "HTTPS server listening" in logs and not resetting:
            # the web server is up; make sure it really answers
            try:
                v = Visitor().start()
                v.end()
                return logs
            except Exception:
                pass
        if not running and restarts < 3:
            restarts += 1
            compose("up", "-d", "--no-build", "bridge")
        time.sleep(3)
    raise RuntimeError("bridge did not come up:\n" + bridge_logs()[-3000:])


def utds(*sims, room=None):
    """Undecryptable messages seen by the simulated devices (optionally only in one room: a new device can never
    read the history from before it existed, and that is expected)."""
    return [(s.name, e.get("sender")) for s in sims for e in s.events if e["event"] == "utd" and (room is None or e.get("room") == room)]


def wait_log(text, timeout=120):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if text in bridge_logs():
            return True
        time.sleep(2)
    return False


def chat(sims, first="Allo, j'ai besoin de parler"):
    """A visitor starts a chat; every sim in `sims` should be invited. Returns (visitor, room_id, marks)."""
    marks = [s.mark() for s in sims]
    v = Visitor().start()
    v.open_stream()
    status, body = v.say(first)
    R.check("visitor message accepted", status == 200, f"{status} {body}")
    room = None
    for s, m in zip(sims, marks):
        r = s.wait_room(m, 60)
        room = room or r
    return v, room, marks


# ---------------------------------------------------------------------------------------------------- scenarios

def sc_bringup():
    R.section("1. Bring-up: fresh Synapse, fresh bridge")
    compose("down", "-v", "--remove-orphans", check=False)
    lib.reset_work()
    synapse_config()
    start_synapse()
    for u in ("alice", "bob", "carol", "anna", "dora"):
        register(u)
    # Server admins: "bot" so the bridge can purge deleted rooms from Synapse (without MAS it uses its own
    # token), "admin" for the checks below
    register("bot", admin=True)
    register("admin", admin=True)
    R.check("synapse up and users registered", True)
    compose("up", "-d", "--no-build", "bridge")
    # the first start creates the database and the bot's identity
    time.sleep(20)
    seed_call_center(MACROS)
    start_bridge()
    logs = bridge_logs()
    R.check("web server answers", True)
    R.check("startup crypto check reports OK", wait_log("Crypto check: OK", 120), "no 'Crypto check: OK' line")
    R.check("bridge holds its cross-signing keys", wait_log("the bridge holds its cross-signing keys", 150))
    R.check("no CRYPTO CHECK FAILED", "CRYPTO CHECK FAILED" not in bridge_logs())


STATE = {}


def sc_roundtrip():
    R.section("2. A visitor writes, two respondents see it, one answers (fresh devices)")
    alice = Sim("alice", "alice-phone", cross_sign=True)
    bob = Sim("bob", "bob-laptop")
    STATE["alice"], STATE["bob"] = alice, bob
    ma, mb = alice.mark(), bob.mark()
    v, room, _ = chat([alice, bob])
    R.check("room created and Alice joined", room is not None)
    R.check("Alice reads the visitor's first message", alice.wait_message("Allo", ma, 60) is not None)
    R.check("Bob reads the visitor's first message", bob.wait_message("Allo", mb, 60) is not None)
    R.check("no undecryptable message for Alice or Bob", not [e for e in alice.events + bob.events if e["event"] == "utd"])
    R.check("the 'newcomerweb' macro reaches the visitor automatically", v.wait_text(WELCOME, 30) is not None, str(v.texts()))
    R.check("the room tells respondents how the chat works", alice.wait_message("!aide", ma, 20) is not None or alice.wait_message("!help", ma, 20) is not None)
    time.sleep(5)  # attention ping after respondents joined
    R.check("respondents get a 'new web chat waiting' ping", alice.wait_message("New web chat", ma, 20) is not None or alice.wait_message("Nouveau clavardage", ma, 20) is not None)
    m = alice.mark()
    sent = alice.send(room, "Bonjour, je suis là pour vous écouter.")
    R.check("Alice's reply is sent", sent and sent["ok"], str(sent))
    R.check("the visitor receives Alice's reply", v.wait_text("je suis là pour vous écouter", 30) is not None, str(v.texts()))
    R.check("no 'could not be decrypted' warning in the bridge log", "could not be decrypted" not in bridge_logs())
    STATE["visitor"], STATE["room"] = v, room
    return v, room


def sc_second_device():
    R.section("3. The same respondent on a second device (phone + computer)")
    v, room = STATE["visitor"], STATE["room"]
    alice2 = Sim("alice", "alice-computer")
    STATE["alice2"] = alice2
    m = alice2.mark()
    # a new chat so the second device is part of the room from the start
    v2, room2, _ = chat([alice2], "Deuxième conversation")
    R.check("second device joined the new room", room2 is not None)
    R.check("second device reads the message", alice2.wait_message("Deuxième", m, 60) is not None, f"utd={utds(alice2)} events={[e['event'] for e in alice2.events[m:]]}")
    sent = alice2.send(room2, "Réponse depuis l'ordinateur")
    R.check("second device's reply sent", sent and sent["ok"], str(sent))
    R.check("visitor receives the computer's reply", v2.wait_text("Réponse depuis l'ordinateur", 30) is not None, str(v2.texts()))
    alice = STATE["alice"]
    m = alice.mark()
    sent = alice.send(room2, "Réponse depuis le téléphone")
    R.check("first device replies in the same room", sent and sent["ok"], str(sent))
    R.check("visitor receives the phone's reply", v2.wait_text("Réponse depuis le téléphone", 30) is not None, str(v2.texts()))
    STATE["visitor2"], STATE["room2"] = v2, room2


def sc_reload():
    R.section("4. The visitor reloads or changes page (history + resume)")
    v, room = STATE["visitor"], STATE["room"]
    status, h = v.history()
    texts = [m["text"] for m in h.get("messages", [])] if status == 200 else []
    R.check("history lists both sides in order", status == 200 and "Allo, j'ai besoin de parler" in texts[0] and any("écouter" in t for t in texts), str(texts))
    R.check("history marks who wrote what", {m["from"] for m in h.get("messages", [])} == {"me", "them"})
    last = h.get("last", 0)
    v.close_stream()                                      # the page is gone
    alice = STATE["alice"]
    sent = alice.send(room, "Message pendant que vous changiez de page")
    R.check("respondent can write while the visitor is between pages", sent and sent["ok"])
    time.sleep(2)
    v.events.clear()
    v.open_stream(last_event_id=last)                    # the next page opens the stream, resuming after the last line shown
    R.check("the line written meanwhile is delivered on reconnect", v.wait_text("changiez de page", 20) is not None, str(v.texts()))
    R.check("lines already shown are not repeated", not [t for t in v.texts() if "je suis là pour vous écouter" in t], str(v.texts()))
    status, h2 = v.history()
    R.check("history still complete after the reconnection", any("changiez de page" in m["text"] for m in h2.get("messages", [])))
    status, _ = Visitor().history()
    R.check("history without a session is refused (401)", status == 401, str(status))


def sc_commands():
    R.section("5. Commands typed by respondents get a visible answer")
    v, room = STATE["visitor2"], STATE["room2"]
    alice, bob, alice2 = STATE["alice"], STATE["bob"], STATE["alice2"]
    m = alice.mark()
    alice.send(room, "!help")
    R.check("!help answers in the room", alice.wait_message("!ok", m, 30) is not None)
    m = alice.mark()
    alice.send(room, "!aide")
    R.check("!aide answers in French", alice.wait_message("Commandes", m, 30) is not None or alice.wait_message("commandes", m, 30) is not None)
    m = alice.mark()
    alice.send(room, "!nimportequoi")
    R.check("an unknown command is answered (never silent)", alice.wait_message("inconnue", m, 30) is not None or alice.wait_message("Unknown", m, 30) is not None)
    m = alice.mark()
    alice.send(room, "#merci")
    R.check("a macro is delivered to the visitor", v.wait_text(MERCI, 30) is not None, str(v.texts()))
    R.check("the room confirms the macro", alice.wait_message("succès", m, 30) is not None)
    m = alice.mark()
    alice.send(room, "!stop")
    R.check("!stop on a web chat explains it does not apply", alice.wait_message("ne s'applique pas", m, 30) is not None or alice.wait_message("does not apply", m, 30) is not None)
    # !ok: private room, others removed
    mb = bob.mark()
    m = alice.mark()
    alice.send(room, "!ok")
    R.check("!ok is acknowledged in the room", alice.wait_message("Salle privée", m, 30) is not None or alice.wait_message("Private room", m, 30) is not None)
    R.check("!ok removes the other respondents from the room", bob.wait(lambda e: e["event"] == "removed" and e["room"] == room, 40, mb) is not None)
    R.check("the visitor can still be answered after !ok", (alice.send(room, "Seulement moi maintenant") or {}).get("ok") and v.wait_text("Seulement moi", 30) is not None)


def sc_end_deletes_room():
    R.section("6. Ending a chat removes the room from every respondent")
    v, room = STATE["visitor2"], STATE["room2"]
    alice, alice2 = STATE["alice"], STATE["alice2"]
    ma, m2 = alice.mark(), alice2.mark()
    alice.send(room, "!fin")
    R.check("the visitor is told the chat is over", v.wait_event("closed", 30))
    R.check("the room is removed for the answering respondent", alice.wait(lambda e: e["event"] == "removed" and e["room"] == room, 60, ma) is not None)
    time.sleep(3)
    joined = [r for r in alice.rooms() if r["room"] == room and r["state"] in ("Joined", "Invited")]
    R.check("the room is no longer in the respondent's room list", not joined, str(joined))
    status, _ = v.history()
    R.check("the ended session is gone (401)", status == 401, str(status))
    admin_token = lib.synapse_admin_token()
    deadline = time.time() + 60
    while lib.synapse_room_exists(room, admin_token) and time.time() < deadline:
        time.sleep(2)
    R.check("the room and its history are purged from Synapse (Law 25)", not lib.synapse_room_exists(room, admin_token))
    # a chat the visitor ends himself
    v3, room3, _ = chat([alice, STATE["bob"]], "Je veux juste dire merci")
    alice.wait_message("merci", 0, 60)
    mb = STATE["bob"].mark()
    v3.end()
    R.check("a visitor-ended chat is also removed for the respondents", alice.wait(lambda e: e["event"] == "removed" and e["room"] == room3, 60) is not None)


def sc_typing():
    R.section("7. No reminder while a respondent is typing")
    alice, bob = STATE["alice"], STATE["bob"]
    ma, mb = alice.mark(), bob.mark()
    quiet, room_q, _ = chat([alice, bob], "Personne ne tape ici")
    typing, room_t, _ = chat([alice, bob], "Quelqu'un tape ici")
    R.check("two chats open", room_q and room_t and room_q != room_t)
    stop_at = time.time() + 85
    seen_quiet = seen_typing = False
    last_typing = 0
    while time.time() < stop_at:
        if time.time() - last_typing > 15:
            alice.cmd(cmd="typing", room=room_t, on=True)          # keeps the typing notice alive
            last_typing = time.time()
        time.sleep(3)
        for ev in bob.events[mb:]:
            if ev["event"] == "message" and "répondu depuis" in ev["body"] or ev["event"] == "message" and "Nobody has answered" in ev["body"]:
                if ev["room"] == room_q:
                    seen_quiet = True
                if ev["room"] == room_t:
                    seen_typing = True
    alice.cmd(cmd="typing", room=room_t, on=False)
    R.check("the quiet chat gets its 1-minute reminder", seen_quiet)
    R.check("the chat where someone types gets none", not seen_typing)
    quiet.end()
    typing.end()


def sc_restart():
    R.section("8. The bridge restarts: chats still work, nothing is asked of respondents")
    alice = STATE["alice"]
    compose("restart", "bridge")
    start_bridge()
    R.check("crypto check OK after restart", wait_log("Crypto check: OK", 150))
    R.check("the bridge trusts its own identity", wait_log("our own identity is trusted", 30) or "trusted it again" in bridge_logs())
    m = alice.mark()
    v, room, _ = chat([alice], "Après le redémarrage")
    R.check("Alice reads a message after the restart", alice.wait_message("Après le redémarrage", m, 60) is not None)
    sent = alice.send(room, "Je réponds après le redémarrage")
    R.check("reply works after the restart", sent and sent["ok"] and v.wait_text("après le redémarrage", 30) is not None, str(v.texts()))
    v.end()


def delete_device(user, device_id):
    """Deletes a device with the user's password (this rig's Synapse has no MAS; in production MAS does it)."""
    import urllib.error
    import urllib.request

    def call(method, path, body, token):
        req = urllib.request.Request(lib.SYNAPSE + path, data=json.dumps(body).encode(), method=method,
                                     headers={"Content-Type": "application/json", **({"Authorization": "Bearer " + token} if token else {})})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}")

    _, login = call("POST", "/_matrix/client/v3/login", {"type": "m.login.password", "identifier": {"type": "m.id.user", "user": user},
                                                       "password": lib.PASSWORD, "initial_device_display_name": "e2e cleanup"}, None)
    token = login["access_token"]
    _, challenge = call("DELETE", f"/_matrix/client/v3/devices/{device_id}", {}, token)
    status, _ = call("DELETE", f"/_matrix/client/v3/devices/{device_id}", {"auth": {
        "type": "m.login.password", "identifier": {"type": "m.id.user", "user": user}, "password": lib.PASSWORD,
        "session": challenge.get("session")}}, token)
    call("POST", "/_matrix/client/v3/logout", {}, token)
    return status == 200


def logs_since(since, text, timeout=150):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if text in bridge_logs(since):
            return True
        time.sleep(3)
    return False


def sc_stray_device():
    R.section("8b. A device of the bridge is left unsigned: the bridge says so loudly, and is clean again once it is gone")
    # Reported 2026-11: a volunteer's second phone could not write in her user room ("VoipSMS has unverified
    # devices" in Element X, red message, nothing reached the bridge) until she pressed "Send anyway". Element X
    # blocks messages to a verified person who has a device that is not cross-signed, e.g. left by an earlier login.
    stray = Sim("bot", "stray-bridge-device", settle=2, auto_verify=False)
    stray_id = stray.device
    stray.quit()
    R.check("an extra, unsigned device of the bridge exists", bool(stray_id), str(stray_id))
    since = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    compose("restart", "bridge")
    start_bridge()
    R.check("the startup check names the bridge's unsigned device", logs_since(since, "not cross-signed") and stray_id in bridge_logs(since),
            f"device {stray_id}")
    # Here an admin signs it out (in production the bridge does it through MAS when MAS_ADMIN_CLIENT_ID is set)
    R.check("the stray device is signed out", delete_device("bot", stray_id))
    since = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    compose("restart", "bridge")
    start_bridge()
    R.check("then every device of the bridge is cross-signed again", logs_since(since, "every device of the bridge is cross-signed"))
    R.check("and the bridge no longer complains", "not cross-signed" not in bridge_logs(since))


def stop_bridge():
    compose("stop", "bridge", check=False)


def copy_data(dst_name):
    in_synapse_image(f"rm -rf /w/{dst_name} && mkdir -p /w/{dst_name} && cp -a /w/db /w/session /w/{dst_name}/", [(WORK / "bridge", "/w")])


def switch_bridge_device():
    """The recovery that keeps everyone's trust: log the bridge in as a new device, then copy the existing
    cross-signing identity into its new store (docs/encryption-recovery.md). Returns (old_device, new_device)."""
    old_device = lib.bridge_device()
    stop_bridge()
    copy_data("backup-before-newdevice-%d" % int(time.time()))
    lib.own_files()
    old_session = WORK / "bridge" / "session.old"
    (WORK / "bridge" / "session").rename(old_session)
    old_name = json.loads(old_session.read_text())["client_session"]["db_name"]
    compose("up", "-d", "--no-build", "bridge")
    R.check("a fresh login does NOT reset the existing identity", wait_log("NOT resetting it", 120), "no 'NOT resetting' line")
    stop_bridge()
    new_session = lib.bridge_session()
    new_device, new_name = new_session["user_session"]["device_id"], new_session["client_session"]["db_name"]
    R.check("the bridge now has a new device id", new_device != old_device, f"{old_device} -> {new_device}")
    out = sh([str(DOCTOR), "import-identity", str(old_session), str(WORK / "bridge" / "db" / old_name),
              str(WORK / "bridge" / "session"), str(WORK / "bridge" / "db" / new_name)], check=False)
    R.check("crypto-doctor copies the cross-signing identity", "master=true self-signing=true user-signing=true" in out, out[-300:])
    start_bridge()
    R.check("crypto check OK on the new device", wait_log("Crypto check: OK", 150))
    R.check("bridge holds its cross-signing keys", wait_log("the bridge holds its cross-signing keys", 30))
    keys = lib.repo_script("check-bridge-keys.sh", f"@bot:{SERVER}", new_device, "sbe2e-synapse-1")
    R.check("Synapse holds 50 valid one-time keys for the new device", "invalid=0" in keys and "OK" in keys, keys[-200:])
    return old_device, new_device


def everyone_can_chat(devices, label):
    """A new chat: every device is invited, reads the visitor, and the first and last can answer."""
    marks = [s.mark() for s in devices]
    v = Visitor().start()
    v.open_stream()
    v.say(f"Test {label}")
    rooms = [s.wait_room(m, 90) for s, m in zip(devices, marks)]
    R.check(f"{label}: every respondent device is invited", all(rooms), str(rooms))
    room_id = next((r for r in rooms if r), None)
    for s, m in zip(devices, marks):
        R.check(f"{label}: {s.name} reads the visitor", s.wait_message(f"Test {label}", m, 60) is not None, f"utd={utds(s)}")
    for s in (devices[0], devices[-1]):
        text = f"Réponse de {s.name} ({label})"
        sent = s.send(room_id, text)
        R.check(f"{label}: {s.name} answers and the visitor gets it", sent and sent["ok"] and v.wait_text(text, 40) is not None, str(v.texts()))
    R.check(f"{label}: no undecryptable messages in the new chat", not utds(*devices, room=room_id), str(utds(*devices, room=room_id)))
    v.end()


def still_all_verified(label):
    """A person verified before must stay verified (all devices) whatever happens to the bridge."""
    carol = STATE.get("carol")
    if not carol:
        return
    room = next((r["room"] for r in carol.rooms() if "Salle Utilisateur" in r["name"] and r["state"] == "Joined"), None)
    m = carol.mark()
    carol.send(room, "toujours vérifiée?")
    note = carol.wait(lambda e: e["event"] == "message" and ("Tous vos appareils sont vérifiés" in e["body"] or "Certains de vos appareils" in e["body"]), 120, m)
    R.check(f"{label}: a person verified earlier still has ALL devices verified (no new confirmation)",
            note is not None and "Tous vos appareils sont vérifiés" in note["body"], str(note and note["body"][:200]))
    R.check(f"{label}: no new verification request was sent to her", not carol.wait(lambda e: e["event"] == "verification", 5, m))


def sc_newdevice():
    R.section("9. The bridge gets a new device (the phone incident): nobody has to do anything")
    alice, bob = STATE["alice"], STATE["bob"]
    switch_bridge_device()
    fresh = Sim("alice", "alice-after-newdevice")
    STATE["fresh"] = fresh
    everyone_can_chat([alice, bob, fresh], "after the bridge changed device")
    still_all_verified("after the bridge changed device")


def sc_verification():
    R.section("10. One confirmation covers the person: the bridge verifies a respondent once")
        # Carol (a respondent with exactly two devices) enables recovery on her phone, then signs in on a computer and
    # enters the recovery key (the Element flow)
    alice = Sim("carol", "carol-phone", cross_sign=True)
    STATE["carol"] = alice
    m = alice.mark()
    alice.cmd(cmd="recovery_enable")
    key = alice.wait(lambda e: e["event"] == "recovery_key", 60, m)
    R.check("Carol saves a recovery key on her first device", key and key.get("key"), str(key))
    computer = Sim("carol", "carol-computer", settle=2, auto_verify=False)
    STATE["alice3"] = computer
    mc = computer.mark()
    computer.cmd(cmd="recover", key=key["key"])
    rec = computer.wait(lambda e: e["event"] == "recovered", 90, mc)
    R.check("her computer accepts the recovery key", rec and rec["ok"], str(rec))
    computer.cmd(cmd="sign_self")
    sg = computer.wait(lambda e: e["event"] == "signed", 60, mc)
    R.check("the computer signs itself (both devices now belong to Carol's identity)", sg and sg["ok"], str(sg))
    # The bridge posted "Answer this message to start verification" in Carol's user room: she answers (anything)
    room = None
    deadline = time.time() + 90
    while time.time() < deadline and not room:
        room = next((r["room"] for r in alice.rooms() if "Salle Utilisateur" in r["name"] and r["state"] == "Joined"), None)
        time.sleep(2)
    R.check("the bridge made a user room for Carol", room is not None)
    mv = alice.mark()
    alice.send(room, "ok")
    alice.wait(lambda e: e["event"] == "verification" and e["state"] == "emoji confirmed", 120, mv)
    deadline = time.time() + 60
    while time.time() < deadline and "Successfully verified device @carol" not in bridge_logs():
        time.sleep(2)
    R.check("the verification completes with a single emoji confirmation", "Successfully verified device @carol" in bridge_logs(),
            str([e["state"] for e in alice.events[mv:] if e["event"] == "verification"]))
    R.check("the volunteer only had to confirm once", [e["state"] for e in alice.events[mv:] if e["event"] == "verification"].count("emoji confirmed") == 1)
    time.sleep(10)
    logs = bridge_logs()
    R.check("the bridge logs the verification", "Successfully verified" in logs or "already verified" in logs)
    # Answering again: the bridge now trusts ALL of Carol's devices and says so
    mm = alice.mark()
    alice.send(room, "encore")
    note = alice.wait(lambda e: e["event"] == "message" and ("Tous vos appareils sont vérifiés" in e["body"] or "All your devices are verified" in e["body"]), 120, mm)
    R.check("the bridge tells Carol that ALL her devices are verified (no second confirmation)", note is not None,
            str([e["body"][:90] for e in alice.messages()[-3:]]))
    R.check("it listed both of her devices", note is not None and note["body"].count("✅") >= 2, str(note and note["body"][:200]))

def sc_elementx():
    R.section("10b. Element X: the volunteer's phone starts the emoji comparison itself")
    # Reported 2026-10-12 (a respondent on an iPhone): "Commencer la vérification" spun forever. Element X starts the
    # comparison itself; the bridge must answer that start (and only start one itself if the phone does not).
    # @anna < @bot, like a respondent whose id sorts before the bot's in production. The bridge waits VERIFY_START_DELAY_SECS (1 here).
    lib.add_respondent("anna")
    try:
        anna = Sim("anna", "anna-iphone", cross_sign=True, start_sas=True)
        STATE["anna"] = anna
        # Like any volunteer, she first shares a conversation with the bridge (that is how it learns her devices)
        v, _, _ = chat([anna], "Allo, quelqu'un?")
        v.end()
        room = None
        deadline = time.time() + 120
        while time.time() < deadline and not room:
            room = next((r["room"] for r in anna.rooms() if "Salle Utilisateur" in r["name"] and r["state"] == "Joined"), None)
            time.sleep(2)
        R.check("the bridge made a user room for Anna", room is not None)
        mv = anna.mark()
        anna.send(room, "ok")
        states = lambda: [e["state"] for e in anna.events[mv:] if e["event"] == "verification"]
        anna.wait(lambda e: e["event"] == "verification" and e["state"] == "started by us", 90, mv)
        R.check("her phone started the comparison itself (Element X)", "started by us" in states(), str(states()))
        done = anna.wait(lambda e: e["event"] == "verification" and e["state"] == "done", 90, mv)
        R.check("the verification finishes instead of spinning forever", done is not None, str(states()))
        deadline = time.time() + 30
        while time.time() < deadline and "Successfully verified device @anna" not in bridge_logs():
            time.sleep(2)
        R.check("the bridge verified her device", "Successfully verified device @anna" in bridge_logs())
        R.check("she confirmed the emoji only once", states().count("emoji confirmed") == 1, str(states()))
    finally:
        lib.add_respondent("anna", active=False)


def sc_interrupted():
    R.section("10c. A verification interrupted halfway: the volunteer is told, and writing again restarts it")
    lib.add_respondent("dora")
    try:
        dora = Sim("dora", "dora-iphone", cross_sign=True, start_sas=True)
        STATE["dora"] = dora
        v, _, _ = chat([dora], "Allo?")
        v.end()
        room = None
        deadline = time.time() + 120
        while time.time() < deadline and not room:
            room = next((r["room"] for r in dora.rooms() if "Salle Utilisateur" in r["name"] and r["state"] == "Joined"), None)
            time.sleep(2)
        R.check("the bridge made a user room for Dora", room is not None)
        dora.cmd(cmd="cancel_sas", on=True)
        m = dora.mark()
        dora.send(room, "ok")
        R.check("she presses Cancel at the emoji step", dora.wait(lambda e: e["event"] == "verification" and e["state"] == "cancelled by the volunteer", 90, m) is not None)
        R.check("the bridge tells her the verification was interrupted and how to restart",
                dora.wait(lambda e: e["event"] == "message" and "La vérification a été interrompue" in e["body"], 60, m) is not None)
        dora.cmd(cmd="cancel_sas", on=False)
        m = dora.mark()
        dora.send(room, "on recommence")
        R.check("writing again starts a new verification that completes", dora.wait(lambda e: e["event"] == "verification" and e["state"] == "done", 90, m) is not None,
                str([e["state"] for e in dora.events[m:] if e["event"] == "verification"]))
    finally:
        lib.add_respondent("dora", active=False)


def sbchat_api(command, **extra):
    import urllib.request
    body = json.dumps({"command": command, "api_token": "e2e-token", **extra}).encode()
    req = urllib.request.Request(BRIDGE + "/sbchat_api", data=body, method="POST",
                                 headers={"Authorization": "Bearer e2e-token", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


def sc_agreement():
    R.section("16. Volunteers sign the confidentiality agreement in Element (Law 25)")
    carol = STATE.get("carol")
    if not carol:
        R.check("needs the 'verification' scenario first (Carol, verified)", False)
        return
    room = next((r["room"] for r in carol.rooms() if "Salle Utilisateur" in r["name"] and r["state"] == "Joined"), None)
    m = carol.mark()
    carol.send(room, "!entente")
    text = carol.wait(lambda e: e["event"] == "message" and "ENTENTE DE CONFIDENTIALITÉ" in e["body"], 60, m)
    R.check("!entente shows the agreement in her user room", text is not None)
    R.check("the privacy officer is written into the text", text is not None and "Responsable E2E, vie.privee@e2e.test" in text["body"])
    R.check("it says how to sign", carol.wait(lambda e: e["event"] == "message" and "!signer" in e["body"], 30, m) is not None)
    R.check("asking to read it does not start a verification", not carol.wait(lambda e: e["event"] == "verification", 5, m))
    m = carol.mark()
    carol.send(room, "!signer Carol Test")
    rec = carol.wait(lambda e: e["event"] == "message" and "Entente de confidentialité signée" in e["body"], 60, m)
    R.check("!signer gives her a receipt", rec is not None)
    body = rec["body"] if rec else ""
    R.check("the receipt shows her name, account, a verified device and the fingerprint",
            "Carol Test" in body and "@carol:e2e.test" in body and "vérifié / verified ✅" in body and "Empreinte" in body, body[:400])
    m = carol.mark()
    carol.send(room, "!signer Carol Test")
    R.check("signing twice says it is already signed", carol.wait(lambda e: e["event"] == "message" and "déjà signé" in e["body"], 60, m) is not None)
    m = carol.mark()
    carol.send(room, "!signer")
    R.check("!signer without a name explains how", carol.wait(lambda e: e["event"] == "message" and "prénom et nom" in e["body"], 60, m) is not None)
    # What SwitchBoard sees
    listing = json.loads(sbchat_api("list_agreement_signatures")["message"])
    people = {p["respondent"]: p for p in listing["respondents"]}
    R.check("SwitchBoard: the agreement is configured, with version and fingerprint", listing["configured"] and listing["version"] and len(listing["sha256"]) == 64)
    c = people.get("@carol:e2e.test")
    R.check("SwitchBoard: Carol has signed the current text", c is not None and c["signed_current"] and c["latest"]["typed_name"] == "Carol Test" and c["latest"]["device_verified"], str(c))
    a = people.get("@alice:e2e.test")
    R.check("SwitchBoard: Alice has not signed", a is not None and not a["signed_current"], str(a))
    # "Send now" from SwitchBoard reaches the volunteers who have not signed, not Carol
    alice = STATE["alice"]
    ma, mc = alice.mark(), carol.mark()
    sent = json.loads(sbchat_api("send_agreement")["message"])["sent"]
    R.check("SwitchBoard 'send now' sends it to those who have not signed", sent >= 1, str(sent))
    R.check("Alice receives the agreement in her user room", alice.wait(lambda e: e["event"] == "message" and "ENTENTE DE CONFIDENTIALITÉ" in e["body"], 90, ma) is not None)
    R.check("Carol, who signed, does not get it again", not carol.wait(lambda e: e["event"] == "message" and "Merci de lire et de signer" in e["body"], 10, mc))
    R.check("the signing is in the log without the agreement text", "signed version" in bridge_logs() and "Ce qui est confidentiel" not in bridge_logs())


def sc_shortnames():
    R.section("17. A calendar can name a respondent without the homeserver (@carol for @carol:e2e.test)")
    r = sbchat_api("change_respondents", respondent=["@alice", " carol ", "@bob:e2e.test"])
    R.check("SwitchBoard's list with short names is accepted", r.get("querystatus") == "Success", str(r))
    R.check("the bridge completes them with its own homeserver",
            sorted(json.loads(r["message"].replace("'", '"'))) == ["@alice:e2e.test", "@bob:e2e.test", "@carol:e2e.test"], r.get("message"))
    m = STATE["alice"].mark()
    v, room, _ = chat([STATE["alice"]], "Allo, par le nom court?")
    R.check("a respondent named by a short name still gets the chats", room is not None)
    v.end()


def sc_incident():
    R.section("11. Replay of the incident: store and Synapse out of step, then the runbook")
    alice, bob = STATE["alice"], STATE["bob"]
    device = lib.bridge_device()
    name = lib.bridge_session()["client_session"]["db_name"]
    stop_bridge()
    copy_data("snapshot-healthy")
    lib.own_files()
    db = WORK / "bridge" / "db"
    # 1. the encryption store vanishes (restore onto an empty directory): the bridge makes a new account
    (db / name).rename(db / (name + ".healthy"))
    (db / name).mkdir()
    compose("up", "-d", "--no-build", "bridge")
    wait_log("HTTPS server listening", 120)
    time.sleep(20)
    stop_bridge()
    lib.own_files()
    (db / name).rename(db / (name + ".fresh"))
    # 2. the old store is put back (the 6 October restore)
    (db / (name + ".healthy")).rename(db / name)
    compose("up", "-d", "--no-build", "bridge")
    wait_log("HTTPS server listening", 120)
    time.sleep(75)
    logs = bridge_logs()
    keys = lib.repo_script("check-bridge-keys.sh", f"@bot:{SERVER}", device, "sbe2e-synapse-1")
    mismatch = "CRYPTO CHECK FAILED" in logs
    R.check("the replay creates the mismatch (otherwise the rest proves nothing)", mismatch and "invalid=0" not in keys, keys[-200:])
    R.check("the bridge says so loudly at startup (CRYPTO CHECK FAILED + pointer to the runbook)", mismatch and "encryption-recovery.md" in logs)
    R.check("scripts/check-bridge-keys.sh detects it", "PROBLEM" in keys, keys[-200:])
    # 3. a respondent's brand new device cannot reach the bridge: it must be told, not left guessing
    fresh = Sim("alice", "alice-during-incident")
    STATE["fresh2"] = fresh
    m = fresh.mark()
    v = Visitor().start()
    v.open_stream()
    v.say("Pendant l'incident")
    room = fresh.wait_room(m, 90)
    R.check("the new device is invited to the room", room is not None)
    fresh.wait(lambda e: e["event"] == "message", 30, m)
    fresh.send(room, "Ceci ne devrait pas arriver au visiteur")
    notice = fresh.wait(lambda e: e["event"] == "message" and ("PAS reçu" in e["body"] or "NOT received" in e["body"]), 60, m)
    R.check("the room tells the respondent the message was NOT received (never silent)", notice is not None, str([e["body"][:60] for e in fresh.messages()]))
    R.check("the visitor did not get the lost message", not any("ne devrait pas arriver" in t for t in v.texts()))
    v.end()
    # 4. the runbook: log the bridge in as a new device and copy the cross-signing identity from the old store
    stop_bridge()
    lib.own_files()
    show = sh([str(DOCTOR), "show", str(WORK / "bridge" / "session"), str(db / name), str(db / (name + ".fresh"))], check=False)
    R.check("crypto-doctor shows the stores and their cross-signing state", "device key (ed25519)" in show and "cross-signing" in show, show[-300:])
    switch_bridge_device()
    fresh3 = Sim("alice", "alice-after-recovery")
    STATE["fresh3"] = fresh3
    everyone_can_chat([alice, bob, fresh, fresh3], "after the recovery")
    still_all_verified("after the recovery")


def sc_widget():
    R.section("12. The real website widget against the real bridge (start, reply, page change, end)")
    if not lib.WIDGET_DRIVER.exists():
        R.check("widget driver available (set WPCHAT_REPO)", False, str(lib.WIDGET_DRIVER))
        return
    alice = STATE["alice"]
    w = lib.Widget()
    STATE["widget"] = w
    loaded = w.load()
    state = w.texts()
    R.check("a first visit shows the consent screen", loaded and not loaded["restored"] and state["consent"] and not state["form"])
    m = alice.mark()
    w.cmd(cmd="start")
    ok = w.wait(lambda e: e["event"] == "texts", 1) or True
    deadline = time.time() + 30
    while time.time() < deadline and not (w.texts() or {}).get("form"):
        time.sleep(0.5)
    R.check("the chat opens after pressing start", (w.texts() or {}).get("form"))
    w.cmd(cmd="say", text="Bonjour depuis le vrai widget")
    room = alice.wait_room(m, 90)
    R.check("a respondent is invited and reads the widget's message", room is not None and alice.wait_message("vrai widget", m, 60) is not None)
    sent = alice.send(room, "Je vous lis, bonjour")
    R.check("the widget shows the respondent's reply", sent and sent["ok"] and w.wait_text("Je vous lis, bonjour", 40) is not None)
    R.check("the widget shows the automatic welcome (newcomerweb)", w.wait_text(WELCOME, 20) is not None)
    # the visitor goes to another page of the site
    page2 = w.load()
    R.check("another page restores the chat from the saved session", page2 and page2["restored"])
    t = w.wait_text("Je vous lis, bonjour", 30)
    R.check("the earlier conversation is back on the new page", t is not None and any("vrai widget" in x for x in t["texts"]), str(t and t["texts"]))
    t = w.texts()
    R.check("no consent screen again, the form is ready", t["form"] and not t["consent"])
    sent = alice.send(room, "Vous êtes toujours là?")
    t = w.wait_text("toujours là", 40)
    R.check("a reply written after the page change reaches the new page", t is not None)
    R.check("no line is shown twice", t and len(t["texts"]) == len(set(t["texts"])), str(t and t["texts"]))
    w.cmd(cmd="say", text="Oui, je suis encore là")
    R.check("the visitor can keep writing on the new page", alice.wait_message("encore là", m, 40) is not None)
    # ending the chat
    ma = alice.mark()
    w.cmd(cmd="end")
    R.check("ending the chat removes the room for the respondent", alice.wait(lambda e: e["event"] == "removed" and e["room"] == room, 60, ma) is not None)
    page3 = w.load()
    t = w.texts()
    R.check("after the end, a new page starts from scratch (nothing kept)", page3 and not page3["restored"] and t["consent"] and not t["saved"])


def set_respondents_active(active):
    lib.in_synapse_image("import sqlite3\nc=sqlite3.connect('/w/db/users.db')\nc.execute('UPDATE respondant SET is_active=%d WHERE id_call_center=2')\nc.commit()\n" % (1 if active else 0),
                         [(WORK / "bridge", "/w")], interpreter="python")


def sc_no_respondent():
    R.section("13. Nobody on duty: the visitor gets the call center's no_respondent macro")
    lib.in_synapse_image("import sqlite3\nc=sqlite3.connect('/w/db/users.db')\nc.execute(\"INSERT INTO macros (id_call_center, macro_name, macro_text, macro_tags) VALUES (2,'absent','%s','no_respondent')\")\nc.commit()\n" % NOBODY,
                         [(WORK / "bridge", "/w")], interpreter="python")
    set_respondents_active(False)
    time.sleep(12)  # the bridge caches the configuration for 10 s
    v = Visitor()
    status, body = v._req("POST", "/web/session", {})
    R.check("a new chat is refused with 503 no_respondent", status == 503 and body.get("error") == "no_respondent", f"{status} {body}")
    R.check("the answer carries the no_respondent macro", body.get("message") == NOBODY, str(body))
    if lib.WIDGET_DRIVER.exists():
        w = lib.Widget()
        STATE["widget2"] = w
        w.load()
        w.cmd(cmd="start")
        t = w.wait_text(NOBODY, 30)
        R.check("the real widget shows the macro to the visitor", t is not None, str(w.texts()))
        R.check("no chat form is opened", t is not None and not t["form"])
    set_respondents_active(True)
    time.sleep(12)
    v2 = Visitor().start()
    R.check("when respondents are back on duty, chats open again", v2.token is not None)
    v2.end()


def sc_names():
    R.section("14. A visitor's name follows their network address (like an SMS caller's number)")
    alice = STATE["alice"]

    def room_name_for(address, text):
        m = alice.mark()
        v = Visitor(forwarded_for=address).start()
        status, body = v.say(text)
        if status != 200:
            return f"(visitor message refused: {status} {body})"
        # Only the chat room counts: the bridge may invite her to her user room at the same moment
        name = None
        deadline = time.time() + 60
        while time.time() < deadline and name is None:
            joined = {e["room"] for e in alice.events[m:] if e["event"] == "joined"}
            name = next((r["name"] for r in alice.rooms() if r["room"] in joined and r["name"].startswith("Web ")), None)
            time.sleep(2)
        if name is None:
            print("    debug: joined", {e["room"] for e in alice.events[m:] if e["event"] == "joined"}, "rooms", alice.rooms())
        v.end()
        return name

    first = room_name_for("203.0.113.9", "premier passage")
    again = room_name_for("203.0.113.9", "je reviens")
    other = room_name_for("198.51.100.4", "quelqu'un d'autre")
    R.check("a web room is named 'Web <first name> <letter> <code>'", first is not None and first.startswith("Web ") and len(first.split()) == 4, str(first))
    R.check("the same address gets the same name when it comes back", first is not None and first == again, f"{first} / {again}")
    R.check("another address gets another name", other is not None and other != first, f"{first} / {other}")
    R.check("the name does not show the address", first is not None and "203" not in first and "113" not in first)


def sc_followup():
    R.section("15. A live conversation: no 'nobody answered' ping when a reply takes more than a minute")
    alice, bob = STATE["alice"], STATE["bob"]
    m = bob.mark()
    v, room, _ = chat([alice, bob], "Bonjour, j'ai besoin d'aide")
    alice.wait_message("besoin d'aide", 0, 60)
    alice.send(room, "Bonjour, je suis là")
    R.check("the visitor gets the first answer", v.wait_text("je suis là", 30) is not None)
    time.sleep(3)
    mb = bob.mark()
    v.say("Merci, je vous explique ma situation")
    start = time.time()
    reminders = lambda since: [e for e in bob.events[since:] if e["event"] == "message" and e.get("room") == room and "⏰" in e["body"]]
    while time.time() - start < 100:          # the respondent takes over a minute to reply (no typing notice)
        time.sleep(3)
    R.check("no reminder 1 to 1.5 minutes into a live conversation (the reported bug)", not reminders(mb), str([e["body"][:70] for e in reminders(mb)]))
    while time.time() - start < 150 and not reminders(mb):
        time.sleep(3)
    got = reminders(mb)
    R.check("after the follow-up delay (2 min in this rig, 10 by default) one gentle reminder is posted",
            len(got) == 1 and "attend une réponse" in got[0]["body"], str([e["body"][:90] for e in got]))
    v.end()


SCENARIOS = {
    "bringup": sc_bringup,
    "roundtrip": sc_roundtrip,
    "second_device": sc_second_device,
    "names": sc_names,
    "reload": sc_reload,
    "widget": sc_widget,
    "commands": sc_commands,
    "end": sc_end_deletes_room,
    "typing": sc_typing,
    "followup": sc_followup,
    "restart": sc_restart,
    "stray_device": sc_stray_device,
    "verification": sc_verification,
    "elementx": sc_elementx,
    "interrupted": sc_interrupted,
    "agreement": sc_agreement,
    "shortnames": sc_shortnames,
    "newdevice": sc_newdevice,
    "incident": sc_incident,
    "no_respondent": sc_no_respondent,
}


def main():
    names = [a for a in sys.argv[1:] if not a.startswith("--")] or list(SCENARIOS)
    try:
        for name in names:
            SCENARIOS[name]()
    except Exception as e:
        R.check(f"scenario aborted in '{name}': {e!r}", False)
        import traceback
        traceback.print_exc()
        print("--- bridge log tail ---")
        print(bridge_logs()[-2500:])
    finally:
        for key in ("alice", "bob", "alice2", "alice3", "carol", "carol3", "widget2", "fresh", "fresh2", "fresh3", "widget", "anna", "dora"):
            if key in STATE:
                STATE[key].quit()
        if "--keep" not in sys.argv:
            compose("down", "-v", check=False)
    ok = R.summary()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
