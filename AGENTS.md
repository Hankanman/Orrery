# Contributing to Orrery (agents & humans)

Guidance for working in this repo. It's the single source of truth for AI coding
agents (Claude Code reads it via `CLAUDE.md`'s `@AGENTS.md` import; Codex, Cursor,
Aider, Gemini CLI, etc. read this file directly) and a useful orientation for
human contributors. See [`README.md`](README.md) for the product pitch,
[`DESIGN.md`](DESIGN.md) for the full spec, and the [docs site](https://hankanman.github.io/Orrery/)
for the feature tour.

## What this is

Orrery is a **Tauri 2** desktop app for Linux: a Rust core (scanning, git, host
APIs, caching, AI) exposes data to a **React + TypeScript** webview over IPC. A
local SQLite cache makes the grid paint instantly and work offline.

## Layout

```
src/                  Frontend (React + TS + Tailwind + shadcn/ui)
  pages/              Route views: GridView, InboxView, FeedView, ExploreView, SettingsView
  components/         RepoCard, RepoDrawer, BrandIcon, LangIcon, Virtual* ; layout/ ; ui/ (shadcn)
  lib/                ipc.ts (Rust bridge), repos-context.tsx (state), format.ts, launchers.ts,
                      brand-icons.ts / lang-icons.ts (generated), mock-*.ts (demo fixtures)
  index.css           The design system — CSS custom properties + all component styles
  router.tsx          Code-based TanStack Router
src-tauri/src/        Rust core
  commands.rs         #[tauri::command] entry points (the IPC surface)
  scan.rs git_ops.rs  Repo discovery + libgit2 status
  forge.rs inbox.rs   GitHub/GitLab REST/GraphQL (reqwest)
  ai.rs               Ollama HTTP (summaries, commit msgs, embeddings)
  cache.rs            SQLite (rusqlite, bundled) — snapshot, host enrichment, favorites, AI cache
  config.rs model.rs  TOML config + shared serde types
  appearance.rs tray.rs  zbus theme/accent, system tray
docs/                 VitePress site (deployed to GitHub Pages)
```

When you add a field to a shared type, change it in **both** `src-tauri/src/model.rs`
(serde, `camelCase`) and `src/types.ts` / `src/lib/ipc.ts`, and update every Rust
`Repo { .. }` struct literal (scan.rs + the test fixtures in cache.rs/ai.rs).

## Commands

```bash
pnpm install
pnpm tauri dev        # run the desktop app (hot reload)
pnpm dev              # frontend-only in the browser (uses mock fixtures, no Rust)
pnpm build            # tsc + vite build (run before committing frontend changes)
pnpm test             # Vitest
pnpm tauri:build      # release bundles (deb + rpm + AppImage; NO_STRIP=true)
pnpm docs:dev         # docs site locally
( cd src-tauri && cargo test && cargo check )   # Rust
```

## Definition of done

Before you consider a change complete: **`pnpm build` + `pnpm test` pass**, and for
Rust changes **`cargo test` + `cargo check` pass with no new warnings**. `tsc` is
part of `pnpm build`, so type errors fail the build. Match the surrounding code's
style; don't introduce a formatter/linter pass over untouched code.

## Conventions that matter here

- **Flat design is a performance contract.** WebKitGTK is CPU-bound on the NVIDIA
  path, so `index.css` deliberately avoids `backdrop-filter`, gradients, fixed
  backgrounds, and large shadow repaints inside the webview. Keep it flat; style
  with the `--orr-*` tokens and elevation borders/shadows. See
  [`docs/rendering-performance.md`](docs/rendering-performance.md).
- **`aiReady` gates all AI UI.** When AI is disabled/unreachable, every AI
  affordance is *hidden*, not broken. Gate new AI features on `aiReady` from
  `repos-context`.
- **Cache helpers are testable via `_on(conn)`.** Public `cache::*` fns open a
  connection; the logic lives in `*_on(conn)` variants tested against an in-memory
  SQLite. Follow that split when adding cache functions.
- **Generated icon data is committed; the source packages are devDependencies.**
  `brand-icons.ts` (simple-icons) and `lang-icons.ts` (devicon) are generated and
  committed so the client bundle never pulls the full icon barrels. Regenerate by
  extracting paths from the dev package — never import the barrel at runtime.
- **Demo fixtures power the browser build.** Views fall back to `src/lib/mock-*.ts`
  when `!isTauri()`, so `pnpm dev`/`pnpm preview` show realistic, fictional, public
  data. Keep fixtures fictional and public (used for docs screenshots).
- **`RepoCard` is memoized** and the context callbacks are stable — preserve object
  identity for unchanged repos so batched enrich/summarize updates only re-render
  changed cards. Lists are virtualized (`Virtual*`); keep new long lists windowed.
- **Security in `forge.rs`:** a GitLab token is only ever sent to `gitlab.com` or an
  explicitly trusted self-hosted host — never to an arbitrary domain from a repo
  remote. Don't loosen this.

## Docs & screenshots

The docs site lives in `docs/` and auto-deploys to GitHub Pages on pushes that touch
it. Screenshots are captured from the **browser demo build** (`pnpm preview`) with
Mission Control's **visibility filter set to Public**, so private repos never appear
in published images. Put new screenshots in `docs/public/shots/`.

## Commits & PRs

- **`main` is protected — never push to it directly. All changes go through a pull request.**
  Branch (`git checkout -b ...`), push the branch, open a PR (`gh pr create`), let CI go green,
  then merge.
- **Linear history is enforced** — merge with **squash or rebase**, not a merge commit
  (`gh pr merge --squash --delete-branch`). No force-pushes to `main`.
- The ruleset requires a PR but **0 approving reviews**, so a solo contributor can self-merge once
  CI passes. Wait for CI (build + tests + clippy) to be green before merging.
- Small, focused commits; imperative subject line; explain the *why* in the body.
- Don't commit secrets, tokens, or anything under `~/.config/orrery` / `~/.local/share/orrery`.
