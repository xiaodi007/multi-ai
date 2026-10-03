#!/bin/sh
set -eu
umask 077

fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
warn() { printf '%s\n' "Preserved later modification: $*" >&2; }
if [ "$#" -eq 1 ] && { [ "$1" = --help ] || [ "$1" = -h ]; }; then
  printf '%s\n' 'Usage: rollback-skills.sh BACKUP_DIR' 'Restores launcher/skill entries without touching later user edits. Repeat safely.'
  exit 0
fi
[ "$#" -eq 1 ] || fail 'Usage: rollback-skills.sh BACKUP_DIR'
backup_dir=$1
[ -d "$backup_dir" ] && [ ! -L "$backup_dir" ] && [ -f "$backup_dir/manifest.version" ] || fail 'Not a skill backup'
[ "$(cat "$backup_dir/manifest.version")" = 1 ] || fail 'Unsupported backup version'
[ "$(cat "$backup_dir/uid")" = "$(id -u)" ] || fail 'Backup belongs to another user'
install_home=$(cat "$backup_dir/home")
case "$install_home" in /*) ;; *) fail 'Invalid home in backup' ;; esac
case "$install_home" in *'
'*|*"$(printf '\t')"*) fail 'Invalid home in backup' ;; esac
[ -d "$install_home" ] && [ "$(CDPATH= cd -- "$install_home" && pwd -P)" = "$install_home" ] || fail 'Original home directory has changed'
count=0
if [ -f "$backup_dir/entry_count" ]; then count=$(cat "$backup_dir/entry_count"); fi
case "$count" in 0|1|2|3|4|5|6) ;; *) fail 'Invalid entry count in backup' ;; esac
safe_target() {
  target=$1
  ancestor=$(dirname -- "$target")
  while [ "$ancestor" != "$install_home" ]; do
    [ "$ancestor" != / ] || fail "Target escaped home: $target"
    if [ -L "$ancestor" ]; then warn "$target (parent became a symlink)"; return 1; fi
    ancestor=$(dirname -- "$ancestor")
  done
  if [ -d "$target" ] || { [ -e "$target" ] && [ ! -f "$target" ] && [ ! -L "$target" ]; }; then warn "$target (no longer a file)"; return 1; fi
}
matches() {
  match_target=$1; match_entry=$2; match_phase=$3
  [ -f "$match_entry/$match_phase.kind" ] && [ -f "$match_entry/$match_phase.mode" ] || return 1
  case "$(cat "$match_entry/$match_phase.kind")" in
    symlink) [ -L "$match_target" ] && [ "$(readlink "$match_target")" = "$(cat "$match_entry/$match_phase.target")" ] && [ "$(stat -f '%Lp' "$match_target")" = "$(cat "$match_entry/$match_phase.mode")" ] ;;
    file) [ -f "$match_target" ] && [ ! -L "$match_target" ] && [ "$(shasum -a 256 "$match_target" | awk '{print $1}')" = "$(cat "$match_entry/$match_phase.sha256")" ] && [ "$(stat -f '%Lp' "$match_target")" = "$(cat "$match_entry/$match_phase.mode")" ] ;;
    *) return 1 ;;
  esac
}
index=$count
temporary=''
cleanup() { status=$1; trap - 0 INT TERM; if [ -n "$temporary" ] && { [ -e "$temporary" ] || [ -L "$temporary" ]; }; then rm -f "$temporary"; fi; exit "$status"; }
trap 'cleanup $?' 0
trap 'exit 130' INT
trap 'exit 143' TERM
while [ "$index" -gt 0 ]; do
  entry="$backup_dir/entries/$index"
  [ -f "$entry/path" ] || fail "Missing manifest entry: $index"
  relative=$(cat "$entry/path")
  case "$relative" in
    .claude/bin/ai-review|.claude/skills/ai-review/SKILL.md|.claude/skills/multi-llm-pipeline/SKILL.md|.claude/skills/multi-llm-pipeline/bin/run.sh|.codex/skills/multi-llm-pipeline/SKILL.md|.codex/skills/multi-llm-pipeline/bin/run.sh) ;;
    *) fail 'Unknown path in skill manifest' ;;
  esac
  target="$install_home/$relative"
  if safe_target "$target"; then
    if matches "$target" "$entry" before; then
      :
    elif matches "$target" "$entry" after; then
      matches "$entry/before" "$entry" before || fail "Damaged original snapshot: $relative"
      temporary="$(dirname -- "$target")/.multi-ai-skills-restore.$$.$index"
      [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail "Temporary entry already exists: $temporary"
      cp -pP "$entry/before" "$temporary"
      # Verify both snapshot and current entry immediately before publication.
      matches "$target" "$entry" after || fail "Entry changed during rollback: $relative"
      mv -f "$temporary" "$target"
      temporary=''
      printf '%s\n' "Restored: $target"
    else
      warn "$target"
    fi
  fi
  index=$((index - 1))
done
printf '%s\n' restored > "$backup_dir/rollback.status"
printf '%s\n' "Skill rollback complete. Backup retained at $backup_dir"
