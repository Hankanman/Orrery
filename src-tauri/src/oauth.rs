//! GitHub OAuth device flow (#18) + token resolution.
//!
//! The device flow needs a registered OAuth app `client_id` (set in config).
//! For enrichment we resolve a token from, in order: the stored OAuth token,
//! `$ORRERY_GITHUB_TOKEN`, or the `gh` CLI — so public + already-authenticated
//! setups work without configuring an OAuth app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const SCOPE: &str = "read:user public_repo";

fn token_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery").join("github_token"))
}

pub fn stored_github_token() -> Option<String> {
    token_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_token(token: &str) -> Result<(), String> {
    let path = token_path().ok_or("no data directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Create owner-only from the start (no umask race) — the token is a secret.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, token).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cli_token(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin).args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Resolve a GitHub token: stored OAuth → env → `gh auth token`.
pub fn github_token() -> Option<String> {
    stored_github_token()
        .or_else(|| std::env::var("ORRERY_GITHUB_TOKEN").ok().filter(|s| !s.is_empty()))
        .or_else(|| cli_token("gh"))
}

/// Resolve a GitLab token: env → `glab auth token`.
pub fn gitlab_token() -> Option<String> {
    std::env::var("ORRERY_GITLAB_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| cli_token("glab"))
}

/// True if any GitHub token is available.
pub fn github_authed() -> bool {
    github_token().is_some()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
}

/// Begin the device flow: returns the code the user enters at the URL.
pub async fn device_start(client_id: &str) -> Result<DeviceStart, String> {
    #[derive(Deserialize)]
    struct Resp {
        device_code: String,
        user_code: String,
        verification_uri: String,
        interval: u64,
    }
    let resp: Resp = reqwest::Client::new()
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(DeviceStart {
        user_code: resp.user_code,
        verification_uri: resp.verification_uri,
        device_code: resp.device_code,
        interval: resp.interval,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    /// "authorized" | "authorization_pending" | "slow_down" | "expired_token" | "access_denied" | "error"
    pub status: String,
}

/// Poll once for the token. On success, persists it.
pub async fn device_poll(client_id: &str, device_code: &str) -> Result<PollResult, String> {
    #[derive(Deserialize)]
    struct Resp {
        access_token: Option<String>,
        error: Option<String>,
    }
    let resp: Resp = reqwest::Client::new()
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(token) = resp.access_token {
        let token = token.trim();
        if token.is_empty() || !token.is_ascii() || token.len() > 255 {
            return Err("received a malformed access token".into());
        }
        save_token(token)?;
        return Ok(PollResult { status: "authorized".into() });
    }
    Ok(PollResult {
        status: resp.error.unwrap_or_else(|| "error".into()),
    })
}

/// Forget the stored OAuth token (sign out).
pub fn sign_out() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}
