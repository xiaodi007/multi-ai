#!/bin/sh
set -eu
umask 077

fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
warn() { printf '%s\n' "Preserved modified entry: $*" >&2; }
if [ "$#" -eq 1 ] && { [ "$1" = --help ] || [ "$1" = -h ]; }; then
  printf '%s\n' 'Usage: rollback.sh BACKUP_DIR' 'Restores installation-owned entries; later user edits are preserved.'
  exit 0
fi
[ "$#" -eq 1 ] || fail 'Usage: rollback.sh BACKUP_DIR'
backup_dir=$1
[ -d "$backup_dir" ] && [ -f "$backup_dir/manifest.version" ] || fail 'Not an installation backup'
[ "$(cat "$backup_dir/manifest.version")" = 1 ] || fail 'Unsupported backup version'
[ "$(cat "$backup_dir/uid")" = "$(id -u)" ] || fail 'Backup belongs to another user'
install_home=$(cat "$backup_dir/home")
config_dir=$(cat "$backup_dir/config_dir")
installed=$(cat "$backup_dir/installed_binary")
version=$(cat "$backup_dir/version")
[ "$install_home" = "${HOME:?HOME is required}" ] || fail 'Use the HOME of the original installation'
bin_dir="$install_home/.local/bin"
[ "$installed" = "$install_home/.local/lib/multi-ai/$version/multi-ai" ] || fail 'Invalid binary target in backup'

restore() {
  target=$1; snapshot=$2
  kind=$(cat "$snapshot.kind")
  case "$kind" in
    absent) rm -f "$target" ;;
    file|symlink)
      [ -f "$snapshot.before" ] || [ -L "$snapshot.before" ] || fail "Missing snapshot: $snapshot.before"
      rm -f "$target"
      cp -pP "$snapshot.before" "$target" ;;
    *) fail "Invalid snapshot type: $kind" ;;
  esac
}
matches_snapshot() {
  target=$1; snapshot=$2
  case "$(cat "$snapshot.kind")" in
    absent) [ ! -e "$target" ] && [ ! -L "$target" ] ;;
    symlink) [ -L "$target" ] && [ "$(readlink "$target")" = "$(readlink "$snapshot.before")" ] ;;
    file) [ -f "$target" ] && [ ! -L "$target" ] && cmp -s "$target" "$snapshot.before" && [ "$(stat -f '%Lp' "$target")" = "$(stat -f '%Lp' "$snapshot.before")" ] ;;
    *) return 1 ;;
  esac
}
for name in multi-ai ai-review llm-run; do
  target="$bin_dir/$name"
  if [ -L "$target" ] && [ "$(readlink "$target")" = "$installed" ]; then
    restore "$target" "$backup_dir/entries/$name"
  elif matches_snapshot "$target" "$backup_dir/entries/$name"; then
    :
  elif [ -e "$target" ] || [ -L "$target" ]; then
    # Already restored or replaced by the user: leave it exactly as it is.
    warn "$target"
  elif [ ! -f "$backup_dir/install.status" ]; then
    # Failed installation may have removed an old entry immediately before ln failed.
    restore "$target" "$backup_dir/entries/$name"
  fi
done

for name in profiles.json routes.json; do
  snapshot="$backup_dir/config/$name"
  target="$config_dir/$name"
  # Existing configuration was never overwritten, so rollback does not rewrite it.
  if [ "$(cat "$snapshot.kind")" = absent ] && [ -f "$snapshot.after.sha256" ]; then
    expected=$(cat "$snapshot.after.sha256")
    if [ -f "$target" ] && [ ! -L "$target" ] && [ "$(shasum -a 256 "$target" | awk '{print $1}')" = "$expected" ]; then
      rm -f "$target"
    elif [ -e "$target" ] || [ -L "$target" ]; then
      warn "$target"
    fi
  fi
done

if [ -f "$installed" ] && [ ! -L "$installed" ] && [ "$(shasum -a 256 "$installed" | awk '{print $1}')" = "$(cat "$backup_dir/binary.after.sha256")" ]; then
  restore "$installed" "$backup_dir/binary"
elif matches_snapshot "$installed" "$backup_dir/binary"; then
  :
elif [ -e "$installed" ] || [ -L "$installed" ]; then
  warn "$installed"
fi
if [ "$(cat "$backup_dir/binary.kind")" = absent ]; then rmdir "$install_home/.local/lib/multi-ai/$version" 2>/dev/null || true; fi
printf '%s\n' "Rollback complete. Backup retained at $backup_dir"
