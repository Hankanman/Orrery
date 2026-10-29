# Getting started

Install a packaged build, or build from source. Both are covered below.

## Install a release

Grab the latest artifact for your distro from the [releases page](https://github.com/Hankanman/Orrery/releases):

```sh
# Optional: verify the download against the release's SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing

# AppImage — portable, no install
chmod +x Orrery-x86_64.AppImage
./Orrery-x86_64.AppImage

# Debian / Ubuntu
sudo apt install ./orrery_*_amd64.deb

# Fedora
sudo dnf install ./orrery-*.x86_64.rpm
```

The **AppImage** bundles a `llama.cpp` engine, so the [local-AI](./local-ai) features work out of the box once you download a model. The `.deb` and `.rpm` don't; use Ollama or a `llama-server` on your `PATH` (see [Local AI](./local-ai)). Code search needs [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`) installed.

## Prerequisites

Orrery is a native Rust app on [GPUI](https://www.gpui.rs) — no webview. Building
from source needs:

- **Rust** — the toolchain pinned in `rust-toolchain.toml` (rustup auto-selects it)
- **GPUI system libraries** — Vulkan, Wayland/XCB, `libxkbcommon`, `fontconfig`,
  plus a C/C++ toolchain, `cmake` and `pkg-config`

The repo ships a setup script that installs these across dnf/apt/pacman/zypper:

```sh
bash scripts/setup.sh
```

Or install them by hand. On a Debian/Ubuntu base:

```sh
sudo apt install libvulkan-dev libwayland-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libxcb1-dev libfontconfig1-dev libssl-dev \
  build-essential cmake pkg-config
```

On Fedora:

```sh
sudo dnf install vulkan-loader-devel vulkan-headers mesa-vulkan-drivers \
  wayland-devel libxkbcommon-devel libxkbcommon-x11-devel libxcb-devel \
  fontconfig-devel openssl-devel gcc gcc-c++ make cmake pkgconf-pkg-config
```

> Node + pnpm are **optional** — only the docs site and the icon generator use them.

## Clone and run

```sh
git clone https://github.com/Hankanman/Orrery.git
cd Orrery
cargo run -p orrery          # first build is slow: it compiles all of GPUI
```

On first launch, open **Settings → Workspace roots** and point Orrery at the directories where you keep your projects (defaults to `~/dev`). Then **Save**.

## Build a release

Tagged releases (`v*`) build the bundles in CI. To produce them locally:

```sh
cargo build --release --locked -p orrery
cargo install --locked cargo-deb@3.8.0 cargo-generate-rpm@0.21.0
cargo deb -p orrery --no-build          # → target/debian/*.deb
cargo generate-rpm -p crates/orrery \
  --metadata-overwrite packaging/rpm-requires.toml   # → target/generate-rpm/*.rpm
bash packaging/fetch-llama.sh           # optional: bundle llama-server in the AppImage
bash packaging/appimage.sh              # → dist-appimage/Orrery-x86_64.AppImage (fetches pinned linuxdeploy)
bash packaging/collect-artifacts.sh     # → dist/ + SHA256SUMS
```

CI runs this same sequence as a dry run on every pull request (`.github/workflows/package.yml`), so packaging breakage shows up before a tag.

## Useful commands

| Command | What it does |
|---|---|
| `cargo run -p orrery` | Run the desktop app |
| `cargo build --workspace` | Build everything |
| `cargo test --workspace` | Run the tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint |
| `cargo fmt --all` | Format |
| `bash scripts/dev-clean.sh` | Force an asset re-embed and relaunch |
| `pnpm icons` | Regenerate the committed icon SVGs (optional) |

Next: the [feature tour](./mission-control) or [configuration reference](./configuration).
