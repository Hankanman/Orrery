# Fleet hygiene

::: info Status
The engine for these features is in `orrery-core`. The views and cards that surface them are still being built, so this page describes what Orrery checks and how it stays safe, ahead of the UI.
:::

Fleet hygiene covers four things that pile up quietly across many repos: build artefacts eating disk, work that only exists on this machine, stray worktrees, and vulnerable dependencies.

## Disk usage and the artefact janitor

Orrery looks in each repo for the usual build-artefact directories and measures them:

| Kind | Directory | Only when |
|---|---|---|
| Cargo target | `target/` | a `Cargo.toml` sits beside it |
| node_modules | `node_modules/` | |
| Python venv | `.venv/`, `venv/` | it contains `pyvenv.cfg` |
| dist / build | `dist/`, `build/` | |
| Next.js cache | `.next/` | |
| Python bytecode | `__pycache__/` | |
| Gradle cache | `.gradle/` | |

Every kind must also be **ignored by git**, with **no tracked files** under it. A force-added file inside an ignored `dist/` still counts as tracked, so that `dist/` is left alone.

Measuring is fast and bounded:

- the walk runs in parallel;
- it never follows symlinks or crosses into another filesystem;
- hard-linked files (pnpm stores, for example) are counted once;
- a pathological tree stops at an entry budget or a time limit and is marked as a lower bound.

Sizes are cached for a day.

Purging deletes only the kinds you pick and reports the bytes freed. Every directory is checked again right before deletion. Orrery refuses anything that:

- is a symlink;
- resolves outside the repo (through a symlinked parent, `..`, or an absolute path elsewhere);
- isn't ignored by git;
- has tracked files under it;
- isn't a known artefact kind.

Refused paths are listed with the reason.

## At-risk work

Some work only exists on your disk, or has been parked for so long that it's easy to forget. For each repo, Orrery reports:

- **Unpushed branches**: local branches with no upstream whose commits aren't on any remote branch, and branches ahead of their upstream. A local branch that points at an already-pushed commit isn't flagged.
- **Old stashes**: stashes older than 30 days.
- **Long-dirty working trees**: uncommitted changes that have been there for 7 days or more. Orrery records when it first sees a repo dirty, so this clock starts on the first scan after you upgrade.
- **No remote**: a repo with commits but no remote at all, so nothing in it is backed up.
- **Orphaned commits**: commits made on a detached HEAD that aren't on any branch.

You can change both thresholds. These findings feed the attention list alongside CI failures and review requests:

| Finding | Severity |
|---|---|
| Unpushed branches | Attention |
| Orphaned commits | Attention |
| Old stashes | Info |
| No remote | Info |
| Long-dirty tree | Adds "dirty for N days" to the existing uncommitted-changes item |

## Linked worktrees

`git worktree add` creates checkouts whose `.git` is a file, not a directory. Orrery now treats these as members of the fleet and marks each one as a worktree of its main checkout, with its branch, so the grid can group them under the parent card.

Orrery finds them in two ways: through the main checkout's own worktree list, wherever the worktrees live, and by scanning your roots, which catches worktrees whose main checkout is outside them. Submodules also use a `.git` file, but Orrery doesn't count them as worktrees.

Orrery skips worktrees that tools manage:

- Any worktree whose path, below the main checkout or its root, includes a directory on your **ignore** list. The default list includes `.claude`, which covers `.claude/worktrees/`.
- Orrery's own agent-dispatch worktrees under `~/.local/share/orrery/worktrees/`, which the Agents view already tracks.

For a worktree, the at-risk checks cover only that checkout: its dirty state and any detached HEAD. Branches, stashes and remotes are shared with the main checkout, so they're reported there once.

## Security advisories

Orrery shows the number of known-vulnerable dependencies in each repo, broken down by severity.

- **GitHub repos** use open [Dependabot alerts](https://docs.github.com/en/code-security/dependabot/dependabot-alerts). Reading them needs a token with Dependabot-alert access: the `security_events` scope for a classic token, or the "Dependabot alerts: read" permission for a fine-grained one. If GitHub answers 403 or 404 (no permission, alerts disabled, or the repo isn't visible), the repo shows as *unavailable* with the reason; it isn't treated as an error. An expired token, a rate limit or an outage keeps the last known counts.
- **Other repos** are audited locally, but only if the tool is already installed:
  - `cargo audit` runs when the repo has a `Cargo.lock` and `cargo-audit` is installed.
  - `npm audit --package-lock-only` runs when the repo has a `package-lock.json` and `npm` is installed.

  Orrery never installs anything and never runs an audit without a lockfile, because `cargo audit` would otherwise write one into your repo. Each run is limited to 90 seconds and is killed if it takes longer. `cargo audit` may refresh its advisory database, and `npm audit` sends the dependency list to your npm registry, as they do when you run them yourself.

Results are cached for six hours. Any repo with open advisories gets a **vulnerable dependencies** item in the attention list, at Attention severity.
