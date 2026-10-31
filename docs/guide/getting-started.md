# Getting started

Install a packaged build, or build from source. Both are covered below.

## Install / update in one line

```sh
curl -fsSL https://raw.githubusercontent.com/Hankanman/Orrery/main/scripts/install.sh | bash
```

This installs the latest release AppImage for your user, with no root needed:

- the download is checked against the release's `SHA256SUMS`;
- it's stored in `~/.local/opt/orrery`;
- it's launched by a small `~/.local/bin/orrery` script;
- it gets an app-menu entry (`orrery.desktop`) and icon.

**AppImageLauncher:** if you have [AppImageLauncher](https://github.com/TheAssassin/AppImageLauncher) installed, it normally offers to "integrate" any AppImage you run, copying it into `~/Applications` with a second menu entry. The launcher script and menu entry set `APPIMAGELAUNCHER_DISABLE=1`, so the installed copy runs as it is. If AppImageLauncher already integrated a copy, remove it (right-click its menu entry → *Remove AppImage from system*). Keep the `orrery.desktop` entry this script installs: the global shortcut relies on it (see [below](#the-desktop-entry-and-the-global-shortcut)).

**Updating:** run the same command again. If you're already on the latest release it downloads nothing, and just refreshes the launcher and menu entry.

**Options:**

- `ORRERY_VERSION=v2026.10.1` installs a specific tag.
- `ORRERY_PREFIX=…` changes the install location from `~/.local`.

**Uninstalling:** append `-s -- --uninstall` to the command. Your config and cache are left in place.

## Install a release manually

Or grab the latest artifact for your distro from the [releases page](https://github.com/Hankanman/Orrery/releases):

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

### The desktop entry and the global shortcut

The summon shortcut (<kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>O</kbd>) goes through the desktop's GlobalShortcuts portal, which needs to know which app is asking. Orrery registers the app id `orrery` with the portal at startup, and the portal accepts it only if a desktop entry named `orrery.desktop` is installed. The packages and the install script provide one. A source build doesn't, so install the repo's copy once:

```sh
install -Dm644 packaging/orrery.desktop ~/.local/share/applications/orrery.desktop
```

Without it the log shows `couldn't register app id 'orrery' … App info not found`, and on KDE the shortcut fails with `An app id is required`. With it you'll see `registered app id 'orrery'` and `bound 'activate' → Ctrl+Alt+O`.

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
