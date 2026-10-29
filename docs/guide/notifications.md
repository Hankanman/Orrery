# Notifications & tray

Orrery keeps watch in the background so you don't have to keep checking. It polls GitHub on a timer (every few minutes) and surfaces anything that needs you as a desktop notification and a tray glance.

## What you get notified about

- **New pull requests** come straight from the background GitHub poll.
- **Review requests** and **CI failures** come from the *attention model*: the single ranked "needs you now" list that also drives the sidebar badges, the grid's Attention filter and the tray. Both are *urgent* items there. A review request comes from your GitHub inbox; a CI failure means the latest **default-branch** CI run of one of your GitHub repos failed (checked by a central CI pass that refreshes every few minutes). When a new urgent item appears, Orrery notifies once.
- **Agent finished** fires when a dispatched agent session ends with commits on its branch (see [Fleet operations](./fleet)); it also shows an in-app toast.

In **Settings → Notifications** there's a master switch, an **Urgent attention items** switch (review requests and CI failures), and a toggle per event (new PR, review requested, CI failure, agent finished).

Notifications are de-duplicated, so you're told about each thing once, and the seen set is persisted, so a restart doesn't re-notify what's still pending. On first run Orrery seeds what's already there silently, so you don't get a burst of notifications for history the moment you enable it.

::: info GitHub only (for now)
Notifications, the Inbox and CI status currently use GitHub. GitLab support is planned.
:::

## Close to tray

Closing the window **hides Orrery to the system tray** rather than quitting, so the background poller keeps running and notifications keep arriving. The tray icon doubles as a quick glance:

- A summary of what currently needs attention.
- Shortcuts to recently used repos.
- **Quit** to actually exit.

Prefer Orrery fully closed? Quit from the tray menu.

::: tip Summon it back
A global shortcut — <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>O</kbd> — brings the window back to the front from anywhere. It's registered through the XDG **GlobalShortcuts portal** (KDE Plasma 6, GNOME), where you can rebind it in your desktop's shortcut settings. There's no X11 key-grab fallback, so it's best-effort: on X11 sessions and on desktops without that portal there's no global shortcut.
:::
