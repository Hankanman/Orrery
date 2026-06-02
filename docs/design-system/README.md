# Orrery — UI & Design System

> Every repo in orbit. A dense, "mission-control" dashboard that feels native on
> the Linux desktop it runs on.

This is the living spec for Orrery's UI. It describes what is actually
implemented in [`src/index.css`](../../src/index.css),
[`src/lib/appearance.ts`](../../src/lib/appearance.ts) and the React components —
not an aspirational mock. When you change the design, change it here too.

The brand mark lives at [`assets/orbit-mark.svg`](assets/orbit-mark.svg).

---

## 1. Philosophy

Orrery looks like a piece of instrumentation: deep flat surfaces, hairline
borders, tight radii, a single bright accent, and meaning-bearing colour for
git/host/AI state. It is **adaptive, not dark-only** — the app mirrors the
desktop it runs on.

Four principles, in priority order:

1. **Native first.** Light or dark, accent colour, and (on KDE) the actual window
   surface colours come from the OS. The app should never look out of place next
   to Dolphin or Discover.
2. **Flat by design — performance is a feature.** No `backdrop-filter` blur, no
   gradient washes, no glows. Surfaces are solid (or a flat translucent fill) and
   separated by borders + a subtle elevation shadow. This isn't only an
   aesthetic: the app ships in **WebKitGTK**, which composites blur/gradients on
   the **CPU** (badly, especially on NVIDIA — see
   [docs/rendering-performance.md](../rendering-performance.md)). A flat UI is
   *much* smoother in the webview, and looks identical in a browser. **Do not
   reintroduce glassmorphism.**
3. **Structure is branded, colour adapts.** The layout, elevation, radii, motion
   and the *semantic* hues (git status, host star, AI) are Orrery's identity and
   stay constant. The neutral palette and the primary accent follow the system.
4. **Dense but legible.** 4–5 cards per row, a fixed-width type scale, mono for
   data. Information per pixel is high; nothing is cramped.

---

## 2. Theming architecture (OS integration)

This is the part that makes Orrery feel native. It is a three-layer cascade.

```
  CSS defaults                Rust reads the desktop              React applies
 (src/index.css)              (src-tauri/appearance.rs)        (lib/appearance.ts)
 ──────────────               ─────────────────────────        ───────────────────
 :root  → light               XDG portal:                      applyAppearance():
 .dark  → dark      ──▶         org.freedesktop.appearance ──▶   • toggle .dark
 (sensible brand               • color-scheme                    • root.style.colorScheme
  defaults if the              • accent-color                    • set --primary etc.
  OS exposes nothing)        kdeglobals (KDE/Qt):                  from accent (+ contrast fg)
                               • AccentColor                      • set surfaces from
                               • Colors:Window/View                 window/view colours
                               SettingChanged → live re-emit     listens "appearance-changed"
```

### Layer 1 — CSS defaults
`:root` defines a clean light palette; `.dark` defines the deep blue-black
"mission control" palette. These render correctly with **zero** OS data (e.g. in
the browser dev preview, or on a desktop that exposes no appearance settings).

### Layer 2 — Rust reads the desktop (`src-tauri/src/appearance.rs`)
- **XDG Desktop Portal** `org.freedesktop.appearance` over D-Bus (`zbus`) gives
  `color-scheme` (dark/light/no-preference) and `accent-color` (3 doubles 0–1).
- **`~/.config/kdeglobals`** is parsed when present: `AccentColor`,
  `Colors:Window/BackgroundNormal+ForegroundNormal`, `Colors:View/BackgroundNormal`.
  This is what Qt apps actually paint, so borrowing it makes us match KDE exactly.
  KDE's `AccentColor` is preferred over the portal's (often tinted) one.
- A background task subscribes to `SettingChanged` and re-emits the
  `appearance-changed` Tauri event, so theme/accent flips apply **live**.
- Everything degrades gracefully: no bus, no portal, or no kdeglobals → we fall
  back to the CSS defaults.

### Layer 3 — React applies it (`src/lib/appearance.ts`, `useSystemAppearance` hook)
`applyAppearance(appearance)` is idempotent and does three things:

1. **Color scheme** — toggles `.dark` and sets `root.style.colorScheme` (so
   native form controls and scrollbars follow too). Falls back to
   `prefers-color-scheme` when the OS says "no preference".
2. **Accent** — writes the system accent into `--primary`, `--ring`,
   `--sidebar-primary`, `--sidebar-ring`. The accent foreground is chosen by WCAG
   relative luminance (dark text on a light accent, light text on a dark accent),
   so the accent is always legible.
3. **Surfaces (hybrid, KDE)** — when window/view colours are available, anchors
   `--background`/`--card`/`--sidebar`/etc. on the desktop's window colour and
   derives the elevation ramp by mixing **toward the foreground** (a small mix
   lightens in dark, darkens in light — correct in both). When unavailable, these
   vars are cleared and the branded `.dark`/`:root` palette takes over.

> **The seam to respect:** `--primary` (and the surface vars) are runtime-owned.
> Never hard-code a brand cyan into a component — read `--primary` / `--orr-accent`
> so the system accent flows through. The orbit-cyan `#38dbf0` in `.dark` is only
> the *default* for when the OS exposes no accent.

---

## 3. Color

Two token families coexist:

- **shadcn/Tailwind tokens** (`--background`, `--primary`, `--ok` …) — the
  adaptive neutrals + accent. Light in `:root`, dark in `.dark`, overridden at
  runtime by the OS. Exposed to Tailwind via `@theme inline`.
- **`--orr-*` tokens** — Orrery's own structure (glass, elevation, fixed semantic
  hues). Namespaced so they never collide with shadcn.

### 3.1 Surfaces & neutrals (adaptive)

| Role | Light (`:root`) | Dark (`.dark`) default |
|---|---|---|
| `--background` | near-white slate | `#0a0e16` (the void) |
| `--card` / `--popover` | white | `#121826` (raised) |
| `--foreground` | near-black slate | `#eef2f8` |
| `--primary` | steel blue (OS accent) | `#38dbf0` orbit cyan (OS accent) |
| `--border` / `--input` | light slate | mid slate |

> These are the *defaults*. On a themed desktop they are replaced by the OS
> accent and (on KDE) borrowed window/view colours.

### 3.2 Foreground tiers (`--orr-fg-0…3`)
A four-step text ramp, used for primary text → captions → disabled.

| Token | Dark | Use |
|---|---|---|
| `--orr-fg-0` | `#eef2f8` | primary text, repo names |
| `--orr-fg-1` | `#afb9cb` | secondary text, descriptions |
| `--orr-fg-2` | `#6c778c` | captions, metadata |
| `--orr-fg-3` | `#434e63` | disabled, faint icons |

### 3.3 Surfaces & elevation (`--orr-*`)
Flat panels separated by hairline borders and a subtle elevation shadow — **no
blur**. The `--orr-glass*` fills are now just flat (slightly translucent) tints
over a solid background, not backdrop-blurred glass.

- `--orr-glass` / `--orr-glass-hover` / `--orr-glass-2` — panel fills (dark: low
  white alphas `0.035 / 0.06 / 0.085`).
- `--orr-border` / `--orr-border-strong` — hairlines (white alphas `0.075 / 0.14`).
- `--orr-shadow-2/3/-pop` — elevation ramp (card → drawer → popover). This is the
  *only* form of depth we use.
- `--orr-inset-top` — 1px top highlight that gives panels a "lit edge".
- `--orr-scrim` — modal/drawer backdrop (a flat dark wash, no blur).

> Header and sidebar use **solid** `--background`. `--orr-blur` and the `*-glow`
> tokens still exist but are **unused** — kept only so the values are documented;
> don't wire them back into `backdrop-filter`, gradients, or shadows.

### 3.4 Semantic hues — **fixed, meaning-bearing**
These do **not** follow the OS; their meaning is the point. Values are tuned per
mode (richer in light, neon in dark).

| Token | Meaning | Dark | Light |
|---|---|---|---|
| `--orr-clean` / `--ok` | clean / synced | `#3dd68c` | `#22a96b` |
| `--orr-dirty` / `--warn` | uncommitted / ahead | `#ff9e45` | `#e08a2b` |
| `--orr-behind` / `--danger` | behind / conflict | `#ff6b6b` | `#e05656` |
| `--orr-star` | host stars / favourites | `#ffc24b` | `#f5a623` |
| `--orr-ai` | AI features | `#a78bfa` | `#8b5cf6` |

The matching `*-glow` tokens are retained but no longer painted as halos (flat
design). Use the solid hue for icons/text; the only remaining use of a `*-glow`
value is a flat low-alpha hover *tint* on a couple of buttons.

### 3.5 Launch-action colours (`--orr-act-*`)
Each repo action has its own identity colour (icon + hover tint), on a shared
flat/muted button base (`.orr-cbtn`). The colour is set per-variant via an
`--act` custom property.

| Token | Action | Dark | Light |
|---|---|---|---|
| `--orr-act-ide` | Open in IDE | `#5b9dff` | `#2f6fdb` |
| `--orr-act-agent` | Terminal agent | `#a78bfa` | `#7c4ddb` |
| `--orr-act-folder` | Reveal folder | `#f5b94b` | `#c9871d` |
| `--orr-act-host` | Open on GitHub/GitLab | `#46c8a0` | `#1f9e76` |

### 3.6 Language dots (`--lang-*`)
GitHub-linguist colours for ~17 common languages, plus `--lang-default`. Resolved
in [`src/lib/format.ts`](../../src/lib/format.ts) via `languageColor()`.

---

## 4. Type

Typeface: **Geist Sans** (UI) and **Geist Mono** (data), bundled offline via
`@fontsource/geist-sans` / `@fontsource/geist-mono` (imported in
[`src/main.tsx`](../../src/main.tsx); weights 400/500/600 sans, 400/500 mono).
`--font-sans` / `--font-mono` lead with Geist then fall back to `system-ui` so the
app still reads natively if fonts fail to load.

Scale (CSS shorthand tokens, `weight size/line-height family`):

| Token | Value | Use |
|---|---|---|
| `--text-h3` | `500 16px/1.35` sans | section headers |
| `--text-body` | `400 14px/1.55` sans | body, descriptions |
| `--text-small` | `400 13px/1.45` sans | secondary |
| `--text-micro` | `500 11px/1.3` sans, `tracking 0.085em` | labels, tags (UPPERCASE) |
| `--text-data` | `500 13px/1.3` mono | counts, hashes, paths |
| `--text-data-sm` | `500 12px/1.3` mono | dense data |

**Rule:** anything that is data (commit hashes, counts, branch names, paths, repo
slugs) is **mono**. Prose is sans.

---

## 5. Shape & motion

**Radii** — tight, hardware-console feel:
`--r-xs 4` · `--r-sm 6` · `--r-md 10` · `--r-lg 14` · `--r-xl 20` (px).
(shadcn's `--radius` ramp maps onto the same feel for shadcn components.)

**Motion** — quick and "instrument"-like:
- `--dur-fast 120ms`, `--dur-base 200ms`
- `--ease-out cubic-bezier(.16,1,.3,1)` for entrances/hovers
- `--ease-spring cubic-bezier(.34,1.56,.64,1)` for lifts/pops

Cards lift slightly on hover (transform only) and brighten their border + fill —
no glow. Backgrounds are flat: no radial washes, no starfield. Entrance
animations (the launch "build", card stagger, drawer slide) use transform +
opacity only, so there's no layout shift and nothing for the CPU compositor to
choke on. All motion respects `prefers-reduced-motion` and the in-app
**Settings → Motion → Reduce motion** toggle.

---

## 6. Components

Implemented as `.orr-*` classes in the `@layer components` block of
`src/index.css`. Catalog (class → what it is):

**Layout**
- `.orr-body` — persistent sidebar + main split (lives in `AppShell`)
- `.orr-sidebar` — persistent rail: fixed primary nav (`.orr-sb-sec` of
  `.orr-sb-item`) on top, a per-screen `.orr-sb-slot` below (filled via
  `useSidebarSlot`, see `src/lib/sidebar-slot.tsx`), `.orr-sb-foot` at the bottom
- `.orr-main`, `.orr-header`, `.orr-toolbar`, `.orr-brand`, `.orr-mark` — top chrome
- `.orr-grid` — responsive repo grid

**Repo card** (`src/components/RepoCard.tsx`)
- `.orr-card` (+ `.orr-card-head/-host/-name/-slug/-desc/-status/-badge/-acts/-fav/-ai`)

**Controls**
- `.orr-cbtn` — primary action button (`.ide` = filled accent variant)
- `.orr-iconbtn` — square icon button
- `.orr-chip` / `.orr-chiprow` — filter chips
- `.orr-seg` / `.orr-sortpill` — segmented toggle / sort pill
- `.orr-search` — command/search field
- `.orr-tag` — micro UPPERCASE label
- `.orr-st` — status dot/pill

**Surfaces & views**
- `.orr-inbox` (+ `-head/-list/-row`) — Inbox/Feed
- `.orr-settings` (+ `-head/-body`), `.orr-roots`
- `.orr-star-grid` / `.orr-star-card` — starred browser
- `.orr-briefing` — AI daily briefing banner
- `.orr-md` — rendered markdown (READMEs, AI output)
- `.orr-empty`, `.orr-skel` / `.orr-skel-line` — empty & loading states
- `.orr-spinner` — brand loading spinner (flat orbit mark)
- `.orr-progress` / `.orr-activity` — scan/activity indicator (header)
- `.orr-card-ai`, `.orr-mark` — AI/brand accents

shadcn/ui primitives (new-york) are used for dialogs, drawers, command palette
(`cmdk`), inputs, etc.; they inherit the adaptive tokens automatically.

---

## 7. Rules of thumb

- **Read tokens, don't hard-code colour.** Use `--primary` / `--orr-accent` for the
  accent so the OS accent flows through. Use the semantic hues for state.
- **Both themes, always.** Every new surface must look right in light *and* dark —
  test by toggling the OS theme (the app follows live).
- **Mono for data, sans for prose.**
- **Flat, always.** No `backdrop-filter`, no `radial`/`linear-gradient`
  backgrounds, no glow `box-shadow`/`drop-shadow`. Define surfaces with borders +
  a subtle elevation shadow. This is both the look and a hard performance
  constraint (CPU-bound WebKitGTK) — see Philosophy #2.
- **Density over decoration.** Prefer a tighter radius and a hairline border to a
  heavy fill. Lift with a transform on hover, not with bright backgrounds.
- **Semantics are sacred.** Green = clean, amber = dirty/ahead, red = behind,
  gold = stars, violet = AI. Don't repurpose them.

---

## 8. Where things live

| Concern | File |
|---|---|
| All tokens + `.orr-*` components | `src/index.css` |
| OS theme/accent/surface application | `src/lib/appearance.ts` + `src/hooks/useSystemAppearance.ts` |
| Desktop reads (portal + kdeglobals) | `src-tauri/src/appearance.rs` |
| Language colour resolution | `src/lib/format.ts` |
| Brand mark | `docs/design-system/assets/orbit-mark.svg` |
