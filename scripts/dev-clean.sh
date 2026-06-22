#!/usr/bin/env bash
#
# dev-clean — run Orrery from a clean slate.
#
# Use this when a relaunch keeps showing stale state — most commonly a changed
# app icon or anything embedded at compile time (assets are embedded via
# rust-embed) that `cargo` doesn't treat as a source change, so a normal
# `cargo run` reuses the old binary.
#
# What it does:
#   1. Stops any running orrery process.
#   2. Forces the asset build script to re-run by touching the embedded assets
#      (recompiles ONLY our crate, not its deps).
#   3. Runs `cargo run -p orrery`.
#
# Flags:
#   --deep   hard reset: full `cargo clean` (slow — rebuilds every dependency).
#   --no-run clean only; don't launch afterwards.
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

echo "▸ stopping any running orrery…"
# Bracket trick: the cwd contains "Orrery", so a bare match self-matches.
pkill -f '[t]arget/debug/orrery' 2>/dev/null || true

if [[ "$DEEP" == "1" ]]; then
  echo "▸ deep clean: full cargo clean (this will rebuild all deps)…"
  cargo clean
else
  echo "▸ forcing icon/asset re-embed (rebuilds our crate only)…"
  touch crates/orrery/assets/* 2>/dev/null || true
fi

if [[ "$RUN" == "1" ]]; then
  echo "▸ starting orrery…"
  exec cargo run -p orrery
else
  echo "✓ clean complete (skipped launch)"
fi
