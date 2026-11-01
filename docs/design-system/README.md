# Orrery — UI & Design System

> Every repo in orbit. A dense, "mission-control" dashboard that feels native on
> the Linux desktop it runs on.

This is the living spec for Orrery's UI. It describes what is actually
implemented in the native GPUI app — the design tokens in
[`crates/orrery/src/theme.rs`](https://github.com/Hankanman/Orrery/blob/main/crates/orrery/src/theme.rs), the desktop
integration in
[`crates/orrery-platform/src/appearance.rs`](https://github.com/Hankanman/Orrery/blob/main/crates/orrery-platform/src/appearance.rs),
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

1. **Native first.** The app follows the desktop's **light/dark preference**
   and borrows its **accent colour** (KDE/portal) at runtime, so it harmonises
   with the rest of the session. It shouldn't look
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

The app ships two palettes with the same token set: the **dark** "mission
control" theme (its native look) and a **light** one. Settings → Appearance →
Theme picks *System* (the default: follow the desktop's colour scheme, live),
*Dark* or *Light*. Either way the accent is overridden with the desktop's at
runtime.

```
  Token defaults                Platform reads the desktop          App applies
 (theme.rs)                     (orrery-platform::appearance)       (main.rs / live.rs)
 ────────────────────           ───────────────────────────        ──────────────────
 Theme::dark() / ::light()      XDG portal + kdeglobals (zbus):     Mode::resolve(config.theme,
 as u32 0xRRGGBB        ──▶        • color-scheme               ──▶        color-scheme)
                                   • accent-color / AccentColor     Theme::for_mode(mode)
                                  SettingChanged → live re-emit       .with_system_accent(a)
                                                                    → apply_gpui_component_theme
                                                                    rebuilt live on the appearance
                                                                    signal and on a Settings change
```

### Layer 1 — token defaults (`theme.rs`)
`Theme::dark()` and `Theme::light()` define the two palettes as `u32`
`0xRRGGBB` values, one per `--orr-*`/accent token the UI touches, plus a `mode`
field. The handful of CSS surfaces that were `color-mix`/translucent are
**pre-blended to opaque sRGB** here (the flat-design contract means nothing
layers translucency anyway). Call sites read them as `rgb(theme.fg0)` etc. and
never branch on the mode: a view that reads tokens is right in both themes.

### Layer 2 — platform reads the desktop (`orrery-platform/src/appearance.rs`)
- The **XDG Desktop Portal** `org.freedesktop.appearance` over D-Bus (`zbus`)
  provides `color-scheme` and `accent-color`; KDE's `~/.config/kdeglobals`
  `AccentColor` is preferred when present (it's what Qt apps paint, so borrowing
  it matches KDE).
- A background task subscribes to `SettingChanged` and re-emits, so scheme and
  accent flips apply **live**. Everything degrades gracefully: no bus/portal →
  dark with the built-in orbit-cyan accent.

### Layer 3 — the app applies it (`main.rs` at startup, `OrreryApp::apply_theme` live)
`Mode::resolve` turns the Settings choice plus the portal's `color-scheme` into
a palette (*System* with no desktop preference stays dark).
`Theme::with_system_accent(Some((r,g,b)))` then overrides `primary` and
recomputes the derived accent tokens and the heatmap ramp:

| Derived token | Dark | Light |
|---|---|---|
| `primary` | the system accent as is | the accent deepened (toward black, same hue) until it holds **4.5:1** on the page |
| `accent_bright` | accent + 22% white | accent + 22% black (the "emphasised" accent is darker on a light page) |
| `accent_wash` | accent 12% over page | accent 10% over page |
| `accent_badge` | accent 20% over page | accent 18% over page |
| `border_accent` | accent 40% over page | accent 40% over page |

Desktop accents are usually tuned for dark panels (KDE's default `#3daee9` is
2.6:1 on white), which is why light mode adapts rather than copies it.

`OrreryApp::apply_theme` rebuilds the `Theme` on every appearance signal and
whenever the Settings theme changes, then calls
`theme::apply_gpui_component_theme(&theme, cx)`, which loads
[gpui-component](https://github.com/longbridge/gpui-component)'s own palette
for the mode (so anything we don't map matches), maps our tokens onto its
shadcn-style `Theme` (its `Hsla` fields) so its widgets (inputs, switch,
markdown, popovers, toasts) match the rest of the UI, and picks the matching
code-highlight theme. The windows are then refreshed.

> **The seam to respect:** `primary` and the derived accent tokens are
> runtime-owned. Never hard-code a brand cyan — read `theme.primary` /
> `theme.accent_*` so the system accent flows through. The orbit-cyan `#38dbf0`
> (deepened to `#087a8c` in light mode) is only the *default* for when the OS
> exposes no accent.
>
> **What the app doesn't control.** The window frame is drawn by the
> compositor (KWin server-side decorations), so it follows the desktop's colour
> scheme, not the app's theme. The tray glyph sits on the desktop's panel, so it
> follows the desktop's `color-scheme` too (a light glyph on dark panels, a dark
> one on light panels), whatever theme the app shows.
>
> Not implemented yet: borrowing KDE's window/view *surface* colours.

---

## 3. Color

The token set lives on the `Theme` struct. Both palettes fill every token;
`primary` + the derived accent tokens are replaced by the system accent at
runtime (see the table above).

### 3.1 Surfaces & neutrals

| Token | Dark | Light | Role |
|---|---|---|---|
| `page` | `#0a0e16` | `#f4f7fa` | window background |
| `surface` | `#13161e` | `#ffffff` | card / panel background |
| `surface_hover` | `#191c24` | `#eff3f7` | hovered surface |
| `button_bg` | `#1f222a` | `#ebf0f5` | button / input fill |
| `border` | `#1c2028` | `#d8dfe7` | hairline border |
| `border_strong` | `#2c3037` | `#bac5d1` | emphasised border |
| `border_accent` | `#1b4b56` | `#96c5ce` | accent-tinted edge (accent @40% over page) |
| `scrim` | black @40% | slate `#0e1726` @30% | modal backdrop |

**The light palette is designed, not inverted.** The page is a near-white with a
slight cool (blue-grey) bias, so the orbit-cyan identity still reads; cards are
pure white lifted off it by hairline borders that stay visible on white (still
no shadows: flat design). Where dark mode lifts on hover, light mode sinks a
step: hover and fills are a touch darker than the card.

### 3.2 Foreground tiers (`fg0…3`)
A four-step text ramp. In light mode it's a cool slate ink, and every tier,
including the faint `fg3` that labels and notes use, passes WCAG AA for body
text (4.5:1) on every surface.

| Token | Dark | Light | Light on page | Use |
|---|---|---|---|---|
| `fg0` | `#eef2f8` | `#0e1726` | 16.7:1 | primary text, repo names |
| `fg1` | `#afb9cb` | `#334155` | 9.6:1 | secondary text, descriptions |
| `fg2` | `#6c778c` | `#4b596c` | 6.6:1 | captions, metadata |
| `fg3` | `#434e63` | `#5d6a7e` | 5.1:1 | labels, notes, faint icons |

### 3.3 Identity & accent (runtime-overridden)

| Token | Dark default | Light default | Notes |
|---|---|---|---|
| `primary` | `#38dbf0` orbit cyan | `#087a8c` deep orbit teal (4.7:1) | replaced by the system accent |
| `accent_bright` | `#64e3f3` | `#065f6d` | emphasised accent: links, active labels |
| `accent_wash` | `#102730` | `#dceaef` | active nav bg |
| `accent_badge` | `#133742` | `#cae0e6` | nav count badge |
| `danger_badge` | `#3b2127` | `#eddfe3` | urgent badge / danger hover |

### 3.4 Semantic hues — **fixed, meaning-bearing**
These do **not** follow the OS; their meaning is the point. Light mode keeps
each hue family but deepens it from the dark theme's neon until it passes as
text on the page (they all colour small labels).

| Token | Meaning | Dark | Light | Light on page |
|---|---|---|---|---|
| `clean` | clean / synced | `#3dd68c` | `#157a47` | 5.0:1 |
| `dirty` | uncommitted / ahead | `#ff9e45` | `#b0520a` | 4.8:1 |
| `behind` | behind / conflict | `#ff6b6b` | `#b92f38` | 5.5:1 |
| `star` | host stars / favourites | `#ffc24b` | `#9a6408` | 4.7:1 |
| `ai` | AI features | `#a78bfa` | `#7444d6` | 5.5:1 |

The contrast unit test in `theme.rs` (`light_palette_meets_wcag_aa`) computes
these ratios for every text tier, the accent and the semantic hues against the
page, cards, hover and button fills, plus labels on the accent and danger
washes; run it with `--nocapture` to print them.

### 3.5 Launch-action colours (`act_*`)
Each repo card action has its own identity colour (icon + hover tint).

| Token | Action | Dark | Light |
|---|---|---|---|
| `act_ide` | Open in IDE | `#5b9dff` | `#2560d8` |
| `act_agent` | Terminal agent | `#a78bfa` | `#7444d6` |
| `act_folder` | Reveal folder | `#f5b94b` | `#9a6408` |
| `act_host` | Open on GitHub/GitLab | `#46c8a0` | `#0f766e` |

### 3.6 Heatmap ramp (`heat`)
The contribution graph's five steps: an empty cell (white @5% over the dark
page; slate ink @7% over the light page, so the no-commit grid still shows on
near-white), then the accent at 22% / 42% / 68% over the page, then the full
accent. It's rebuilt with the accent, so it follows the system accent and the
theme switch.

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
| OS colour scheme + accent read (portal + kdeglobals) | `crates/orrery-platform/src/appearance.rs` |
| Live re-theming on scheme/accent change | `crates/orrery/src/live.rs` → `OrreryApp::apply_theme` (`shell/mod.rs`) |
| Theme choice (System / Dark / Light) | `config.theme`, Settings → Appearance (`views/settings.rs`) |
| Icons (lucide + brand + devicon) | `crates/orrery/assets/icons/`, `src/icon.rs` |
| Brand mark | `docs/design-system/assets/orbit-mark.svg` |
