#!/usr/bin/env bash
#
# dev-clean — launch `tauri dev` from a clean slate.
#
# Use this when a relaunch keeps showing stale state — most commonly a changed
# app icon, splash, or anything embedded at compile time that `cargo` doesn't
# treat as a source change (so a normal `pnpm tauri dev` reuses the old binary).
#
# What it does:
#   1. Stops any dev process holding the Vite port (1420).
#   2. Clears the Vite cache + dist (stale frontend bundles).
#   3. Touches tauri.conf.json so the tauri build script re-runs and re-embeds
#      icons/assets — this recompiles ONLY our crate (~5s), not its deps.
#   4. Starts `pnpm tauri dev`.
#
# Flags:
#   --deep   hard reset: full `cargo clean` (slow — rebuilds every dependency).
#   --no-run clean only; don't launch dev afterwards.
set -euo pipefail

cd "$(dirname "$0")/.."

DEEP=0
RUN=1
for arg in "$@"; do
  case "$arg" in
    --deep)   DEEP=1 ;;
    --no-run) RUN=0 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

VITE_PORT=1420

echo "▸ stopping anything on port $VITE_PORT…"
# fuser is most reliable; fall back to lsof. Don't fail if nothing is there.
if command -v fuser >/dev/null 2>&1; then
  fuser -k "${VITE_PORT}/tcp" 2>/dev/null || true
elif command -v lsof >/dev/null 2>&1; then
  lsof -ti "tcp:${VITE_PORT}" 2>/dev/null | xargs -r kill 2>/dev/null || true
fi
# Bracket trick: the cwd contains "Orrery", so a bare `pgrep orrery` self-matches.
pkill -f '[o]rrery/src-tauri/target' 2>/dev/null || true

echo "▸ clearing frontend caches…"
rm -rf node_modules/.vite dist

if [[ "$DEEP" == "1" ]]; then
  echo "▸ deep clean: full cargo clean (this will rebuild all deps)…"
  cargo clean --manifest-path src-tauri/Cargo.toml
else
  echo "▸ forcing icon/asset re-embed (rebuilds our crate only)…"
  touch src-tauri/tauri.conf.json
fi

if [[ "$RUN" == "1" ]]; then
  echo "▸ starting tauri dev…"
  exec pnpm tauri dev
else
  echo "✓ clean complete (skipped launch)"
fi
