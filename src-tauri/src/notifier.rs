//! Tauri adapter for the background attention poller. On an interval it polls
//! GitHub via `orrery_platform::notifier`, refreshes the tray quick-glance, and
//! fires a native desktop notification for each newly-appeared item.

use std::time::Duration;

use tauri::AppHandle;

use crate::config;
use crate::tray;

const POLL_SECS: u64 = 180;

/// Spawn the poll loop. Runs once immediately (seeds the dedupe snapshot and
/// paints the tray), then every `POLL_SECS`.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            run_once(&app).await;
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        }
    });
}

async fn run_once(app: &AppHandle) {
    let result = orrery_platform::notifier::poll(&config::load()).await;

    // The tray glance always reflects current state (passive readout).
    tray::update(app, &result.lines);

    // Notify only the newly-appeared, opt-in items (already filtered by poll()).
    for notice in &result.fresh {
        let _ = orrery_platform::notify::send(&notice.title, &notice.body).await;
    }
}
