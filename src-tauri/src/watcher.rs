//! Tauri adapter over `orrery_platform::watcher`. Emits a debounced
//! `repos-changed` webview event so the UI can rescan when repos change on disk.

use tauri::{AppHandle, Emitter};

/// Spawn the filesystem watcher, emitting `repos-changed` on each change batch.
pub fn spawn(app: AppHandle) {
    orrery_platform::watcher::spawn(move || {
        let _ = app.emit("repos-changed", ());
    });
}
