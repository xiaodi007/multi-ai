#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
install_home=${HOME:?HOME is required}
templates=''; codex_dir=''; claude_dir=''; platforms=both
usage() {
  cat <<'EOF'
Usage: install-skills.sh [--home DIR] [--codex-dir DIR] [--claude-dir DIR]
                         [--only codex|claude|both] [--templates DIR]
Install a new multi-ai skill; preserve an existing skill directory unchanged.
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
for relative in SKILL.md scripts/run.sh agents/openai.yaml; do
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
printf '%s\n' 1 > "$backup_dir/manifest.version"
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
install_skill() {
  target=$1
  if [ -e "$target" ] || [ -L "$target" ]; then
    printf '%s\n' "Preserved existing skill: $target"
    printf '%s\n' "$target" >> "$backup_dir/preserved.paths"
    return
  fi
  safe_path "$target"
  entry_count=$((entry_count + 1)); entry="$backup_dir/entries/$entry_count"
  mkdir "$entry"
  printf '%s\n' "$target" > "$entry/path"
  : > "$entry/files.tsv"
  for relative in scripts/run.sh agents/openai.yaml SKILL.md; do
    mode=644; [ "$relative" != scripts/run.sh ] || mode=755
    printf '%s\t%s\t%s\n' "$relative" "$(shasum -a 256 "$templates/$relative" | awk '{print $1}')" "$mode" >> "$entry/files.tsv"
  done
  printf '%s\n' "$entry_count" > "$backup_dir/entry_count"
  mkdir -p "$(dirname -- "$target")"
  mkdir "$target" || fail "Skill appeared during installation: $target"
  printf '%s\n' owned > "$entry/status"
  mkdir "$target/scripts" "$target/agents"
  # Publish the discoverable entry last, after the runner is ready.
  for relative in scripts/run.sh agents/openai.yaml SKILL.md; do
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
