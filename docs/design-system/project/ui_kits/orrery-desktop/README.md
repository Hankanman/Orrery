# Orrery Desktop — UI Kit

A high-fidelity, interactive recreation of the **Orrery** desktop app (Tauri + React + Tailwind + shadcn/ui). This is a *cosmetic* recreation for design work — not production code — built to match the locked spec in the source repo's [`DESIGN.md`](https://github.com/Hankanman/Orrery/blob/main/DESIGN.md).

> **Source of truth:** Orrery shipped no implementation when this kit was built — only a design spec. Every screen here realizes that spec (dark/dense "mission control", neon accents, card anatomy, cmdk palette, IDE/agent launchers) plus the brief: *transparency, blur, motion*. When the real app lands components, reconcile against them — see https://github.com/Hankanman/Orrery.

## Run it

Open [`index.html`](./index.html) — no build step. It loads React + Babel from CDN and the foundation tokens from `../../colors_and_type.css`.

**Interactive flow:** opens on the **first-run onboarding** (empty state with the orbital animation) → click **Add a directory** → watch the **scan** animation → land in the **mission-control grid**. From there:
- **⌘K / click the search bar** → command palette (cmdk) with repo + command search, arrow-key nav.
- **Click any card** → repo detail drawer (AI summary, git state, host stats, recent commits).
- **⌘, / gear icon** → settings (launcher templates, scan depth, toggles).
- **Sidebar** filters by root + language; **chip row** filters by status (dirty/ahead/starred/stale); **sort pill** cycles activity/name/stars; **grid/list** toggle; **star** to favorite; **Open in IDE / Agent** fire a launch toast.

**Feed view (Phase 4).** Switch to **Feed** in the sidebar (or run *Open feed* in the palette) for a GitHub-explore-style activity stream of the repos you follow — releases (with version pill + notes), star milestones, pushes (with commit list), new repos, and issues, grouped by day. Filter chips (All / Releases / Activity / Stars), a **Following** list with follow toggles, and a **Trending in your orbit** rail. Clicking a local repo's event opens its detail drawer; remote repos fire a "view on host" toast.

## Files

| File | What it is |
|---|---|
| `index.html` | Entry — loads scripts + mounts `<App>` |
| `styles.css` | All kit styles (imports the design-system tokens) |
| `data.js` | Mock repo fixtures (`window.ORR_DATA`) modeled on real repos |
| `icons.jsx` | `Icon` component + inline **Lucide** path data (the spec's icon set) |
| `components.jsx` | `TitleBar`, `Sidebar`, `RepoCard`, `StatusRow`, `EmptyState`, `Scanning` |
| `Feed.jsx` | `FeedView` + `FeedEvent` — the Phase-4 followed/starred activity stream |
| `overlays.jsx` | `CommandPalette`, `RepoDetail`, `Settings` |
| `app.jsx` | `App` — flow/state, toolbar, filters, sort, launch toasts |

## Components (lift these into real designs)

- **TitleBar** — custom Linux-native window chrome: brand lockup, roots summary, command-bar search, rescan/settings, window controls.
- **Sidebar** — roots + language facets with live counts; active state in orbit cyan.
- **RepoCard** — *the* component. Glass + blur, language dot, name, slug/path, README line, AI-summary indicator, mono status row (`⎇ branch ↑↓ ● changes`), host row (stars/time/host), IDE + Agent launchers. Hover = lift + cyan glow. Supports `grid` and `list` views.
- **CommandPalette** — cmdk overlay: glass, scale-in, grouped commands + repos, full keyboard nav.
- **FeedView / FeedEvent** — Phase-4 activity stream: day-grouped release/push/star/newrepo/issue events, Following list, Trending rail.- **RepoDetail** — right-hand drawer: violet AI-summary block, git-state grid, host-stats grid, commit list, launch actions.
- **Settings** — modal: launcher command templates, scan depth, live-watch / local-AI / star-field toggles.
- **EmptyState / Scanning** — first-run onboarding + scan animation, both using the orbital motif.

## Notes & corners cut

- **Mock data only** — no real `git2`, hosts, or llama.cpp. Statuses, stars, AI blurbs, and commits are fixtures.
- **Icons are Lucide inline** (real path data) rather than the npm package, so the kit is dependency-free.
- **Window controls** are decorative (the macOS-style dots read clearly as window controls; swap for your platform's chrome in production).
- Fonts (Geist / Geist Mono) load from Google Fonts via the tokens file.
