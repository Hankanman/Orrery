//! Tauri IPC commands exposed to the frontend.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{AppConfig, Host, HostInfo, Repo};
use crate::{cache, config, forge, launch, oauth, scan};

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

/// Fetch host enrichment (stars/topics/issues/release) for a repo, cached for
/// 6h. On network failure, falls back to any stale cache (offline support).
#[tauri::command]
pub async fn enrich_repo(host: Host, domain: String, slug: String) -> Result<HostInfo, String> {
    let now = now_unix();
    if let Some(fresh) = cache::cached_host_info(&slug, 6 * 3600, now) {
        return Ok(fresh);
    }
    let token = match host {
        // GitHub requests always go to api.github.com, so the token can't leak
        // to the repo's (untrusted) remote domain.
        Host::Github => oauth::github_token(),
        // Only attach a GitLab token for gitlab.com or an explicitly trusted
        // self-hosted host — never to an arbitrary domain from a repo remote.
        Host::Gitlab => {
            let trusted = domain == "gitlab.com" || config::load().gitlab_hosts.iter().any(|h| h == &domain);
            if trusted {
                oauth::gitlab_token()
            } else {
                None
            }
        }
    };
    match forge::fetch(host, &domain, &slug, token.as_deref()).await {
        Ok(info) => {
            cache::store_host_info(&slug, &info, now);
            Ok(info)
        }
        Err(e) => cache::cached_host_info(&slug, i64::MAX, now).ok_or(e),
    }
}

#[tauri::command]
pub async fn github_login_start() -> Result<oauth::DeviceStart, String> {
    let client_id = config::load().github_client_id;
    if client_id.is_empty() {
        return Err("Set a GitHub OAuth client id in settings first.".into());
    }
    oauth::device_start(&client_id).await
}

#[tauri::command]
pub async fn github_login_poll(device_code: String) -> Result<oauth::PollResult, String> {
    let client_id = config::load().github_client_id;
    if client_id.is_empty() {
        return Err("Set a GitHub OAuth client id in settings first.".into());
    }
    oauth::device_poll(&client_id, &device_code).await
}

#[tauri::command]
pub fn github_auth_status() -> bool {
    oauth::github_authed()
}

#[tauri::command]
pub fn github_sign_out() {
    oauth::sign_out();
}
