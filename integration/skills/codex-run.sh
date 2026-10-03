#!/bin/sh
set -eu
exec "${HOME:?HOME is required}/.claude/skills/multi-llm-pipeline/bin/run.sh" "$@"
