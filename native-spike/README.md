# Orrery native spike (Phase 0)

**Throwaway.** Lives only on `spike/native-gpui`. Its single job: answer the
go/no-go question for a full native rewrite —

> Does GPU-native rendering kill the WebKitGTK CPU tax on the NVIDIA path?

It renders Orrery's repo grid in [GPUI](https://github.com/zed-industries/zed)
(Zed's GPU-native, Vulkan-backed toolkit) against the **real** SQLite cache, so
you can run it next to the Tauri build and compare CPU directly.

## What it reuses (no drift)

The Rust core is untouched. `model.rs` and `cache.rs` are pulled in verbatim via
`#[path]` includes — the spike calls exactly one core function,
`cache::load_repos()`, which reads `~/.local/share/orrery/cache.sqlite`. The
whole point of going native is that this core survives a rewrite; the spike
proves it links and renders without a webview or IPC layer.

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
cd native-spike
cargo run --release      # first build is slow: it compiles all of GPUI
```

> Requires Rust **1.95.0** (GPUI v1.6.3 uses `std::hint::cold_path`, stabilized
> in 1.95). A `rust-toolchain.toml` pins it, so rustup auto-installs/selects it;
> the repo default of 1.94 fails with `E0658`.

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
pidstat -h -u -p $(pgrep -f orrery-native-spike) 1 20

# Tauri build (separately) — measure the WebKit render process, not just the shell:
pnpm tauri dev   # or the release binary
# WebKitGTK spawns helper processes; sum them:
pidstat -h -u -p ALL 1 20 | grep -i 'WebKit\|orrery'
```

Things to record for the go/no-go:

| Metric | Native spike | Tauri build |
| --- | --- | --- |
| CPU % idle (window focused, no input) | | |
| CPU % while scrolling the grid | | |
| Peak RSS | | |
| Cold-start to first paint | | |
| Binary size | | |

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
