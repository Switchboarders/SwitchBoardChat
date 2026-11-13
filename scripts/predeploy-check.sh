#!/bin/sh
# Run before every deploy: unit tests, a fresh image, and the end-to-end suite (a throwaway Synapse in Docker,
# simulated respondents with real encryption, the real widget). About 20 minutes. Nothing touches production.
#
#   scripts/predeploy-check.sh            full run
#   scripts/predeploy-check.sh quick      unit tests + the short end-to-end scenarios (no 90 s typing test, no incident replay)
set -eu
cd "$(dirname "$0")/.."
cargo test
docker build -t sbchat:e2e .
(cd tools/crypto-doctor && cargo build)
(cd tests/e2e/respondent-sim && cargo build)
cd tests/e2e
if [ "${1:-}" = quick ]; then
  python3 run.py bringup roundtrip second_device reload widget commands end restart
else
  python3 run.py
fi
