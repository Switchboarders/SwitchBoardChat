#!/bin/sh
# Quick checks of a running installation, to run after every deploy and from time to time (cron).
# Nothing is created on the server except one throwaway web session that is ended at once.
#
#   scripts/prod-smoke.sh https://chat.example.org https://www.example.org  [ssh-host]  [bridge-container]  [synapse-container]  @bridge:server
#
# With an ssh host it also reads the bridge's log (startup lines) and checks the bridge's one-time keys on Synapse.
set -u
URL="${1:?usage: $0 chat-url website-origin [ssh-host [bridge-container [synapse-container [@bridge:server]]]]}"
ORIGIN="${2:?website origin, e.g. https://www.example.org}"
SSH="${3:-}"
BRIDGE="${4:-sb-chat}"
SYNAPSE="${5:-synapse}"
BRIDGE_USER="${6:-}"
fail=0
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
ok() { printf 'PASS  %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; fail=1; }

code=$(curl -s -o "$tmp" -w '%{http_code}' -X POST -H "Origin: $ORIGIN" -H 'Content-Type: application/json' -d '{}' "$URL/web/session")
if [ "$code" = 503 ] && grep -q no_respondent "$tmp"; then ok "web chat answers: nobody is on duty right now (503 no_respondent), as set by the respondent schedule"
elif [ "$code" = 200 ]; then ok "web chat accepts a session"; else bad "web chat session: HTTP $code (a 428 means a proof-of-work challenge: expected only under abuse)"; fi
token=$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "$tmp")
if [ -n "$token" ]; then
  h=$(curl -s -o /dev/null -w '%{http_code}' -H "Origin: $ORIGIN" -H "Authorization: Bearer $token" "$URL/web/history")
  [ "$h" = 200 ] && ok "history endpoint answers" || bad "history endpoint: HTTP $h"
  s=$(curl -s -m 4 -o /dev/null -w '%{http_code}' -H "Origin: $ORIGIN" -H "Authorization: Bearer $token" -H 'Last-Event-ID: 1' "$URL/web/stream")
  [ "$s" = 200 ] || [ "$s" = 000 ] && ok "stream opens (with Last-Event-ID)" || bad "stream: HTTP $s"
  curl -s -o /dev/null -X POST -H "Origin: $ORIGIN" -H "Authorization: Bearer $token" "$URL/web/end"
fi
n=$(curl -s -o /dev/null -w '%{http_code}' -H "Origin: $ORIGIN" "$URL/web/history")
[ "$n" = 401 ] && ok "history refuses a request without a session" || bad "history without a session: HTTP $n"
pre=$(curl -s -o /dev/null -D - -X OPTIONS -H "Origin: $ORIGIN" -H 'Access-Control-Request-Method: GET' -H 'Access-Control-Request-Headers: authorization,last-event-id' "$URL/web/stream" | tr -d '\r' | grep -i '^access-control-allow-headers')
echo "$pre" | grep -qi 'last-event-id' && ok "CORS allows Authorization and Last-Event-ID" || bad "CORS preflight does not allow last-event-id"
other=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Origin: https://not-allowed.example' -H 'Content-Type: application/json' -d '{}' "$URL/web/session")
[ "$other" = 403 ] && ok "an unlisted website origin is refused" || bad "unlisted origin: HTTP $other"

if [ -n "$SSH" ]; then
  # only the lines that matter, since the container's last start (the rest of the log is noise)
  logs=$(ssh -o BatchMode=yes "$SSH" "docker logs --since \$(docker inspect -f '{{.State.StartedAt}}' $BRIDGE) $BRIDGE 2>&1 | grep -E 'HTTPS server listening|Crypto check|CRYPTO CHECK|could not be decrypted'" | sed 's/\x1b\[[0-9;]*m//g')
  echo "$logs" | grep -q 'HTTPS server listening' && ok "log: web server started" || bad "log: no 'HTTPS server listening' (the bridge may be stuck at startup)"
  echo "$logs" | grep -q 'Crypto check: OK' && ok "log: crypto check OK" || bad "log: no 'Crypto check: OK' yet (wait a minute after a restart)"
  echo "$logs" | grep -qE 'our own identity is trusted|trusted it again' && ok "log: the bridge trusts its own identity (people it verified stay verified)" || bad "log: the bridge does not trust its own identity: verified people will look unverified (docs/encryption-recovery.md section 5)"
  echo "$logs" | grep -q 'CRYPTO CHECK FAILED' && bad "log: CRYPTO CHECK FAILED, see docs/encryption-recovery.md" || ok "log: no crypto failure"
  c=$(echo "$logs" | grep -c 'could not be decrypted')
  [ "$c" -eq 0 ] && ok "log: no undecryptable respondent message" || bad "log: $c undecryptable respondent message(s): see docs/encryption-recovery.md section 4"
  if [ -n "$BRIDGE_USER" ]; then
    dev=$(ssh -o BatchMode=yes "$SSH" "docker exec $BRIDGE sh -c 'cat /app/data/session'" 2>/dev/null | sed -n 's/.*"device_id":"\([^"]*\)".*/\1/p')
    if [ -n "$dev" ]; then
      out=$(ssh -o BatchMode=yes "$SSH" sh -s -- "$BRIDGE_USER" "$dev" "$SYNAPSE" < "$(dirname "$0")/check-bridge-keys.sh")
      echo "$out" | grep -q 'invalid=0' && ok "Synapse holds valid one-time keys for the bridge ($dev)" || bad "bridge keys on Synapse are not valid: $out"
    else
      bad "could not read the bridge device id"
    fi
  fi
fi
[ "$fail" = 0 ] && echo "ALL GOOD" || echo "PROBLEMS FOUND"
exit "$fail"
