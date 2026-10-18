#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
binary=${MULTI_AI_TEST_BINARY:-"$repo_dir/target/debug/multi-ai"}
test_root=$(mktemp -d "${TMPDIR:-/tmp}/multi-ai-install.XXXXXX")
test_root=$(CDPATH= cd -- "$test_root" && pwd -P)
trap 'rm -rf "$test_root"' 0 INT TERM
test_path=/usr/bin:/bin:/usr/sbin:/sbin
checks=0
fail() { printf '%s\n' "FAIL: $*" >&2; exit 1; }
check() { "$@" || fail "Check failed: $*"; checks=$((checks + 1)); }
new_home() { install_home="$test_root/$1 home"; mkdir -p "$install_home"; }
install() {
  env -i HOME="$install_home" PATH="$test_path" TMPDIR="$test_root" \
    /bin/sh "$repo_dir/scripts/install.sh" --binary "$binary" "$@"
}
rollback() {
  env -i HOME="$install_home" PATH="$test_path" TMPDIR="$test_root" \
    /bin/sh "$repo_dir/scripts/rollback.sh" "$@"
}
backup_from() { sed -n 's/^BACKUP_DIR=//p' "$1"; }
check test -x "$binary"
version=$("$binary" --version | awk 'NR == 1 && $1 == "multi-ai" {print $2}')
check test -n "$version"

# 1: Compatibility remains the default, and the managed list is explicit.
new_home default
install > "$test_root/default.log"
default_backup=$(backup_from "$test_root/default.log")
installed="$install_home/.local/lib/multi-ai/$version/multi-ai"
for name in multi-ai ai-review llm-run; do
  check test -L "$install_home/.local/bin/$name"
  check test "$(readlink "$install_home/.local/bin/$name")" = "$installed"
done
printf '%s\n' multi-ai ai-review llm-run > "$test_root/default-entries"
check cmp "$test_root/default-entries" "$default_backup/managed-entries"
check test "$(cat "$default_backup/manifest.version")" = 2
check test -f "$install_home/.config/multi-ai/profiles.json"
check test -f "$install_home/.config/multi-ai/routes.json"

# 2: Rollback removes unchanged new entries/config, and can be repeated.
rollback "$default_backup" > "$test_root/default-rollback.log"
rollback "$default_backup" > "$test_root/default-repeat.log"
for name in multi-ai ai-review llm-run; do
  check test ! -e "$install_home/.local/bin/$name"
  check test ! -L "$install_home/.local/bin/$name"
done
check test ! -e "$installed"
check test ! -e "$install_home/.config/multi-ai/profiles.json"
check test ! -e "$install_home/.config/multi-ai/routes.json"

# 3: A fresh no-legacy installation creates only the canonical binary entry.
new_home canonical
install --no-legacy-aliases > "$test_root/canonical.log"
canonical_backup=$(backup_from "$test_root/canonical.log")
check test -L "$install_home/.local/bin/multi-ai"
for name in ai-review llm-run; do
  check test ! -e "$install_home/.local/bin/$name"
  check test ! -L "$install_home/.local/bin/$name"
  check test ! -e "$canonical_backup/entries/$name.kind"
done
check test "$(cat "$canonical_backup/managed-entries")" = multi-ai
rollback "$canonical_backup" > "$test_root/canonical-rollback.log"
check test ! -L "$install_home/.local/bin/multi-ai"

# 4: --no-legacy-aliases preserves existing files, symlinks and configurations.
new_home preserve
mkdir -p "$install_home/.local/bin" "$install_home/.config/multi-ai"
printf '%s\n' user-ai-review > "$install_home/.local/bin/ai-review"
chmod 751 "$install_home/.local/bin/ai-review"
ln -s user-llm-target "$install_home/.local/bin/llm-run"
printf '%s\n' '{"user":"profiles"}' > "$install_home/.config/multi-ai/profiles.json"
printf '%s\n' '{"user":"routes"}' > "$install_home/.config/multi-ai/routes.json"
chmod 640 "$install_home/.config/multi-ai/profiles.json"
cp -p "$install_home/.config/multi-ai/profiles.json" "$test_root/user-profiles"
cp -p "$install_home/.config/multi-ai/routes.json" "$test_root/user-routes"
install --no-legacy-aliases > "$test_root/preserve.log"
preserve_backup=$(backup_from "$test_root/preserve.log")
rollback "$preserve_backup" > "$test_root/preserve-rollback.log"
check test "$(cat "$install_home/.local/bin/ai-review")" = user-ai-review
check test "$(stat -f '%Lp' "$install_home/.local/bin/ai-review")" = 751
check test "$(readlink "$install_home/.local/bin/llm-run")" = user-llm-target
check cmp "$test_root/user-profiles" "$install_home/.config/multi-ai/profiles.json"
check cmp "$test_root/user-routes" "$install_home/.config/multi-ai/routes.json"
check test "$(stat -f '%Lp' "$install_home/.config/multi-ai/profiles.json")" = 640

# 5: Unmanaged legacy directories are not preflighted or touched.
new_home unmanaged-directory
mkdir -p "$install_home/.local/bin/ai-review" "$install_home/.local/bin/llm-run"
printf '%s\n' user-file > "$install_home/.local/bin/ai-review/keep"
install --no-legacy-aliases > "$test_root/unmanaged.log"
rollback "$(backup_from "$test_root/unmanaged.log")" > "$test_root/unmanaged-rollback.log"
check test "$(cat "$install_home/.local/bin/ai-review/keep")" = user-file
check test -d "$install_home/.local/bin/llm-run"

# 6: Later user edits to the canonical entry, config and binary survive rollback.
new_home later-edits
install > "$test_root/edits.log"
edits_backup=$(backup_from "$test_root/edits.log")
installed="$install_home/.local/lib/multi-ai/$version/multi-ai"
rm "$install_home/.local/bin/multi-ai"
printf '%s\n' later-entry > "$install_home/.local/bin/multi-ai"
printf '%s\n' later-config >> "$install_home/.config/multi-ai/routes.json"
printf '%s\n' later-binary >> "$installed"
cp "$install_home/.config/multi-ai/routes.json" "$test_root/later-routes"
cp "$installed" "$test_root/later-binary"
rollback "$edits_backup" > "$test_root/edits-rollback.log" 2>&1
check test "$(cat "$install_home/.local/bin/multi-ai")" = later-entry
check cmp "$test_root/later-routes" "$install_home/.config/multi-ai/routes.json"
check cmp "$test_root/later-binary" "$installed"

# 7: A damaged managed list is rejected before changing any installed entry.
new_home checksum
install --no-legacy-aliases > "$test_root/checksum.log"
checksum_backup=$(backup_from "$test_root/checksum.log")
printf '%s\n' ai-review >> "$checksum_backup/managed-entries"
if rollback "$checksum_backup" > "$test_root/checksum-rollback.log" 2>&1; then fail 'Corrupted managed list accepted'; fi
check grep -q 'Managed entry checksum mismatch' "$test_root/checksum-rollback.log"
check test -L "$install_home/.local/bin/multi-ai"

# 8: Even a correctly hashed list cannot introduce arbitrary paths.
printf '%s\n' multi-ai ../../outside > "$checksum_backup/managed-entries"
shasum -a 256 "$checksum_backup/managed-entries" | awk '{print $1}' > "$checksum_backup/managed-entries.sha256"
if rollback "$checksum_backup" > "$test_root/unsafe-list.log" 2>&1; then fail 'Unsafe managed list accepted'; fi
check grep -q 'Invalid managed entry list' "$test_root/unsafe-list.log"
check test -L "$install_home/.local/bin/multi-ai"

# 9: New backups must not silently fall back to managing all aliases.
rm "$checksum_backup/managed-entries"
if rollback "$checksum_backup" > "$test_root/missing-list.log" 2>&1; then fail 'Missing managed list accepted'; fi
check grep -q 'Missing regular backup metadata' "$test_root/missing-list.log"
check test -L "$install_home/.local/bin/multi-ai"

# 10: Original v1 backups without a managed list retain three-entry rollback.
new_home old-backup
mkdir -p "$install_home/.local/bin"
printf '%s\n' original-entry > "$install_home/.local/bin/ai-review"
chmod 750 "$install_home/.local/bin/ai-review"
install > "$test_root/old.log"
old_backup=$(backup_from "$test_root/old.log")
printf '%s\n' 1 > "$old_backup/manifest.version"
rm "$old_backup/managed-entries" "$old_backup/managed-entries.sha256"
rollback "$old_backup" > "$test_root/old-rollback.log"
check test "$(cat "$install_home/.local/bin/ai-review")" = original-entry
check test "$(stat -f '%Lp' "$install_home/.local/bin/ai-review")" = 750
check test ! -L "$install_home/.local/bin/multi-ai"
check test ! -L "$install_home/.local/bin/llm-run"

# 11: Config import failure restores the previous versioned binary and entries.
new_home failed-install
mkdir -p "$install_home/.local/bin" "$install_home/.local/lib/multi-ai/$version"
printf '%s\n' prior-entry > "$install_home/.local/bin/multi-ai"
printf '%s\n' prior-binary > "$install_home/.local/lib/multi-ai/$version/multi-ai"
printf '%s\n' invalid-json > "$test_root/bad-profiles"
if install --no-legacy-aliases --legacy-profiles "$test_root/bad-profiles" > "$test_root/failed.log" 2>&1; then fail 'Invalid config import succeeded'; fi
check test "$(cat "$install_home/.local/bin/multi-ai")" = prior-entry
check test "$(cat "$install_home/.local/lib/multi-ai/$version/multi-ai")" = prior-binary
check test ! -e "$install_home/.config/multi-ai/profiles.json"

printf '%s\n' "PASS: 11 isolated installation scenarios, $checks assertions; no model calls or user credentials"
