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

# Fetch linuxdeploy (cached between runs if present).
LD="$OUT/linuxdeploy-x86_64.AppImage"
if [[ ! -x "$LD" ]]; then
  curl -fsSL -o "$LD" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
  chmod +x "$LD"
fi

# Extract-and-run avoids needing a working FUSE mount in CI containers. NO_STRIP
# because the bundled strip chokes on the .relr.dyn section modern linkers emit.
export APPIMAGE_EXTRACT_AND_RUN=1
export NO_STRIP=1
export OUTPUT="Orrery-x86_64.AppImage"

"$LD" \
  --appdir "$APPDIR" \
  --desktop-file "$APPDIR/usr/share/applications/orrery.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/512x512/apps/orrery.png" \
  --output appimage

mv "$OUTPUT" "$OUT/$OUTPUT"
echo "✓ built $OUT/$OUTPUT"
