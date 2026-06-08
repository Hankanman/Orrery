# Maintenance & tools

Two utility views for the housekeeping that piles up across a workspace.

## Cleanup

The **Cleanup** view (a branch janitor) scans every repo for branches that are safe to delete — ones that are **merged** or whose **upstream is gone** — and lets you prune them in bulk instead of `git branch -d`-ing repo by repo.

- Branches are grouped by repo, with why each is prunable.
- Nothing is deleted without your say-so; the current branch is never offered.
- A clean workspace just says so — *"No merged or gone-upstream branches across your repos."*

It pairs well with the per-repo branch list and [worktree manager](./repo-drawer) in the drawer.

## Dev Tools

The **Dev Tools** view is an offline utility belt — the small conversions and generators you'd otherwise hit a sketchy web tool for, here and private. Pick from the category rail or search:

| Category | Tools |
|---|---|
| **Generate** | UUID generator, Hash (SHA) |
| **Encode** | URL encode / decode, Base64 encode / decode |
| **Data** | JSON format / minify, JWT decoder |
| **Convert** | Timestamp converter, Number base converter, Colour converter |
| **Text** | Case converter, Regex tester |

Everything runs locally in the webview — no input ever leaves your machine, which matters when you're pasting a token into a JWT decoder.
