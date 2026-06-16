//! Tauri adapter over `orrery_platform::appearance`. The portal/kdeglobals
//! reading now lives in orrery-platform (UI-agnostic); this just exposes it as
//! an IPC command and bridges live changes to a webview event.

use orrery_platform::appearance::Appearance;
use tauri::{AppHandle, Emitter};

/// One-shot read of the current desktop appearance.
#[tauri::command]
pub async fn get_appearance() -> Appearance {
    orrery_platform::appearance::read().await
}

/// Emit `appearance-changed` whenever the desktop theme or accent changes.
pub fn spawn_watcher(app: AppHandle) {
    orrery_platform::appearance::watch(move |appearance| {
        let _ = app.emit("appearance-changed", appearance);
    });
}
