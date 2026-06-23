# orrery

The native GPUI desktop app — the `orrery` binary. All logic lives in
[`orrery-core`](../orrery-core); this crate is purely the UI (theme, cards,
shell, views) plus the thin async/foreground plumbing. Desktop integration
(tray, notifications, appearance, watcher) lives in
[`orrery-platform`](../orrery-platform).

## Run

```bash
cargo run -p orrery          # debug build + launch (GPUI cached → ~2s)
cargo run -p orrery --release
```

GPUI renders on the GPU (blade/Vulkan) and talks Wayland/X11 directly, so the
build needs the Vulkan/Wayland/xkbcommon/fontconfig headers — `bash
scripts/setup.sh` installs them. Requires the toolchain pinned in
`rust-toolchain.toml` (rustup auto-selects it).

The grid paints from the SQLite cache (`~/.local/share/orrery/cache.sqlite`) and
then refreshes live; on first run, open **Settings → Workspace roots** and point
it at your project directories.

## Layout

```
src/
  main.rs              app/window setup, key bindings, close-to-tray
  shell.rs             header + sidebar nav + view switching + OrreryApp state
  card.rs              the RepoCard
  drawer.rs            repo detail drawer (Overview/Changes/PR/Notes/Readme)
  palette.rs           command palette (Ctrl+K): actions + repos + code/semantic search
  views/               inbox feed explore cleanup agents devtools settings newproject
  theme.rs             the design system as --orr-* tokens → gpui colors
  data.rs              orrery_core::model → flat render-ready Row
  task.rs live.rs      async (tokio) bridge + background→foreground signal wiring
  assets/              embedded fonts + generated icon SVGs (rust-embed)
```

## Hygiene

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These are the CI gates. Lint policy lives in `[lints]` in each crate's
`Cargo.toml`.
