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
pnpm tauri build       # produce a release bundle
pnpm build            # frontend-only build (tsc + vite)
```

> **Wayland note:** the `tauri` script sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
> Without it, WebKitGTK can crash on some GPU/compositor combinations with
> `Error 71 (Protocol error) dispatching to Wayland display`. If you still hit
> rendering issues, try `GDK_BACKEND=x11 pnpm tauri dev`.

## License

TBD.
