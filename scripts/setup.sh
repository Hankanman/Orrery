#!/usr/bin/env bash
#
# setup — get a machine ready to dev Orrery from a clean slate.
#
# Orrery is a Tauri 2 app: a React/TS frontend (Node + pnpm) over a Rust core
# that links against WebKitGTK. `pnpm tauri dev` shells out to `cargo`, so a
# missing Rust toolchain or missing WebKitGTK headers fail the build long before
# any of our code runs. This script makes all of that present and idempotent —
# run it as many times as you like; it only does work that's actually missing.
#
# What it ensures:
#   1. System build deps + WebKitGTK 4.1 (dnf / apt / pacman / zypper).
#   2. The Rust toolchain (rustup + stable) — installs if `cargo` is absent.
#   3. Node is present and new enough for Vite (>= 20).
#   4. pnpm (via Corepack, pinned to packageManager in package.json).
#   5. JS deps (`pnpm install`) and Rust deps (`cargo fetch`).
#
# Flags:
#   --no-fetch    skip `pnpm install` / `cargo fetch` (system + toolchains only).
#   --skip-system don't touch system packages (no sudo) — toolchains + deps only.
#   -y, --yes     pass non-interactive/assume-yes to the package manager.
#
# Safe to re-run. The only step that needs sudo is the system-package install;
# everything else is per-user.
set -euo pipefail

cd "$(dirname "$0")/.."

FETCH=1
SYSTEM=1
ASSUME_YES=0
for arg in "$@"; do
  case "$arg" in
    --no-fetch)    FETCH=0 ;;
    --skip-system) SYSTEM=0 ;;
    -y|--yes)      ASSUME_YES=1 ;;
    -h|--help)
      sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# ── helpers ─────────────────────────────────────────────────────────────────
say()  { printf '\n\033[1m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# Pick a privilege-escalation prefix only when we actually need one.
SUDO=""
if [[ "$SYSTEM" == "1" && "$(id -u)" != "0" ]]; then
  if have sudo; then SUDO="sudo"; else
    warn "no sudo and not root — system packages may fail; use --skip-system to bypass"
  fi
fi

# ── 1. system build deps + WebKitGTK ────────────────────────────────────────
install_system_deps() {
  if [[ "$SYSTEM" != "1" ]]; then
    warn "skipping system packages (--skip-system)"
    return
  fi

  if have dnf; then
    say "installing system deps (dnf)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO dnf install "${yes[@]}" \
      webkit2gtk4.1-devel \
      gtk3-devel \
      libsoup3-devel \
      openssl-devel \
      librsvg2-devel \
      libappindicator-gtk3-devel \
      patchelf file curl wget \
      gcc gcc-c++ make pkgconf-pkg-config
  elif have apt-get; then
    say "installing system deps (apt)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO apt-get update
    $SUDO apt-get install "${yes[@]}" \
      libwebkit2gtk-4.1-dev \
      libgtk-3-dev \
      libsoup-3.0-dev \
      libssl-dev \
      librsvg2-dev \
      libayatana-appindicator3-dev \
      patchelf file curl wget \
      build-essential pkg-config
  elif have pacman; then
    say "installing system deps (pacman)…"
    local noconfirm=(); [[ "$ASSUME_YES" == "1" ]] && noconfirm=(--noconfirm)
    $SUDO pacman -S --needed "${noconfirm[@]}" \
      webkit2gtk-4.1 gtk3 libsoup3 openssl librsvg \
      libappindicator-gtk3 patchelf file curl wget base-devel
  elif have zypper; then
    say "installing system deps (zypper)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO zypper install "${yes[@]}" \
      webkit2gtk3-soup2-devel gtk3-devel libsoup-devel libopenssl-devel \
      librsvg-devel libappindicator3-devel patchelf file curl wget \
      gcc gcc-c++ make pkg-config
  else
    warn "unknown package manager — install WebKitGTK 4.1 + a C toolchain manually:"
    warn "  webkit2gtk-4.1, gtk3, libsoup3, openssl, librsvg2, appindicator, patchelf, gcc/g++/make, pkg-config"
    return
  fi
  ok "system deps present"
}

# ── 2. Rust toolchain ───────────────────────────────────────────────────────
install_rust() {
  # rustup drops cargo in ~/.cargo/bin, which a fresh shell may not yet have on PATH.
  [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

  # `cargo`/`rustc` on PATH are usually rustup *proxies*: they exist even when no
  # toolchain is installed, none is the default, or the toolchain is only partially
  # installed. A half-installed toolchain is the nastiest case — `cargo --version`
  # succeeds while `rustc -vV` fails with "Missing manifest", so a later build dies
  # confusingly. Probe BOTH for real before trusting the install.
  rust_ok() { have cargo && have rustc && cargo --version >/dev/null 2>&1 && rustc --version >/dev/null 2>&1; }

  if rust_ok; then
    ok "rust toolchain present ($(cargo --version))"
    return
  fi

  if have rustup; then
    # rustup is here but the default toolchain is missing/partial. `toolchain install
    # --force` both selects stable and recovers a partially-installed one.
    say "repairing rust toolchain (rustup toolchain install stable)…"
    rustup toolchain install stable --force --no-self-update
    rustup default stable
  else
    say "installing rust toolchain (rustup)…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
  fi

  if rust_ok; then
    ok "installed $(cargo --version)"
  else
    warn "rust toolchain still not runnable — try: rustup toolchain install stable --force"
    return 1
  fi
}

# ── 3. Node ─────────────────────────────────────────────────────────────────
check_node() {
  if ! have node; then
    warn "Node.js not found — install Node >= 20 (nvm, fnm, or your distro) then re-run."
    return 1
  fi
  local major
  major="$(node -p 'process.versions.node.split(".")[0]')"
  if (( major < 20 )); then
    warn "Node $(node --version) is too old; Vite needs >= 20. Upgrade then re-run."
    return 1
  fi
  ok "node $(node --version)"
}

# ── 4. pnpm (Corepack, pinned by package.json) ──────────────────────────────
ensure_pnpm() {
  if have pnpm; then
    ok "pnpm $(pnpm --version)"
    return
  fi
  if have corepack; then
    say "enabling pnpm via Corepack…"
    corepack enable >/dev/null 2>&1 || true
    corepack prepare --activate >/dev/null 2>&1 || true
  fi
  if have pnpm; then
    ok "pnpm $(pnpm --version)"
  else
    warn "pnpm not found and Corepack unavailable — run: npm install -g pnpm"
    return 1
  fi
}

# ── 5. project deps ─────────────────────────────────────────────────────────
fetch_deps() {
  [[ "$FETCH" != "1" ]] && { warn "skipping dependency fetch (--no-fetch)"; return; }

  say "installing JS deps (pnpm install)…"
  pnpm install
  ok "node_modules ready"

  if have cargo && cargo --version >/dev/null 2>&1; then
    say "fetching Rust crates (cargo fetch)…"
    cargo fetch --manifest-path src-tauri/Cargo.toml
    ok "cargo registry primed"
  fi
}

# ── run ─────────────────────────────────────────────────────────────────────
install_system_deps
install_rust
NODE_OK=1; check_node || NODE_OK=0
PNPM_OK=1; ensure_pnpm || PNPM_OK=0
if [[ "$NODE_OK" == "1" && "$PNPM_OK" == "1" ]]; then
  fetch_deps
else
  warn "skipping dependency fetch until Node + pnpm are sorted out"
fi

say "setup complete"
if have cargo && cargo --version >/dev/null 2>&1 && [[ "$NODE_OK" == "1" && "$PNPM_OK" == "1" ]]; then
  ok "you're ready — run: pnpm tauri dev"
  # If rust was just installed, the current interactive shell still lacks it.
  if ! command -v cargo >/dev/null 2>&1 || [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    warn "open a new shell (or run: source \"\$HOME/.cargo/env\") so cargo is on PATH"
  fi
else
  warn "resolve the warnings above, then re-run: pnpm bootstrap"
fi
