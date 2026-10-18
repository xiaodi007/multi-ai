#!/bin/sh
set -eu

# Keep the old script name while installing/upgrading only the canonical skill.
# Legacy skills and launchers are left for the host to review and remove.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
exec sh "$script_dir/install-skills.sh" --upgrade "$@"
