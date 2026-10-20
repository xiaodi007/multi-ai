#!/bin/sh
set -eu
umask 022

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
if [ "$#" -gt 0 ]; then
  case "$1" in --help|-h) printf '%s\n' 'Usage: scripts/package.sh' 'Builds a local macOS arm64 release; uploads nothing.'; exit 0 ;; *) printf '%s\n' 'Usage: scripts/package.sh' >&2; exit 1 ;; esac
fi
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || { printf '%s\n' 'Packaging is tested only on macOS arm64.' >&2; exit 1; }
cd "$repo_dir"
cargo build --release --locked --target aarch64-apple-darwin
binary="$repo_dir/target/aarch64-apple-darwin/release/multi-ai"
version=$("$binary" --version | awk 'NR == 1 && $1 == "multi-ai" {print $2}')
case "$version" in [0-9]*) ;; *) printf '%s\n' 'Invalid binary version.' >&2; exit 1 ;; esac
case "$version" in *[!0-9A-Za-z._+-]*) printf '%s\n' 'Unsafe binary version.' >&2; exit 1 ;; esac
package_name="multi-ai-$version-aarch64-apple-darwin"
mkdir -p "$repo_dir/dist"
stage=$(mktemp -d "$repo_dir/dist/.package-XXXXXX")
trap 'rm -rf "$stage"' 0 INT TERM
mkdir -p "$stage/$package_name/bin" "$stage/$package_name/docs" "$stage/$package_name/integration/skills"
cp -p "$binary" "$stage/$package_name/bin/multi-ai"
cp -p "$script_dir/install.sh" "$script_dir/rollback.sh" "$script_dir/update-skills.sh" "$script_dir/rollback-skills.sh" \
  "$script_dir/install-skills.sh" "$script_dir/rollback-agent-skills.sh" "$stage/$package_name/"
cp -Rp "$repo_dir/integration/skills/multi-ai" "$stage/$package_name/integration/skills/"
cp -p "$script_dir/reclassify-implement.py" "$stage/$package_name/"
cp -p "$repo_dir/README.md" "$stage/$package_name/"
cp -p "$repo_dir/docs/COMPATIBILITY.md" "$stage/$package_name/docs/"
cp -p "$repo_dir/docs/AGENT-INTEGRATION.md" "$stage/$package_name/docs/"
cp -p "$repo_dir/docs/DELEGATION.md" "$stage/$package_name/docs/"
cp -p "$repo_dir/docs/CLI-ISOLATION.md" "$stage/$package_name/docs/"
cp -p "$repo_dir/docs/VALIDATION-v0.2.md" "$stage/$package_name/docs/"
cp -p "$repo_dir/docs/TOKEN-BENCHMARK-v0.2.md" "$stage/$package_name/docs/"
chmod 755 "$stage/$package_name/bin/multi-ai" "$stage/$package_name/install.sh" "$stage/$package_name/rollback.sh" "$stage/$package_name/update-skills.sh" "$stage/$package_name/rollback-skills.sh"
chmod 755 "$stage/$package_name/install-skills.sh" "$stage/$package_name/rollback-agent-skills.sh" "$stage/$package_name/integration/skills/multi-ai/scripts/run.sh"
(
  cd "$stage/$package_name"
  shasum -a 256 bin/multi-ai install.sh rollback.sh update-skills.sh rollback-skills.sh install-skills.sh rollback-agent-skills.sh README.md docs/COMPATIBILITY.md docs/AGENT-INTEGRATION.md docs/DELEGATION.md docs/CLI-ISOLATION.md docs/VALIDATION-v0.2.md docs/TOKEN-BENCHMARK-v0.2.md \
    reclassify-implement.py \
    integration/skills/multi-ai/SKILL.md \
    integration/skills/multi-ai/scripts/run.sh integration/skills/multi-ai/agents/openai.yaml \
    integration/skills/multi-ai/references/delegation.md integration/skills/multi-ai/references/route-traits.md > SHA256SUMS
  shasum -a 256 -c SHA256SUMS
)
archive="$repo_dir/dist/$package_name.tar.gz"
temporary_archive="$stage/$package_name.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$temporary_archive" -C "$stage" "$package_name"
mv -f "$temporary_archive" "$archive"
(
  cd "$repo_dir/dist"
  shasum -a 256 "$package_name.tar.gz" > "$package_name.tar.gz.sha256"
)
destination="$repo_dir/dist/$package_name"
if [ -e "$destination" ]; then
  previous="$repo_dir/dist/.previous-$package_name-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  mv "$destination" "$previous"
  printf '%s\n' "Previous unpacked package preserved: $previous"
fi
mv "$stage/$package_name" "$destination"
printf '%s\n' "Package: $destination" "Archive: $archive"
