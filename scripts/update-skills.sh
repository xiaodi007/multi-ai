#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
install_home=${HOME:?HOME is required}
templates=''
usage() {
  cat <<'USAGE'
Usage: update-skills.sh [--home DIR] [--templates DIR]
Update existing ai-review and multi-llm-pipeline launchers/instructions after install.sh.
Missing skills remain missing. Prints BACKUP_DIR for rollback-skills.sh.
USAGE
}
fail() { printf '%s\n' "Error: $*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --home|--templates)
      option=$1; shift; [ "$#" -gt 0 ] || fail "$option needs a directory"
      if [ "$option" = --home ]; then install_home=$1; else templates=$1; fi ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
  shift
done
[ "$(uname -s)" = Darwin ] || fail 'This script is tested only on macOS'
safe_path() {
  case "$1" in *'
'*|*"$(printf '\t')"*) fail 'Tabs and newlines in paths are unsupported' ;; esac
}
safe_path "$install_home"
[ -d "$install_home" ] || fail "Home directory does not exist: $install_home"
install_home=$(CDPATH= cd -- "$install_home" && pwd -P)
if [ -z "$templates" ]; then
  if [ -d "$script_dir/../integration/skills" ]; then templates="$script_dir/../integration/skills";
  else templates="$script_dir/integration/skills"; fi
fi
safe_path "$templates"
[ -d "$templates" ] || fail "Skill templates directory does not exist: $templates"
templates=$(CDPATH= cd -- "$templates" && pwd -P)
[ -f "$script_dir/rollback-skills.sh" ] || fail 'rollback-skills.sh must accompany this script'
for name in claude-ai-review.md claude-pipeline.md codex-pipeline.md claude-run.sh codex-run.sh; do
  [ -f "$templates/$name" ] && [ ! -L "$templates/$name" ] || fail "Missing regular template: $name"
done
for name in multi-ai ai-review llm-run; do
  [ -f "$install_home/.local/bin/$name" ] && [ -x "$install_home/.local/bin/$name" ] || fail "Install the native command first: $name"
done
version=$("$install_home/.local/bin/multi-ai" --version | awk 'NR == 1 && $1 == "multi-ai" {print $2}')
case "$version" in [0-9]*) ;; *) fail 'Installed command has no native multi-ai version' ;; esac
case "$version" in *[!0-9A-Za-z._+-]*) fail 'Unsafe version' ;; esac
installed="$install_home/.local/lib/multi-ai/$version/multi-ai"
[ -f "$installed" ] && [ ! -L "$installed" ] && [ -x "$installed" ] || fail 'Versioned native binary is missing'
expected_binary=$(shasum -a 256 "$installed" | awk '{print $1}')
for name in multi-ai ai-review llm-run; do
  [ "$(shasum -a 256 "$install_home/.local/bin/$name" | awk '{print $1}')" = "$expected_binary" ] || fail "Command does not use the installed native binary: $name"
done
# Never traverse a symlinked application/skill directory or replace a directory.
# Relative file symlinks are supported: the entry is replaced, not its referent.
safe_target() {
  target=$1
  ancestor=$(dirname -- "$target")
  while [ "$ancestor" != "$install_home" ]; do
    [ "$ancestor" != / ] || fail "Target escaped home: $target"
    [ ! -L "$ancestor" ] || fail "Refusing symlinked parent directory: $ancestor"
    ancestor=$(dirname -- "$ancestor")
  done
  if [ -L "$target" ]; then
    [ -f "$target" ] && [ ! -d "$target" ] || fail "Unknown symlink target: $target"
    safe_path "$(readlink "$target")"
  elif [ -e "$target" ]; then
    [ -f "$target" ] || fail "Refusing non-file entry: $target"
  fi
}
for relative in .claude/bin/ai-review .claude/skills/ai-review/SKILL.md .claude/skills/multi-llm-pipeline/SKILL.md .claude/skills/multi-llm-pipeline/bin/run.sh .codex/skills/multi-llm-pipeline/SKILL.md .codex/skills/multi-llm-pipeline/bin/run.sh; do
  safe_target "$install_home/$relative"
done
state_dir="$install_home/.local/state/multi-ai"
safe_target "$state_dir/backups/.skills-marker"
mkdir -p "$state_dir/backups"
chmod 700 "$state_dir/backups"
backup_dir=$(mktemp -d "$state_dir/backups/skills-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXX")
chmod 700 "$backup_dir"
printf '%s\n' 1 > "$backup_dir/manifest.version"
printf '%s\n' "$(id -u)" > "$backup_dir/uid"
printf '%s\n' "$install_home" > "$backup_dir/home"
printf '%s\n' "$installed" > "$backup_dir/installed_binary"
printf '%s\n' "$expected_binary" > "$backup_dir/installed.sha256"
printf '%s\n' changing > "$backup_dir/update.status"
mkdir "$backup_dir/entries"
printf 'path\tkind\tsha256\tsymlink_target\tmode\n' > "$backup_dir/manifest.tsv"
entry_count=0
temporary=''
on_exit() {
  status=$1
  trap - 0 INT TERM
  if [ -n "$temporary" ] && { [ -e "$temporary" ] || [ -L "$temporary" ]; }; then rm -f "$temporary"; fi
  if [ "$status" -ne 0 ]; then
    printf '%s\n' "Skill update failed. BACKUP_DIR=$backup_dir" >&2
    sh "$script_dir/rollback-skills.sh" "$backup_dir" >&2 || true
  fi
  exit "$status"
}
trap 'on_exit $?' 0
trap 'exit 130' INT
trap 'exit 143' TERM
matches_before() {
  target=$1; entry=$2
  case "$(cat "$entry/before.kind")" in
    symlink) [ -L "$target" ] && [ "$(readlink "$target")" = "$(cat "$entry/before.target")" ] && [ "$(stat -f '%Lp' "$target")" = "$(cat "$entry/before.mode")" ] ;;
    file) [ -f "$target" ] && [ ! -L "$target" ] && [ "$(shasum -a 256 "$target" | awk '{print $1}')" = "$(cat "$entry/before.sha256")" ] && [ "$(stat -f '%Lp' "$target")" = "$(cat "$entry/before.mode")" ] ;;
    *) return 1 ;;
  esac
}
update_entry() {
  relative=$1; template=$2; mode=$3
  target="$install_home/$relative"
  if [ ! -e "$target" ] && [ ! -L "$target" ]; then printf '%s\n' "Skipped missing entry: $target"; return; fi
  safe_target "$target"
  entry_count=$((entry_count + 1))
  entry="$backup_dir/entries/$entry_count"
  mkdir "$entry"
  printf '%s\n' "$relative" > "$entry/path"
  cp -pP "$target" "$entry/before"
  stat -f '%Lp' "$target" > "$entry/before.mode"
  if [ -L "$target" ]; then
    printf '%s\n' symlink > "$entry/before.kind"
    readlink "$target" > "$entry/before.target"
  else
    printf '%s\n' file > "$entry/before.kind"
    shasum -a 256 "$target" | awk '{print $1}' > "$entry/before.sha256"
    if [ "$mode" = preserve ]; then mode=$(cat "$entry/before.mode"); fi
  fi
  [ "$mode" != preserve ] || mode=644
  temporary="$(dirname -- "$target")/.multi-ai-skills.$$.$entry_count"
  [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail "Temporary entry already exists: $temporary"
  if [ "$template" = native-entry ]; then
    printf '%s\n' symlink > "$entry/after.kind"
    printf '%s\n' "$install_home/.local/bin/ai-review" > "$entry/after.target"
    ln -s "$install_home/.local/bin/ai-review" "$temporary"
  else
    printf '%s\n' file > "$entry/after.kind"
    cp "$templates/$template" "$temporary"
    chmod "$mode" "$temporary"
    shasum -a 256 "$temporary" | awk '{print $1}' > "$entry/after.sha256"
  fi
  stat -f '%Lp' "$temporary" > "$entry/after.mode"
  after_hash=''; after_link=''
  if [ -f "$entry/after.sha256" ]; then after_hash=$(cat "$entry/after.sha256"); fi
  if [ -f "$entry/after.target" ]; then after_link=$(cat "$entry/after.target"); fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$relative" "$(cat "$entry/after.kind")" "$after_hash" "$after_link" "$(cat "$entry/after.mode")" >> "$backup_dir/manifest.tsv"
  printf '%s\n' "$entry_count" > "$backup_dir/entry_count"
  matches_before "$target" "$entry" || fail "Entry changed during update: $target"
  printf '%s\n' ready > "$entry/status"
  mv -f "$temporary" "$target"
  temporary=''
  printf '%s\n' published > "$entry/status"
  printf '%s\n' "Updated: $target"
}
update_entry .claude/bin/ai-review native-entry preserve
update_entry .claude/skills/ai-review/SKILL.md claude-ai-review.md preserve
update_entry .claude/skills/multi-llm-pipeline/SKILL.md claude-pipeline.md preserve
update_entry .claude/skills/multi-llm-pipeline/bin/run.sh claude-run.sh 755
update_entry .codex/skills/multi-llm-pipeline/SKILL.md codex-pipeline.md preserve
update_entry .codex/skills/multi-llm-pipeline/bin/run.sh codex-run.sh 755
printf '%s\n' complete > "$backup_dir/update.status"
printf '%s\n' "BACKUP_DIR=$backup_dir" "MANIFEST=$backup_dir/manifest.tsv" 'Restore skills before running the installation rollback.'
