# Mission Control

Mission Control is the home view: a windowed grid of every repo Orrery found across your workspace roots. It's built to scale to hundreds of repos — the grid is virtualized, so only what's on screen is rendered.

![Mission Control — the repo grid](/shots/mission-control.png)

## The repo card

Each card fuses local git state with host enrichment and an optional AI summary:

- **Language logo** — a full-colour mark for the detected primary language.
- **Slug & path** — `owner/repo` (if it has a remote) and the abbreviated on-disk path.
- **Description** — the README's first line, with the AI summary on its own line when available.
- **Git status row** — branch, ahead/behind divergence, and uncommitted-change count. Clean repos stay quiet — no badge, since clean is the unremarkable default.
- **Host row** — a lock for private repos, CI state, stars, latest release, last-commit time, and the host icon.
- **Launch actions** — Open in IDE, Agent, reveal Folder, and Open on the host. See [Launchers](./launchers).

## Filtering & sorting

The toolbar and chip row narrow the grid:

- **Visibility** — `All` / `Public` / `Private`. Public means a non-private remote; private covers private remotes *and* local-only repos (which aren't published anywhere).
- **Chips** — `Dirty`, `Ahead`, `Starred`, `Stale` (multi-select).
- **Attention** — a quick toggle for repos that need it: uncommitted work, unpushed/behind commits, or no recent activity.
- **Sort** — the toolbar's sort button cycles Activity (latest commit first), Name, and Stars (most host stars first; repos with the same count, including every unstarred one, by name).
- **Tags, Roots & Languages** — the sidebar filters by project tag, workspace root and detected language.

The activity graph at the top summarises recent commit activity across the visible repos; toggle it from the toolbar.

### Saved views & projects

The sidebar helps carve up a large workspace:

- **Project tags** — label repos to group related work across roots (`work`, `client-x`). Add and remove tags in the drawer's **Overview**: type a tag and press <kbd>Enter</kbd> or **Add**, and click a chip's × to remove it. Tags are stored lowercase, with spaces turned into `-`. They show as chips on cards and list rows, and the sidebar's **TAGS** section lists each one with its repo count: click a tag to filter the grid to it, and click it again to clear the filter.
- **Saved views** — capture the current filter/sort/visibility combination, including a tag filter, as a named preset and jump back to it in one click. Presets persist locally.

## Selecting repos

Each card has a checkbox; select one or more (or use the toolbar's select-all) to bring up the **fleet bar** for batch git operations across the selection. See [Fleet operations](./fleet).

## List view

Switch to a compact, single-line view from the segmented control — useful when you're scanning a lot of repos at once.

![Mission Control — list view](/shots/list-view.png)

## The repo drawer

Click a card to slide out a detail drawer with the repo's branches, recent commits, staged-diff view, README, a PR/CI panel, and per-repo notes — plus the same launch actions in the footer.

![The repo detail drawer](/shots/repo-drawer.png)

It's covered in full on its own page: [The repo drawer](./repo-drawer).

## Command palette

Press <kbd>⌘K</kbd> / <kbd>Ctrl K</kbd> to search repos and run commands without leaving the keyboard.

It also does **cross-repo code search**: type a query and Orrery runs [ripgrep](https://github.com/BurntSushi/ripgrep) across your repos, returning matching files and lines you can open directly — a fast way to find that one call site across the whole fleet.
