#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
binary=''
legacy_profiles="${HOME:?HOME is required}/.claude/skills/multi-llm-pipeline/references/model-profiles.json"
legacy_explicit=0
usage() {
  cat <<'EOF'
Usage: install.sh [--binary FILE] [--legacy-profiles FILE]
Install multi-ai on macOS Apple Silicon. Existing config is preserved.
The successful installation prints BACKUP_DIR for rollback.sh.
EOF
}
fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary|--legacy-profiles)
      option=$1; shift; [ "$#" -gt 0 ] || fail "$option needs a file"
      if [ "$option" = --binary ]; then binary=$1; else legacy_profiles=$1; legacy_explicit=1; fi ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
  shift
done
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || fail "This release is tested only on macOS arm64"
[ -f "$script_dir/rollback.sh" ] || fail "rollback.sh must accompany install.sh"
if [ "$legacy_explicit" -eq 1 ] && [ ! -f "$legacy_profiles" ]; then fail "Legacy profiles file not found: $legacy_profiles"; fi
if [ -z "$binary" ]; then
  for candidate in "$script_dir/bin/multi-ai" "$script_dir/multi-ai" "$script_dir/../target/aarch64-apple-darwin/release/multi-ai" "$script_dir/../target/release/multi-ai"; do
    if [ -f "$candidate" ]; then binary=$candidate; break; fi
  done
fi
[ -n "$binary" ] && [ -f "$binary" ] && [ -x "$binary" ] || fail "Pass --binary with a built executable"
case "$binary" in /*) ;; *) binary="$PWD/$binary" ;; esac
case "$(file -b "$binary")" in *Mach-O*arm64*) ;; *) fail "The executable must be a macOS arm64 Mach-O binary" ;; esac
version=$("$binary" --version | awk 'NR == 1 && $1 == "multi-ai" {print $2}')
case "$version" in [0-9]*) ;; *) fail "Invalid binary version" ;; esac
case "$version" in *[!0-9A-Za-z._+-]*) fail "Unsafe binary version" ;; esac

install_home=${HOME:?HOME is required}
bin_dir="$install_home/.local/bin"
config_dir=${MULTI_AI_CONFIG_DIR:-"$install_home/.config/multi-ai"}
state_dir=${MULTI_AI_STATE_DIR:-"$install_home/.local/state/multi-ai"}
case "$config_dir" in /*) ;; *) config_dir="$PWD/$config_dir" ;; esac
case "$state_dir" in /*) ;; *) state_dir="$PWD/$state_dir" ;; esac
lib_dir="$install_home/.local/lib/multi-ai/$version"
installed="$lib_dir/multi-ai"

safe_path() {
  case "$1" in *'
'*|*"$(printf '\t')"*) fail "Paths containing tabs or newlines are unsupported" ;; esac
}
for value in "$install_home" "$bin_dir" "$config_dir" "$state_dir" "$lib_dir" "$binary" "$legacy_profiles"; do safe_path "$value"; done
for name in multi-ai ai-review llm-run; do
  if [ -d "$bin_dir/$name" ] && [ ! -L "$bin_dir/$name" ]; then fail "Refusing to replace directory: $bin_dir/$name"; fi
done
for name in profiles.json routes.json; do
  if [ -d "$config_dir/$name" ] && [ ! -L "$config_dir/$name" ]; then fail "Configuration is a directory: $config_dir/$name"; fi
done
if [ -d "$installed" ] && [ ! -L "$installed" ]; then fail "Binary target is a directory: $installed"; fi

mkdir -p "$state_dir/backups"
chmod 700 "$state_dir/backups"
backup_dir=$(mktemp -d "$state_dir/backups/install-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXX")
chmod 700 "$backup_dir"
mkdir "$backup_dir/entries" "$backup_dir/config"
printf '%s\n' 1 > "$backup_dir/manifest.version"
printf '%s\n' "$install_home" > "$backup_dir/home"
printf '%s\n' "$config_dir" > "$backup_dir/config_dir"
printf '%s\n' "$installed" > "$backup_dir/installed_binary"
printf '%s\n' "$version" > "$backup_dir/version"
printf '%s\n' "$(id -u)" > "$backup_dir/uid"

snapshot() {
  original=$1; destination=$2
  if [ -L "$original" ]; then
    printf '%s\n' symlink > "$destination.kind"
    cp -pP "$original" "$destination.before"
  elif [ -f "$original" ]; then
    printf '%s\n' file > "$destination.kind"
    cp -pP "$original" "$destination.before"
  elif [ -e "$original" ]; then
    fail "Unsupported existing entry: $original"
  else
    printf '%s\n' absent > "$destination.kind"
  fi
}
for name in multi-ai ai-review llm-run; do snapshot "$bin_dir/$name" "$backup_dir/entries/$name"; done
for name in profiles.json routes.json; do snapshot "$config_dir/$name" "$backup_dir/config/$name"; done
snapshot "$installed" "$backup_dir/binary"
shasum -a 256 "$binary" | awk '{print $1}' > "$backup_dir/binary.after.sha256"

install_started=0
temporary_binary=''
on_exit() {
  status=$1
  trap - 0 INT TERM
  if [ -n "$temporary_binary" ] && [ -f "$temporary_binary" ]; then rm -f "$temporary_binary"; fi
  if [ "$status" -ne 0 ]; then
    printf '%s\n' "Installation failed. BACKUP_DIR=$backup_dir" >&2
    if [ "$install_started" -eq 1 ]; then sh "$script_dir/rollback.sh" "$backup_dir" >&2 || true; fi
  fi
  exit "$status"
}
trap 'on_exit $?' 0
trap 'exit 130' INT
trap 'exit 143' TERM

install_started=1
mkdir -p "$bin_dir" "$lib_dir" "$config_dir"
temporary_binary="$lib_dir/.multi-ai.install.$$"
cp -p "$binary" "$temporary_binary"
chmod 755 "$temporary_binary"
mv -f "$temporary_binary" "$installed"
temporary_binary=''
config_status=0
if [ -f "$legacy_profiles" ] && [ ! -e "$config_dir/profiles.json" ] && [ ! -L "$config_dir/profiles.json" ]; then
  "$installed" install-config --from "$legacy_profiles" > "$backup_dir/install-config.log" 2>&1 || config_status=$?
else
  "$installed" install-config > "$backup_dir/install-config.log" 2>&1 || config_status=$?
fi
for name in profiles.json routes.json; do
  if [ "$(cat "$backup_dir/config/$name.kind")" = absent ] && [ -f "$config_dir/$name" ] && [ ! -L "$config_dir/$name" ]; then
    chmod 600 "$config_dir/$name"
    shasum -a 256 "$config_dir/$name" | awk '{print $1}' > "$backup_dir/config/$name.after.sha256"
  fi
done
[ "$config_status" -eq 0 ] || fail "Config import failed; see $backup_dir/install-config.log"

for name in multi-ai ai-review llm-run; do
  rm -f "$bin_dir/$name"
  ln -s "$installed" "$bin_dir/$name"
done
printf '%s\n' complete > "$backup_dir/install.status"
printf '%s\n' "Installed multi-ai $version at $installed" "BACKUP_DIR=$backup_dir"
case ":${PATH:-}:" in *":$bin_dir:"*) ;; *) printf '%s\n' "Add $bin_dir to PATH to use the installed commands." ;; esac
