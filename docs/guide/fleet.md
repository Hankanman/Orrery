# Fleet operations

Orrery isn't just a viewer — it operates across many repos at once, and keeps track of the work you launch from it.

## Bulk actions

Select repos in [Mission Control](./mission-control) (each card has a checkbox) and the **fleet bar** appears with batch git operations that run across the whole selection:

| Action | What it does |
|---|---|
| **Fetch** | Fetch all remotes. |
| **Pull** | Fast-forward only — never creates a merge or rewrites local work. |
| **Stash** | Stash uncommitted changes. |
| **Checkout default** | Switch each repo to its default branch. |
| **Run command** | Run a single constrained command in each repo. |

Results stream back **per repo** as each finishes — done, skipped (e.g. nothing to pull), or error — and a long run can be **cancelled** mid-flight. Clear the selection from the bar when you're done.

::: tip The command runner is constrained
**Run command** is not a shell. The command is token-split and executed directly (no shell interpretation, no globbing, no `&&`/pipes), so a selection-wide run can't smuggle in destructive shell tricks. It's for safe, repeatable commands like `git gc` or a formatter, not arbitrary scripting.
:::

## Agent & terminal sessions

The **Agents** view is a dashboard of the terminal coding-agent sessions running on your machine. Orrery finds them by scanning `/proc`, so it sees any session — not just ones launched from a card: a process counts if its program is a known agent CLI and its working directory sits inside one of your repos (or a dispatched agent worktree).

- See each session's repo, its command line, pid, and how long it's been running.
- **Terminate** a session, or jump to the repo in your **IDE** or **file manager**. *(Planned: **Reopen** a session in the same repo.)*
- Dispatched worktrees (the drawer's **Dispatch** with a fresh worktree) get their own cards. When a dispatched session ends with commits on its branch, the card flips to *finished* and offers **Review**, **Open PR**, or **Discard**.
- The list drops dead sessions automatically and refreshes on a short poll while the view is open.

It's the answer to "what did I leave running, and where?" across a busy day.
