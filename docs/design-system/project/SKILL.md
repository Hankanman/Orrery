---
name: orrery-design
description: Use this skill to generate well-branded interfaces and assets for Orrery, either for production or throwaway prototypes/mocks/etc. Orrery is a Linux-native "mission control" desktop app that puts every git repo in orbit — dark, dense, neon-accented, with transparency, blur, and motion. Contains essential design guidelines, colors, type, fonts, assets, and UI kit components for prototyping.
user-invocable: true
---

Read the `README.md` file within this skill, and explore the other available files (`colors_and_type.css` for all tokens, `preview/` for visual specimens, `ui_kits/orrery-desktop/` for the high-fidelity component recreation, `assets/` for the brand mark).

If creating visual artifacts (slides, mocks, throwaway prototypes, etc), copy assets out and create static HTML files for the user to view. If working on production code, you can copy assets and read the rules here to become an expert in designing with this brand.

Key things to honor (full detail in `README.md`):
- **Dark only.** Deep blue-black "space" backgrounds; glass surfaces use translucent white + `backdrop-filter: blur()`.
- **Three accent hues map to the three data sources:** orbit **cyan** `#38DBF0` (local git / primary / interactive), star **amber** `#FFC24B` (host stars/favorites), AI **violet** `#A78BFA` (local-AI / generated). Plus git status colors and the GitHub-linguist language palette.
- **Two fonts, both Geist:** Geist Sans for humans, Geist Mono for machine data (slugs, paths, branches, counts, code).
- **Dense, data-rich layout** (4–5 cards/row), tight radii, hairline white-alpha borders, deep soft shadows + colored accent glow on active elements.
- **Motion is quick & instrument-like:** `ease-out` 200ms default, springy pops, hover lift + glow, orbital loaders. Respect `prefers-reduced-motion`.
- **Icons are Lucide** (the shadcn/ui set). Outline, ~1.5–2px stroke, `currentColor`. App mark is Lucide `orbit`. One brand emoji (🪐) in docs only — never in product UI.
- **Voice:** terse, confident, CLI-economical; the space metaphor (orbit, mission control, launchpad, stars) is structural, not decorative. Tagline always lowercase: *every repo in orbit*.

If the user invokes this skill without any other guidance, ask them what they want to build or design, ask some questions, and act as an expert designer who outputs HTML artifacts _or_ production code, depending on the need.
