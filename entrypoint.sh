#!/bin/bash
set -euo pipefail
# Everything the bridge writes (database, session, crypto store, logs, keys) is owner-only
umask 077
mkdir -p /app/data/web_server_keys /app/data/logs /app/data/db
/app/create_ssl_certificates.sh
# The data volume may come from an older image that ran as root
chown -R sb-chat:sb-chat /app/data
chmod -R go-rwx /app/data
# Certificate renewal (daemon, root)
cron
exec setpriv --reuid=sb-chat --regid=sb-chat --init-groups --inh-caps=-all --bounding-set=-all "$@"
