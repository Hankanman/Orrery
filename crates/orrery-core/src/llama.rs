//! Bundled llama.cpp backend (#21). Spawns a local `llama-server` sidecar and
//! talks to its native HTTP API (`/health`, `/completion`). The engine binary
//! is discovered at runtime — a configured path, then the app data dir's
//! `bin/`, then `PATH` — so the app degrades to "unavailable" (rather than
//! failing to build/launch) when it isn't present. Release builds ship a
//! `llama-server` as a bundled resource (fetched in CI); on first use (and
//! after an upgrade) it's unpacked into the app data `bin/` so it's found by
//! the same lookup. Models are GGUF files in the app data dir. Generation only;
//! embeddings stay on the Ollama path.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use crate::ai::{GenParams, READ_TIMEOUT};

/// A running `llama-server` for a specific model. Process-lifetime singleton —
/// like the shared HTTP clients — so successive generate calls reuse it.
/// Dropping it kills and reaps the process: a bare `std::process::Child` drop
/// would leave the server (and its loaded model's RAM/VRAM) running.
struct Server {
    child: Child,
    port: u16,
    model: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static SERVER: LazyLock<Mutex<Option<Server>>> = LazyLock::new(|| Mutex::new(None));

/// Serializes the whole check → spawn → health-wait sequence in
/// [`ensure_running`], so two concurrent generate calls can't both spawn a
/// server (orphaning one). Async because it's held across the health polls;
/// `SERVER` itself stays a std mutex so [`shutdown`] works from sync code.
static LIFECYCLE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The bundled llama runtime dir (`<prefix>/lib/orrery/llama-runtime`),
/// recorded once at startup — resolving it needs the executable's install
/// prefix, which the discovery path lacks. Empty (or absent) in dev/source
/// builds; populated by the release CI fetch.
static BUNDLED_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record the bundled llama runtime dir. Called from app setup in
/// `crates/orrery/src/main.rs` with the dir resolved relative to the running
/// executable; a no-op if called more than once.
pub fn set_bundled_dir(dir: PathBuf) {
    let _ = BUNDLED_DIR.set(dir);
}

/// Marker file in app-data `bin/` recording which bundle was materialized.
const BUNDLE_MARKER: &str = ".orrery-bundle";

/// Copy a shipped llama runtime into the writable app-data `bin/`, marking the
/// server executable. We run from app-data rather than straight out of the
/// bundle so it works even when the bundle is a read-only mount (the AppImage
/// squashfs) and so the executable bit is guaranteed regardless of how the
/// bundler copied the resource. Re-copies when the bundle changes (an app
/// upgrade) or a previous copy didn't finish — see [`materialize_into`]. Does
/// nothing when no runtime was bundled; checked once per process.
fn materialize_bundled() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let Some(src) = BUNDLED_DIR
        .get()
        .filter(|d| d.join("llama-server").is_file())
    else {
        return;
    };
    let Some(dest) = data_dir().map(|d| d.join("bin")) else {
        return;
    };
    if materialize_into(src, &dest, &bundle_stamp(src)) {
        DONE.store(true, Ordering::Relaxed);
    }
}

/// Identity of a bundle: the app version plus the shipped server's size and
/// mtime, so both an upgrade and a re-fetched runtime trigger a refresh.
fn bundle_stamp(src: &Path) -> String {
    let meta = std::fs::metadata(src.join("llama-server")).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}:{len}:{mtime}", env!("CARGO_PKG_VERSION"))
}

/// Materialize `src` into `dest` unless `dest`'s marker already records
/// `stamp`. Files are copied into a sibling staging dir first and then renamed
/// into place one by one, and the marker is written last — so an interrupted
/// copy never leaves a marker, and the next call redoes it rather than running
/// a half-copied runtime. Soname symlinks are preserved. Returns true when
/// `dest` is current.
fn materialize_into(src: &Path, dest: &Path, stamp: &str) -> bool {
    let marker = dest.join(BUNDLE_MARKER);
    if dest.join("llama-server").is_file()
        && std::fs::read_to_string(&marker).ok().as_deref() == Some(stamp)
    {
        return true;
    }
    let Some(parent) = dest.parent() else {
        return false;
    };
    let staging = parent.join(format!("bin.staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let staged = stage_copy(src, &staging);
    let ok = staged.is_ok_and(|names| {
        std::fs::create_dir_all(dest).is_ok()
            && names
                .iter()
                .all(|n| std::fs::rename(staging.join(n), dest.join(n)).is_ok())
            && write_atomic(&marker, stamp.as_bytes()).is_ok()
    });
    let _ = std::fs::remove_dir_all(&staging);
    ok
}

/// Copy every file (and symlink) in `src` into a fresh `staging` dir, making
/// the server executable. Returns the staged names.
fn stage_copy(src: &Path, staging: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
    std::fs::create_dir_all(staging)?;
    let mut names = Vec::new();
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let ty = e.file_type()?;
        let to = staging.join(e.file_name());
        if ty.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(e.path())?, &to)?;
            #[cfg(not(unix))]
            std::fs::copy(e.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), &to)?;
        } else {
            continue;
        }
        names.push(e.file_name());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let server = staging.join("llama-server");
        if let Ok(meta) = std::fs::metadata(&server) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&server, perms)?;
        }
    }
    Ok(names)
}

/// Write `bytes` to a temp sibling of `path`, then rename it into place.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn client() -> reqwest::Client {
    // Connect + per-read timeouts: a dead sidecar fails the /health probe fast
    // and a wedged one can't hang a caller forever. No overall request timeout,
    // since CPU inference of a completion can legitimately run long.
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}

/// Client for model downloads: HTTPS only (redirects included), so a model
/// can't be swapped by anyone on the network path.
fn download_client() -> reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}

fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery"))
}

/// Where downloaded GGUF models live.
pub fn models_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("models"))
}

/// Locate the `llama-server` binary: a configured override, then the app data
/// dir's `bin/`, then `PATH`.
fn server_binary() -> Option<PathBuf> {
    let cfg = crate::config::load();
    if !cfg.llama_server_path.trim().is_empty() {
        let p = PathBuf::from(cfg.llama_server_path.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    // Unpack a bundled runtime into app-data bin/ (first use / after an
    // upgrade), so the check below finds it just like a user-installed binary.
    materialize_bundled();
    if let Some(p) = data_dir().map(|d| d.join("bin").join("llama-server")) {
        if p.is_file() {
            return Some(p);
        }
    }
    which::which("llama-server").ok()
}

/// The configured GGUF model file, if it exists on disk.
fn model_path() -> Option<PathBuf> {
    let cfg = crate::config::load();
    let raw = cfg.llama_model_path.trim();
    if raw.is_empty() {
        return None;
    }
    let p = PathBuf::from(raw);
    p.is_file().then_some(p)
}

/// True when `path` has any execute permission bit set. A binary that exists
/// but isn't executable (e.g. copied without mode bits) would only fail later
/// at spawn time, so `available()` checks it up front.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// GGUF fixed header: magic (4) + version (4) + tensor count (8) + metadata
/// kv count (8).
const GGUF_HEADER_LEN: u64 = 24;

/// True when `path` looks like a real GGUF model: at least a full fixed
/// header long and starting with the `GGUF` magic. A cheap 4-byte read — not
/// a full parse — but enough to reject empty/truncated downloads and files
/// that aren't GGUF at all.
fn valid_gguf(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if f.metadata().map(|m| m.len()).unwrap_or(0) < GGUF_HEADER_LEN {
        return false;
    }
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).is_ok() && &magic == b"GGUF"
}

/// `GET /health` on a local sidecar, bounded so a wedged server reads as down.
async fn healthy(port: u16) -> bool {
    client()
        .get(format!("http://127.0.0.1:{port}/health"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// True when the llama.cpp backend can serve generation. If a sidecar is
/// already running, this is a live `GET /health` probe (mirroring the Ollama
/// `/api/version` check). Otherwise it stays cheap — no server spawn, since a
/// model load can take many seconds — but goes beyond bare existence: the
/// engine binary must be executable and the model must pass the GGUF header
/// check, so `ai_ready` isn't reported from a corrupt binary or a truncated
/// download. (The server itself still spawns lazily, on first generate.)
pub async fn available() -> bool {
    let running_port = {
        let guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|s| s.port)
    };
    if let Some(port) = running_port {
        // A dead sidecar refuses the connection immediately, so this stays fast.
        return healthy(port).await;
    }
    let (Some(bin), Some(model)) = (server_binary(), model_path()) else {
        return false;
    };
    is_executable(&bin) && valid_gguf(&model)
}

/// A parsed model download request: the target filename and an optional
/// expected SHA-256 (lowercase hex).
#[derive(Debug, PartialEq, Eq)]
struct ModelUrl {
    name: String,
    sha256: Option<String>,
}

/// Validate a model URL: HTTPS only, path ending in a `.gguf` filename. An
/// expected checksum may ride in the fragment as `#sha256=<64 hex>`.
fn parse_model_url(url: &str) -> Result<ModelUrl, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("model URL must use https://".into());
    }
    let name = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .map(|s| {
            urlencoding::decode(s)
                .map(|c| c.into_owned())
                .unwrap_or(s.into())
        })
        .filter(|s| !s.is_empty())
        .ok_or("could not derive a filename from the URL")?;
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return Err("URL has an unsafe filename".into());
    }
    if !name.to_lowercase().ends_with(".gguf") {
        return Err("URL must point to a .gguf file".into());
    }
    let sha256 = match parsed.fragment().and_then(|f| f.strip_prefix("sha256=")) {
        Some(hex) => Some(normalize_sha256(hex)?),
        None => None,
    };
    Ok(ModelUrl { name, sha256 })
}

fn normalize_sha256(hex: &str) -> Result<String, String> {
    let hex = hex.trim().to_lowercase();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hex)
    } else {
        Err("checksum must be 64 hex characters (SHA-256)".into())
    }
}

/// Download a GGUF model from `url` into [`models_dir`], reporting
/// `(downloaded, total)` bytes via `on_progress`. Returns the final path.
/// `total` is 0 when the server sends no Content-Length. A `#sha256=<hex>`
/// URL fragment is verified like [`download_model_verified`]'s checksum.
pub async fn download_model(
    url: &str,
    on_progress: impl FnMut(u64, u64),
) -> Result<String, String> {
    download_model_verified(url, None, on_progress).await
}

/// [`download_model`] with an explicit expected SHA-256 (overrides any URL
/// fragment). HTTPS only. Streams to a `.part` file, then checks the checksum
/// (if any) and the GGUF header before renaming into place; the partial is
/// removed on any failure. Refuses to replace a valid model that's already
/// installed under the same name.
pub async fn download_model_verified(
    url: &str,
    sha256: Option<&str>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<String, String> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let mut target = parse_model_url(url)?;
    if let Some(hex) = sha256.filter(|s| !s.trim().is_empty()) {
        target.sha256 = Some(normalize_sha256(hex)?);
    }
    let dir = models_dir().ok_or("no models directory")?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let final_path = dir.join(&target.name);
    let part_path = dir.join(format!("{}.part", target.name));
    {
        let p = final_path.clone();
        let exists_valid = tokio::task::spawn_blocking(move || valid_gguf(&p))
            .await
            .unwrap_or(false);
        if exists_valid {
            return Err(format!("{} is already installed", target.name));
        }
    }

    let resp = download_client()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);

    let fail = |part: &Path, e: String| {
        let _ = std::fs::remove_file(part);
        e
    };
    let mut file = tokio::fs::File::create(&part_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| fail(&part_path, e.to_string()))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| fail(&part_path, e.to_string()))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush()
        .await
        .map_err(|e| fail(&part_path, e.to_string()))?;
    file.sync_all()
        .await
        .map_err(|e| fail(&part_path, e.to_string()))?;
    drop(file);

    if total > 0 && downloaded != total {
        return Err(fail(
            &part_path,
            format!("download incomplete ({downloaded} of {total} bytes)"),
        ));
    }
    if let Some(expected) = &target.sha256 {
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &got != expected {
            return Err(fail(&part_path, "checksum mismatch".into()));
        }
    }
    let p = part_path.clone();
    if !tokio::task::spawn_blocking(move || valid_gguf(&p))
        .await
        .unwrap_or(false)
    {
        return Err(fail(
            &part_path,
            "downloaded file is not a GGUF model".into(),
        ));
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| fail(&part_path, e.to_string()))?;
    Ok(final_path.to_string_lossy().into_owned())
}

/// Downloaded GGUF models as (filename, size_bytes).
pub fn installed_models() -> Vec<(String, u64)> {
    models_dir()
        .map(|d| installed_models_in(&d))
        .unwrap_or_default()
}

/// The `.gguf` files in `dir` as (filename, size_bytes); in-progress `.part`
/// downloads and anything else are ignored.
fn installed_models_in(dir: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                Some((p.file_name()?.to_string_lossy().into_owned(), size))
            } else {
                None
            }
        })
        .collect()
}

/// Ask the OS for an unused localhost port (bind to :0, read it back, release).
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Spawn `llama-server` for `model` on `port`. On Linux the child is asked to
/// receive SIGTERM when its parent goes away (`PR_SET_PDEATHSIG`), so even a
/// crash of the app doesn't orphan the server and its loaded model. (The
/// signal tracks the spawning *thread*; that's a long-lived tokio worker, and
/// [`ensure_running`] health-checks and respawns a server that went away.)
fn spawn_server(bin: &Path, model: &Path, port: u16) -> std::io::Result<Child> {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("-m")
        .arg(model)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id();
        // SAFETY: the closure runs between fork and exec and only makes
        // async-signal-safe libc calls (prctl, getppid); no allocation.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The parent died before prctl took effect: don't start.
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }
    cmd.spawn()
}

/// Ensure a `llama-server` is running for the configured model; return its base
/// URL. Reuses a live, healthy server for the same model, else (re)spawns and
/// waits for `/health` to go green. The whole sequence runs under
/// [`LIFECYCLE`], so concurrent callers share one server.
async fn ensure_running() -> Result<String, String> {
    let _lifecycle = LIFECYCLE.lock().await;
    let bin = server_binary().ok_or("llama-server binary not found")?;
    let model = model_path().ok_or("no model selected — download one first")?;

    // Reuse an already-running server for the same model — if it's still alive
    // and answering; a crashed or wedged one is replaced below.
    let reuse = {
        let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_mut()
            .filter(|s| s.model == model)
            .and_then(|s| matches!(s.child.try_wait(), Ok(None)).then_some(s.port))
    };
    if let Some(port) = reuse {
        if healthy(port).await {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
    }

    // Kill (and reap, via Drop) any prior server before starting a new one.
    drop(SERVER.lock().unwrap_or_else(|e| e.into_inner()).take());

    let port = free_port().ok_or("no free port for llama-server")?;
    let child = spawn_server(&bin, &model, port).map_err(|e| format!("spawn llama-server: {e}"))?;
    *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Server { child, port, model });

    // Poll /health until the model has loaded (up to ~30s for a small model),
    // bailing early if the process exits (bad model, missing lib, …).
    for _ in 0..60 {
        if healthy(port).await {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        let exited = SERVER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .is_none_or(|s| !matches!(s.child.try_wait(), Ok(None)));
        if exited {
            drop(SERVER.lock().unwrap_or_else(|e| e.into_inner()).take());
            return Err("llama-server exited during startup".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Don't leave a half-started server holding RAM; the next call retries.
    drop(SERVER.lock().unwrap_or_else(|e| e.into_inner()).take());
    Err("llama-server did not become ready in time".into())
}

/// Generate text from `prompt` via the llama.cpp sidecar, within `params`'
/// token budget.
pub async fn generate(prompt: &str, params: GenParams) -> Result<String, String> {
    let base = ensure_running().await?;
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        content: String,
    }
    let body = serde_json::json!({
        "prompt": prompt,
        "n_predict": params.max_tokens,
        "temperature": params.temperature,
        "stream": false,
    });
    let resp = client()
        .post(format!("{base}/completion"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("llama-server {}", resp.status()));
    }
    let parsed: Resp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed.content.trim().to_string())
}

/// Kill the running sidecar. Called from the app's quit path so the server and
/// its loaded model don't outlive the app.
pub fn shutdown() {
    drop(SERVER.lock().unwrap_or_else(|e| e.into_inner()).take());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_returns_something() {
        assert!(free_port().is_some());
    }

    #[test]
    fn valid_gguf_accepts_gguf_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("model.gguf");
        // Magic + version + tensor count + kv count (all little-endian).
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        std::fs::write(&p, &bytes).unwrap();
        assert!(valid_gguf(&p));
    }

    #[test]
    fn valid_gguf_rejects_wrong_magic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("model.gguf");
        std::fs::write(&p, vec![0u8; GGUF_HEADER_LEN as usize]).unwrap();
        assert!(!valid_gguf(&p));
    }

    #[test]
    fn valid_gguf_rejects_truncated_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("model.gguf");
        // Right magic but shorter than the fixed header — a truncated download.
        std::fs::write(&p, b"GGUF").unwrap();
        assert!(!valid_gguf(&p));
        assert!(!valid_gguf(&dir.path().join("nope.gguf")));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_tracks_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("llama-server");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&p));
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&p));
        assert!(!is_executable(&dir.path().join("missing")));
    }

    #[test]
    fn installed_models_lists_only_gguf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.gguf"), b"x").unwrap();
        std::fs::write(dir.path().join("b.gguf.part"), b"zz").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"y").unwrap();
        assert_eq!(installed_models_in(dir.path()), vec![("a.gguf".into(), 1)]);
        assert!(installed_models_in(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn parse_model_url_requires_https_gguf() {
        let ok = parse_model_url("https://hf.co/x/resolve/main/m-Q4.gguf?download=true").unwrap();
        assert_eq!(ok.name, "m-Q4.gguf");
        assert_eq!(ok.sha256, None);
        assert!(parse_model_url("http://hf.co/m.gguf").is_err());
        assert!(parse_model_url("https://hf.co/m.bin").is_err());
        assert!(parse_model_url("https://hf.co/").is_err());
        assert!(parse_model_url("https://hf.co/%2E%2E%2Fevil.gguf").is_err());
        assert!(parse_model_url("not a url").is_err());

        let hex = "AB".repeat(32);
        let with_sum = parse_model_url(&format!("https://hf.co/m.gguf#sha256={hex}")).unwrap();
        assert_eq!(with_sum.sha256, Some("ab".repeat(32)));
        assert!(parse_model_url("https://hf.co/m.gguf#sha256=xyz").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn materialize_copies_once_and_refreshes_on_new_stamp() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("bundle");
        let dest = root.path().join("bin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("llama-server"), b"v1").unwrap();
        std::fs::write(src.join("libllama.so.1"), b"lib").unwrap();
        std::os::unix::fs::symlink("libllama.so.1", src.join("libllama.so")).unwrap();

        assert!(materialize_into(&src, &dest, "s1"));
        assert_eq!(std::fs::read(dest.join("llama-server")).unwrap(), b"v1");
        let mode = std::fs::metadata(dest.join("llama-server"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        assert!(std::fs::symlink_metadata(dest.join("libllama.so"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(dest.join(BUNDLE_MARKER)).unwrap(),
            "s1"
        );

        // Same stamp: no recopy.
        std::fs::write(src.join("llama-server"), b"v2").unwrap();
        assert!(materialize_into(&src, &dest, "s1"));
        assert_eq!(std::fs::read(dest.join("llama-server")).unwrap(), b"v1");
        // New stamp (upgrade): refreshed.
        assert!(materialize_into(&src, &dest, "s2"));
        assert_eq!(std::fs::read(dest.join("llama-server")).unwrap(), b"v2");
        // A missing marker (interrupted copy) also forces a redo.
        std::fs::remove_file(dest.join(BUNDLE_MARKER)).unwrap();
        std::fs::write(src.join("llama-server"), b"v3").unwrap();
        assert!(materialize_into(&src, &dest, "s2"));
        assert_eq!(std::fs::read(dest.join("llama-server")).unwrap(), b"v3");
        // No staging dir left behind.
        let leftovers: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("bin.staging"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
