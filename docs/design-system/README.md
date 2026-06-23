# Orrery — UI & Design System

> Every repo in orbit. A dense, "mission-control" dashboard that feels native on
> the Linux desktop it runs on.

This is the living spec for Orrery's UI. It describes what is actually
implemented in the native GPUI app — the design tokens in
[`crates/orrery/src/theme.rs`](../../crates/orrery/src/theme.rs), the desktop
integration in
[`crates/orrery-platform/src/appearance.rs`](../../crates/orrery-platform/src/appearance.rs),
and the GPUI views — not an aspirational mock. When you change the design, change
it here too.

> Orrery was originally a Tauri 2 + React/TypeScript app whose design system
> lived in a CSS file (`src/index.css`) consumed by shadcn/React components. It's
> now a **native Rust app on GPUI** — the tokens carry over as Rust values and
> are applied to GPUI elements directly (there are no CSS classes). If you find
> references to `.orr-*` CSS classes, `src/index.css`, or `src/lib/appearance.ts`
> anywhere, they're stale.

The brand mark lives at [`assets/orbit-mark.svg`](assets/orbit-mark.svg).

---

## 1. Philosophy

Orrery looks like a piece of instrumentation: deep flat surfaces, hairline
borders, tight radii, a single bright accent, and meaning-bearing colour for
git/host/AI state.

Four principles, in priority order:

1. **Native first.** The app borrows the desktop's **accent colour** (KDE/portal)
   at runtime, so it harmonises with the rest of the session. It shouldn't look
   out of place next to Dolphin or Discover.
2. **Flat by design.** No blur, no gradient washes, no glows. Surfaces are solid
   and separated by borders + a subtle elevation shadow. The native app
   GPU-composites (blade/Vulkan), so this is no longer the hard CPU constraint it
   was under WebKitGTK — but it stays the look, and keeping overdraw low keeps the
   GPU path cheap. **Don't reintroduce glassmorphism.**
3. **Structure is branded, accent adapts.** The layout, elevation, radii, motion
   and the *semantic* hues (git status, host star, AI) are Orrery's identity and
   stay constant. Only the primary accent follows the system.
4. **Dense but legible.** 4–5 cards per row, a fixed-width type scale, mono for
   data. Information per pixel is high; nothing is cramped.

---

## 2. Theming architecture (OS integration)

The app ships a single **dark** "mission control" theme and overrides its accent
with the desktop's at runtime.

```
  Token defaults                Platform reads the desktop          App applies
 (theme.rs Theme::dark)         (orrery-platform::appearance)       (live.rs / main.rs)
 ────────────────────           ───────────────────────────        ──────────────────
 the dark --orr-* palette       XDG portal + kdeglobals (zbus):     Theme::dark()
 as u32 0xRRGGBB        ──▶        • accent-color / AccentColor ──▶   .with_system_accent(a)
                                  SettingChanged → live re-emit      → recompute primary +
                                                                       derived accent tokens;
                                                                     rebuilt live on the
                                                                     appearance signal
```

### Layer 1 — token defaults (`theme.rs`)
`Theme::dark()` defines the deep blue-black palette as `u32` `0xRRGGBB` values,
one per `--orr-*`/accent token the UI touches. The handful of CSS surfaces that
were `color-mix`/translucent are **pre-blended to opaque sRGB** here (the
flat-design contract means nothing layers translucency anyway). Call sites read
them as `rgb(theme.fg0)` etc.

### Layer 2 — platform reads the desktop (`orrery-platform/src/appearance.rs`)
- The **XDG Desktop Portal** `org.freedesktop.appearance` over D-Bus (`zbus`)
  provides `accent-color`; KDE's `~/.config/kdeglobals` `AccentColor` is preferred
  when present (it's what Qt apps paint, so borrowing it matches KDE).
- A background task subscribes to `SettingChanged` and re-emits, so accent flips
  apply **live**. Everything degrades gracefully — no bus/portal → the built-in
  orbit-cyan accent stays.

### Layer 3 — the app applies it (`main.rs` at startup, `live.rs` live)
`Theme::with_system_accent(Some((r,g,b)))` overrides `primary` and recomputes the
derived accent tokens (`accent_bright` = +22% white, `accent_wash` = 12% over
page, `accent_badge` = 20%, `border_accent` = 40%). At startup `main.rs` reads the
accent once; `live.rs` then rebuilds the `Theme` on every appearance signal so a
system accent change repaints immediately.

`theme::apply_gpui_component_theme(&theme, cx)` maps these tokens onto
[gpui-component](https://github.com/longbridge/gpui-component)'s shadcn-style
`Theme` (its `Hsla` fields) so its widgets (inputs, switch, markdown, popovers)
match the rest of the UI.

> **The seam to respect:** `primary` and the derived accent tokens are
> runtime-owned. Never hard-code a brand cyan — read `theme.primary` /
> `theme.accent_*` so the system accent flows through. The orbit-cyan `#38dbf0`
> is only the *default* for when the OS exposes no accent.
>
> Not (yet) ported from the webview version: light mode, and borrowing KDE's
> window/view *surface* colours. The native theme is dark-only with accent-only
> adaptation.

---

## 3. Color

The token set lives on the `Theme` struct. Values below are the dark defaults
(`Theme::dark()`); `primary` + the four derived accent tokens are replaced by the
system accent at runtime.

### 3.1 Surfaces & neutrals

| Token | Value | Role |
|---|---|---|
| `page` | `#0a0e16` | the void (window background) |
| `surface` | `#13161e` | card / panel background |
| `surface_hover` | `#191c24` | hovered surface |
| `button_bg` | `#1f222a` | button / input fill |
| `border` | `#1c2028` | hairline border |
| `border_strong` | `#2c3037` | emphasised border |
| `border_accent` | `#1b4b56` | accent-tinted edge (accent @40% over page) |

### 3.2 Foreground tiers (`fg0…3`)
A four-step text ramp.

| Token | Value | Use |
|---|---|---|
| `fg0` | `#eef2f8` | primary text, repo names |
| `fg1` | `#afb9cb` | secondary text, descriptions |
| `fg2` | `#6c778c` | captions, metadata |
| `fg3` | `#434e63` | disabled, faint icons |

### 3.3 Identity & accent (runtime-overridden)

| Token | Default | Notes |
|---|---|---|
| `primary` | `#38dbf0` orbit cyan | replaced by the system accent |
| `accent_bright` | `#64e3f3` | accent + 22% white |
| `accent_wash` | `#102730` | accent 12% over page — active nav bg |
| `accent_badge` | `#133742` | accent 20% over page — nav count badge |

### 3.4 Semantic hues — **fixed, meaning-bearing**
These do **not** follow the OS; their meaning is the point.

| Token | Meaning | Value |
|---|---|---|
| `clean` | clean / synced | `#3dd68c` |
| `dirty` | uncommitted / ahead | `#ff9e45` |
| `behind` | behind / conflict | `#ff6b6b` |
| `star` | host stars / favourites | `#ffc24b` |
| `ai` | AI features | `#a78bfa` |

### 3.5 Launch-action colours (`act_*`)
Each repo card action has its own identity colour (icon + hover tint).

| Token | Action | Value |
|---|---|---|
| `act_ide` | Open in IDE | `#5b9dff` |
| `act_agent` | Terminal agent | `#a78bfa` |
| `act_folder` | Reveal folder | `#f5b94b` |
| `act_host` | Open on GitHub/GitLab | `#46c8a0` |

### 3.6 Language colours
Devicon brand SVGs (multicolour) render the language mark per card via
`theme::devicon_stem(language)` + `assets/icons/devicon/`; `theme::lang_color`
provides a fallback dot colour.

---

## 4. Type

The native app uses the platform fonts GPUI resolves — the system **sans-serif**
for UI/prose and **monospace** for data (`font_family("monospace")`). It does not
bundle a typeface. (The old webview build shipped Geist via `@fontsource`; that
didn't carry over — re-add an embedded face here if a consistent brand font
matters.)

Sizes the UI uses (logical px, on the `Theme`): `text_h3` 16 (section headers),
`text_small` 13 (body/secondary), `text_data_sm` 12 (dense mono data). Larger
one-off sizes (e.g. the card repo name) are set inline.

**Rule:** anything that is data (commit hashes, counts, branch names, paths, repo
slugs) is **mono**. Prose is sans.

---

## 5. Shape & motion

**Radii** (px, on the `Theme`): `r_xs` 4 · `r_sm` 6 · `r_md` 10 · `r_lg` 14 —
tight, hardware-console feel. gpui-component widgets pick up `r_md` via the bridge.

**Motion** — quick and "instrument"-like, done with GPUI element animations
(transform + opacity; no layout shift). Backgrounds stay flat: no radial washes,
no starfield, no glow.

---

## 6. Components

There are **no CSS classes** — every surface is a GPUI element styled inline from
the `Theme` tokens. The catalog by source file:

- **Shell** (`shell.rs`) — the 52px header (brand, roots·repos, search, +/rescan),
  the left nav rail, the main column + view switch.
- **Repo card** (`card.rs`) — name/host/slug/description, git status row,
  language mark, the four launch actions, favourite + AI accents.
- **Repo drawer** (`drawer.rs`) — the right sheet: Overview / Changes / PR / Notes
  / Readme tabs.
- **Command palette** (`palette.rs`) — Ctrl+K overlay: actions + repos + code +
  semantic results.
- **Views** (`views/`) — inbox, feed, explore, cleanup, agents, devtools,
  settings, newproject.

Widgets come from **gpui-component**: text inputs (`Input`/`InputState`), the
notifications/Settings `Switch`, the scan-depth `NumberInput`, markdown rendering
(`text::markdown`), and popover/modal layers (via `Root`). They inherit the
adaptive tokens through `apply_gpui_component_theme`.

---

## 7. Rules of thumb

- **Read tokens, don't hard-code colour.** Use `theme.primary` / `theme.accent_*`
  for the accent so the OS accent flows through; use the semantic hues for state.
- **Flat, always.** No blur, no gradient backgrounds, no glow shadows. Define
  surfaces with borders + a subtle elevation shadow — see Philosophy #2.
- **Mono for data, sans for prose.**
- **Density over decoration.** Prefer a tighter radius and a hairline border to a
  heavy fill; lift with a transform on hover, not bright backgrounds.
- **Semantics are sacred.** Green = clean, amber = dirty/ahead, red = behind,
  gold = stars, violet = AI. Don't repurpose them.

---

## 8. Where things live

| Concern | File |
|---|---|
| All design tokens (`Theme`) | `crates/orrery/src/theme.rs` |
| gpui-component theme bridge | `crates/orrery/src/theme.rs` (`apply_gpui_component_theme`) |
| OS accent read (portal + kdeglobals) | `crates/orrery-platform/src/appearance.rs` |
| Live re-theming on accent change | `crates/orrery/src/live.rs` |
| Icons (lucide + brand + devicon) | `crates/orrery/assets/icons/`, `src/icon.rs` |
| Brand mark | `docs/design-system/assets/orbit-mark.svg` |
