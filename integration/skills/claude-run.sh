#!/bin/sh
set -eu
exec "${HOME:?HOME is required}/.local/bin/llm-run" "$@"
