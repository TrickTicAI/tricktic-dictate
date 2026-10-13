#!/usr/bin/env bash
# Runs a bounded private-worker smoke and stops its owned workers.
set -euo pipefail
exec python3 "$(dirname "$0")/start-test-worker.py" "$@"
