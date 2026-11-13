#!/bin/sh
# Deletes the one-time and fallback keys Synapse holds for the bridge's device, after backing the Synapse
# database up, then restarts Synapse (it caches key counts). The bridge uploads fresh keys by itself.
# ONLY useful when the bridge's local encryption store is the right one (crypto-doctor show matches Synapse's
# device key); otherwise the new keys will be invalid again.
#
#   scripts/reset-bridge-keys.sh @bridge:matrix.example.org DEVICEID [synapse-container]
set -eu
USER_ID="${1:?usage: $0 @bridge:server DEVICE_ID [synapse-container]}"
DEVICE_ID="${2:?usage: $0 @bridge:server DEVICE_ID [synapse-container]}"
SYNAPSE="${3:-synapse}"
printf 'Back up the Synapse database, delete the keys of %s %s and restart %s? [y/N] ' "$USER_ID" "$DEVICE_ID" "$SYNAPSE"
read -r answer
[ "$answer" = "y" ] || { echo "cancelled"; exit 1; }
STAMP=$(date +%Y%m%d-%H%M)
docker exec -i "$SYNAPSE" python - "$STAMP" <<'PY'
import sqlite3, sys
src = sqlite3.connect('file:/data/homeserver.db?mode=ro', uri=True)
dst = sqlite3.connect('/data/homeserver.db.before-otk-fix-' + sys.argv[1])
src.backup(dst); dst.close(); src.close()
print('backup written: /data/homeserver.db.before-otk-fix-' + sys.argv[1])
PY
docker exec -i "$SYNAPSE" python - "$USER_ID" "$DEVICE_ID" <<'PY'
import sqlite3, sys
db = sqlite3.connect('/data/homeserver.db', timeout=30)
a = db.execute('delete from e2e_one_time_keys_json where user_id=? and device_id=?', (sys.argv[1], sys.argv[2])).rowcount
b = db.execute('delete from e2e_fallback_keys_json where user_id=? and device_id=?', (sys.argv[1], sys.argv[2])).rowcount
db.commit()
print('deleted one-time keys: %d, fallback keys: %d' % (a, b))
PY
docker restart "$SYNAPSE" >/dev/null
echo "Synapse restarted. Restart the bridge, wait a minute, then run scripts/check-bridge-keys.sh"
