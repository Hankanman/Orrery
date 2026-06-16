# Orrery native spike (Phase 0)

**Throwaway.** Lives only on `spike/native-gpui`. Its single job: answer the
go/no-go question for a full native rewrite —

> Does GPU-native rendering kill the WebKitGTK CPU tax on the NVIDIA path?

It renders Orrery's repo grid in [GPUI](https://github.com/zed-industries/zed)
(Zed's GPU-native, Vulkan-backed toolkit) against the **real** SQLite cache, so
you can run it next to the Tauri build and compare CPU directly.

## What it reuses (no drift)

All logic lives in the **`orrery-core`** crate (`crates/orrery-core`) — scan,
git, host APIs, AI, cache, config, launchers — with zero UI and zero Tauri.
`orrery-app` depends on it directly; the grid reads `orrery_core::cache::load_repos()`,
which loads `~/.local/share/orrery/cache.sqlite`. The legacy Tauri shell
(`src-tauri/`) re-exports the same core, so both apps share one source of truth
until the Tauri shell is deleted at cutover.

(Earlier this crate `#[path]`-included `model.rs`/`cache.rs` directly from
`src-tauri` — a throwaway-spike shortcut, now replaced by the real crate.)

What it does **not** reuse: the 1,655-line CSS design system (it can't cross the
native boundary). Card styling is re-approximated in `src/main.rs` with the same
dark `--orr-*` palette.

## Prerequisites

GPU-native rendering needs a working Vulkan stack. On this box it's already
present (NVIDIA RTX 3080 Ti, proprietary driver, Wayland/KDE). To verify
elsewhere:

```bash
vulkaninfo --summary | grep deviceName        # must list a real GPU
pkg-config --exists vulkan wayland-client xkbcommon fontconfig freetype2 && echo OK
```

## Build & run

```bash
cd crates/orrery-app
cargo run --release      # first build is slow: it compiles all of GPUI
```

> Requires Rust **1.95.0** (GPUI v1.6.3 uses `std::hint::cold_path`, stabilized
> in 1.95). A `rust-toolchain.toml` pins it, so rustup auto-installs/selects it;
> the repo default of 1.94 fails with `E0658`.

For a fast edit/run loop (debug build, GPUI cached → ~2s):

```bash
cargo run            # build + launch; prints "[native] loaded N repos…"
```

## Hygiene (lint + format)

```bash
cargo fmt            # format this crate's own sources
cargo fmt -- --check # CI-style: fail if unformatted
cargo clippy         # lint (warnings); add `-- -D warnings` to fail on any
```

`rustfmt`/`clippy` ship via the workspace-pinned toolchain (`components` in the
root `rust-toolchain.toml`). Logic lives in its own `orrery-core` crate now, so
the hygiene tools here only touch UI code. `cargo clippy --workspace -- -D
warnings` is green. Lint policy lives in `[lints]` in each crate's `Cargo.toml`.

You should get a populated grid only if the cache is warm. If you see
"No cached repos", launch the Tauri app once (`pnpm tauri dev`) to populate
`cache.sqlite`, then relaunch the spike.

## Measuring CPU vs. the Tauri build

The comparison that matters is **idle + scroll repaint cost** on the NVIDIA path,
since that's where WebKitGTK juggers. Run each binary on its own and watch CPU
for the *same* interaction (scroll the grid up and down for ~15s).

```bash
# Native spike
cargo run --release &
# find its pid, then sample:
pidstat -h -u -p $(pgrep -f orrery-app) 1 20

# Tauri build (separately) — measure the WebKit render process, not just the shell:
pnpm tauri dev   # or the release binary
# WebKitGTK spawns helper processes; sum them:
pidstat -h -u -p ALL 1 20 | grep -i 'WebKit\|orrery'
```

## Result (measured 2026-06-16, RTX 3080 Ti / NVIDIA / KDE Wayland)

Same interaction (scroll the 22-repo grid), `pidstat 1` samples. The Tauri
figure sums the shell process and `WebKitWebProcess` (the renderer).

| Metric | Native spike (GPUI) | Tauri build (WebKitGTK) |
| --- | --- | --- |
| CPU % idle (focused, no input) | ~0–1% | ~0–1% |
| CPU % scrolling — average | **~5.7%** | **~43%** (WebKit ~34% + shell ~9%) |
| CPU % scrolling — peak | 7% | ~62% (WebKit 53% + shell 9%) |
| Binary size (stripped) | 25M, no webview runtime dep | 18M + system webkit2gtk |

**~7.5× lower scroll CPU.** The webview renderer alone sat at 30–50% sustained
while the native window never crossed 7%. Both idle near zero — WebKitGTK's tax
is a *repaint* cost, which is exactly what the scroll rows show. The spike's UI
is simpler than the real GridView, so the shipped port won't be a clean 7.5×,
but the hypothesis (the webview renderer is the scroll-time CPU hog) is
confirmed. **Verdict: GO.**

If the native column is convincingly lower on the scroll/idle CPU rows, Phase 0
is a GO and the full rewrite is justified. If GPUI misbehaves on the NVIDIA path
(blank window, tearing, crashes), re-run the spike against **Iced** (wgpu→Vulkan)
before drawing any conclusion — the data layer here is toolkit-agnostic, so only
`src/main.rs`'s render code changes.

## Caveats

- `uniform_list` here windows the grid (only visible rows build elements),
  mirroring Orrery's `VirtualRepoGrid`. That's the fair comparison; a
  non-virtualized render would flatter neither side.
- GPUI is pinned to Zed tag `v1.6.3` for reproducibility. It is not on
  crates.io.
- No interactivity beyond scrolling — this is a render-cost probe, not a port.
