#!/usr/bin/env bash
# Installs (or updates the configuration of) switchboard-chat with Docker Compose, from this checkout.
# Designed to be called by a larger installer (Synapse + SwitchBoard + this bridge) as well as by hand.
# Full contract: docs/install.md.
#
#   scripts/install.sh --homeserver-url https://matrix.example.org --bot-user @sms:matrix.example.org \
#                      --bot-password-file /run/secrets/bot [options]
#   scripts/install.sh --check [--output-json FILE]
#
# Exit codes: 0 ok · 2 bad arguments · 3 missing dependency · 4 start failed · 5 check failed
set -euo pipefail
umask 077

readonly EXIT_OK=0 EXIT_ARGS=2 EXIT_DEPS=3 EXIT_START=4 EXIT_CHECK=5
readonly READY_LINE="The client is ready"

ROOT=$(cd "$(dirname "$0")/.." && pwd)
ENV_FILE="$ROOT/.env"
DATA_DIR="$ROOT/sb-chat"
TOKEN_FILE="$ROOT/admin-api-token"
CONTAINER=sb-chat

HOMESERVER_URL="${SBCHAT_HOMESERVER_URL:-}"
BOT_USER="${SBCHAT_BOT_USER:-}"
BOT_PASSWORD="${SBCHAT_BOT_PASSWORD:-}"
BOT_PASSWORD_FILE=""
VIRTUAL_HOST_NAME="${SBCHAT_VIRTUAL_HOST:-}"
WEB_ORIGINS="${SBCHAT_WEB_ALLOWED_ORIGINS:-}"
WEB_CALL_CENTER="${SBCHAT_WEB_CALL_CENTER_ID:-}"
WEB_PUBLIC="${SBCHAT_WEB_PUBLIC_URL:-}"
TRUSTED="${SBCHAT_TRUSTED_PROXIES:-}"
HTTP_BIND_ADDR="${SBCHAT_HTTP_BIND:-}"
MAS_CLIENT_ID="${SBCHAT_MAS_ADMIN_CLIENT_ID:-}"
MAS_CLIENT_SECRET="${SBCHAT_MAS_ADMIN_CLIENT_SECRET:-}"
MAS_CLIENT_SECRET_FILE=""
NETWORK="${SBCHAT_DOCKER_NETWORK:-}"
OUTPUT_JSON=""
CHECK_ONLY=0
START=1
INTERACTIVE=1
READY_TIMEOUT=300

usage() {
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options (each also readable from the SBCHAT_* environment variable in brackets):
  --homeserver-url URL          Matrix homeserver (Synapse) URL                     [SBCHAT_HOMESERVER_URL]
  --bot-user @user:server       the bot's Matrix account (created beforehand in MAS) [SBCHAT_BOT_USER]
  --bot-password-file FILE      file holding the bot's password (preferred)
                                (or SBCHAT_BOT_PASSWORD; never pass it on the command line)
  --virtual-host NAME           host name for the self-signed certificate (default: the homeserver's host)
  --web-origins LIST            web chat: allowed website origins, comma-separated  [SBCHAT_WEB_ALLOWED_ORIGINS]
  --web-call-center ID          web chat: call center id of the widget              [SBCHAT_WEB_CALL_CENTER_ID]
  --web-public-url URL          web chat: https address browsers use (behind the proxy) [SBCHAT_WEB_PUBLIC_URL]
  --trusted-proxies LIST        reverse proxy addresses/CIDRs for X-Forwarded-For   [SBCHAT_TRUSTED_PROXIES]
  --http-bind ADDR:PORT         plain HTTP listener for the reverse proxy           [SBCHAT_HTTP_BIND]
  --mas-admin-client-id ID      MAS admin client (optional)                         [SBCHAT_MAS_ADMIN_CLIENT_ID]
  --mas-admin-client-secret-file FILE                                            [SBCHAT_MAS_ADMIN_CLIENT_SECRET]
  --docker-network NAME         external Docker network shared with the proxy (default: web)
  --no-start                    write the configuration and build, do not start
  --check                       change nothing; report whether the installation is healthy
  --output-json FILE            write a machine-readable summary (mode 0600)
  --non-interactive             never prompt (default when stdin is not a terminal)
  --ready-timeout SECONDS       how long to wait for the bridge to be ready (default 300)
  -h, --help
EOF
}

die() { local code=$1; shift; printf 'install.sh: %s\n' "$*" >&2; exit "$code"; }
say() { printf '%s\n' "$*" >&2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --homeserver-url) HOMESERVER_URL=${2-}; shift 2 ;;
        --bot-user) BOT_USER=${2-}; shift 2 ;;
        --bot-password-file) BOT_PASSWORD_FILE=${2-}; shift 2 ;;
        --bot-password) die $EXIT_ARGS "--bot-password would expose the password in the process list: use --bot-password-file or SBCHAT_BOT_PASSWORD" ;;
        --virtual-host) VIRTUAL_HOST_NAME=${2-}; shift 2 ;;
        --web-origins) WEB_ORIGINS=${2-}; shift 2 ;;
        --web-call-center) WEB_CALL_CENTER=${2-}; shift 2 ;;
        --web-public-url) WEB_PUBLIC=${2-}; shift 2 ;;
        --trusted-proxies) TRUSTED=${2-}; shift 2 ;;
        --http-bind) HTTP_BIND_ADDR=${2-}; shift 2 ;;
        --mas-admin-client-id) MAS_CLIENT_ID=${2-}; shift 2 ;;
        --mas-admin-client-secret-file) MAS_CLIENT_SECRET_FILE=${2-}; shift 2 ;;
        --docker-network) NETWORK=${2-}; shift 2 ;;
        --no-start) START=0; shift ;;
        --check) CHECK_ONLY=1; shift ;;
        --output-json) OUTPUT_JSON=${2-}; shift 2 ;;
        --non-interactive) INTERACTIVE=0; shift ;;
        --ready-timeout) READY_TIMEOUT=${2-}; shift 2 ;;
        -h|--help) usage; exit $EXIT_OK ;;
        *) usage >&2; die $EXIT_ARGS "unknown option: $1" ;;
    esac
done
[ -t 0 ] || INTERACTIVE=0
[[ "$READY_TIMEOUT" =~ ^[0-9]+$ ]] || die $EXIT_ARGS "--ready-timeout must be a number of seconds"

# ---------------------------------------------------------------------------------------------- helpers

need() { command -v "$1" >/dev/null 2>&1 || die $EXIT_DEPS "missing dependency: $1"; }

check_deps() {
    need docker
    need openssl
    need awk
    docker compose version >/dev/null 2>&1 || die $EXIT_DEPS "missing dependency: docker compose (v2 plugin)"
    docker info >/dev/null 2>&1 || die $EXIT_DEPS "cannot talk to the Docker daemon (are you in the docker group?)"
}

# Value of KEY in the env file (empty if absent)
env_get() { [ -f "$ENV_FILE" ] && awk -v k="$1" 'index($0, k "=") == 1 { print substr($0, length(k) + 2); exit }' "$ENV_FILE" || true; }

# Sets KEY=VALUE in the env file: replaces an active line, else a commented "# KEY=" line, else appends
env_set() {
    local key=$1 value=$2 tmp
    tmp=$(mktemp "$ENV_FILE.XXXXXX")
    awk -v k="$key" -v v="$value" '
        BEGIN { done = 0 }
        !done && index($0, k "=") == 1 { print k "=" v; done = 1; next }
        { print }
        END { if (!done) exit 3 }
    ' "$ENV_FILE" > "$tmp" && { mv "$tmp" "$ENV_FILE"; return; }
    awk -v k="$key" -v v="$value" '
        BEGIN { done = 0 }
        !done && index($0, "# " k "=") == 1 { print k "=" v; done = 1; next }
        { print }
        END { if (!done) print k "=" v }
    ' "$ENV_FILE" > "$tmp"
    mv "$tmp" "$ENV_FILE"
}

# Values go into a line-based .env read by Docker Compose and the bridge: refuse what they could misread
safe_value() {
    local name=$1 value=$2
    case "$value" in
        *$'\n'*|*$'\r'*|*'$'*|*'"'*|*"'"*|*'`'*|*'\'*|*' '*|*'#'*)
            die $EXIT_ARGS "$name contains a character not allowed in .env (space, quote, \$, #, backslash or newline)" ;;
    esac
}

read_secret_file() {
    local file=$1 what=$2
    [ -r "$file" ] || die $EXIT_ARGS "cannot read $what file: $file"
    head -n1 "$file" | tr -d '\r\n'
}

prompt() {
    local var=$1 text=$2 secret=${3:-0} value
    [ "$INTERACTIVE" = 1 ] || return 0
    if [ "$secret" = 1 ]; then read -r -s -p "$text: " value; echo >&2; else read -r -p "$text: " value; fi
    printf -v "$var" '%s' "$value"
}

write_json() {
    [ -n "$OUTPUT_JSON" ] || return 0
    local status=$1 webhook=$2 ready=$3
    ( umask 077; cat > "$OUTPUT_JSON" <<EOF
{
  "status": "$status",
  "install_dir": "$ROOT",
  "env_file": "$ENV_FILE",
  "data_dir": "$DATA_DIR",
  "admin_token_file": "$( [ -f "$TOKEN_FILE" ] && echo "$TOKEN_FILE" )",
  "container": "$CONTAINER",
  "ready": $ready,
  "webhook_url": "$webhook",
  "webhook_note": "voip.ms SMS URL: append ?to={TO}&from={FROM}&message={MESSAGE}&id={ID}&timestamp={TIMESTAMP}&sbchatapikey=<call center key> (SwitchBoard sets this per call center)"
}
EOF
    )
    chmod 600 "$OUTPUT_JSON"
}

webhook_url() {
    local host
    host=$(env_get VIRTUAL_HOST)
    [ -n "$host" ] && echo "https://$host:3443/voip_api" || true
}

bridge_ready() {
    local started
    started=$(docker inspect -f '{{.State.StartedAt}}' "$CONTAINER" 2>/dev/null) || return 1
    [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" = true ] || return 1
    docker logs --since "$started" "$CONTAINER" 2>&1 | grep -c "$READY_LINE" >/dev/null  # -c reads all: -q would SIGPIPE docker logs under pipefail
}

# ---------------------------------------------------------------------------------------------- --check

if [ "$CHECK_ONLY" = 1 ]; then
    check_deps
    need curl
    problems=0
    report() { if [ "$1" = ok ]; then say "PASS  $2"; else say "FAIL  $2"; problems=1; fi; }
    if [ -f "$ENV_FILE" ]; then
        report ok ".env exists"
        mode=$(stat -c %a "$ENV_FILE")
        [ "$mode" = 600 ] && report ok ".env is mode 600" || report bad ".env is mode $mode (should be 600)"
        for key in HOMESERVER_URL MATRIX_USERNAME MATRIX_PASSWORD PHONE_HASH_SECRET PHONE_ENC_KEY VIRTUAL_HOST; do
            [ -n "$(env_get "$key")" ] && report ok "$key is set" || report bad "$key is empty"
        done
    else
        report bad ".env missing (run the installer first)"
    fi
    [ -f "$TOKEN_FILE" ] && report ok "admin API token file present" || report bad "admin API token file missing ($TOKEN_FILE)"
    if bridge_ready; then report ok "bridge running and ready"; ready=true; else report bad "bridge not running or not ready"; ready=false; fi
    url=$(webhook_url)
    if [ -n "$url" ]; then
        code=$(curl -sk -o /dev/null -w '%{http_code}' -m 10 "https://127.0.0.1:3443/voip_api" 2>/dev/null || true)
        case "$code" in 200|403|429) report ok "SMS webhook answers on port 3443" ;; *) report bad "SMS webhook on port 3443: HTTP ${code:-none}" ;; esac
    fi
    if [ "$problems" = 0 ]; then write_json ok "$url" "$ready"; exit $EXIT_OK; fi
    write_json check_failed "$url" "$ready"
    exit $EXIT_CHECK
fi

# ---------------------------------------------------------------------------------------------- install

check_deps
need curl

[ -n "$BOT_PASSWORD_FILE" ] && BOT_PASSWORD=$(read_secret_file "$BOT_PASSWORD_FILE" "bot password")
[ -n "$MAS_CLIENT_SECRET_FILE" ] && MAS_CLIENT_SECRET=$(read_secret_file "$MAS_CLIENT_SECRET_FILE" "MAS client secret")

# An existing installation keeps its values unless new ones are given
[ -n "$HOMESERVER_URL" ] || HOMESERVER_URL=$(env_get HOMESERVER_URL)
[ -n "$BOT_USER" ] || BOT_USER=$(env_get MATRIX_USERNAME)
[ -n "$BOT_PASSWORD" ] || BOT_PASSWORD=$(env_get MATRIX_PASSWORD)
[ -n "$HOMESERVER_URL" ] || prompt HOMESERVER_URL "Homeserver URL (e.g. https://matrix.example.org)"
[ -n "$BOT_USER" ] || prompt BOT_USER "Bot Matrix user (e.g. @sms:matrix.example.org)"
[ -n "$BOT_PASSWORD" ] || prompt BOT_PASSWORD "Bot password" 1

[ -n "$HOMESERVER_URL" ] || die $EXIT_ARGS "--homeserver-url is required"
[ -n "$BOT_USER" ] || die $EXIT_ARGS "--bot-user is required"
[ -n "$BOT_PASSWORD" ] || die $EXIT_ARGS "the bot password is required (--bot-password-file or SBCHAT_BOT_PASSWORD)"
[[ "$HOMESERVER_URL" =~ ^https?://[A-Za-z0-9.-]+(:[0-9]+)?/?$ ]] || die $EXIT_ARGS "--homeserver-url must look like https://matrix.example.org"
[[ "$BOT_USER" =~ ^@[a-z0-9._=/+-]+:[A-Za-z0-9.-]+(:[0-9]+)?$ ]] || die $EXIT_ARGS "--bot-user must look like @sms:matrix.example.org"
[ "${#BOT_PASSWORD}" -ge 12 ] || die $EXIT_ARGS "the bot password must be at least 12 characters"
[ -n "$NETWORK" ] || NETWORK=$(env_get SBCHAT_DOCKER_NETWORK)
[ -n "$NETWORK" ] || NETWORK=web
[[ "$NETWORK" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] || die $EXIT_ARGS "--docker-network must be a Docker network name"
[ -n "$VIRTUAL_HOST_NAME" ] || VIRTUAL_HOST_NAME=$(env_get VIRTUAL_HOST)
[ -n "$VIRTUAL_HOST_NAME" ] || VIRTUAL_HOST_NAME=$(printf '%s' "$HOMESERVER_URL" | sed -E 's#^https?://##; s#[:/].*$##')
[[ "$VIRTUAL_HOST_NAME" =~ ^[A-Za-z0-9.-]+$ ]] || die $EXIT_ARGS "--virtual-host must be a host name"
[ -z "$WEB_CALL_CENTER" ] || [[ "$WEB_CALL_CENTER" =~ ^[0-9]+$ ]] || die $EXIT_ARGS "--web-call-center must be a number"
[ -z "$WEB_PUBLIC" ] || [[ "$WEB_PUBLIC" =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?/?$ ]] || die $EXIT_ARGS "--web-public-url must look like https://chat.example.org"
for pair in "bot password:$BOT_PASSWORD" "web origins:$WEB_ORIGINS" "trusted proxies:$TRUSTED" "HTTP bind:$HTTP_BIND_ADDR" \
            "MAS client id:$MAS_CLIENT_ID" "MAS client secret:$MAS_CLIENT_SECRET"; do
    safe_value "${pair%%:*}" "${pair#*:}"
done

# .env: created from the example on first install; secrets generated once and never replaced
if [ ! -f "$ENV_FILE" ]; then
    cp "$ROOT/.env-example" "$ENV_FILE"
    say "Created $ENV_FILE"
fi
chmod 600 "$ENV_FILE"
[ -n "$(env_get PHONE_HASH_SECRET)" ] || { env_set PHONE_HASH_SECRET "$(openssl rand -hex 32)"; say "Generated PHONE_HASH_SECRET"; }
[ -n "$(env_get PHONE_ENC_KEY)" ] || { env_set PHONE_ENC_KEY "$(openssl rand -hex 32)"; say "Generated PHONE_ENC_KEY"; }
env_set HOMESERVER_URL "${HOMESERVER_URL%/}"
env_set MATRIX_USERNAME "$BOT_USER"
env_set MATRIX_PASSWORD "$BOT_PASSWORD"
env_set VIRTUAL_HOST "$VIRTUAL_HOST_NAME"
env_set SBCHAT_DOCKER_NETWORK "$NETWORK"
[ -z "$WEB_ORIGINS" ] || env_set WEB_ALLOWED_ORIGINS "$WEB_ORIGINS"
[ -z "$WEB_CALL_CENTER" ] || env_set WEB_CALL_CENTER_ID "$WEB_CALL_CENTER"
[ -z "$WEB_PUBLIC" ] || env_set WEB_PUBLIC_URL "${WEB_PUBLIC%/}"
[ -z "$TRUSTED" ] || env_set TRUSTED_PROXIES "$TRUSTED"
[ -z "$HTTP_BIND_ADDR" ] || env_set HTTP_BIND "$HTTP_BIND_ADDR"
[ -z "$MAS_CLIENT_ID" ] || env_set MAS_ADMIN_CLIENT_ID "$MAS_CLIENT_ID"
[ -z "$MAS_CLIENT_SECRET" ] || env_set MAS_ADMIN_CLIENT_SECRET "$MAS_CLIENT_SECRET"
chmod 600 "$ENV_FILE"
say "Configuration written to $ENV_FILE (mode 600)"

cd "$ROOT"
if ! docker network inspect "$NETWORK" >/dev/null 2>&1; then
    docker network create "$NETWORK" >/dev/null
    say "Created Docker network $NETWORK"
fi
say "Building the image (several minutes the first time)..."
docker compose build -q >&2 || die $EXIT_START "docker compose build failed"

# Admin API token (for SwitchBoard): printed by the bridge on stdout only, kept in a 0600 file
token=$(docker compose run --rm -T --no-deps "$CONTAINER" /app/switchboard-chat admin-token 2>/dev/null \
    | grep -E '^[a-z0-9]{128}$' | tail -n1 || true)
[ -n "$token" ] || die $EXIT_START "could not read the admin API token (see: docker compose run --rm $CONTAINER /app/switchboard-chat admin-token)"
( umask 077; printf '%s\n' "$token" > "$TOKEN_FILE" )
chmod 600 "$TOKEN_FILE"
unset token
say "Admin API token saved to $TOKEN_FILE (mode 600)"

if [ "$START" = 0 ]; then
    write_json configured "$(webhook_url)" false
    say "Configured; not started (--no-start). Start with: docker compose up -d"
    exit $EXIT_OK
fi

docker compose up -d --force-recreate >&2 || die $EXIT_START "docker compose up failed"
say "Waiting for the bridge to log in and sync (up to ${READY_TIMEOUT}s)..."
deadline=$((SECONDS + READY_TIMEOUT))
until bridge_ready; do
    if [ $SECONDS -ge $deadline ]; then
        write_json start_failed "$(webhook_url)" false
        die $EXIT_START "the bridge did not report ready in ${READY_TIMEOUT}s: docker logs $CONTAINER"
    fi
    sleep 3
done
write_json ok "$(webhook_url)" true
say "switchboard-chat is running. SMS webhook base URL: $(webhook_url)"
exit $EXIT_OK
