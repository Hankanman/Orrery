#!/usr/bin/env bash
#
# Fail unless every workspace crate has the same version and, when a tag is
# given (`v2026.7.1` or `2026.7.1`), it matches that version. Prints the version.
#
#   bash packaging/check-version.sh            # crates agree?
#   bash packaging/check-version.sh v2026.7.1  # …and match the tag?
set -euo pipefail

cd "$(dirname "$0")/.."

version_of() {
  # The first `version = "…"` in the manifest is the [package] one.
  sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -n1
}

app="$(version_of crates/orrery/Cargo.toml)"
[[ -n "$app" ]] || { echo "no version in crates/orrery/Cargo.toml" >&2; exit 1; }

for manifest in crates/*/Cargo.toml; do
  v="$(version_of "$manifest")"
  if [[ "$v" != "$app" ]]; then
    echo "$manifest is version '$v', crates/orrery is '$app'" >&2
    exit 1
  fi
done

if [[ $# -ge 1 && -n "$1" ]]; then
  tag="${1#v}"
  if [[ "$tag" != "$app" ]]; then
    echo "tag '$1' does not match the workspace version '$app'" >&2
    exit 1
  fi
fi

echo "$app"
