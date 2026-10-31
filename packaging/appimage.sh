#!/usr/bin/env bash
#
# Build a portable Orrery AppImage from the release binary using linuxdeploy.
# Expects `cargo build --release -p orrery` to have run already. Produces
# dist-appimage/Orrery-x86_64.AppImage.
set -euo pipefail

cd "$(dirname "$0")/.."

OUT="dist-appimage"
APPDIR="$OUT/Orrery.AppDir"
BIN="target/release/orrery"

[[ -x "$BIN" ]] || { echo "missing $BIN — run: cargo build --release -p orrery" >&2; exit 1; }

rm -rf "$OUT"
mkdir -p \
  "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/icons/hicolor/512x512/apps" \
  "$APPDIR/usr/share/icons/hicolor/128x128/apps"

install -Dm755 "$BIN" "$APPDIR/usr/bin/orrery"
install -Dm644 packaging/orrery.desktop "$APPDIR/usr/share/applications/orrery.desktop"
install -Dm644 packaging/icons/icon.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/orrery.png"
install -Dm644 packaging/icons/128x128.png "$APPDIR/usr/share/icons/hicolor/128x128/apps/orrery.png"

# Bundle the llama.cpp runtime if it was fetched (the AppImage is the portable,
# self-contained build — deb/rpm stay lean and rely on Ollama / system llama).
# The app resolves it at <prefix>/lib/orrery/llama-runtime relative to the binary.
if [[ -f packaging/llama-runtime/llama-server ]]; then
  mkdir -p "$APPDIR/usr/lib/orrery/llama-runtime"
  cp -a packaging/llama-runtime/. "$APPDIR/usr/lib/orrery/llama-runtime/"
  echo "  bundled llama-runtime"
fi

# Fetch a pinned linuxdeploy release (not `continuous`, which changes under us)
# and verify it. Bump LINUXDEPLOY_TAG and LINUXDEPLOY_SHA256 together; the
# digest is on the release page (`gh api
# repos/linuxdeploy/linuxdeploy/releases/tags/<tag> --jq '.assets[] | [.name, .digest] | @tsv'`).
LINUXDEPLOY_TAG="1-alpha-20251107-1"
LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
LD="$OUT/linuxdeploy-x86_64.AppImage"
curl -fsSL -o "$LD" \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_TAG}/linuxdeploy-x86_64.AppImage"
echo "${LINUXDEPLOY_SHA256}  $LD" | sha256sum -c -
chmod +x "$LD"

# Extract-and-run avoids needing a working FUSE mount in CI containers. NO_STRIP
# because the bundled strip chokes on the .relr.dyn section modern linkers emit.
export APPIMAGE_EXTRACT_AND_RUN=1
export NO_STRIP=1
# On a desktop with AppImageLauncher, stop it intercepting linuxdeploy (and
# the appimagetool it runs) with an "integrate this AppImage?" dialog.
export APPIMAGELAUNCHER_DISABLE=1
export OUTPUT="Orrery-x86_64.AppImage"

# Libraries to take from the host instead of bundling. linuxdeploy already
# skips the AppImage excludelist (glibc, libGL, libxcb, libX11, fontconfig,
# freetype, libwayland-client, ...); these are the extra ones it would
# otherwise pull in from the (older) build host:
# - libxkbcommon(-x11) read the host's XKB data and Compose tables under
#   /usr/share/X11. An older bundled copy can't parse a newer host's tables
#   ("unrecognized keysym dead_hamza") and Compose sequences break. Every
#   desktop that can run a GUI ships both.
# - libxcb-xkb, libXau and libXdmcp come with libxcb (and go wherever it goes),
#   which the excludelist already leaves to the host; bundling them pairs our
#   copies with the host's libxcb. Dropping libXdmcp also drops libbsd/libmd,
#   which only it pulled in.
EXCLUDE_LIBS=(
  'libxkbcommon.so*'
  'libxkbcommon-x11.so*'
  'libxcb-xkb.so*'
  'libXau.so*'
  'libXdmcp.so*'
)
exclude_args=()
for lib in "${EXCLUDE_LIBS[@]}"; do
  exclude_args+=(--exclude-library "$lib")
done

"$LD" \
  --appdir "$APPDIR" \
  "${exclude_args[@]}" \
  --desktop-file "$APPDIR/usr/share/applications/orrery.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/512x512/apps/orrery.png" \
  --output appimage

mv "$OUTPUT" "$OUT/$OUTPUT"
echo "✓ built $OUT/$OUTPUT"
