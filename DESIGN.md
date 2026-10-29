# Orrery — Design Spec

> A Linux-native command center that puts every repo in your dev directories into orbit — live git status at a glance, one-click launch into your IDE or a terminal coding agent, enriched with multi-host data and local-AI summaries.

This is the living design reference. Decisions here were settled during the design phase (June 2026) and are considered locked unless explicitly revisited.

## Identity

- **Name:** Orrery · **Tagline:** *every repo in orbit*
- **App id:** `com.orrery.app` · **Window title:** "Orrery"
- **Org/domain (when needed):** `orreryhq` / `orrery.app` · `orrery.dev`

An orrery is a clockwork model of a solar system — orbiting bodies you survey at a glance. The metaphor maps directly: your projects as a little system you oversee, with "stars" built in.

## Stack

> History: Orrery began as a Tauri 2 + React/TypeScript app (Rust core ↔ webview
> over IPC). It was rewritten as a **native Rust GPUI app** — the webview was
> CPU-bound on the NVIDIA path; GPU compositing removed that bottleneck. The
> tables below describe the current stack.

| Layer | Choice | Notes |
|---|---|---|
| UI | **GPUI** (Zed's GPU UI framework) | Native Rust, in-process — no webview, no IPC. `gpui-component` for widgets |
| Rendering | **`blade` (Vulkan)** | GPU-composited; Wayland/X11 direct |
| Aesthetic | **Dark, dense, "mission control"** | 4–5 cards/row, data-rich, neon accents |
| Git | **`git2`** (libgit2) | No per-repo subprocess |
| Persistence | **SQLite** + **TOML config** | `~/.local/share/orrery/` (cache, models), `~/.config/orrery/config.toml` |
| Hosts | **`GitForge` providers** | GitHub + GitLab (incl. self-hosted), device-flow OAuth |
| AI | **Ollama** or embedded **llama.cpp** | Ollama over HTTP, or a bundled `llama-server` (GGUF weights downloaded on first run) |

## Architecture

A three-crate Cargo workspace; the UI calls the core directly (no IPC boundary).

```
┌──────────────────────────────── orrery-core ────────────────────────────────┐
│  Config (toml)   Scanner (walk→.git)   GitMeta (git2)   Launcher (templates) │
│  Cache (SQLite)  GitForge providers ── GitHub | GitLab(+self-hosted)         │
│  AiService (Ollama / bundled llama.cpp + downloaded GGUF)                    │
└──────────────────────────────────────────────────────────────────────────┘
        ↑ direct calls (sync git/fs on a bg pool; network via a tokio bridge)
┌──────────────────────── orrery (GPUI app) ──────┬─── orrery-platform ────────┐
│  Shell · Card · Drawer · Command palette ·       │  appearance · tray ·       │
│  views/ (Inbox/Feed/Explore/Cleanup/Agents/      │  notify · watcher ·        │
│  DevTools/Settings) · new-project dialog         │  shortcut · agent detect   │
└──────────────────────────────────────────────────┴────────────────────────┘
```

## Repo identity / name resolution

Display name resolves in this order (works offline from Phase 1; hosts only enrich):

1. **README H1** (`# Next.js`) → human display name (large on card)
2. **Host description** → tagline/subtitle (Phase 2)
3. **Remote slug** `owner/repo` → fallback **and** the host join key
4. **Directory name** → final fallback (shown small beside/below the display name)

> Note: GitHub/GitLab repos have no separate human "display name" field — the API gives a slug + description. The prettier title almost always lives in the README H1, which is why it's the primary source and available offline.

## Card anatomy (MVP)

```
┌─────────────────────────────────────┐
│ ●  Display Name        ⌥ Rust        │  big name + language dot/badge
│    owner/repo · ~/dev/folder         │  slug + path, small
│    First line of README description  │  enrichment
│  ⎇ main   ↑2 ↓0   ● 3 changes        │  branch · ahead/behind · dirty count
│  ⟳ last commit 4h ago     [stale?]   │  activity (heuristic)
│  [ Open in IDE ]   [ ◗ Agent ]       │  launchers
└─────────────────────────────────────┘
```

## Multi-host (`GitForge` trait, Phase 2)

Match by parsing the **remote URL host** → route to the right provider. The card model stays uniform; only the provider plumbing differs.

- **GitHub** — `api.github.com`, device-flow OAuth
- **GitLab** — `gitlab.com` **and configurable self-hosted base URLs** (e.g. `gitlab.acme.io`), device-flow / PAT
- Future drop-ins: Gitea / Codeberg, Bitbucket

## Roadmap

The original four phases have all shipped; the native GPUI rewrite replaced the
Tauri/React shell along the way.

- **Phase 1 — Local-first grid (shipped).** Config · scanner (depth + ignore globs, worktree-aware) · git metadata · dark dense grid · sort/filter/search · command palette · IDE/agent launcher · heuristic language/type/activity.
- **Phase 2 — Multi-host sync (shipped).** `GitForge` (GitHub + GitLab/self-hosted) enrichment, GitHub device-flow auth, stars/topics/releases/issues on cards, offline cache.
- **Phase 3 — Local AI (shipped).** Ollama or a bundled llama.cpp engine (AppImage); repo summaries, commit messages, changelogs, briefing, Ollama-backed semantic search.
- **Phase 4 — Starred / followed browser (shipped).** Inbox / Feed / Explore views (GitHub).
- **Since then (shipped).** Native GPUI rewrite · inotify live-watch · tray + notifications · attention engine (one ranked "needs you now" list behind badges, tray and notifications) · central CI pass · fleet bulk actions · Agents view (`/proc` session detection, dispatched worktrees, outcome detection) · hunk-level staging + commit composer · Cleanup and Dev Tools views.
- **Planned.** GitLab in Inbox / Feed / CI / notifications (enrichment only today) · project tags · light mode · Gitea / Codeberg / Bitbucket forges.

## Settled defaults

**Scanning**
- Locate repos by finding `.git`; do **not** recurse into a repo once found (submodules count as one repo).
- Configurable scan **depth** (default 3) and **ignore globs** (`node_modules`, `.cache`, `vendor`, `target`, …).
- Support **multiple root directories**.
- **Worktree-aware** — surface worktrees under their parent repo, not as duplicates.
- `inotify` **live-watch** (targeted: roots, repo roots and `.git` dirs, debounced) triggers a rescan; manual **refresh** stays available.

**Config & data (XDG)**
- Config: `~/.config/orrery/config.toml`
- Cache + SQLite + models: `~/.local/share/orrery/`

**Launcher**
- `{path}`-templated commands, e.g. IDE `code {path}`, agent `kitty --working-directory {path} -e claude`.
- PATH-detected sensible defaults (`code` / `zed` / `nvim`; agent via the user's terminal emulator).

**Heuristic classification (Phase 1, no AI)**
- Project type / primary language from manifest + extension signals (`Cargo.toml` → Rust, `package.json` → Node, `pyproject.toml`/`requirements.txt` → Python, `go.mod` → Go, …).
- Activity signal (`active` / `stale`) from last-commit recency.
