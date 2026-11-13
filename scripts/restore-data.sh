#!/bin/sh
# Restores `session` AND `db/` from ONE backup folder made by backup-data.sh (never one without the other).
# Keeps the current data in a "replaced-<date>" folder first.
#
#   scripts/restore-data.sh backup-20261008-1500 [compose-dir] [service]
set -eu
BACKUP="${1:?usage: $0 backup-folder-name [compose-dir] [service]}"
COMPOSE_DIR="${2:-.}"
SERVICE="${3:-sb-chat}"
case "$BACKUP" in
  backup-[0-9]*|replaced-[0-9]*) ;;
  *) echo "not a backup folder name (backup-YYYYMMDD-HHMM): $BACKUP" >&2; exit 2 ;;
esac
case "$BACKUP" in *[!A-Za-z0-9-]*) echo "invalid backup folder name: $BACKUP" >&2; exit 2 ;; esac
cd "$COMPOSE_DIR"
STAMP=$(date +%Y%m%d-%H%M)
IMAGE=$(docker compose images -q "$SERVICE" | head -n1)
docker compose stop "$SERVICE"
# Names go in as environment variables, never into the shell command text
docker run --rm -v "$PWD/$SERVICE:/d" -e BACKUP="$BACKUP" -e STAMP="$STAMP" --entrypoint sh "$IMAGE" -c '
  set -e
  umask 077
  [ -f "/d/$BACKUP/session" ] && [ -d "/d/$BACKUP/db" ] || { echo "the backup must contain both session and db/"; exit 1; }
  mkdir -p "/d/replaced-$STAMP" && mv /d/session /d/db "/d/replaced-$STAMP/"
  cp -a "/d/$BACKUP/session" "/d/$BACKUP/db" /d/
  echo "restored session + db from $BACKUP; previous data kept in replaced-$STAMP"'
echo "Backups and replaced-* folders hold encrypted numbers and the crypto store: delete them when no longer needed (Law 25)."
docker compose up -d --no-build "$SERVICE"
echo "Check the log: it must show 'HTTPS server listening' and the 'Crypto check: OK' line after a minute."
