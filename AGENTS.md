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
  shell/                    OrreryApp state + Render; header, sidebar, grid, overlay,
                            attention, passes, confirm; actions/ = per-surface handlers
  card.rs drawer/ palette.rs  RepoCard, the repo drawer (+ its actions), the command palette
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
cargo build --workspace --locked    # build everything
cargo test --workspace --locked     # tests
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all                     # format (CI runs --check)
cargo deny check                    # advisories/licences/bans (CI gate; `cargo install cargo-deny --locked`)
pnpm install && pnpm icons          # regenerate the committed icon SVGs (optional)
pnpm docs:dev                       # docs site locally
```

GPUI needs system libs to build/run (Vulkan, Wayland/XCB, libxkbcommon,
fontconfig). `bash scripts/setup.sh` installs them per-distro.

## Definition of done

Before you consider a change complete, all of these must pass. They are exactly
the CI gates:

- `cargo build --workspace --locked`
- `cargo test --workspace --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo fmt --all --check`
- `cargo deny check`

Then **smoke-run the app**. The gates have missed a startup panic before (a tokio
timer built outside its runtime). Run
`timeout -s INT 25 ./target/debug/orrery 2>&1 | tail -20` and check that it doesn't
panic; exit 124, from the timeout, is success. It uses your real config and cache.
Leave no `orrery` process running afterwards. A panic-free run is still a weak
signal for render-path changes, so reason carefully about layout and `shape_line`.
For visual changes, screenshot the running window
(`spectacle -b -n -a -o shot.png` on KDE) and look at it.

Match the surrounding code's style, and don't introduce a formatter or linter pass
over untouched code.

## Working as an agent (and in parallel)

- **One worktree per task.** Each branch gets its own git worktree, created from
  the latest `origin/main`.
- **Build in place.** Run plain `cargo …` from the worktree root; it builds into
  that worktree's own `target/`.
  - Don't set `CARGO_TARGET_DIR`.
  - Never share a target dir across worktrees: cargo reports false "Fresh"
    results and runs stale test binaries built from another tree.
- **Stay in scope.** When several agents run at once, each owns a stated set of
  files. If a fix needs something outside your scope, report it rather than
  editing.
- **Keep commands checkable by the permission system.** Anything the shell
  parser can't analyse makes the human approve it by hand. Avoid:
  - environment-variable prefixes (`FOO=bar cargo …`);
  - variable or command expansions (`$VAR`, `${…}`, `$?`, `$(…)`, backticks);
  - `cd other/dir && git …`;
  - in-place edits with `sed -i`, `awk`, `perl -i`, inline `python` or heredoc
    redirects;
  - compound commands: `;`/`&&` chains, pipes, and `find | xargs`;
  - paths that climb out of the worktree (`../..`).

  Instead:
  - read files with the editor's read and search tools. If no search tool is
    available, use one plain `grep -rn 'pattern' crates/…` or `ls` with no pipe;
  - make file changes with the editor tools;
  - keep Bash for `cargo`, `git` (no `cd` first, no `-C`), `gh` and launching
    the app, one plain command per call from the worktree root;
  - use literal paths;
  - put multi-step logic in a small script file run with `bash ./script.sh`.
- **Verify before changing.** Check every claimed issue against the code first.
  If it turns out to be wrong, skip it and say why in the PR.
- **PR body.** List each change, anything skipped and why, and what a human
  should check by eye in the running app.
- **Agents don't merge.** The coordinating session (or the human) merges once
  CI is green.

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
- **Branches must be up to date with `main` before merging** (`gh pr update-branch <n>`, then wait for
  CI again). When several PRs are queued, merge them one at a time.
- **The required status check is named `Rust (fmt + clippy + test)`.** Don't rename that CI job. A
  renamed job never reports under the required name, so every PR is silently blocked.
- **Versioning follows [Semantic Versioning](https://semver.org).** Tags are `vMAJOR.MINOR.PATCH`,
  and the three crate versions must match the tag (`packaging/check-version.sh`). Pushing a tag
  builds a *draft* release; a human publishes it.
- Small, focused commits; imperative subject line; explain the *why* in the body.
- Don't commit secrets, tokens, or anything under `~/.config/orrery` / `~/.local/share/orrery`.
