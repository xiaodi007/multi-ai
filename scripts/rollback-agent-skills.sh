#!/bin/sh
set -eu
umask 077
fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
if [ "$#" -eq 1 ] && { [ "$1" = --help ] || [ "$1" = -h ]; }; then
  printf '%s\n' 'Usage: rollback-agent-skills.sh BACKUP_DIR' 'Remove unchanged new skills or restore unchanged managed upgrades. Preserve later edits.'
  exit 0
fi
[ "$#" -eq 1 ] || fail 'Usage: rollback-agent-skills.sh BACKUP_DIR'
backup_dir=$1
[ -d "$backup_dir" ] && [ ! -L "$backup_dir" ] || fail 'Not an agent-skill backup'
[ "$(cat "$backup_dir/backup.kind")" = agent-skills ] || fail 'Unsupported backup'
case "$(cat "$backup_dir/manifest.version")" in 1|2) ;; *) fail 'Unsupported backup version' ;; esac
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
      "$match_target/SKILL.md"|"$match_target/scripts/run.sh"|"$match_target/agents/openai.yaml"|"$match_target/references/delegation.md"|"$match_target/references/route-traits.md")
        [ -f "$path" ] && [ ! -L "$path" ] || exit 1
        relative=${path#"$match_target/"}
        awk -F '\t' -v relative="$relative" '$1 == relative { found=1 } END { exit !found }' "$match_entry/files.tsv" || exit 1 ;;
      "$match_target/scripts"|"$match_target/agents") [ -d "$path" ] && [ ! -L "$path" ] || exit 1 ;;
      "$match_target/references")
        [ -d "$path" ] && [ ! -L "$path" ] || exit 1
        awk -F '\t' '$1 ~ /^references\// { found=1 } END { exit !found }' "$match_entry/files.tsv" || exit 1 ;;
      *) exit 1 ;;
    esac
  done || return 1
  while IFS="$(printf '\t')" read -r relative expected mode; do
    case "$relative" in SKILL.md|scripts/run.sh|agents/openai.yaml|references/delegation.md|references/route-traits.md) ;; *) return 1 ;; esac
    file="$match_target/$relative"
    if [ ! -e "$file" ] && [ ! -L "$file" ]; then
      [ "$(cat "$match_entry/status")" != complete ] || return 1
    else
      [ -f "$file" ] && [ ! -L "$file" ] || return 1
      [ "$(shasum -a 256 "$file" | awk '{print $1}')" = "$expected" ] && [ "$(stat -f '%Lp' "$file")" = "$mode" ] || return 1
    fi
  done < "$match_entry/files.tsv"
}
restore_upgrade() {
  upgrade_target=$1; upgrade_entry=$2
  safe_target "$upgrade_target" || { printf '%s\n' "Preserved changed target: $upgrade_target" >&2; return; }
  while IFS="$(printf '\t')" read -r relative expected mode; do
    case "$relative" in SKILL.md|scripts/run.sh|agents/openai.yaml|references/delegation.md|references/route-traits.md) ;; *) fail 'Invalid managed backup path' ;; esac
    file="$upgrade_target/$relative"; parent=$(dirname -- "$file")
    if [ -L "$parent" ] || [ -L "$file" ] || [ ! -f "$file" ] || [ "$(shasum -a 256 "$file" | awk '{print $1}')" != "$expected" ] || [ "$(stat -f '%Lp' "$file")" != "$mode" ]; then
      printf '%s\n' "Preserved later modification: $file" >&2; continue
    fi
    before=$(awk -F '\t' -v path="$relative" '$1 == path { print $2 "\t" $3 }' "$upgrade_entry/before.tsv")
    [ -n "$before" ] || fail 'Missing prior file identity'
    before_hash=$(printf '%s\n' "$before" | cut -f 1); before_mode=$(printf '%s\n' "$before" | cut -f 2)
    if [ "$before_hash" = absent ]; then
      rm -f "$file"
    else
      original="$upgrade_entry/before/$relative"
      [ -f "$original" ] && [ ! -L "$original" ] || fail 'Missing regular backup file'
      [ "$(shasum -a 256 "$original" | awk '{print $1}')" = "$before_hash" ] || fail 'Backup content hash mismatch'
      case "$before_mode" in 644|755) ;; *) fail 'Invalid prior file mode' ;; esac
      temporary="$parent/.multi-ai-rollback.$$"
      [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail 'Temporary path already exists'
      cp "$original" "$temporary"; chmod "$before_mode" "$temporary"
      [ ! -L "$file" ] && [ "$(shasum -a 256 "$file" | awk '{print $1}')" = "$expected" ] || { rm -f "$temporary"; fail 'File changed during rollback'; }
      mv "$temporary" "$file"
    fi
    printf '%s\n' "Restored managed file: $file"
  done < "$upgrade_entry/files.tsv"
  while IFS= read -r relative; do
    case "$relative" in scripts|agents|references) ;; *) fail 'Invalid created directory' ;; esac
    if [ -d "$upgrade_target/$relative" ] && [ ! -L "$upgrade_target/$relative" ]; then rmdir "$upgrade_target/$relative" 2>/dev/null || true; fi
  done < "$upgrade_entry/directories"
}
while [ "$count" -gt 0 ]; do
  entry="$backup_dir/entries/$count"; target=$(cat "$entry/path")
  if [ -f "$entry/status" ] && { [ -e "$target" ] || [ -L "$target" ]; }; then
    if [ -f "$entry/kind" ] && [ "$(cat "$entry/kind")" = upgrade ]; then
      restore_upgrade "$target" "$entry"
    elif safe_target "$target" && matches "$target" "$entry"; then
      # Recheck immediately before removal; changed or extra files preserve the whole skill.
      matches "$target" "$entry" || fail "Skill changed during rollback: $target"
      while IFS="$(printf '\t')" read -r relative expected mode; do rm -f "$target/$relative"; done < "$entry/files.tsv"
      for directory in "$target/scripts" "$target/agents" "$target/references" "$target"; do
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
