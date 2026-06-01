//! Tauri IPC commands exposed to the frontend.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{AppConfig, Repo};
use crate::{cache, config, launch, scan};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn get_config() -> AppConfig {
    config::load()
}

#[tauri::command]
pub fn set_config(config: AppConfig) -> Result<(), String> {
    config::save(&config)
}

/// Cached repo snapshot for instant paint before a fresh scan completes.
#[tauri::command]
pub fn cached_repos() -> Vec<Repo> {
    cache::load_repos()
}

/// Scan the configured roots for repos (runs off the UI thread), refresh the
/// cache, and return the results.
#[tauri::command]
pub async fn scan_repos() -> Result<Vec<Repo>, String> {
    let repos = tauri::async_runtime::spawn_blocking(|| {
        let cfg = config::load();
        let favorites = cache::favorites();
        scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &favorites, now_unix())
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = cache::store_repos(&repos);
    Ok(repos)
}

#[tauri::command]
pub fn set_favorite(id: String, favorite: bool) -> Result<bool, String> {
    cache::set_favorite(&id, favorite)
}

#[tauri::command]
pub fn open_in_ide(id: String) -> Result<(), String> {
    launch::launch(&config::load().ide_command, &id)
}

#[tauri::command]
pub fn open_agent(id: String) -> Result<(), String> {
    launch::launch(&config::load().agent_command, &id)
}
