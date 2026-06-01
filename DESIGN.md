# Orrery — Design Spec

> A Linux-native command center that puts every repo in your dev directories into orbit — live git status at a glance, one-click launch into your IDE or a terminal coding agent, enriched with multi-host data and local-AI summaries.

This is the living design reference. Decisions here were settled during the design phase (June 2026) and are considered locked unless explicitly revisited.

## Identity

- **Name:** Orrery · **Tagline:** *every repo in orbit*
- **App id:** `com.orrery.app` · **Window title:** "Orrery"
- **Org/domain (when needed):** `orreryhq` / `orrery.app` · `orrery.dev`

An orrery is a clockwork model of a solar system — orbiting bodies you survey at a glance. The metaphor maps directly: your projects as a little system you oversee, with "stars" built in.

## Stack (locked)

| Layer | Choice | Notes |
|---|---|---|
| Shell | **Tauri 2** | Rust core ↔ webview over IPC; small binary, native feel |
| Frontend | **Vite + React + TypeScript + Tailwind + shadcn/ui** | TanStack Router for client-side routing. *Not* Next.js — static export disables everything useful in a desktop app. |
| Aesthetic | **Dark, dense, "mission control"** | 4–5 cards/row, data-rich, neon accents |
| Git | **`git2`** (libgit2) | No per-repo subprocess |
| Persistence | **SQLite** + **TOML config** | `~/.local/share/orrery/` (cache, models), `~/.config/orrery/config.toml` |
| Hosts *(Ph.2)* | **`GitForge` trait** | GitHub (`octocrab`) + GitLab (incl. self-hosted), device-flow OAuth |
| AI *(Ph.3)* | embedded **llama.cpp** | Inference engine shipped in binary; GGUF weights downloaded on first run |

## Architecture

```
┌─────────────────────────── Rust core (src-tauri) ───────────────────────────┐
│  Config (toml)   Scanner (walk→.git)   GitMeta (git2)   Launcher (templates) │
│  Cache (SQLite)  GitForge providers ── GitHub | GitLab(+self-hosted)         │
│  AiService (llama.cpp, bundled engine + downloaded GGUF)                     │
└───────────────────────────────── IPC commands ──────────────────────────────┘
                                      ↕
┌──────────────────────── Frontend (Vite + React) ────────────────────────────┐
│  Grid view · Card · Filters/sort/search · Command palette (cmdk) · Settings  │
│  Starred/Followed browser (Ph.4)                                             │
└──────────────────────────────────────────────────────────────────────────┘
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

- **Phase 1 — Local-first grid (current).** Config · scanner (depth + ignore globs, worktree-aware) · git metadata · dark dense grid · sort/filter/search · command palette · IDE/agent launcher · heuristic language/type/activity. Zero external deps, usable daily.
- **Phase 2 — Multi-host sync.** `GitForge` (GitHub + GitLab/self-hosted), device-flow auth, stars/topics/releases/issues on cards, offline cache.
- **Phase 3 — Local AI summaries.** Bundled llama.cpp, per-repo "what is this / recent activity" blurbs.
- **Phase 4 — Starred / followed browser.** Separate view for discovering starred + followed repos across hosts.

## Settled defaults

**Scanning**
- Locate repos by finding `.git`; do **not** recurse into a repo once found (submodules count as one repo).
- Configurable scan **depth** (default 3) and **ignore globs** (`node_modules`, `.cache`, `vendor`, `target`, …).
- Support **multiple root directories**.
- **Worktree-aware** — surface worktrees under their parent repo, not as duplicates.
- Manual **refresh** for MVP; `inotify` live-watch is a Phase 1.5 addition.

**Config & data (XDG)**
- Config: `~/.config/orrery/config.toml`
- Cache + SQLite + models: `~/.local/share/orrery/`

**Launcher**
- `{path}`-templated commands, e.g. IDE `code {path}`, agent `kitty --working-directory {path} -e claude`.
- PATH-detected sensible defaults (`code` / `zed` / `nvim`; agent via the user's terminal emulator).

**Heuristic classification (Phase 1, no AI)**
- Project type / primary language from manifest + extension signals (`Cargo.toml` → Rust, `package.json` → Node, `pyproject.toml`/`requirements.txt` → Python, `go.mod` → Go, …).
- Activity signal (`active` / `stale`) from last-commit recency.
