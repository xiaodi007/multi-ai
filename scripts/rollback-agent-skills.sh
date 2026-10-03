#!/bin/sh
set -eu
umask 077
fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
if [ "$#" -eq 1 ] && { [ "$1" = --help ] || [ "$1" = -h ]; }; then
  printf '%s\n' 'Usage: rollback-agent-skills.sh BACKUP_DIR' 'Remove only newly installed skills whose files remain unchanged. Existing skills stay untouched.'
  exit 0
fi
[ "$#" -eq 1 ] || fail 'Usage: rollback-agent-skills.sh BACKUP_DIR'
backup_dir=$1
[ -d "$backup_dir" ] && [ ! -L "$backup_dir" ] || fail 'Not an agent-skill backup'
[ "$(cat "$backup_dir/backup.kind")" = agent-skills ] && [ "$(cat "$backup_dir/manifest.version")" = 1 ] || fail 'Unsupported backup'
[ "$(cat "$backup_dir/uid")" = "$(id -u)" ] || fail 'Backup belongs to another user'
install_home=$(cat "$backup_dir/home")
[ -d "$install_home" ] && [ "$(CDPATH= cd -- "$install_home" && pwd -P)" = "$install_home" ] || fail 'Original home has changed'
[ "$install_home" != / ] || fail 'Invalid home'
count=$(cat "$backup_dir/entry_count")
case "$count" in 0|1|2) ;; *) fail 'Invalid entry count' ;; esac
safe_target() {
  case "$1" in "$install_home"/*/multi-ai) ;; *) return 1 ;; esac
  case "$1" in *'
'*|*"$(printf '\t')"*|*/../*|*/./*|*//*) return 1 ;; esac
  ancestor=$(dirname -- "$1")
  while [ "$ancestor" != "$install_home" ]; do
    [ ! -L "$ancestor" ] || return 1
    ancestor=$(dirname -- "$ancestor")
  done
  [ ! -L "$1" ] && [ -d "$1" ]
}
matches() {
  match_target=$1; match_entry=$2
  # Any unknown file, nested directory or symlink makes the whole skill user-owned.
  find "$match_target" -mindepth 1 -print | while IFS= read -r path; do
    case "$path" in
      "$match_target/SKILL.md"|"$match_target/scripts/run.sh"|"$match_target/agents/openai.yaml") [ -f "$path" ] && [ ! -L "$path" ] || exit 1 ;;
      "$match_target/scripts"|"$match_target/agents") [ -d "$path" ] && [ ! -L "$path" ] || exit 1 ;;
      *) exit 1 ;;
    esac
  done || return 1
  while IFS="$(printf '\t')" read -r relative expected mode; do
    case "$relative" in SKILL.md|scripts/run.sh|agents/openai.yaml) ;; *) return 1 ;; esac
    file="$match_target/$relative"
    if [ ! -e "$file" ] && [ ! -L "$file" ]; then
      [ "$(cat "$match_entry/status")" != complete ] || return 1
    else
      [ -f "$file" ] && [ ! -L "$file" ] || return 1
      [ "$(shasum -a 256 "$file" | awk '{print $1}')" = "$expected" ] && [ "$(stat -f '%Lp' "$file")" = "$mode" ] || return 1
    fi
  done < "$match_entry/files.tsv"
}
while [ "$count" -gt 0 ]; do
  entry="$backup_dir/entries/$count"; target=$(cat "$entry/path")
  if [ -f "$entry/status" ] && { [ -e "$target" ] || [ -L "$target" ]; }; then
    if safe_target "$target" && matches "$target" "$entry"; then
      # Recheck immediately before removal; changed or extra files preserve the whole skill.
      matches "$target" "$entry" || fail "Skill changed during rollback: $target"
      rm -f "$target/SKILL.md" "$target/scripts/run.sh" "$target/agents/openai.yaml"
      for directory in "$target/scripts" "$target/agents" "$target"; do
        if [ -d "$directory" ]; then rmdir "$directory"; fi
      done
      printf '%s\n' "Removed unchanged installed skill: $target"
    else
      printf '%s\n' "Preserved later modification: $target" >&2
    fi
  fi
  count=$((count - 1))
done
printf '%s\n' restored > "$backup_dir/rollback.status"
printf '%s\n' "Agent skill rollback complete. Backup retained at $backup_dir"
