# Notifications & tray

Orrery keeps watch in the background so you don't have to keep checking. It polls your git hosts on a timer and surfaces anything that needs you as a desktop notification and a tray glance.

## What you get notified about

In **Settings → Notifications**, toggle each event independently:

- **New pull requests** — a PR you should know about appears.
- **Review requested** — someone asked for your review.
- **CI failure** — a check suite goes red.

Notifications are de-duplicated, so you're told about each thing once. On first run Orrery seeds what's already there silently, so you don't get a burst of notifications for history the moment you enable it.

## Close to tray

Closing the window **hides Orrery to the system tray** rather than quitting, so the background poller keeps running and notifications keep arriving. The tray icon doubles as a quick glance:

- A summary of what currently needs attention.
- Shortcuts to recently used repos.
- **Quit** to actually exit.

Prefer Orrery fully closed? Quit from the tray menu.

::: tip Summon it back
A global shortcut — <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>O</kbd> — brings the window back to the front from anywhere. (Best-effort on native Wayland, which may gate global shortcuts behind a portal.)
:::
