#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
install_home=${HOME:?HOME is required}
templates=''; codex_dir=''; claude_dir=''; platforms=both; upgrade=0
managed_paths='scripts/run.sh agents/openai.yaml references/delegation.md references/route-traits.md SKILL.md'
usage() {
  cat <<'EOF'
Usage: install-skills.sh [--home DIR] [--codex-dir DIR] [--claude-dir DIR]
                         [--only codex|claude|both] [--templates DIR] [--upgrade]
Install a new multi-ai skill; preserve an existing skill directory unchanged.
--upgrade backs up recognized v0.1/current managed files and updates them;
user changes and unknown files remain, with a merge guide in the backup.
Defaults: ~/.agents/skills (Codex), ~/.claude/skills (Claude Code).
Use --codex-dir ~/.codex/skills for a host that discovers the older location.
Custom skill directories must be inside the selected home. No keys are copied.
Rollback: rollback-agent-skills.sh BACKUP_DIR
EOF
}
fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --home|--codex-dir|--claude-dir|--only|--templates)
      option=$1; shift; [ "$#" -gt 0 ] || fail "$option needs a value"
      case "$option" in
        --home) install_home=$1 ;; --codex-dir) codex_dir=$1 ;; --claude-dir) claude_dir=$1 ;;
        --only) platforms=$1 ;; --templates) templates=$1 ;;
      esac ;;
    --help|-h) usage; exit 0 ;;
    --upgrade) upgrade=1 ;;
    *) fail "Unknown argument: $1" ;;
  esac
  shift
done
case "$platforms" in both|codex|claude) ;; *) fail '--only must be codex, claude, or both' ;; esac
[ -d "$install_home" ] || fail 'Home must exist'
install_home=$(CDPATH= cd -- "$install_home" && pwd -P)
[ "$install_home" != / ] || fail 'Home cannot be the filesystem root'
[ -n "$codex_dir" ] || codex_dir="$install_home/.agents/skills"
[ -n "$claude_dir" ] || claude_dir="$install_home/.claude/skills"
if [ -z "$templates" ]; then
  if [ -d "$script_dir/integration/skills/multi-ai" ]; then templates="$script_dir/integration/skills/multi-ai"
  else templates="$script_dir/../integration/skills/multi-ai"; fi
fi
[ -f "$script_dir/rollback-agent-skills.sh" ] || fail 'rollback-agent-skills.sh must accompany this script'
for relative in $managed_paths; do
  [ -f "$templates/$relative" ] && [ ! -L "$templates/$relative" ] || fail "Missing regular template: $relative"
done
safe_path() {
  case "$1" in *'
'*|*"$(printf '\t')"*) fail 'Tabs and newlines in paths are unsupported' ;; esac
  case "$1" in "$install_home"/*) ;; *) fail "Target is outside the selected home: $1" ;; esac
  case "$1" in */../*|*/./*|*/..|*/.|*//* ) fail "Use a normalized absolute path: $1" ;; esac
  ancestor=$(dirname -- "$1")
  while [ "$ancestor" != "$install_home" ]; do
    [ ! -L "$ancestor" ] || fail "Refusing a symlinked parent: $ancestor"
    ancestor=$(dirname -- "$ancestor")
  done
}
case "$platforms" in codex|both) safe_path "$codex_dir/multi-ai" ;; esac
case "$platforms" in claude|both) safe_path "$claude_dir/multi-ai" ;; esac
safe_path "$install_home/.local/state/multi-ai/backups/agent-skills"
mkdir -p "$install_home/.local/state/multi-ai/backups"
backup_dir=$(mktemp -d "$install_home/.local/state/multi-ai/backups/agent-skills-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXX")
mkdir "$backup_dir/entries"
printf '%s\n' agent-skills > "$backup_dir/backup.kind"
printf '%s\n' 2 > "$backup_dir/manifest.version"
printf '%s\n' "$(id -u)" > "$backup_dir/uid"
printf '%s\n' "$install_home" > "$backup_dir/home"
printf '%s\n' 0 > "$backup_dir/entry_count"
printf '%s\n' installing > "$backup_dir/install.status"
entry_count=0; temporary=''
on_exit() {
  status=$1; trap - 0 INT TERM
  if [ -n "$temporary" ] && { [ -e "$temporary" ] || [ -L "$temporary" ]; }; then rm -f "$temporary"; fi
  if [ "$status" -ne 0 ]; then
    printf '%s\n' "Skill installation failed. BACKUP_DIR=$backup_dir" >&2
    sh "$script_dir/rollback-agent-skills.sh" "$backup_dir" >&2 || true
  fi
  exit "$status"
}
trap 'on_exit $?' 0
trap 'exit 130' INT
trap 'exit 143' TERM
known_old_hash() {
  # Published v0.1 templates; local stamps cannot turn user files into managed files.
  case "$1" in
    SKILL.md) printf '%s\n' aecf72ade60b1efe09937520eea23ef0b1d83e364ea61caa0b14aa1c3adf38e2 ;;
    scripts/run.sh) printf '%s\n' 61ad395a814f5476a2003a9e9168e1e2e11f42253f118975a4054c8f7e4b19ac ;;
    agents/openai.yaml) printf '%s\n' 424c5c3e6a673125585e40562b6ffd291bd49d3b3a97c05fdf0e1ab7913b8aee ;;
    *) printf '%s\n' unknown ;;
  esac
}
merge_note() {
  printf '%s\n' "Preserved user file: $1" >&2
  printf '%s\n' "- Preserved $1. Compare with $2 and merge deliberately; do not overwrite the directory." >> "$backup_dir/MERGE.md"
}
upgrade_skill() {
  target=$1; entry=''
  for relative in $managed_paths; do
    safe_path "$target/$relative"
    mode=644; [ "$relative" != scripts/run.sh ] || mode=755
    expected=$(shasum -a 256 "$templates/$relative" | awk '{print $1}')
    before_hash=absent; before_mode=absent
    if [ -e "$target/$relative" ] || [ -L "$target/$relative" ]; then
      if [ ! -f "$target/$relative" ] || [ -L "$target/$relative" ]; then
        merge_note "$target/$relative" "$templates/$relative"; continue
      fi
      before_hash=$(shasum -a 256 "$target/$relative" | awk '{print $1}')
      before_mode=$(stat -f '%Lp' "$target/$relative")
      if [ "$before_hash" = "$expected" ] && [ "$before_mode" = "$mode" ]; then continue; fi
      old_hash=$(known_old_hash "$relative")
      if [ "$before_mode" != "$mode" ] || { [ "$before_hash" != "$old_hash" ] && [ "$before_hash" != "$expected" ]; }; then
        merge_note "$target/$relative" "$templates/$relative"; continue
      fi
    fi
    if [ -z "$entry" ]; then
      entry_count=$((entry_count + 1)); entry="$backup_dir/entries/$entry_count"
      mkdir "$entry" "$entry/before"
      printf '%s\n' "$target" > "$entry/path"
      printf '%s\n' upgrade > "$entry/kind"
      : > "$entry/files.tsv"; : > "$entry/before.tsv"; : > "$entry/directories"
      printf '%s\n' owned > "$entry/status"
      printf '%s\n' "$entry_count" > "$backup_dir/entry_count"
    fi
    if [ "$before_hash" != absent ]; then
      mkdir -p "$(dirname -- "$entry/before/$relative")"
      cp -p "$target/$relative" "$entry/before/$relative"
      [ "$(shasum -a 256 "$entry/before/$relative" | awk '{print $1}')" = "$before_hash" ] || fail 'File changed during backup'
    fi
    printf '%s\t%s\t%s\n' "$relative" "$before_hash" "$before_mode" >> "$entry/before.tsv"
    printf '%s\t%s\t%s\n' "$relative" "$expected" "$mode" >> "$entry/files.tsv"
    parent=$(dirname -- "$target/$relative")
    if [ ! -d "$parent" ]; then mkdir "$parent"; printf '%s\n' "${parent#"$target/"}" >> "$entry/directories"; fi
    temporary="$parent/.multi-ai-install.$$"
    [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail 'Temporary path already exists'
    cp "$templates/$relative" "$temporary"; chmod "$mode" "$temporary"
    if [ "$before_hash" = absent ]; then
      [ ! -e "$target/$relative" ] && [ ! -L "$target/$relative" ] || fail 'File appeared during installation'
    else
      [ ! -L "$target/$relative" ] && [ "$(shasum -a 256 "$target/$relative" | awk '{print $1}')" = "$before_hash" ] || fail 'File changed during installation'
    fi
    mv "$temporary" "$target/$relative"; temporary=''
  done
  if [ -n "$entry" ]; then printf '%s\n' complete > "$entry/status"; printf '%s\n' "Upgraded managed skill files: $target"
  else printf '%s\n' "No managed changes: $target"; fi
}
install_skill() {
  target=$1
  if [ -e "$target" ] || [ -L "$target" ]; then
    if [ "$upgrade" -eq 1 ] && [ -d "$target" ] && [ ! -L "$target" ]; then
      upgrade_skill "$target"; return
    fi
    printf '%s\n' "Preserved existing skill: $target"
    printf '%s\n' "$target" >> "$backup_dir/preserved.paths"
    return
  fi
  safe_path "$target"
  entry_count=$((entry_count + 1)); entry="$backup_dir/entries/$entry_count"
  mkdir "$entry"
  printf '%s\n' "$target" > "$entry/path"
  printf '%s\n' new > "$entry/kind"
  : > "$entry/files.tsv"
  for relative in $managed_paths; do
    mode=644; [ "$relative" != scripts/run.sh ] || mode=755
    printf '%s\t%s\t%s\n' "$relative" "$(shasum -a 256 "$templates/$relative" | awk '{print $1}')" "$mode" >> "$entry/files.tsv"
  done
  printf '%s\n' "$entry_count" > "$backup_dir/entry_count"
  mkdir -p "$(dirname -- "$target")"
  mkdir "$target" || fail "Skill appeared during installation: $target"
  printf '%s\n' owned > "$entry/status"
  mkdir "$target/scripts" "$target/agents" "$target/references"
  # Publish the discoverable entry last, after the runner is ready.
  for relative in $managed_paths; do
    mode=644; [ "$relative" != scripts/run.sh ] || mode=755
    temporary="$(dirname -- "$target/$relative")/.multi-ai-install.$$"
    [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail 'Temporary path already exists'
    cp "$templates/$relative" "$temporary"; chmod "$mode" "$temporary"
    mv "$temporary" "$target/$relative"; temporary=''
  done
  printf '%s\n' complete > "$entry/status"
  printf '%s\n' "Installed skill: $target"
}
case "$platforms" in codex|both) install_skill "$codex_dir/multi-ai" ;; esac
case "$platforms" in claude|both) install_skill "$claude_dir/multi-ai" ;; esac
printf '%s\n' complete > "$backup_dir/install.status"
printf '%s\n' "BACKUP_DIR=$backup_dir" 'Codex: $multi-ai or /skills. Claude Code: /multi-ai.' 'If the skill is not visible, start a new agent session.'
if [ -f "$backup_dir/MERGE.md" ]; then printf '%s\n' "MERGE_GUIDE=$backup_dir/MERGE.md"; fi
