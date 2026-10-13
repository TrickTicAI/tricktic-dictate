#!/usr/bin/env bash
set -euo pipefail
# Tauri overrides NSISDIR with its plugin-only cache. Keep that cache untouched.
export NSISDIR="${WRITER_NSIS_ROOT:?Full NSIS toolset required}"
export NSISCONFDIR="$NSISDIR"
exec "${WRITER_MAKENSIS:?Absolute makensis executable required}" "$@"
