//! Tauri adapter for the background attention poller. The poll loop, dedupe, and
//! native desktop notifications all live in `orrery_platform::notifier::watch`;
//! here we only paint the tray quick-glance from each poll's glance lines.

use tauri::AppHandle;

use crate::tray;

/// Start the attention poller, refreshing the tray on every poll.
pub fn spawn(app: AppHandle) {
    orrery_platform::notifier::watch(move |lines| tray::update(&app, &lines));
}
