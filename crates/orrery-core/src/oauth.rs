//! GitHub OAuth device flow (#18) + token resolution.
//!
//! The device flow needs a registered OAuth app `client_id` (set in config).
//! For enrichment we resolve a token from, in order: the stored OAuth token,
//! `$ORRERY_GITHUB_TOKEN`, or the `gh` CLI — so public + already-authenticated
//! setups work without configuring an OAuth app.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::forge;

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
// `repo` (not just `public_repo`) so enrichment can read private repos — needed
// for the public/private filter and the lock badge to work on private repos.
const SCOPE: &str = "read:user repo";

/// Built-in OAuth app client id for the device flow, so sign-in works out of the
/// box with no configuration. The device flow has no client secret, so a client
/// id is not sensitive. A non-empty `github_client_id` in config overrides it
/// (e.g. to point at your own OAuth app).
const DEFAULT_GITHUB_CLIENT_ID: &str = "Ov23liQZt2ALfwxZbINW";

/// The client id to use: the configured one if set, otherwise the built-in default.
pub fn github_client_id() -> String {
    let configured = crate::config::load().github_client_id;
    if configured.trim().is_empty() {
        DEFAULT_GITHUB_CLIENT_ID.to_string()
    } else {
        configured
    }
}

fn token_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery").join("github_token"))
}

/// Trim a candidate token and accept it only if it could plausibly be one:
/// non-empty, ≤255 printable ASCII chars, no whitespace. Tokens end up in an
/// `Authorization` header, so a stray newline or control char from a file,
/// env var or CLI output must never get that far.
fn plausible_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    (!token.is_empty() && token.len() <= 255 && token.chars().all(|c| c.is_ascii_graphic()))
        .then(|| token.to_string())
}

pub fn stored_github_token() -> Option<String> {
    token_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| plausible_token(&s))
}

fn save_token(token: &str) -> Result<(), String> {
    let path = token_path().ok_or("no data directory")?;
    save_token_at(&path, token)
}

/// Write the token to `path`, owner-only (0600). Split out for testing.
fn save_token_at(path: &Path, token: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Create owner-only from the start (no umask race) — the token is a secret.
    // `mode` only applies when the file is *created*, so an existing file
    // (e.g. left 0644 by an older build or a manual copy) is also narrowed
    // before the secret is written into it.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, token).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cli_token(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin)
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    plausible_token(&String::from_utf8_lossy(&out.stdout))
}

fn env_token(var: &str) -> Option<String> {
    std::env::var(var).ok().and_then(|s| plausible_token(&s))
}

/// The resolution order, over already-gathered sources: stored OAuth → env →
/// CLI. The CLI is a closure because it spawns a process — it only runs when
/// the cheaper sources came up empty. Pure for testing.
fn resolve(
    stored: Option<String>,
    env: Option<String>,
    cli: impl FnOnce() -> Option<String>,
) -> Option<String> {
    stored.or(env).or_else(cli)
}

/// Resolve a GitHub token: stored OAuth → env → `gh auth token`.
///
/// Blocking: may spawn `gh`. From async code use [`github_token_async`], and
/// resolve once per pass rather than once per request.
pub fn github_token() -> Option<String> {
    resolve(
        stored_github_token(),
        env_token("ORRERY_GITHUB_TOKEN"),
        || cli_token("gh"),
    )
}

/// Resolve a GitLab token: env → `glab auth token`. Blocking, like
/// [`github_token`]; prefer [`gitlab_token_async`] from async code.
pub fn gitlab_token() -> Option<String> {
    resolve(None, env_token("ORRERY_GITLAB_TOKEN"), || cli_token("glab"))
}

/// Run a blocking token lookup without stalling an async worker: on the
/// blocking pool when inside a tokio runtime, inline otherwise.
async fn off_worker(f: fn() -> Option<String>) -> Option<String> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.spawn_blocking(f).await.ok().flatten(),
        Err(_) => f(),
    }
}

/// [`github_token`] for async callers — the `gh` spawn runs on tokio's
/// blocking pool instead of a reactor worker.
pub async fn github_token_async() -> Option<String> {
    off_worker(github_token).await
}

/// [`gitlab_token`] for async callers (see [`github_token_async`]).
pub async fn gitlab_token_async() -> Option<String> {
    off_worker(gitlab_token).await
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
    let resp: Resp = forge::client()
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
    let resp: Resp = forge::client()
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
        let token = plausible_token(&token).ok_or("received a malformed access token")?;
        save_token(&token)?;
        return Ok(PollResult {
            status: "authorized".into(),
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn resolution_prefers_stored_then_env_then_cli() {
        assert_eq!(resolve(s("stored"), s("env"), || s("cli")), s("stored"));
        assert_eq!(resolve(None, s("env"), || s("cli")), s("env"));
        assert_eq!(resolve(None, None, || s("cli")), s("cli"));
        assert_eq!(resolve(None, None, || None), None);
    }

    #[test]
    fn cli_is_not_spawned_when_a_cheaper_source_answers() {
        let mut ran = false;
        let got = resolve(None, s("env"), || {
            ran = true;
            s("cli")
        });
        assert_eq!(got, s("env"));
        assert!(!ran, "the gh/glab spawn must be skipped");
    }

    #[test]
    fn plausible_token_trims_and_rejects_junk() {
        assert_eq!(plausible_token("  gho_abc123\n"), s("gho_abc123"));
        assert_eq!(plausible_token(""), None);
        assert_eq!(plausible_token("   \n"), None);
        // Interior whitespace / control chars could split or inject a header.
        assert_eq!(plausible_token("abc\r\nX-Evil: 1"), None);
        assert_eq!(plausible_token("abc def"), None);
        assert_eq!(plausible_token("tok\u{0}en"), None);
        assert_eq!(plausible_token("tökén"), None, "non-ASCII");
        assert_eq!(plausible_token(&"a".repeat(256)), None, "too long");
        assert!(plausible_token(&"a".repeat(255)).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn save_token_narrows_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orrery").join("github_token");

        // Fresh file → 0600.
        save_token_at(&path, "first").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        // An existing world-readable file is narrowed, then overwritten.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_token_at(&path, "second").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }
}
