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
  [ -f "$test_home/$platform/skills/multi-ai/references/delegation.md" ] || fail "Missing $platform delegation reference"
  [ -f "$test_home/$platform/skills/multi-ai/references/route-traits.md" ] || fail "Missing $platform route traits"
  [ ! -e "$test_home/$platform/skills/ai-review" ] || fail 'Installed deprecated ai-review skill'
  [ ! -e "$test_home/$platform/skills/multi-llm-pipeline" ] || fail 'Installed deprecated pipeline skill'
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

# Recognized v0.1 files upgrade independently of unknown files, and restore to
# their exact original bytes/modes. The fixture is fixed, not derived from HEAD.
upgrade_home="$test_root/upgrade home"
mkdir "$upgrade_home"
old_fixture="$repo_dir/tests/fixtures/skill-v0.1"
for platform in .agents .claude; do
  target="$upgrade_home/$platform/skills/multi-ai"
  mkdir -p "$target/scripts" "$target/agents"
  cp "$old_fixture/SKILL.md" "$target/SKILL.md"
  cp "$old_fixture/scripts/run.sh" "$target/scripts/run.sh"
  cp "$old_fixture/agents/openai.yaml" "$target/agents/openai.yaml"
  chmod 644 "$target/SKILL.md" "$target/agents/openai.yaml"
  chmod 755 "$target/scripts/run.sh"
  printf '%s\n' user-owned > "$target/custom.txt"
done
sh "$repo_dir/scripts/install-skills.sh" --home "$upgrade_home" > "$test_root/old-preserved.log"
cmp "$old_fixture/SKILL.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md" || fail 'Default install upgraded old skill'
[ ! -e "$upgrade_home/.agents/skills/multi-ai/references" ] || fail 'Default install added reference to old skill'
sh "$repo_dir/scripts/install-skills.sh" --home "$upgrade_home" --upgrade > "$test_root/upgraded.log"
upgrade_backup=$(backup_from "$test_root/upgraded.log")
for platform in .agents .claude; do
  target="$upgrade_home/$platform/skills/multi-ai"
  cmp "$repo_dir/integration/skills/multi-ai/SKILL.md" "$target/SKILL.md" || fail 'Known old skill not upgraded'
  [ -f "$target/references/delegation.md" ] || fail 'Upgrade omitted reference'
  [ -f "$target/references/route-traits.md" ] || fail 'Upgrade omitted route traits'
  [ "$(cat "$target/custom.txt")" = user-owned ] || fail 'Upgrade changed unknown file'
done
printf '%s\n' 'later user decision' >> "$upgrade_home/.claude/skills/multi-ai/SKILL.md"
cp "$upgrade_home/.claude/skills/multi-ai/SKILL.md" "$test_root/later.md"
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$upgrade_backup" > "$test_root/upgrade-rollback.log" 2>&1
cmp "$old_fixture/SKILL.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md" || fail 'Upgrade rollback did not restore old content'
cmp "$test_root/later.md" "$upgrade_home/.claude/skills/multi-ai/SKILL.md" || fail 'Upgrade rollback overwrote later change'
[ ! -e "$upgrade_home/.agents/skills/multi-ai/references" ] || fail 'Upgrade rollback left added empty reference tree'
[ -f "$upgrade_home/.agents/skills/multi-ai/custom.txt" ] || fail 'Upgrade rollback removed unknown file'
[ "$(stat -f '%Lp' "$upgrade_home/.agents/skills/multi-ai/scripts/run.sh")" = 755 ] || fail 'Upgrade rollback lost runner mode'

# An already customized old entry remains untouched, while managed companions
# can update; merge guidance identifies the preserved entry.
printf '%s\n' 'old local customization' >> "$upgrade_home/.agents/skills/multi-ai/SKILL.md"
cp "$upgrade_home/.agents/skills/multi-ai/SKILL.md" "$test_root/local-old.md"
sh "$repo_dir/scripts/install-skills.sh" --home "$upgrade_home" --only codex --upgrade > "$test_root/custom-upgrade.log" 2>&1
custom_backup=$(backup_from "$test_root/custom-upgrade.log")
cmp "$test_root/local-old.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md" || fail 'Customized old entry overwritten'
[ -f "$custom_backup/MERGE.md" ] || fail 'Missing merge guidance'
sh "$repo_dir/scripts/rollback-agent-skills.sh" "$custom_backup" > "$test_root/custom-rollback.log" 2>&1
cmp "$test_root/local-old.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md" || fail 'Customized entry changed during rollback'

# Corrupted prior bytes cannot be restored over an otherwise managed file.
cp "$old_fixture/SKILL.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md"
sh "$repo_dir/scripts/install-skills.sh" --home "$upgrade_home" --only codex --upgrade > "$test_root/corrupt-upgrade.log"
corrupt_backup=$(backup_from "$test_root/corrupt-upgrade.log")
printf '%s\n' corruption >> "$corrupt_backup/entries/1/before/SKILL.md"
if sh "$repo_dir/scripts/rollback-agent-skills.sh" "$corrupt_backup" > "$test_root/corrupt-rollback.log" 2>&1; then fail 'Corrupted backup restored'; fi
cmp "$repo_dir/integration/skills/multi-ai/SKILL.md" "$upgrade_home/.agents/skills/multi-ai/SKILL.md" || fail 'Damaged prior entry was published'

# The compatibility updater now upgrades only multi-ai and leaves legacy skill
# directories and launchers byte-for-byte unchanged.
canonical_home="$test_root/canonical home"
mkdir -p "$canonical_home/.claude/skills/ai-review" "$canonical_home/.claude/skills/multi-llm-pipeline/bin" \
  "$canonical_home/.codex/skills/multi-llm-pipeline" "$canonical_home/.claude/bin"
for relative in .claude/skills/ai-review/SKILL.md .claude/skills/multi-llm-pipeline/SKILL.md \
  .claude/skills/multi-llm-pipeline/bin/run.sh .codex/skills/multi-llm-pipeline/SKILL.md .claude/bin/ai-review; do
  printf '%s\n' "user-owned $relative" > "$canonical_home/$relative"
done
sh "$repo_dir/scripts/update-skills.sh" --home "$canonical_home" > "$test_root/canonical-updater.log"
for relative in .claude/skills/ai-review/SKILL.md .claude/skills/multi-llm-pipeline/SKILL.md \
  .claude/skills/multi-llm-pipeline/bin/run.sh .codex/skills/multi-llm-pipeline/SKILL.md .claude/bin/ai-review; do
  [ "$(cat "$canonical_home/$relative")" = "user-owned $relative" ] || fail 'Updater changed a legacy entry'
done
for platform in .agents .claude; do
  [ -f "$canonical_home/$platform/skills/multi-ai/references/route-traits.md" ] || fail 'Updater omitted canonical skill'
done
printf '%s\n' 'PASS: fresh install, forwarding, preservation, safe rollback, v0.1 managed upgrade, merge guide, checksum guards, symlink protection, route traits and canonical updater'
