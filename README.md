<div align="center">

# 🪐 Orrery

**every repo in orbit**

A Linux-native command center that puts every repo in your dev directories into orbit — live git status at a glance, one-click launch into your IDE or a terminal coding agent, enriched with multi-host data and local-AI summaries.

</div>

---

> **Status:** 🚧 Early development. Design phase complete; building Phase 1 (local-first grid). See [`DESIGN.md`](./DESIGN.md) for the full spec and [the issues](../../issues) for the roadmap.

## What is it?

Point Orrery at the directories where you keep your projects. It discovers every git repo inside them and lays them out in a dark, dense "mission control" grid. Each card fuses three sources of truth:

1. **Local git** — branch, ahead/behind, uncommitted changes, last commit, detected languages
2. **Your git host** *(GitHub & GitLab, incl. self-hosted)* — stars, topics, releases, issues
3. **Local AI** — a synthesized "what is this / what's been happening" blurb, generated on-device

…and every card is a launchpad: one click to open the repo in your IDE, or to drop a terminal coding agent (Claude Code, etc.) straight into it.

## Why?

There's no great *workspace dashboard* for Linux. GitKraken is heavy and git-focused; GitHub Desktop has no Linux build and is single-repo. Orrery is the at-a-glance morning view of everything you're working on — and the fastest way to jump back in.

## Stack

| Layer | Choice |
|---|---|
| Shell | [Tauri 2](https://tauri.app) — Rust core ↔ webview |
| Frontend | Vite + React + TypeScript + Tailwind + [shadcn/ui](https://ui.shadcn.com) |
| Git | `git2` (libgit2) |
| Persistence | SQLite + TOML config (XDG dirs) |
| Hosts | `GitForge` trait → GitHub (`octocrab`) + GitLab (incl. self-hosted) |
| Local AI | embedded llama.cpp, GGUF weights fetched on first run |

## Roadmap

- **Phase 1 — Local-first grid.** Scan → git metadata → grid → IDE/agent launcher. Zero external deps.
- **Phase 2 — Multi-host sync.** GitHub + GitLab (self-hosted), device-flow OAuth, stars/topics/releases on cards.
- **Phase 3 — Local AI summaries.** Bundled inference, per-repo blurbs.
- **Phase 4 — Starred / followed browser.** Discover your starred + followed repos across hosts.

Track progress in [the issue list](../../issues).

## Building

Prerequisites: a recent **Rust** toolchain, **Node + pnpm**, and the Tauri Linux
system libraries (`webkit2gtk-4.1`, `gtk3`, `libsoup-3.0`, `librsvg2`, plus a C
toolchain and `pkg-config`).

```bash
pnpm install          # install JS deps
pnpm tauri dev        # run the desktop app (Vite + Rust core)
pnpm tauri:build      # produce release bundles (deb + rpm + AppImage)
pnpm build            # frontend-only build (tsc + vite)
```

`tauri:build` runs `tauri build` with `NO_STRIP=true`. On modern distros (e.g.
Fedora) the linker emits a `.relr.dyn` section that the old `strip` bundled
inside `linuxdeploy` can't parse, which otherwise aborts the AppImage stage;
`NO_STRIP` skips that pass. Plain `pnpm tauri build` still works for deb/rpm.

## Linux display backend

On Linux the app configures two environment variables at startup (in
`run()`, before GTK/WebKit initialize). Both are only set if you haven't
already set them, so either can be overridden from the environment.

- **`WEBKIT_DISABLE_DMABUF_RENDERER=1`** — WebKitGTK's DMABUF renderer is
  broken on many drivers (notably NVIDIA), producing blank/garbled webviews
  or `Error 71 (Protocol error) dispatching to Wayland display`. It's
  disabled by default.
- **`GDK_BACKEND=x11` on KDE + Wayland only** — KWin only draws its
  server-side window decoration for X11/XWayland windows; GTK refuses
  server-side decorations on native Wayland, so a Wayland window gets a
  foreign-looking client-side titlebar instead of the system decoration.
  Forcing XWayland on KDE Wayland lets KWin draw the native titlebar.
  GNOME, wlroots, and X11 sessions are left untouched (CSD is the expected
  convention there).

This is decided at **runtime**, so a single build behaves correctly across
distros, desktops, and package formats — no per-package flags needed.

**Overrides:** run with `GDK_BACKEND=wayland orrery` to force native Wayland
(client-side decorations) on KDE, or `WEBKIT_DISABLE_DMABUF_RENDERER=0` to
re-enable the DMABUF renderer.

### Rendering smoothness on NVIDIA

WebKitGTK GPU-accelerates far less than Chromium, so the webview can feel
juddery where a browser is smooth — worst on NVIDIA, where we disable the
DMABUF renderer (above) and thereby give up accelerated compositing. Two things
help:

- **The app keeps WebKitGTK's accelerated compositor pinned on**
  (`hardware-acceleration-policy: ALWAYS`). By default it's on-demand and tears
  down between layers, which on NVIDIA shows as judder that disappears the
  instant you open the web inspector ([tauri#10566](https://github.com/tauri-apps/tauri/issues/10566)).
- **`ORRERY_WEBKIT_ACCEL=1`** keeps the DMABUF renderer *enabled* (skips the
  disable). On a modern stack — NVIDIA **open** kernel module + recent
  WebKitGTK, non-transparent window — DMABUF often works and is dramatically
  smoother. Try `ORRERY_WEBKIT_ACCEL=1 pnpm tauri dev`; if the window renders
  normally (not blank/garbled), this is the real fix and worth making default
  for your setup. If you're on a hybrid/Wayland NVIDIA box, also ensure
  `nvidia_drm.modeset=1` is set as a kernel parameter.

The CSS also drops CPU-bound effects (backdrop-filter blur, fixed backgrounds,
large shadow repaints) inside the webview only — the `pnpm dev` browser build
keeps the full glass.

## License

TBD.
