#!/usr/bin/env bash
#
# Fetch the pinned llama.cpp `llama-server` build and flatten it (binary + its
# shared libraries) into packaging/llama-runtime/, where appimage.sh bundles it.
# Only the AppImage ships it; deb/rpm stay lean (Ollama / user-installed llama).
#
# Used by both the release workflow and the CI packaging dry-run, so a broken
# archive layout or a dropped soname symlink (#204) fails on the PR, not at tag
# time. To bump the engine: change LLAMA_TAG *and* LLAMA_SHA256 together (the
# digest is on the GitHub release page, or: `gh api
# repos/ggml-org/llama.cpp/releases/tags/<tag> --jq '.assets[] | [.name, .digest] | @tsv'`).
set -euo pipefail

cd "$(dirname "$0")/.."

LLAMA_TAG="${LLAMA_TAG:-b9551}"
LLAMA_SHA256="${LLAMA_SHA256:-e8d4b47e859edab254aaf911ccb8703e73e536799df66efca7d12939f8557a8c}"
ASSET="llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz"
DEST="packaging/llama-runtime"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/llama.tar.gz" \
  "https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/${ASSET}"
echo "${LLAMA_SHA256}  $work/llama.tar.gz" | sha256sum -c -

mkdir -p "$work/x"
tar xzf "$work/llama.tar.gz" -C "$work/x"

rm -rf "$DEST"
mkdir -p "$DEST"
# Archive layout varies by tag; flatten llama-server + its shared libs.
# Keep symlinks (-type l, cp -P): the sonames (libllama.so.0 → …) are
# same-directory relative links, and linuxdeploy fails without them.
find "$work/x" \( -type f -o -type l \) -name 'llama-server' -exec cp -P {} "$DEST/" \;
find "$work/x" \( -type f -o -type l \) -name '*.so*' -exec cp -P {} "$DEST/" \;

[[ -f "$DEST/llama-server" ]] || { echo "llama-server not found in $ASSET" >&2; exit 1; }
chmod +x "$DEST/llama-server"

# Every symlink must resolve inside the flattened dir (a link to a path that
# only existed in the archive's tree would dangle once bundled).
dangling="$(find "$DEST" -xtype l)"
if [[ -n "$dangling" ]]; then
  echo "dangling symlinks in $DEST:" >&2
  echo "$dangling" >&2
  exit 1
fi

# Every shared library llama-server needs must resolve, either from the
# flattened dir or the system — catches a missing soname before linuxdeploy.
missing="$(LD_LIBRARY_PATH="$DEST" ldd "$DEST/llama-server" | grep 'not found' || true)"
if [[ -n "$missing" ]]; then
  echo "llama-server has unresolved libraries:" >&2
  echo "$missing" >&2
  exit 1
fi

ls -la "$DEST"
