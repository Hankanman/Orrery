#!/usr/bin/env bash
#
# Install or update Orrery for the current user — no root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/Hankanman/Orrery/main/scripts/install.sh | bash
#
# Downloads the latest release AppImage (the only build that bundles the
# llama.cpp engine), verifies it against the release's SHA256SUMS, and puts it
# on your PATH with a desktop entry + icon. Re-run it to update, or to repair
# the launcher and desktop entry.
#
# The launcher and the desktop entry run the AppImage with
# APPIMAGELAUNCHER_DISABLE=1, so AppImageLauncher (if installed) doesn't
# "integrate" a second copy into ~/Applications.
#
# Environment overrides:
#   ORRERY_VERSION=v2026.10.1  install a specific tag instead of the latest
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
SEARCH_PROVIDER="$PREFIX/share/gnome-shell/search-providers/orrery-search-provider.ini"

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Single-quote a string for a POSIX shell script.
sh_quote() { printf "'%s'" "${1//\'/\'\\\'\'}"; }
# Quote a path for a desktop entry's Exec key (the spec's double-quote rules).
desktop_quote() { printf '"%s"' "$(printf '%s' "$1" | sed 's/[\\"`$]/\\&/g')"; }

refresh_desktop_caches() {
  command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 &&
    gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

if [[ "${1:-}" == "--uninstall" ]]; then
  say "Removing Orrery from $PREFIX"
  rm -f "$BIN" "$BIN.new" "$DESKTOP" "$ICON" "$SEARCH_PROVIDER" "$APPIMAGE" "$APPIMAGE.new" "$APP_DIR/VERSION"
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

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

installed=""
[[ -f "$APP_DIR/VERSION" ]] && installed="$(cat "$APP_DIR/VERSION")"
if [[ "$installed" == "$tag" && -x "$APPIMAGE" ]]; then
  # Nothing to download, but the launcher and desktop entry below are still
  # rewritten, so a re-run repairs them (and migrates an older install).
  say "Orrery $tag is already installed and up to date."
else
  say "${installed:+Updating Orrery $installed → }${installed:-Installing Orrery }$tag"

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

  mkdir -p "$APP_DIR"
  install -m755 "$tmp/$asset" "$APPIMAGE.new"
  mv -f "$APPIMAGE.new" "$APPIMAGE" # atomic swap, safe while an old copy runs
  printf '%s\n' "$tag" >"$APP_DIR/VERSION"
fi

mkdir -p "$(dirname "$BIN")" "$(dirname "$DESKTOP")" "$(dirname "$ICON")"
# A wrapper, not a symlink: AppImageLauncher hooks every AppImage launch (via
# binfmt_misc) and offers to copy it into ~/Applications with a menu entry of
# its own, unless APPIMAGELAUNCHER_DISABLE is set. It's written beside $BIN and
# renamed over it, so an older install's symlink is replaced, not followed
# (writing through the symlink would clobber the AppImage).
{
  printf '#!/bin/sh\n'
  printf '# Written by Orrery'"'"'s install.sh. APPIMAGELAUNCHER_DISABLE stops\n'
  printf '# AppImageLauncher integrating a second copy of the AppImage.\n'
  printf 'APPIMAGELAUNCHER_DISABLE=1 exec %s "$@"\n' "$(sh_quote "$APPIMAGE")"
} >"$BIN.new"
chmod 755 "$BIN.new"
mv -f "$BIN.new" "$BIN"

# Icon and desktop entry come from the repo at the same tag, so they match the
# installed build (falling back to main for tags older than this layout). A
# failure here isn't fatal — the binary is already in place.
fetch_repo_file() { # <repo path> <dest>
  curl -fsSL -o "$2" "https://raw.githubusercontent.com/$REPO/$tag/$1" 2>/dev/null ||
    curl -fsSL -o "$2" "https://raw.githubusercontent.com/$REPO/main/$1" 2>/dev/null
}
fetch_repo_file packaging/icons/icon.png "$ICON" || say "couldn't fetch the icon (skipped)."
if fetch_repo_file packaging/orrery.desktop "$tmp/orrery.desktop"; then
  # Exec runs the wrapper, so menu launches bypass AppImageLauncher too. Keep
  # the name orrery.desktop: Orrery registers "orrery" as its app id with the
  # desktop portal (for the global shortcut), and the portal only accepts an
  # id with a desktop file of the same name.
  exec_line="Exec=$(desktop_quote "$BIN")"
  awk -v line="${exec_line//\\/\\\\}" '/^Exec=/ { print line; next } { print }' \
    "$tmp/orrery.desktop" >"$DESKTOP"
else
  say "couldn't fetch the desktop entry (skipped)."
fi
# GNOME Shell search provider. GNOME Shell only reads providers from the
# system data dirs (XDG_DATA_DIRS), not ~/.local/share, so on GNOME also say
# how to enable it system-wide; the per-user copy is what that command copies.
mkdir -p "$(dirname "$SEARCH_PROVIDER")"
if fetch_repo_file packaging/orrery-search-provider.ini "$SEARCH_PROVIDER"; then
  case ":${XDG_DATA_DIRS:-/usr/local/share:/usr/share}:" in
    *":$PREFIX/share:"* | *":$PREFIX/share/:"*) ;;
    *)
      if [[ "${XDG_CURRENT_DESKTOP:-}" == *GNOME* ]]; then
        say "To search your repos from the GNOME Activities overview, run once:"
        say "  sudo install -Dm644 '$SEARCH_PROVIDER' /usr/local/share/gnome-shell/search-providers/orrery-search-provider.ini"
      fi
      ;;
  esac
else
  rm -f "$SEARCH_PROVIDER"
  say "couldn't fetch the GNOME search provider (skipped)."
fi
refresh_desktop_caches

# A copy AppImageLauncher integrated earlier (from running the AppImage
# directly) keeps a menu entry of its own. Point it out rather than delete it.
shopt -s nullglob
ail_copies=("$HOME"/Applications/[Oo]rrery*.[Aa]pp[Ii]mage)
shopt -u nullglob
if ((${#ail_copies[@]})); then
  say "Note: AppImageLauncher also has a copy at ${ail_copies[0]}. This install"
  say "replaces it; remove it from its menu entry (right-click → Remove AppImage from system)."
fi

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
