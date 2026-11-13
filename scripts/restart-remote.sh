#!/usr/bin/env bash
# Restarts the bot on the server without rebuilding, so it re-reads its .env
# (docker compose restart does not), then checks it came back.
# Usage: ./scripts/restart-remote.sh
set -euo pipefail

# Settings: scripts/deploy.env (git-ignored, see scripts/deploy.env.example) or the environment.
#   DEPLOY_HOST   ssh host (an alias from ~/.ssh/config)
#   DEPLOY_DIR    checkout of this repository on that host (where docker-compose.yml is)
#   WEBHOOK_URL   public URL of /voip_api, checked at the end (optional)
#   DEPLOY_SUDO   "sudo -n" when the checkout and data belong to root (default), "" otherwise
#   SNAPSHOT_DIR  where database snapshots go on the host (default: the parent of DEPLOY_DIR)
cd "$(dirname "$0")/.."
# shellcheck disable=SC1091
[ -f scripts/deploy.env ] && . scripts/deploy.env
: "${DEPLOY_HOST:?set DEPLOY_HOST (see scripts/deploy.env.example)}"
: "${DEPLOY_DIR:?set DEPLOY_DIR (see scripts/deploy.env.example)}"
WEBHOOK_URL="${WEBHOOK_URL:-}"
DEPLOY_SUDO="${DEPLOY_SUDO-sudo -n}"
SNAPSHOT_DIR="${SNAPSHOT_DIR:-$(dirname "$DEPLOY_DIR")}"
HOST=$DEPLOY_HOST
DIR=$DEPLOY_DIR

# shellcheck disable=SC2087  # the variables below are meant to expand locally
ssh "$HOST" bash -s <<EOF
set -euo pipefail
cd "$DIR"
docker compose up -d --force-recreate 2>&1 | grep -E "Recreate|Started|ERROR|error" || true
STARTED=\$(docker inspect sb-chat --format '{{.State.StartedAt}}')
for _ in \$(seq 90); do
    docker logs sb-chat --since "\$STARTED" 2>&1 | grep -c "The client is ready" >/dev/null && break
    sleep 2
done
echo "Container: \$(docker ps --format '{{.Status}}' --filter name=^sb-chat\$), restarts \$(docker inspect sb-chat --format '{{.RestartCount}}')"
LOG=\$(docker logs sb-chat --since "\$STARTED" 2>&1 | sed 's/\x1b\[[0-9;]*m//g')
echo "\$LOG" | grep -aE " (WARN|ERROR) |panicked|CRYPTO CHECK|rate limits removed|server admin|every device of the bridge" | grep -avE "Resending|Reprocessing Queue|Queue Item" | cut -c1-220 | tail -15 || true
echo "\$LOG" | grep -c "The client is ready" >/dev/null || { echo "The bot did not report ready within 3 minutes" >&2; exit 1; }
EOF

if [ -n "$WEBHOOK_URL" ]; then
    # Without a key the webhook answers 403: that is enough to know it is up
    CODE=$(curl -sk -o /dev/null -w "%{http_code}" -m 10 "$WEBHOOK_URL" || true)
    echo "SMS webhook: HTTP $CODE"
    case "$CODE" in 200|403|429) ;; *) echo "The SMS webhook does not answer" >&2; exit 1 ;; esac
fi
echo "Restarted."
