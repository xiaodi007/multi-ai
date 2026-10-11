#!/bin/sh
set -eu
if [ -n "${MULTI_AI_BIN:-}" ]; then exec "$MULTI_AI_BIN" "$@"; fi
if [ -x "${HOME:?HOME is required}/.local/bin/multi-ai" ]; then
  exec "$HOME/.local/bin/multi-ai" "$@"
fi
exec multi-ai "$@"
