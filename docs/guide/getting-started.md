# Getting started

Orrery is built from source for now. This guide covers the prerequisites and the build.

## Prerequisites

You'll need the Tauri 2 toolchain for Linux:

- **Rust** (stable) — install via [rustup](https://rustup.rs)
- **Node.js** 18+ and **pnpm** — `corepack enable` then `corepack prepare pnpm@latest --activate`
- **System libraries** for WebKitGTK and friends. On a Debian/Ubuntu base:

```sh
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

On Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel
sudo dnf group install "C Development Tools and Libraries"
```

See the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for other distros.

## Clone and run

```sh
git clone https://github.com/Hankanman/Orrery.git
cd Orrery
pnpm install

# Run the desktop app in dev (hot reload)
pnpm tauri dev
```

On first launch, open **Settings → Workspace roots** and point Orrery at the directories where you keep your projects (defaults to `~/dev`). Then **Save & rescan**.

::: tip Wayland + NVIDIA
WebKitGTK's DMABUF renderer can crash or render blank on some NVIDIA/Wayland setups. Orrery disables it automatically on KDE+Wayland; the flat, GPU-light design keeps the UI smooth on the CPU-bound path. See [Rendering performance](/rendering-performance) for the full investigation.
:::

## Build a release

```sh
pnpm tauri:build
```

This produces an AppImage (and other bundles) under `src-tauri/target/release/bundle/`. The `tauri:build` script sets `NO_STRIP=true` to work around a linuxdeploy `strip` incompatibility with newer `.relr.dyn` sections.

## Web demo build

The frontend runs in a browser without the Rust backend, using built-in fictional fixtures — handy for previewing the UI:

```sh
pnpm build && pnpm preview
```

This is exactly how the screenshots in these docs were captured.

## Useful scripts

| Command | What it does |
|---|---|
| `pnpm tauri dev` | Run the desktop app with hot reload |
| `pnpm dev` | Frontend-only dev server (browser, mock data) |
| `pnpm build` | Type-check and build the frontend |
| `pnpm test` | Run the Vitest suite |
| `pnpm tauri:build` | Build release bundles (AppImage, …) |
| `pnpm dev:clean` | Clean dev caches and restart |

Next: the [feature tour](./mission-control) or [configuration reference](./configuration).
