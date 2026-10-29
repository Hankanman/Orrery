#!/usr/bin/env bash
#
# Collect the built packages into dist/ and write dist/SHA256SUMS. Expects
# `cargo deb`, `cargo generate-rpm` and packaging/appimage.sh to have run.
# Shared by the release workflow and the CI packaging dry-run, so a glob that
# matches nothing (or the wrong thing, #205) fails on the PR.
set -euo pipefail

cd "$(dirname "$0")/.."

shopt -s nullglob
debs=(target/debian/*.deb)
rpms=(target/generate-rpm/*.rpm)
# Orrery-* only: appimage.sh also downloads linuxdeploy into dist-appimage.
appimages=(dist-appimage/Orrery-*.AppImage)

expect_one() {
  local what="$1"; shift
  if [[ $# -ne 1 ]]; then
    echo "expected exactly one $what, found $#: $*" >&2
    exit 1
  fi
}
expect_one ".deb" "${debs[@]}"
expect_one ".rpm" "${rpms[@]}"
expect_one "Orrery AppImage" "${appimages[@]}"

rm -rf dist
mkdir -p dist
cp "${debs[@]}" "${rpms[@]}" "${appimages[@]}" dist/

(cd dist && sha256sum -- * > SHA256SUMS && sha256sum -c SHA256SUMS)
ls -la dist
cat dist/SHA256SUMS
