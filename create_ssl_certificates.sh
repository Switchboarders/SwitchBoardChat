#!/bin/bash
set -eu
umask 077
echo "Creating ssl certificates"
# VIRTUAL_HOST comes from the container environment (env_file in docker-compose). Cron does
# not inherit it, so entrypoint.sh saves it in a file; this is not a secret.
if [ -z "${VIRTUAL_HOST:-}" ] && [ -f /app/data/virtual_host ]; then
    VIRTUAL_HOST="$(cat /app/data/virtual_host)"
fi
VIRTUAL_HOST="${VIRTUAL_HOST:-localhost}"
# A host name only: it goes into the certificate subject
if ! [[ "$VIRTUAL_HOST" =~ ^[A-Za-z0-9.-]+$ ]]; then
    echo "VIRTUAL_HOST is not a host name" >&2
    exit 1
fi
echo "$VIRTUAL_HOST" > /app/data/virtual_host
CERT_FILE="/app/data/web_server_keys/cert.pem"
KEY_FILE="/app/data/web_server_keys/key.pem"
DAYS_VALID=365 # Validity period for the new certificate

# Writes the new pair beside the old one, then swaps: a failure never leaves a mismatched pair
generate() {
    openssl req -x509 -newkey rsa:4096 -keyout "$KEY_FILE.new" -out "$CERT_FILE.new" -days "$DAYS_VALID" -sha256 -nodes -subj "/C=CA/ST=Quebec/L=Montreal/O=SwitchBoard-Chat/CN=$VIRTUAL_HOST" -addext "subjectAltName=DNS:$VIRTUAL_HOST"
    mv -f "$KEY_FILE.new" "$KEY_FILE"
    mv -f "$CERT_FILE.new" "$CERT_FILE"
}

# Check if the certificate file exists
if [ ! -f "$CERT_FILE" ]; then
    echo "Certificate file not found. Generating a new one."
    # Generate new certificate
    generate
else
    # Check if the existing certificate has expired
    if openssl x509 -in "$CERT_FILE" -checkend 2000000 > /dev/null 2>&1; then
    echo "Certificate is still valid."
    else
    echo "Certificate has expired. Generating a new one."
    # Generate new certificate
    generate
    fi
fi
