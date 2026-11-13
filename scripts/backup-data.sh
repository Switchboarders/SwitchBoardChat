#!/bin/sh
# Backs up the bridge's `session` file and `db/` folder TOGETHER, from the same moment, with the bridge stopped.
# They must always be restored together: restoring one without the other (or from different moments)
# breaks the bridge's encryption identity (see docs/encryption-recovery.md).
#
#   scripts/backup-data.sh [compose-dir] [service]      (run on the server; default service: sb-chat)
set -eu
COMPOSE_DIR="${1:-.}"
SERVICE="${2:-sb-chat}"
cd "$COMPOSE_DIR"
STAMP=$(date +%Y%m%d-%H%M)
docker compose stop "$SERVICE"
IMAGE=$(docker compose images -q "$SERVICE" | head -n1)
docker run --rm -v "$PWD/$SERVICE:/d" -e STAMP="$STAMP" --entrypoint sh "$IMAGE" -c '
  set -e
  umask 077
  mkdir -p "/d/backup-$STAMP"
  cp -a /d/db /d/session "/d/backup-$STAMP/"
  chmod 700 "/d/backup-$STAMP"
  echo "backup folder: backup-$STAMP in the data directory (session + db from the same moment)"'
echo "Keep backups encrypted and delete them when no longer needed (Law 25): they hold encrypted numbers and the crypto store."
docker compose up -d --no-build "$SERVICE"
