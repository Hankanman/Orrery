#!/usr/bin/env bash
#
# setup — get a machine ready to dev Orrery from a clean slate.
#
# Orrery is a native Rust GPUI desktop app (no webview, no Tauri). It renders
# with GPU (blade/Vulkan) and talks Wayland/X11 directly, so the build needs the
# Vulkan, Wayland/XCB, xkbcommon and fontconfig headers plus a C/C++ toolchain.
# Node + pnpm are optional — only the docs site and the icon generator use them.
#
# What it ensures:
#   1. System build/runtime deps (Vulkan, Wayland/X11, xkbcommon, fontconfig).
#   2. The Rust toolchain (rustup + the pinned channel from rust-toolchain.toml).
#   3. (optional) Node + pnpm for the docs site / icon generation.
#   4. Rust deps fetched (`cargo fetch`).
#
# Flags:
#   --no-fetch    skip dependency fetch (system + toolchains only).
#   --skip-system don't touch system packages (no sudo) — toolchains only.
#   -y, --yes     pass non-interactive/assume-yes to the package manager.
#
# Safe to re-run. The only step that needs sudo is the system-package install.
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
      sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# ── helpers ─────────────────────────────────────────────────────────────────
say()  { printf '\n\033[1m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

SUDO=""
if [[ "$SYSTEM" == "1" && "$(id -u)" != "0" ]]; then
  if have sudo; then SUDO="sudo"; else
    warn "no sudo and not root — system packages may fail; use --skip-system to bypass"
  fi
fi

# ── 1. system build/runtime deps (GPUI: Vulkan + Wayland/X11 + fontconfig) ───
install_system_deps() {
  if [[ "$SYSTEM" != "1" ]]; then
    warn "skipping system packages (--skip-system)"
    return
  fi

  if have dnf; then
    say "installing system deps (dnf)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO dnf install "${yes[@]}" \
      vulkan-loader-devel vulkan-headers mesa-vulkan-drivers \
      wayland-devel libxkbcommon-devel libxkbcommon-x11-devel \
      libxcb-devel fontconfig-devel openssl-devel \
      gcc gcc-c++ make cmake pkgconf-pkg-config
  elif have apt-get; then
    say "installing system deps (apt)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO apt-get update
    $SUDO apt-get install "${yes[@]}" \
      libvulkan-dev vulkan-tools mesa-vulkan-drivers \
      libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
      libxcb1-dev libfontconfig1-dev libssl-dev \
      build-essential cmake pkg-config
  elif have pacman; then
    say "installing system deps (pacman)…"
    local noconfirm=(); [[ "$ASSUME_YES" == "1" ]] && noconfirm=(--noconfirm)
    $SUDO pacman -S --needed "${noconfirm[@]}" \
      vulkan-icd-loader vulkan-headers \
      wayland libxkbcommon libxcb fontconfig openssl \
      base-devel cmake pkgconf
  elif have zypper; then
    say "installing system deps (zypper)…"
    local yes=(); [[ "$ASSUME_YES" == "1" ]] && yes=(-y)
    $SUDO zypper install "${yes[@]}" \
      vulkan-devel wayland-devel libxkbcommon-devel libxkbcommon-x11-devel \
      libxcb-devel fontconfig-devel libopenssl-devel \
      gcc gcc-c++ make cmake pkg-config
  else
    warn "unknown package manager — install these manually:"
    warn "  Vulkan loader+headers, Wayland, libxkbcommon(+x11), libxcb, fontconfig, openssl, cmake, gcc/g++"
    return
  fi
  ok "system deps present"
}

# ── 2. Rust toolchain (channel pinned by rust-toolchain.toml) ────────────────
install_rust() {
  [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
  rust_ok() { have cargo && have rustc && cargo --version >/dev/null 2>&1 && rustc --version >/dev/null 2>&1; }

  if rust_ok; then
    ok "rust toolchain present ($(cargo --version))"
    return
  fi
  if have rustup; then
    say "repairing rust toolchain…"
    rustup show >/dev/null 2>&1 || rustup toolchain install stable --force --no-self-update
  else
    say "installing rust toolchain (rustup)…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
  fi
  rust_ok && ok "installed $(cargo --version)" || warn "rust toolchain still not runnable"
}

# ── 3. (optional) Node + pnpm — docs site + icon generation only ─────────────
ensure_pnpm() {
  if ! have node; then
    warn "Node.js not found — only the docs site / icon generator need it; skipping."
    return 1
  fi
  if have pnpm; then ok "pnpm $(pnpm --version)"; return; fi
  if have corepack; then
    corepack enable >/dev/null 2>&1 || true
    corepack prepare --activate >/dev/null 2>&1 || true
  fi
  have pnpm && ok "pnpm $(pnpm --version)" || { warn "pnpm not found (npm i -g pnpm)"; return 1; }
}

# ── 4. project deps ─────────────────────────────────────────────────────────
fetch_deps() {
  [[ "$FETCH" != "1" ]] && { warn "skipping dependency fetch (--no-fetch)"; return; }
  if have cargo && cargo --version >/dev/null 2>&1; then
    say "fetching Rust crates (cargo fetch)…"
    cargo fetch
    ok "cargo registry primed"
  fi
  if have pnpm; then
    say "installing docs/icon deps (pnpm install)…"
    pnpm install
    ok "node_modules ready"
  fi
}

# ── run ─────────────────────────────────────────────────────────────────────
install_system_deps
install_rust
ensure_pnpm || true
fetch_deps

say "setup complete"
if have cargo && cargo --version >/dev/null 2>&1; then
  ok "you're ready — run: cargo run -p orrery"
  if [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    warn "open a new shell (or run: source \"\$HOME/.cargo/env\") so cargo is on PATH"
  fi
else
  warn "resolve the warnings above, then re-run: bash scripts/setup.sh"
fi
