# Notifications & tray

Orrery keeps watch in the background so you don't have to keep checking. It polls every host you've connected (GitHub, GitLab including trusted self-hosted instances, and Gitea / Forgejo / Codeberg) on a timer (every few minutes) and surfaces anything that needs you as a desktop notification and a tray glance.

## What you get notified about

- **New pull requests** (and GitLab merge requests) come straight from the background poll of each connected host.
- **Review requests** and **CI failures** come from the *attention model*: the single ranked "needs you now" list that also drives the sidebar badges, the grid's Attention filter and the tray. Both are *urgent* items there. A review request comes from your inbox on any connected host; a CI failure means the latest **default-branch** CI run of one of your GitHub or Gitea / Forgejo / Codeberg repos failed (checked by a central CI pass that refreshes every few minutes). When a new urgent item appears, Orrery notifies once.
- The tray glance also lists CI alerts from GitHub's check-suite notifications and GitLab's "pipeline failed" to-dos.
- **Agent finished** fires when a dispatched agent session ends with commits on its branch (see [Fleet operations](./fleet)); it also shows an in-app toast.

In **Settings → Notifications** there's a master switch, an **Urgent attention items** switch (review requests and CI failures), and a toggle per event (new PR, review requested, CI failure, agent finished).

### Notification buttons

New-PR notifications (from GitHub, GitLab or Gitea-family hosts) carry buttons on desktops whose notification server supports actions (KDE Plasma, GNOME and most others):

- **Open** opens the pull or merge request in your browser. Clicking the notification itself does the same.
- **Show in Orrery** brings the window forward on that repo, when it's cloned locally.

On a server without action support you get the same notification without buttons.

Notifications are de-duplicated, so you're told about each thing once, and the seen set is persisted, so a restart doesn't re-notify what's still pending. Each host instance keeps its own seen set: on first run, and the first time a newly connected host answers, Orrery seeds what's already there silently, so you don't get a burst of notifications for history the moment you connect it. If one host is unreachable for a poll, its last known items stay in place.

::: info Host coverage
GitHub, GitLab and Gitea / Forgejo / Codeberg all feed notifications and the attention model. Default-branch CI status covers GitHub and Gitea-family hosts; GitLab pipelines are planned.
:::

## Close to tray

Closing the window **hides Orrery to the system tray** rather than quitting, so the background poller keeps running and notifications keep arriving. The tray icon doubles as a quick glance:

- A summary of what currently needs attention.
- Shortcuts to recently used repos.
- **Quit** to actually exit.

Prefer Orrery fully closed? Quit from the tray menu.

::: tip Summon it back
A global shortcut — <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>O</kbd> — brings the window back to the front from anywhere. It's registered through the XDG **GlobalShortcuts portal** (KDE Plasma 6, GNOME), where you can rebind it in your desktop's shortcut settings. Where there's no such portal and an X display is available (an X11 session), Orrery grabs <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>O</kbd> directly from the X server instead; it works with Caps Lock or Num Lock on, but can't be rebound, and it fails if another app already holds that combination. Under XWayland that grab only sees keys while an X11 window has focus, so on Wayland the portal is what counts. The portal needs Orrery's `orrery.desktop` entry to be installed; every package provides it, and source builds can add it by hand (see [Getting started](./getting-started#the-desktop-entry-and-the-global-shortcut)).
:::
