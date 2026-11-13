"""Helpers for the end-to-end rig: containers, a visitor (website widget protocol) and simulated respondents."""
import json
import os
import re
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
WORK = ROOT / ".work"
COMPOSE = ["docker", "compose", "-f", str(ROOT / "docker-compose.yml")]
SYNAPSE = "http://127.0.0.1:18008"
BRIDGE = "http://127.0.0.1:18088"
ORIGIN = "http://site.test"
SIM = ROOT / "respondent-sim" / "target" / "debug" / "respondent-sim"
DOCTOR = REPO / "tools" / "crypto-doctor" / "target" / "debug" / "crypto-doctor"
PASSWORD = "pw-e2e-1234"
SERVER = "e2e.test"
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def sh(cmd, check=True, timeout=300, input=None):
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, input=input)
    if check and result.returncode != 0:
        raise RuntimeError(f"{' '.join(map(str, cmd))} failed ({result.returncode}): {result.stdout[-800:]} {result.stderr[-800:]}")
    return result.stdout + result.stderr


def compose(*args, **kw):
    return sh(COMPOSE + list(args), **kw)


def in_synapse_image(script, mounts, interpreter="sh"):
    """Run a shell/python snippet as root in a throwaway container (the rig's files are root-owned)."""
    cmd = ["docker", "run", "--rm", "-i"]
    for host, guest in mounts:
        cmd += ["-v", f"{host}:{guest}"]
    cmd += ["--entrypoint", interpreter, "matrixdotorg/synapse:latest", "-s" if interpreter == "sh" else "-"]
    return sh(cmd, input=script)


def bridge_logs(since=None):
    args = ["logs", "--no-color"]
    if since:
        args += ["--since", since]
    args += ["bridge"]
    return ANSI.sub("", compose(*args, check=False))


class Report:
    def __init__(self):
        self.rows = []

    def check(self, name, ok, detail=""):
        self.rows.append((bool(ok), name, detail))
        print(("  PASS " if ok else "  FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""), flush=True)
        return bool(ok)

    def section(self, title):
        print(f"\n== {title}", flush=True)

    def summary(self):
        failed = [r for r in self.rows if not r[0]]
        print(f"\n{len(self.rows) - len(failed)} passed, {len(failed)} failed", flush=True)
        for _, name, detail in failed:
            print(f"  FAILED: {name} {detail}")
        return not failed


# ---------------------------------------------------------------------------------------------- infrastructure

def reset_work():
    if WORK.exists():
        in_synapse_image("rm -rf /w/* /w/.[!.]* 2>/dev/null; true", [(WORK, "/w")])
        shutil.rmtree(WORK, ignore_errors=True)
    (WORK / "synapse").mkdir(parents=True)
    (WORK / "bridge").mkdir(parents=True)
    (WORK / "sims").mkdir(parents=True)


def synapse_config():
    sh(["docker", "run", "--rm", "-e", f"SYNAPSE_SERVER_NAME={SERVER}", "-e", "SYNAPSE_REPORT_STATS=no",
        "-v", f"{WORK / 'synapse'}:/data", "matrixdotorg/synapse:latest", "generate"])
    limit = "{per_second: 10000, burst_count: 10000}"
    extra = f"""
enable_registration: true
enable_registration_without_verification: true
registration_shared_secret: "e2e-shared-secret"
suppress_key_server_warning: true
report_stats: false
rc_message: {limit}
rc_registration: {limit}
rc_login:
  address: {limit}
  account: {limit}
  failed_attempts: {limit}
rc_joins:
  local: {limit}
  remote: {limit}
rc_invites:
  per_room: {limit}
  per_user: {limit}
  per_issuer: {limit}
rc_key_requests: {limit}
rc_room_creation: {limit}
"""
    path = WORK / "synapse" / "homeserver.yaml"
    in_synapse_image(f"cat >> /data/homeserver.yaml <<'EOF'\n{extra}\nEOF\n", [(WORK / "synapse", "/data")])
    _ = path


def start_synapse():
    compose("up", "-d", "synapse")
    deadline = time.time() + 120
    while time.time() < deadline:
        state = sh(["docker", "inspect", "-f", "{{.State.Health.Status}}", "sbe2e-synapse-1"], check=False).strip()
        if state == "healthy":
            return
        time.sleep(2)
    raise RuntimeError("synapse did not become healthy")


def register(user, admin=False):
    args = ["exec", "-T", "synapse", "register_new_matrix_user", "-c", "/data/homeserver.yaml", "-u", user,
            "-p", PASSWORD, "--admin" if admin else "--no-admin", "http://localhost:8008"]
    compose(*args)


def synapse_admin_token():
    """Logs in the e2e admin user (registered with --admin at bring-up) and returns its access token."""
    body = json.dumps({"type": "m.login.password", "identifier": {"type": "m.id.user", "user": "admin"},
                       "password": PASSWORD}).encode()
    req = urllib.request.Request(SYNAPSE + "/_matrix/client/v3/login", data=body, method="POST",
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.loads(r.read())["access_token"]


def synapse_room_exists(room_id, token):
    """True while Synapse still knows the room (the admin API answers 404 once it is deleted and purged)."""
    req = urllib.request.Request(SYNAPSE + "/_synapse/admin/v1/rooms/" + urllib.parse.quote(room_id, safe=""),
                                 headers={"Authorization": "Bearer " + token})
    try:
        with urllib.request.urlopen(req, timeout=20):
            return True
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return False
        raise


def bridge_running():
    return sh(["docker", "inspect", "-f", "{{.State.Running}}", "sbe2e-bridge-1"], check=False).strip() == "true"


def wait_bridge_ready(timeout=240):
    """Waits for the web server to listen. Returns the log text. The first start of a fresh bot resets its identity and exits."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        logs = bridge_logs()
        if "HTTPS server listening" in logs and bridge_running():
            return logs
        if not bridge_running() and "Cross-signing" in logs or (not bridge_running() and "reset" in logs.lower()):
            compose("up", "-d", "--no-build", "bridge")
        time.sleep(3)
    raise RuntimeError("bridge did not come up:\n" + bridge_logs()[-2500:])


def add_respondent(user, active=True):
    """Adds (or removes) a respondent of the test call center while the bridge runs."""
    db = "/w/db/users.db"
    st = ("INSERT INTO respondant (id_call_center, respondant_username, is_active) VALUES (2,'@%s:%s',1)" % (user, SERVER)) if active \
        else ("DELETE FROM respondant WHERE respondant_username='@%s:%s'" % (user, SERVER))
    script = "import sqlite3\nc=sqlite3.connect('%s')\nc.execute(%r)\nc.commit()\n" % (db, st)
    in_synapse_image(script, [(WORK / "bridge", "/w")], interpreter="python")


def seed_call_center(macros=()):
    db = "/w/db/users.db"
    statements = [
        "DELETE FROM respondant WHERE id_call_center=2",
        "DELETE FROM macros WHERE id_call_center=2",
        "DELETE FROM call_center WHERE id_call_center=2",
        "INSERT INTO call_center (id_call_center, sb_chat_api_token, sb_chat_api_token_hashed, voip_ms_username, voip_ms_api_password, call_center_did, privilege_level) VALUES (2,'e2e-token','e2e-token-hash','x','x',5145550100,0)",
        "INSERT INTO respondant (id_call_center, respondant_username, is_active) VALUES (2,'@alice:%s',1)" % SERVER,
        "INSERT INTO respondant (id_call_center, respondant_username, is_active) VALUES (2,'@bob:%s',1)" % SERVER,
        "INSERT INTO respondant (id_call_center, respondant_username, is_active) VALUES (2,'@carol:%s',1)" % SERVER,
    ]
    for name, text, tag in macros:
        statements.append(
            "INSERT INTO macros (id_call_center, macro_name, macro_text, macro_tags) VALUES (2,'%s','%s','%s')" % (name, text.replace("'", "''"), tag)
        )
    script = "import sqlite3\nc=sqlite3.connect('%s')\n" % db
    for st in statements:
        script += "c.execute(%r)\n" % st
    script += "c.commit()\nprint('seeded')\n"
    in_synapse_image(script, [(WORK / "bridge", "/w")], interpreter="python")


# ---------------------------------------------------------------------------------------------- visitor

class Visitor:
    """The website widget's protocol, spoken directly: session, message, stream (SSE), history, end."""

    def __init__(self, origin=ORIGIN, forwarded_for=None):
        self.origin = origin
        # The bridge reads the visitor address from X-Forwarded-For (it sits behind a reverse proxy)
        self.extra = {"X-Forwarded-For": forwarded_for} if forwarded_for else {}
        self.token = None
        self.events = []
        self.cond = threading.Condition()
        self._stop = False
        self._resp = None
        self.last_seq = 0

    def _req(self, method, path, body=None, headers=None, timeout=20):
        h = {"Origin": self.origin, "Content-Type": "application/json", **self.extra}
        if self.token:
            h["Authorization"] = "Bearer " + self.token
        h.update(headers or {})
        data = json.dumps(body).encode() if body is not None else (b"{}" if method == "POST" else None)
        req = urllib.request.Request(BRIDGE + path, data=data, method=method, headers=h)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return r.status, json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            try:
                return e.code, json.loads(e.read() or b"{}")
            except Exception:
                return e.code, {}

    def start(self):
        status, body = self._req("POST", "/web/session", {})
        if status == 428 and body.get("error") == "challenge_required":
            # Same proof of work as the widget: SHA-256(challenge:nonce) with `bits` leading zero bits
            import hashlib
            challenge, bits, nonce = body["challenge"], body["bits"], 0
            while True:
                digest = hashlib.sha256(f"{challenge}:{nonce}".encode()).digest()
                if int.from_bytes(digest, "big") >> (256 - bits) == 0:
                    break
                nonce += 1
            self.solved_challenge = True
            status, body = self._req("POST", "/web/session", {"challenge": challenge, "nonce": str(nonce)})
        assert status == 200 and body.get("token"), f"session failed {status} {body}"
        self.token = body["token"]
        return self

    def say(self, text):
        return self._req("POST", "/web/message", {"text": text})

    def history(self):
        return self._req("GET", "/web/history")

    def end(self):
        return self._req("POST", "/web/end", {})

    def open_stream(self, last_event_id=None):
        self.close_stream()
        self._stop = False
        headers = {"Origin": self.origin, "Authorization": "Bearer " + self.token, "Accept": "text/event-stream"}
        if last_event_id:
            headers["Last-Event-ID"] = str(last_event_id)
        req = urllib.request.Request(BRIDGE + "/web/stream", headers=headers)
        resp = urllib.request.urlopen(req, timeout=300)
        self._resp = resp

        def run():
            name, data, eid = "message", "", ""
            try:
                while not self._stop:
                    line = resp.readline()
                    if not line:
                        break
                    line = line.decode().rstrip("\r\n")
                    if line == "":
                        if data:
                            with self.cond:
                                self.events.append({"event": name, "data": data, "id": eid, "at": time.time()})
                                self.cond.notify_all()
                        name, data, eid = "message", "", ""
                    elif line.startswith(":"):
                        pass
                    elif line.startswith("event:"):
                        name = line[6:].strip()
                    elif line.startswith("data:"):
                        data += line[5:].strip()
                    elif line.startswith("id:"):
                        eid = line[3:].strip()
            except Exception:
                pass

        threading.Thread(target=run, daemon=True).start()
        return self

    def close_stream(self):
        self._stop = True
        if self._resp:
            try:
                self._resp.close()
            except Exception:
                pass
            self._resp = None

    def texts(self):
        out = []
        for e in self.events:
            if e["event"] == "message":
                try:
                    out.append(json.loads(e["data"])["text"])
                except Exception:
                    pass
        return out

    def wait_text(self, needle, timeout=30):
        deadline = time.time() + timeout
        with self.cond:
            while time.time() < deadline:
                for t in self.texts():
                    if needle in t:
                        return t
                self.cond.wait(timeout=0.5)
        return None

    def wait_event(self, name, timeout=30):
        deadline = time.time() + timeout
        with self.cond:
            while time.time() < deadline:
                if any(e["event"] == name for e in self.events):
                    return True
                self.cond.wait(timeout=0.5)
        return False


# ---------------------------------------------------------------------------------------------- respondents

class Sim:
    """One device of a simulated respondent (a respondent-sim process)."""

    counter = 0

    def __init__(self, user, name, cross_sign=False, settle=8, auto_verify=True, start_sas=False):
        Sim.counter += 1
        self.user = user
        self.name = name
        self.store = WORK / "sims" / f"{name}-{Sim.counter}"
        self.store.mkdir(parents=True, exist_ok=True)
        self.events = []
        self.cond = threading.Condition()
        args = [str(SIM), SYNAPSE, user, PASSWORD, str(self.store), name] + (["--cross-sign"] if cross_sign else []) + ([] if auto_verify else ["--no-auto-verify"]) + (["--start-sas"] if start_sas else [])
        self.proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(self.store / 'stderr.log', 'w'), text=True, bufsize=1, env={**os.environ, **({'RUST_LOG': os.environ['SIM_LOG']} if os.environ.get('SIM_LOG') else {})})
        threading.Thread(target=self._read, daemon=True).start()
        self.device = None
        ready = self.wait(lambda e: e["event"] == "ready", 90)
        if not ready:
            raise RuntimeError(f"sim {name} did not become ready")
        self.device = ready.get("device")
        # A real volunteer signs in well before the next chat; the bridge learns a new device through the
        # next sync. Leave it a moment so the tests do not measure that race.
        time.sleep(settle)

    def _read(self):
        for line in self.proc.stdout:
            try:
                ev = json.loads(line)
            except Exception:
                continue
            with self.cond:
                self.events.append(ev)
                self.cond.notify_all()

    def mark(self):
        return len(self.events)

    def wait(self, pred, timeout=30, since=0):
        deadline = time.time() + timeout
        with self.cond:
            while time.time() < deadline:
                for ev in self.events[since:]:
                    if pred(ev):
                        return ev
                self.cond.wait(timeout=0.5)
        return None

    def cmd(self, **c):
        self.proc.stdin.write(json.dumps(c) + "\n")
        self.proc.stdin.flush()

    def send(self, room, text, timeout=30):
        m = self.mark()
        self.cmd(cmd="send", room=room, text=text)
        return self.wait(lambda e: e["event"] == "sent", timeout, m)

    def rooms(self, timeout=15):
        m = self.mark()
        self.cmd(cmd="rooms")
        ev = self.wait(lambda e: e["event"] == "rooms", timeout, m)
        return ev["rooms"] if ev else []

    def messages(self, room=None):
        return [e for e in self.events if e["event"] == "message" and (room is None or e["room"] == room)]

    def wait_message(self, needle, since=0, timeout=40, room=None):
        return self.wait(lambda e: e["event"] == "message" and needle in e["body"] and (room is None or e["room"] == room), timeout, since)

    def wait_room(self, since=0, timeout=40):
        ev = self.wait(lambda e: e["event"] == "joined", timeout, since)
        return ev["room"] if ev else None

    def quit(self):
        try:
            self.cmd(cmd="quit")
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


# ---------------------------------------------------------------------------------------------- bridge data operations

def own_files():
    """Make the bridge's root-owned data readable/writable by the test user (the bridge, running as root, is unaffected)."""
    in_synapse_image(f"chown -R {os.getuid()}:{os.getgid()} /w", [(WORK / "bridge", "/w")])


def bridge_session():
    own_files()
    return json.loads((WORK / "bridge" / "session").read_text())


def bridge_device():
    return bridge_session()["user_session"]["device_id"]


def store_dir(name=None):
    s = bridge_session()
    return WORK / "bridge" / "db" / (name or s["client_session"]["db_name"])


def repo_script(name, *args, stdin=None):
    return sh(["sh", str(REPO / "scripts" / name), *args], input=stdin, check=False, timeout=120)


# ---------------------------------------------------------------------------------------------- the real widget

_WPCHAT_DEFAULT = next((p for p in (REPO.parent / "wp_chat", Path.home() / "wp_chat") if p.is_dir()), REPO.parent / "wp_chat")
WIDGET_DRIVER = Path(os.environ.get("WPCHAT_REPO", str(_WPCHAT_DEFAULT))) / "tests" / "widget-e2e-driver.js"


class Widget:
    """The real website widget (chat.js in a fake DOM) talking to the real bridge."""

    def __init__(self):
        self.proc = subprocess.Popen(["node", str(WIDGET_DRIVER), BRIDGE, ORIGIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.events = []
        self.cond = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()
        self.wait(lambda e: e["event"] == "ready", 15)

    def _read(self):
        for line in self.proc.stdout:
            try:
                ev = json.loads(line)
            except Exception:
                continue
            with self.cond:
                self.events.append(ev)
                self.cond.notify_all()

    def wait(self, pred, timeout=20, since=0):
        deadline = time.time() + timeout
        with self.cond:
            while time.time() < deadline:
                for ev in self.events[since:]:
                    if pred(ev):
                        return ev
                self.cond.wait(timeout=0.3)
        return None

    def cmd(self, **c):
        self.proc.stdin.write(json.dumps(c) + "\n")
        self.proc.stdin.flush()

    def load(self):
        m = len(self.events)
        self.cmd(cmd="load")
        return self.wait(lambda e: e["event"] == "loaded", 15, m)

    def texts(self):
        m = len(self.events)
        self.cmd(cmd="texts")
        return self.wait(lambda e: e["event"] == "texts", 10, m)

    def wait_text(self, needle, timeout=40):
        deadline = time.time() + timeout
        while time.time() < deadline:
            t = self.texts()
            if t and any(needle in x for x in t["texts"]):
                return t
            time.sleep(0.7)
        return None

    def quit(self):
        try:
            self.proc.kill()
        except Exception:
            pass
