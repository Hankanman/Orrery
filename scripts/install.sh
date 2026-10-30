#!/usr/bin/env bash
#
# Install or update Orrery for the current user — no root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/Hankanman/Orrery/main/scripts/install.sh | bash
#
# Downloads the latest release AppImage (the only build that bundles the
# llama.cpp engine), verifies it against the release's SHA256SUMS, and puts it
# on your PATH with a desktop entry + icon. Re-run it to update.
#
# Environment overrides:
#   ORRERY_VERSION=v2026.7.1   install a specific tag instead of the latest
#   ORRERY_PREFIX=~/.local     where to install (bin/, opt/, share/ go under it)
#
#   bash install.sh --uninstall   remove everything this script installed
set -euo pipefail

REPO="Hankanman/Orrery"
PREFIX="${ORRERY_PREFIX:-$HOME/.local}"
APP_DIR="$PREFIX/opt/orrery"
APPIMAGE="$APP_DIR/Orrery.AppImage"
BIN="$PREFIX/bin/orrery"
DESKTOP="$PREFIX/share/applications/orrery.desktop"
ICON="$PREFIX/share/icons/hicolor/512x512/apps/orrery.png"

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

refresh_desktop_caches() {
  command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 &&
    gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

if [[ "${1:-}" == "--uninstall" ]]; then
  say "Removing Orrery from $PREFIX"
  rm -f "$BIN" "$DESKTOP" "$ICON" "$APPIMAGE" "$APP_DIR/VERSION"
  rmdir "$APP_DIR" 2>/dev/null || true
  refresh_desktop_caches
  say "Done. Your config (~/.config/orrery) and cache (~/.local/share/orrery) were left in place."
  exit 0
fi

[[ "$(uname -s)" == "Linux" ]] || die "Orrery is Linux-only."
case "$(uname -m)" in
  x86_64 | amd64) ;;
  *) die "only x86_64 builds are published (this machine is $(uname -m))." ;;
esac
command -v curl >/dev/null 2>&1 || die "curl is required."

if [[ -n "${ORRERY_VERSION:-}" ]]; then
  api="https://api.github.com/repos/$REPO/releases/tags/$ORRERY_VERSION"
else
  api="https://api.github.com/repos/$REPO/releases/latest"
fi
release_json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "$api")" ||
  die "couldn't fetch release info from $api"

tag="$(grep -m1 -o '"tag_name": *"[^"]*"' <<<"$release_json" | sed 's/.*"\([^"]*\)"$/\1/')"
urls="$(grep -o '"browser_download_url": *"[^"]*"' <<<"$release_json" | sed 's/.*"\([^"]*\)"$/\1/')"
appimage_url="$(grep -E '(x86_64|amd64)\.AppImage$' <<<"$urls" | head -n1 || true)"
sums_url="$(grep -E '/SHA256SUMS$' <<<"$urls" | head -n1 || true)"
[[ -n "$tag" && -n "$appimage_url" ]] || die "no x86_64 AppImage found in release ${tag:-?}."

installed=""
[[ -f "$APP_DIR/VERSION" ]] && installed="$(cat "$APP_DIR/VERSION")"
if [[ "$installed" == "$tag" && -x "$APPIMAGE" ]]; then
  say "Orrery $tag is already installed and up to date."
  exit 0
fi
say "${installed:+Updating Orrery $installed → }${installed:-Installing Orrery }$tag"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
asset="$(basename "$appimage_url")"
curl -fL --progress-bar -o "$tmp/$asset" "$appimage_url" || die "download failed."

if [[ -n "$sums_url" ]]; then
  curl -fsSL -o "$tmp/SHA256SUMS" "$sums_url" || die "couldn't fetch SHA256SUMS."
  (cd "$tmp" && grep -E " \*?$asset\$" SHA256SUMS | sha256sum -c --status) ||
    die "checksum mismatch for $asset — not installing."
  say "Checksum verified."
else
  say "Release $tag has no SHA256SUMS; skipping checksum verification."
fi

mkdir -p "$APP_DIR" "$(dirname "$BIN")" "$(dirname "$DESKTOP")" "$(dirname "$ICON")"
install -m755 "$tmp/$asset" "$APPIMAGE.new"
mv -f "$APPIMAGE.new" "$APPIMAGE" # atomic swap, safe while an old copy runs
ln -sf "$APPIMAGE" "$BIN"
printf '%s\n' "$tag" >"$APP_DIR/VERSION"

# Icon and desktop entry come from the repo at the same tag, so they match the
# installed build (falling back to main for tags older than this layout). A
# failure here isn't fatal — the binary is already in place.
fetch_repo_file() { # <repo path> <dest>
  curl -fsSL -o "$2" "https://raw.githubusercontent.com/$REPO/$tag/$1" 2>/dev/null ||
    curl -fsSL -o "$2" "https://raw.githubusercontent.com/$REPO/main/$1" 2>/dev/null
}
fetch_repo_file packaging/icons/icon.png "$ICON" || say "couldn't fetch the icon (skipped)."
if fetch_repo_file packaging/orrery.desktop "$tmp/orrery.desktop"; then
  sed "s|^Exec=.*|Exec=$APPIMAGE|" "$tmp/orrery.desktop" >"$DESKTOP"
else
  say "couldn't fetch the desktop entry (skipped)."
fi
refresh_desktop_caches

# Older AppImage runtimes mount via FUSE 2; flag it if it looks missing.
if ! ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; then
  say "Note: libfuse2 wasn't found. If Orrery won't start, install it"
  say "('sudo apt install libfuse2' / 'sudo dnf install fuse-libs') or set APPIMAGE_EXTRACT_AND_RUN=1."
fi
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) say "Note: $PREFIX/bin isn't on your PATH; add it to launch 'orrery' from a shell." ;;
esac

say "Orrery $tag installed. Launch it from your app menu or run 'orrery'."
