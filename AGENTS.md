# Contributing to Orrery (agents & humans)

Guidance for working in this repo. It's the single source of truth for AI coding
agents (Claude Code reads it via `CLAUDE.md`'s `@AGENTS.md` import; Codex, Cursor,
Aider, Gemini CLI, etc. read this file directly) and a useful orientation for
human contributors. See [`README.md`](README.md) for the product pitch,
[`DESIGN.md`](DESIGN.md) for the full spec, and the [docs site](https://hankanman.github.io/Orrery/)
for the feature tour.

## What this is

Orrery is a **native Rust desktop app for Linux** built on **GPUI** (Zed's GPU UI
framework) — no webview, no Tauri. A Rust core (scanning, git, host APIs, caching,
AI) feeds a GPUI front-end directly, in-process. A local SQLite cache makes the
grid paint instantly and work offline.

> Note: earlier versions were a Tauri 2 + React/TypeScript app. That stack was
> removed at the native cutover; if you find references to `src-tauri/`, `src/`,
> `pnpm tauri`, IPC, or a webview, they're stale — fix them.

## Layout

A three-crate Cargo workspace under `crates/`:

```
crates/orrery-core/src/     Logic (no UI). Reused unchanged from the Tauri era.
  scan.rs git_ops.rs        Repo discovery + libgit2 status/branches/worktrees
  forge.rs inbox.rs         GitHub/GitLab REST/GraphQL (reqwest)
  enrich.rs                 host-enrichment pass (forge → host cache), token-egress gated
  ai.rs llama.rs            Ollama / llama.cpp (summaries, commit msgs, embeddings)
  cache.rs                  SQLite (rusqlite, bundled) — snapshot, enrichment, favorites
  config.rs model.rs        TOML config + shared serde types
  oauth.rs search.rs launch.rs  GitHub device flow, ripgrep search, process launch
crates/orrery-platform/src/ Linux desktop integration (no UI, no Tauri)
  appearance.rs             zbus theme/accent
  tray.rs                   system tray (ksni / StatusNotifierItem)
  notify.rs notifier.rs watcher.rs shortcut.rs  notifications, attention poll, fs watch
  agents.rs                 detect running agent sessions via /proc
crates/orrery/src/          The GPUI app (UI + the `orrery` binary)
  main.rs                   App/window setup, key bindings, close-to-tray
  shell.rs                  Header + sidebar nav + view switching + OrreryApp state
  card.rs drawer.rs palette.rs  RepoCard, the repo drawer, the command palette
  views/                    inbox feed explore cleanup agents devtools settings newproject
  theme.rs                  The design system as `--orr-*` tokens → gpui colors
  data.rs                   core::model → flat render-ready Row
  task.rs live.rs           async (tokio) bridge + background→foreground signal wiring
  assets/                   embedded fonts + generated icon SVGs (rust-embed)
docs/                       VitePress site (deployed to GitHub Pages)
packaging/                  .desktop, app icons, AppImage script, AUR PKGBUILD
```

When you add a field to a shared type, change `crates/orrery-core/src/model.rs`
and update every Rust `Repo { .. }` struct literal (scan.rs + the test fixtures in
cache.rs/ai.rs) and the flattening in `crates/orrery/src/data.rs`.

## Commands

```bash
cargo run -p orrery                 # run the desktop app
cargo build --workspace             # build everything
cargo test --workspace              # tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                     # format (CI runs --check)
pnpm install && pnpm icons          # regenerate the committed icon SVGs (optional)
pnpm docs:dev                       # docs site locally
```

GPUI needs system libs to build/run (Vulkan, Wayland/XCB, libxkbcommon,
fontconfig). `bash scripts/setup.sh` installs them per-distro.

## Definition of done

Before you consider a change complete: **`cargo build --workspace`, `cargo test
--workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo
fmt --all --check` all pass.** These are exactly the CI gates. Match the
surrounding code's style; don't introduce a formatter/linter pass over untouched
code. The GUI can't be smoke-tested headlessly, so a panic-free `cargo run` is a
weak signal for render-path changes — reason about layout/`shape_line` carefully.

## Conventions that matter here

- **Flat design is a performance contract.** Keep the UI flat — style with the
  `--orr-*` tokens (`theme.rs`) and elevation borders/shadows; avoid heavy blur /
  gradient / large-repaint effects. See [`docs/rendering-performance.md`](docs/rendering-performance.md).
- **`aiReady` gates all AI UI.** When AI is disabled/unreachable, every AI
  affordance is *hidden*, not broken. Gate new AI features accordingly.
- **Cache helpers are testable via `_on(conn)`.** Public `cache::*` fns open a
  connection; the logic lives in `*_on(conn)` variants tested against an in-memory
  SQLite. Follow that split when adding cache functions.
- **Generated icon data is committed; the source packages are devDependencies.**
  `assets/icons/` (lucide / simple-icons / devicon SVGs) is generated by
  `crates/orrery/assets/generate-icons.mjs` (`pnpm icons`) and committed, so the
  app never needs npm at build time. Add an icon name to the generator + re-run.
- **Async + threading.** GPUI is single-threaded (entity mutation needs the
  foreground). Network/AI core calls go through the shared tokio runtime in
  `task.rs` (`task::run`); sync git/fs work goes on `cx.background_executor()`.
  Background desktop signals (fs watch, tray, appearance) marshal onto the
  foreground via an `async-channel` drained by one gpui task — see `live.rs`.
- **InputState needs a `Window`.** gpui-component text inputs are created in
  click/nav handlers that have a `&mut Window` (drawer/palette/settings/dialog
  open paths), not in render.
- **Security in `forge.rs`:** a GitLab token is only ever sent to `gitlab.com` or an
  explicitly trusted self-hosted host — never to an arbitrary domain from a repo
  remote. Don't loosen this.

## Docs & screenshots

The docs site lives in `docs/` and auto-deploys to GitHub Pages on pushes that
touch it. The old browser-demo screenshot workflow is gone with the React app;
existing screenshots in `docs/public/shots/` are kept. Re-capture from the native
app if needed, keeping private repos out of published images.

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
