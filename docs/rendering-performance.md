# Rendering performance: WebKitGTK on NVIDIA

> **Historical.** This documents the *original Tauri + WebKitGTK* build and the
> investigation that motivated the rewrite. Orrery is now a **native GPUI app**
> (GPU/Vulkan rendering, no webview), so none of these env vars / CSS overrides
> apply anymore — the CPU-bound repaint tax described here is exactly what going
> native removed. Kept as the record of *why* the rewrite happened.

A record of why the old webview build felt less smooth than the same UI in a
Chromium browser, everything we tried to close the gap, what actually shipped,
and what that depended on.

## TL;DR

- The UI is buttery in a Chromium browser (`pnpm dev`) but judders in the Tauri
  webview, worst on NVIDIA. **Chromium GPU-composites effects that WebKitGTK
  paints on the CPU.**
- On NVIDIA, WebKitGTK's GPU-accelerated path (DMABUF + GBM) **does not work at
  all** — every variation we tried either renders blank or crashes. This is an
  upstream WebKitGTK ↔ NVIDIA limitation, not something we can fix in-app.
- What we ship is the best achievable: **DMABUF disabled** (the only path that
  renders without crashing) + **accelerated compositor pinned on** + **CPU-bound
  CSS effects dropped in the webview** + a **Reduce-motion** escape hatch.
- On **Intel/AMD**, DMABUF works — those users can opt into full acceleration
  with `ORRERY_WEBKIT_ACCEL=1`.

## The setup this was diagnosed on

| | |
|---|---|
| GPU | NVIDIA GeForce RTX 3080 Ti |
| Driver | 595.71.05, **open** kernel module |
| WebKitGTK | 2.52.3 (`webkit2gtk4.1`) |
| OS / desktop | Fedora 44, KDE Plasma, **Wayland** session |
| App runs under | XWayland (we force `GDK_BACKEND=x11` on KDE+Wayland) |
| Tauri / wry webkit binding | tauri 2.11, `webkit2gtk` crate 2.0.2 |

## Why the browser is smooth and the webview isn't

`pnpm dev` opens the frontend in your system Chromium, which GPU-composites
`backdrop-filter`, fixed backgrounds, shadows and transforms. The shipped app
renders the *same* frontend in **WebKitGTK** (the Linux webview behind Tauri/
wry — the deb, rpm and AppImage all use it). WebKitGTK does much of that on the
CPU, and on NVIDIA it can't use its GPU compositing fast path at all (below), so
the same CSS that's free in the browser costs real frames in the wrapper.

## What we tried

### 1. Drop CPU-bound CSS effects in the webview — ✅ shipped

WebKitGTK composites `backdrop-filter: blur`, `background-attachment: fixed`,
and large-radius `box-shadow` on the CPU. With ~20 glass cards each re-blurring
their backdrop every scroll frame, that's the dominant cost. We tag `<html>`
with `.tauri` (only inside the webview) and, there only:

- remove `backdrop-filter` on cards/header/search,
- `background-attachment: fixed` → `scroll`,
- hover animates only the (composited) transform; the shadow swaps without a
  30px glow, and the radial hover-glow overlay is removed.

Over the uniform dark base the glass reads almost identically (cards are defined
by border + shadow). The `pnpm dev` browser build keeps the full glass.
See the `WEBKITGTK … PERFORMANCE` block in `src/index.css`.

### 2. Pin the accelerated compositor on — ✅ shipped

[tauri-apps/tauri#10566](https://github.com/tauri-apps/tauri/issues/10566)
reports the exact same GPU with the tell-tale symptom: *the UI is smooth only
while the web inspector is open*. That means WebKitGTK's accelerated compositor
is **on-demand** and tears down between layers. We force it to stay engaged by
setting `hardware-acceleration-policy: ALWAYS` on the WebKitGTK view at startup
(`src-tauri/src/lib.rs`, via `with_webview` + the version-pinned `webkit2gtk`
binding). Worst case it's a no-op; it can't break rendering. This measurably
helped.

### 3. Enable the DMABUF renderer (full GPU acceleration) — ❌ blank window

`ORRERY_WEBKIT_ACCEL=1 pnpm tauri dev` (skips disabling DMABUF):

```
Failed to create GBM buffer of size 1280x832: Invalid argument
```

→ window opens, **no UI**. NVIDIA's GBM allocator rejects the buffer format/
modifiers WebKitGTK requests. (The NVIDIA GBM backend *is* installed —
`/usr/lib64/gbm/nvidia-drm_gbm.so`, `egl-gbm` package, `15_nvidia_gbm.json` —
so this is a format/modifier incompatibility, not a missing library.)

### 4. Keep DMABUF, skip just GBM — ❌ blank window

`ORRERY_WEBKIT_ACCEL=1 WEBKIT_DMABUF_RENDERER_DISABLE_GBM=1 pnpm tauri dev`:

```
Failed to import DMABuf with file descriptor 54
Failed to import DMABuf with file descriptor 57
```

→ the failure just moved one stage downstream; NVIDIA can't import the DMABuf
file descriptors either. Still blank.

### 5. Native Wayland (+ force NVIDIA GBM backend) — ❌ crash

`GDK_BACKEND=wayland ORRERY_WEBKIT_ACCEL=1 GBM_BACKEND=nvidia-drm pnpm tauri dev`:

```
Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
```

→ immediate crash. This is exactly the Wayland protocol error that disabling
DMABUF exists to prevent.

### Conclusion

Every GPU-accelerated path is blocked: **GBM allocation fails → fd import fails
→ native Wayland crashes.** This is a hard wall in the WebKitGTK ↔ NVIDIA
driver interface, unresolved upstream
([WebKit #262607](https://bugs.webkit.org/show_bug.cgi?id=262607),
[#228268](https://bugs.webkit.org/show_bug.cgi?id=228268)). No env var routes
around it on this stack. Only Igalia/NVIDIA can lift this ceiling.

### 6. Reduce-motion toggle — ✅ shipped (user escape hatch)

Since animation is what's most visibly costly on the CPU path, **Settings →
Motion → Reduce motion** sets a `.reduce-motion` class on `<html>` (localStorage,
applied before first paint) and a universal CSS rule zeroes all animations and
transitions. Default off; additive to the OS `prefers-reduced-motion`. It can't
help raw scroll *repaint*, but it removes animation-driven frame drops.

## What ships (the optimum on NVIDIA)

All decided at **runtime** in `configure_linux_env` + `setup`, so one build is
correct everywhere:

1. **`WEBKIT_DISABLE_DMABUF_RENDERER=1`** — the only path that renders without
   crashing on NVIDIA. Skipped if `ORRERY_WEBKIT_ACCEL=1` (for Intel/AMD).
2. **`GDK_BACKEND=x11` on KDE+Wayland** — so KWin draws the native titlebar
   (unrelated to perf, but part of the same env setup).
3. **`hardware-acceleration-policy: ALWAYS`** — compositor stays engaged.
4. **`.tauri` CSS overrides** — drop backdrop-filter / fixed-bg / heavy shadows.
5. **Reduce-motion toggle** — opt-in stillness.
6. (Release builds also strip + LTO the binary — unrelated to smoothness, see
   the Cargo `[profile.release]`.)

## What this depends on

- **The WebKitGTK ↔ NVIDIA DMABUF/GBM interop.** The ceiling. If a future
  WebKitGTK or NVIDIA driver fixes GBM/DMABuf import, re-run the matrix below —
  we may be able to drop `WEBKIT_DISABLE_DMABUF_RENDERER` and get the fast path.
- **The WebKit env-var contract** (`WEBKIT_DISABLE_DMABUF_RENDERER`,
  `WEBKIT_DMABUF_RENDERER_DISABLE_GBM`, `GDK_BACKEND`). These are WebKitGTK/GTK
  conventions, not ours; they can change across major versions.
- **`webkit2gtk` crate pinned to `=2.0.2`** in `src-tauri/Cargo.toml`. This must
  match the version **wry** links, or the `with_webview(...).inner()` cast won't
  unify and the compositor-pin won't compile. When bumping tauri/wry, check
  `cargo tree -i webkit2gtk` and re-pin.
- **GPU vendor.** NVIDIA = no DMABUF (this doc). Intel/AMD = DMABUF works; those
  users get full acceleration via `ORRERY_WEBKIT_ACCEL=1` and could even make it
  their default.

## Re-test matrix (when WebKitGTK or the NVIDIA driver updates)

Run each and note: does the **UI render** (not blank), any **errors**, is it
**smooth**? If one renders cleanly and is smooth, bake that env combo into
`configure_linux_env` (vendor-detected) and make it default.

```bash
# A — full DMABUF acceleration
ORRERY_WEBKIT_ACCEL=1 pnpm tauri dev
# B — DMABUF renderer, no GBM allocation
ORRERY_WEBKIT_ACCEL=1 WEBKIT_DMABUF_RENDERER_DISABLE_GBM=1 pnpm tauri dev
# C — native Wayland + NVIDIA GBM backend
GDK_BACKEND=wayland ORRERY_WEBKIT_ACCEL=1 GBM_BACKEND=nvidia-drm pnpm tauri dev
```

As of WebKitGTK 2.52.3 + NVIDIA 595.71 (open module): A and B render blank, C
crashes. Plain `pnpm tauri dev` (all defaults) renders and is the smoothest
working config.

## References

- [tauri-apps/tauri#10566 — poor perf until web inspector opened (RTX 3080 Ti)](https://github.com/tauri-apps/tauri/issues/10566)
- [tauri-apps/tauri#9394 — documenting NVIDIA problems](https://github.com/tauri-apps/tauri/issues/9394)
- [tauri-apps/tauri#14924 — NVIDIA DMABUF + transparent windows](https://github.com/tauri-apps/tauri/issues/14924)
- [winfunc/opcode#26 — "Failed to create GBM buffer: Invalid argument"](https://github.com/winfunc/opcode/issues/26)
- [WebKit #262607 — disable DMABUF for NVIDIA proprietary drivers](https://bugs.webkit.org/show_bug.cgi?id=262607)
- [WebKit #228268 — Nvidia rendering broken, almost blank screen](https://bugs.webkit.org/show_bug.cgi?id=228268)
- [Igalia — Graphics improvements in WebKitGTK/WPE 2.46 (Skia)](https://blogs.igalia.com/carlosgc/2024/09/27/graphics-improvements-in-webkitgtk-and-wpewebkit-2-46/)
- [WebKitGTK hardware-acceleration-policy](https://webkitgtk.org/reference/webkit2gtk/2.42.0/property.Settings.hardware-acceleration-policy.html)
