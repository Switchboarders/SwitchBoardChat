#!/bin/sh
# Checks that the one-time keys Synapse holds for the bridge's device are signed by the device's own key.
# Read-only. Run on the server that hosts Synapse (Docker). If "invalid" is not 0, respondents' Element
# cannot open a secure channel with the bridge: see docs/encryption-recovery.md.
#
#   scripts/check-bridge-keys.sh @bridge:matrix.example.org DEVICEID [synapse-container]
set -eu
USER_ID="${1:?usage: $0 @bridge:server DEVICE_ID [synapse-container]}"
DEVICE_ID="${2:?usage: $0 @bridge:server DEVICE_ID [synapse-container]}"
SYNAPSE="${3:-synapse}"
docker exec -i "$SYNAPSE" python - "$USER_ID" "$DEVICE_ID" <<'PY'
import sqlite3, json, sys
from signedjson.sign import verify_signed_json
from signedjson.key import decode_verify_key_base64
user, device = sys.argv[1], sys.argv[2]
db = sqlite3.connect('file:/data/homeserver.db?mode=ro', uri=True)
row = db.execute('select key_json from e2e_device_keys_json where user_id=? and device_id=?', (user, device)).fetchone()
if not row:
    sys.exit('Synapse holds no device keys for %s %s' % (user, device))
device_key = json.loads(row[0])['keys']['ed25519:' + device]
verify_key = decode_verify_key_base64('ed25519', device, device_key)
ok = bad = 0
for (blob,) in db.execute('select key_json from e2e_one_time_keys_json where user_id=? and device_id=?', (user, device)):
    try:
        verify_signed_json(json.loads(blob), user, verify_key); ok += 1
    except Exception:
        bad += 1
print('device key held by Synapse: %s' % device_key)
print('one-time keys: valid=%d invalid=%d' % (ok, bad))
if bad:
    print('PROBLEM: the bridge uploaded keys that do not match its device key (see docs/encryption-recovery.md)')
    sys.exit(1)
print('OK')
PY
