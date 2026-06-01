# 🪐 Orrery — Design System

> **every repo in orbit**
> A Linux-native command center that puts every git repo in your dev directories into orbit — live git status at a glance, one-click launch into your IDE or a terminal coding agent, enriched with multi-host data and on-device AI summaries.

This folder is the **design system** for Orrery: the foundations (color, type, space, motion), the brand voice, an icon strategy, and a high-fidelity **UI kit** recreating the app's core screens. Use it to build branded interfaces, prototypes, marketing pages, or production UI that feel unmistakably *Orrery*.

---

## ⚠️ Important context for whoever reads this

Orrery is **early-stage**. At the time this system was built, the source repo contained **only a design spec** ([`DESIGN.md`](https://github.com/Hankanman/Orrery/blob/main/DESIGN.md)) and a README — **no implementation code, fonts, icons, or visual assets existed yet.** The design phase was "complete" but Phase 1 (the local-first grid) was still being built.

So this design system is a **faithful realization of the locked spec**, not a copy of shipped pixels. Every decision traces back to three anchors:

1. **The spec's locked aesthetic** — *"Dark, dense, mission control · 4–5 cards/row, data-rich, neon accents."*
2. **The chosen stack** — Vite + React + TypeScript + **Tailwind + [shadcn/ui](https://ui.shadcn.com)** + **Lucide** icons + **cmdk** command palette. Where the spec was silent, we followed shadcn/ui conventions (the actual component library Orrery uses).
3. **The brand metaphor + direction** — an *orrery* is a clockwork model of a solar system; you survey orbiting bodies at a glance. Plus the directive: *sleek, modern, dev-focused, with transparency, blur, and motion.*

**If you have access to the live repo, go further than this system.** Read the current state of these sources and reconcile against real components as they land:

- **GitHub:** https://github.com/Hankanman/Orrery
- **Spec:** https://github.com/Hankanman/Orrery/blob/main/DESIGN.md
- **Component library Orrery builds on:** https://ui.shadcn.com · **Icons:** https://lucide.dev · **Palette:** https://tauri.app

---

## The product

Point Orrery at the directories where you keep projects. It discovers every git repo inside them and lays them out in a **dark, dense "mission control" grid**. Each repo is a **card** that fuses three sources of truth — and these three sources are the backbone of the whole color system:

| Source | What it adds | Brand hue |
|---|---|---|
| **Local git** (`git2`/libgit2) | branch, ahead/behind, uncommitted changes, last commit, detected language | **Orbit cyan** `#38DBF0` |
| **Your host** (GitHub/GitLab, incl. self-hosted) | stars, topics, releases, issues | **Star amber** `#FFC24B` |
| **Local AI** (embedded llama.cpp) | on-device "what is this / what's been happening" blurb | **AI violet** `#A78BFA` |

Every card is also a **launchpad**: one click opens the repo in your IDE, or drops a terminal coding agent (Claude Code, etc.) straight into it.

**Surfaces / products in this system:**
- **Orrery Desktop** — the Tauri app. The grid, command palette (cmdk), repo detail, and settings. This is the primary surface; the UI kit lives in [`ui_kits/orrery-desktop/`](./ui_kits/orrery-desktop/).

There is no marketing site, mobile app, or docs site in scope yet — Orrery is desktop-only and pre-launch.

---

## Content fundamentals — voice & tone

Orrery's voice comes straight from its own copy (README + spec) and the dev-tool register of its stack. It is **terse, confident, and a little playful about the space metaphor — but never cutesy in the UI itself.**

**Principles**
- **Lowercase tagline, precise everywhere else.** The tagline is always lowercase: *every repo in orbit*. Headings and UI labels use sentence case or terse Title Case, not ALL CAPS (except micro-labels — see Visual Foundations).
- **Command-line economy.** Copy reads like a good CLI: short, declarative, no filler. *"Scan → git metadata → grid."* *"Zero external deps, usable daily."* Em-dashes and arrows (`→`) carry rhythm.
- **The metaphor is structural, not decorative.** "Orbit", "in orbit", "mission control", "launchpad", "survey at a glance", "stars". Use it for naming and framing — **don't** litter the UI with planet emoji or space puns on every button.
- **Second person, imperative for actions.** *"Point Orrery at your directories."* *"Open in IDE."* *"Drop an agent."* The user is the operator; the app is the console.
- **Honest about state.** Dev-tool candor: *"🚧 Early development."* *"Nothing to build yet."* *"TBD."* Status is stated plainly, including roadmap phases (Phase 1–4).
- **Technical literacy assumed.** Audience is developers. Say `ahead/behind`, `worktree`, `device-flow OAuth`, `GGUF`, `XDG dirs` without hand-holding. Mono type does a lot of the talking.

**Casing & mechanics**
- **I vs you:** Address the user as *you*; the product refers to itself as *Orrery* (third person), never "we" in-product.
- **Emoji:** Exactly one emoji has brand status — **🪐** (ringed planet), used as a mark in docs/README only. Avoid emoji in the UI; use Lucide icons instead. (The README does use a few sparingly: 🚧 for status, 🎨/🏠 in source material — keep this to docs, not product chrome.)
- **Numbers & data:** Always monospace in-UI (`↑2 ↓0`, `3 changes`, `4h ago`, `★ 1.2k`). Relative time over absolute (*"4h ago"*, *"last commit 4h ago"*).
- **Punctuation:** Middle dot `·` separates inline meta (`owner/repo · ~/dev/folder`). Arrow `→` for flow/sequence. Sparing, deliberate.

**Example copy (use as reference)**
- App tagline: *every repo in orbit*
- Empty state: *No repos in orbit yet. Point Orrery at a directory to begin scanning.*
- Card subtitle: `owner/repo · ~/dev/folder`
- Launcher buttons: `Open in IDE` · `◗ Agent`
- Status chips: `↑2 ↓0` · `3 changes` · `stale` · `clean`
- Command palette placeholder: *Search repos, run a command…*

---

## Visual foundations

The whole system is **dark-only**. Orrery is a night-mode mission console; there is no light theme in scope. Everything below is realized in [`colors_and_type.css`](./colors_and_type.css).

### Color
- **Backgrounds are deep blue-black "space"** — a four-step ramp from `--void #06080D` (app chrome) through `--base #0A0E16` (the grid canvas) to solid raised surfaces `--raised #121826` / `--raised-2 #182032` (menus, popovers, hovers).
- **Surfaces are glass, not flat fills.** Cards and panels use translucent white (`--glass` ≈ 3.5% white) over the space background **with `backdrop-filter: blur(16–24px)`** and a hairline border. A faint top sheen (`--inset-top`) gives them a lit edge. This is the core of the "transparency + blur" direction.
- **Three accent hues map to the three data sources** (see table above): **orbit cyan** `#38DBF0` is the primary/interactive neon (links, focus, primary buttons, active orbits); **star amber** `#FFC24B` is host stars & favorites; **AI violet** `#A78BFA` marks anything AI-generated. Each has a `*-glow` rgba for halos.
- **Status is its own semantic set:** clean/ahead green `#3DD68C`, dirty orange `#FF9E45`, behind/conflict red `#FF6B6B`, stale grey `#6C778C`.
- **Language dots use the GitHub-linguist palette** (Rust `#DEA584`, TS `#3178C6`, Go `#00ADD8`, …) so the language badge reads instantly to any dev.
- **Foreground is a 4-tier grey-blue ramp** (`--fg-0`…`--fg-3`) — never pure white on pure black; everything is slightly blue-shifted to sit in the space palette.

### Type
- **Two families, both [Geist](https://vercel.com/font):** **Geist Sans** for everything human (names, headings, body), **Geist Mono** for everything machine (slugs, paths, branch names, counts, code, relative times). The sans/mono split *is* the type system — mono signals "data from the machine."
- Geist is clean, neutral, and engineered for product UI — it carries the "sleek modern, dev-focused" brief without the over-exposure of Inter/Roboto.
- **Micro-labels are the one uppercase moment:** 11px, weight 500, `letter-spacing: 0.085em`, in `--fg-2`. Used for section eyebrows ("MISSION CONTROL", "FILTERS").
- Display/H1 carry slight negative tracking (`-0.01em`); everything else is neutral.
- Scale is **dense** (base UI text 14px, data 12–13px) to honor "data-rich, 4–5 cards/row" — but never below 11px.

### Space & layout
- **Dense grid.** The hero view is a responsive card grid, **4–5 cards per row**, ~12–16px gaps. Information density is a feature, not a bug.
- **Tight radii.** Hardware-console feel, not pill-soft: chips `4px`, buttons/inputs `6px`, cards `10px`, panels/modals `14px`. Nothing is fully rounded except dots and the occasional avatar.
- **Fixed app chrome.** A persistent top bar (search + global actions) and a left rail (roots, filters, sort). The grid scrolls; chrome stays. cmdk command palette overlays everything.

### Backgrounds & atmosphere
- No photographic imagery. The "space" is built from **the dark ramp + subtle radial accent glows** behind focal areas (a faint cyan halo behind the active region), never loud gradients. **Avoid bluish-purple gradient washes** — accent glow is localized, low-opacity, and purposeful.
- Optional **fine star-field / grid texture** at very low opacity is on-brand but must stay subliminal.

### Elevation & shadows
- Shadows are **deep and soft** on near-black (`--shadow-1…3`, `--shadow-pop`), used for menus/popovers/modals and lifted cards.
- **Accents add a colored glow, not just a drop shadow** — `--glow-accent` rings a focused/active element in cyan. Glows are the signature of "lit up" / interactive state.

### Borders
- Hairlines in **white-alpha** (`--border` ≈ 7.5%, `--border-strong` ≈ 14%). Interactive/active edges switch to `--border-accent` (cyan). Focus uses a cyan `--ring`.

### Motion (transparency + blur + **motion** is the brief)
- **Quick and instrument-like.** Default `--ease-out` (a snappy decel) over `--dur-base 200ms`; pops/scale-ins use `--ease-spring` for a tiny overshoot. Nothing slow or floaty.
- **Signature motions:** cards **lift + cyan-glow** on hover; the cmdk palette **scales in** from 98%→100% with a backdrop blur-in; status changes **pulse** their dot; counts **tick** when refreshed. A subtle **orbital** loading spinner (a dot circling) ties to the brand.
- Hover/press states: **hover** = surface lightens (`--glass`→`--glass-hover`) + border brightens + optional glow; **press** = quick scale to ~0.97 + accent dims (`--accent-dim`). Respect `prefers-reduced-motion`.

### Cards (the most important component)
A repo card is: **glass fill + blur**, `10px` radius, hairline border, `--shadow-2`. Top sheen. On hover it lifts, the border goes cyan-tinted, and a soft cyan glow appears. Contents follow the spec's card anatomy: language dot + big display name + language badge; `owner/repo · ~/path` slug; README first line; a mono status row (`⎇ main ↑2 ↓0 ● 3 changes`); activity line; and two launcher buttons. The card *is* the product — get it right and everything else follows.

---

## Iconography

See [`README` → ICONOGRAPHY](#iconography) realized via **[Lucide](https://lucide.dev)** — the icon set shipped with shadcn/ui and therefore Orrery's native set.

- **System:** **Lucide**, loaded from CDN (`lucide@latest`). Outline style, **1.5px–2px stroke**, 24px grid, `currentColor`. This matches shadcn/ui out of the box. No filled/duotone icons, no Material, no Font Awesome.
- **Sizing:** 14–16px inline with text/in buttons; 18–20px for toolbar/nav; dots and language badges are CSS, not icons.
- **The app mark** is Lucide **`orbit`** (a dot with a smaller dot orbiting it) — it *is* the metaphor, and it's a real icon in the set rather than a hand-drawn SVG. Paired with the "Orrery" wordmark in Geist. The 🪐 emoji is the docs/README mark only.
- **Key icons in use:** `orbit` (brand/loading), `git-branch`, `arrow-up`/`arrow-down` (ahead/behind), `circle-dot` (dirty/changes), `git-commit-horizontal`, `star` (favorites), `sparkles` (AI summary), `search`, `command`, `folder-git-2`, `terminal`, `code` / `square-terminal` (launchers), `settings`, `refresh-cw` (scan/refresh), `sliders-horizontal` (filters), `chevron-*`.
- **Emoji & unicode:** One brand emoji (🪐) in docs only. A couple of unicode glyphs earn their place as *typographic* marks in mono data: `⎇` (branch), `↑`/`↓` (ahead/behind), `·` (meta separator), `◗` (agent). These are intentional and consistent — not a free-for-all.
- **No emoji in product chrome.** Status and meaning come from Lucide icons + the color system, never emoji.

> **Substitution flag:** Orrery ships no custom icon assets, so this system uses **Lucide from CDN** — which *is* the spec's icon set (via shadcn/ui), so this is a faithful match, not a guess. If Orrery later adds bespoke marks (a real app logo, custom launcher glyphs), drop them into [`assets/`](./assets/) and update this section.

---

## Font substitution note

Geist & Geist Mono are loaded from **Google Fonts CDN** (see top of `colors_and_type.css`) rather than bundled `.ttf` files, because the repo shipped no font files. Geist is the correct, intended family for a modern shadcn/ui app. **If you want self-hosted fonts** (recommended for the offline Tauri app), download Geist from https://vercel.com/font or `@fontsource/geist-sans` + `@fontsource/geist-mono` and drop them in [`fonts/`](./fonts/). **Flagging this so you can supply the real files if preferred.**

---

## Index — what's in this folder

| Path | What it is |
|---|---|
| [`README.md`](./README.md) | This file — product context, voice, visual foundations, iconography, index |
| [`colors_and_type.css`](./colors_and_type.css) | All foundation tokens: color, type, space, radii, elevation, motion + semantic classes |
| [`SKILL.md`](./SKILL.md) | Agent-Skill manifest so this system works as a Claude Code skill |
| [`preview/`](./preview/) | Design-system preview cards (color, type, components) shown in the Design System tab |
| [`assets/`](./assets/) | Logos & brand marks (wordmark lockups) |
| [`ui_kits/orrery-desktop/`](./ui_kits/orrery-desktop/) | High-fidelity UI kit recreating the desktop app — `index.html` + JSX components |

**Sources this system was built from**
- Orrery repo — https://github.com/Hankanman/Orrery
- Design spec — https://github.com/Hankanman/Orrery/blob/main/DESIGN.md
- shadcn/ui (component library) — https://ui.shadcn.com · Lucide (icons) — https://lucide.dev · Tauri — https://tauri.app

> Explore the GitHub repo directly to build even more accurately as real components land — this system anticipates the spec; the shipping app is the ultimate source of truth.
