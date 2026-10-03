#!/bin/sh
set -eu
umask 077
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/multi-ai-agent-skills.XXXXXX")
test_root=$(CDPATH= cd -- "$test_root" && pwd -P)
trap 'rm -rf "$test_root"' 0 INT TERM
test_home="$test_root/home with spaces"
mkdir "$test_home"
fail() { printf '%s\n' "FAIL: $*" >&2; exit 1; }
install() { sh "$repo_dir/scripts/install-skills.sh" --home "$test_home" "$@"; }
backup_from() { sed -n 's/^BACKUP_DIR=//p' "$1"; }

# A fresh home gets both discoverable skills; runner forwards exact arguments
# using no Python/Node/Cargo executable or external command on PATH.
install > "$test_root/fresh.log"
first_backup=$(backup_from "$test_root/fresh.log")
for platform in .agents .claude; do
  [ -f "$test_home/$platform/skills/multi-ai/SKILL.md" ] || fail "Missing $platform skill"
  [ -x "$test_home/$platform/skills/multi-ai/scripts/run.sh" ] || fail "Runner not executable"
done
cat > "$test_root/fake-binary" <<'EOF'
#!/bin/sh
printf '%s\n' "$#" "$@"
EOF
chmod 755 "$test_root/fake-binary"
MULTI_AI_BIN="$test_root/fake-binary" PATH='' /bin/sh "$test_home/.agents/skills/multi-ai/scripts/run.sh" \
  text glm --input 'a file.md' --out 'b file.md' > "$test_root/arguments.log"
printf '%s\n' 6 text glm --input 'a file.md' --out 'b file.md' > "$test_root/expected.log"
cmp "$test_root/arguments.log" "$test_root/expected.log" || fail 'Runner changed arguments'

# A second install leaves the original directory, edits and unknown files alone.
printf '%s\n' 'local addition' >> "$test_home/.agents/skills/multi-ai/SKILL.md"
printf '%s\n' custom > "$test_home/.agents/skills/multi-ai/custom.txt"
cp "$test_home/.agents/skills/multi-ai/SKILL.md" "$test_root/edited.md"
install > "$test_root/repeat.log"
cmp "$test_root/edited.md" "$test_home/.agents/skills/multi-ai/SKILL.md" || fail 'Existing skill overwritten'
[ -f "$test_home/.agents/skills/multi-ai/custom.txt" ] || fail 'Unknown file removed'
second_backup=$(backup_from "$test_root/repeat.log")
[ "$(cat "$second_backup/entry_count")" = 0 ] || fail 'Preserved skills became managed'
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$second_backup" > "$test_root/second-rollback.log"
[ -f "$test_home/.claude/skills/multi-ai/SKILL.md" ] || fail 'No-op rollback removed existing skill'

# Rollback removes the unchanged new Claude skill while keeping all edited Codex
# files together. Repeating rollback is safe.
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$first_backup" > "$test_root/first-rollback.log" 2>&1
[ ! -e "$test_home/.claude/skills/multi-ai" ] || fail 'Unchanged skill retained'
cmp "$test_root/edited.md" "$test_home/.agents/skills/multi-ai/SKILL.md" || fail 'Modified skill removed'
[ -f "$test_home/.agents/skills/multi-ai/scripts/run.sh" ] || fail 'Modified skill partially removed'
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$first_backup" > "$test_root/repeated-rollback.log" 2>&1

# Old host location is explicit, with no dependence on the other platform.
install --only codex --codex-dir "$test_home/.codex/skills" > "$test_root/compat.log"
compat_backup=$(backup_from "$test_root/compat.log")
[ -f "$test_home/.codex/skills/multi-ai/SKILL.md" ] || fail 'Custom Codex location ignored'
[ ! -e "$test_home/.claude/skills/multi-ai" ] || fail '--only installed the other platform'
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$compat_backup" > "$test_root/compat-rollback.log"
[ ! -e "$test_home/.codex/skills/multi-ai" ] || fail 'Custom Codex location rollback failed'

# Reject parent symlinks rather than writing into another tree, including rollback
# after a parent is replaced later.
mkdir "$test_root/outside"
ln -s "$test_root/outside" "$test_home/redirect"
if install --only codex --codex-dir "$test_home/redirect/skills" > "$test_root/unsafe.log" 2>&1; then fail 'Symlink parent accepted'; fi
[ ! -e "$test_root/outside/skills" ] || fail 'Wrote through symlink parent'
install --only claude > "$test_root/changed-parent.log"
parent_backup=$(backup_from "$test_root/changed-parent.log")
mv "$test_home/.claude" "$test_root/saved-claude"
ln -s "$test_root/saved-claude" "$test_home/.claude"
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$parent_backup" > "$test_root/parent-rollback.log" 2>&1
[ -f "$test_root/saved-claude/skills/multi-ai/SKILL.md" ] || fail 'Rollback traversed changed parent'
printf '%s\n' 'PASS: fresh install, exact runner forwarding, preserve-existing, safe rollback, custom directory and symlink protection'
