//! Bundled llama.cpp backend (#21). Spawns a local `llama-server` sidecar and
//! talks to its native HTTP API (`/health`, `/completion`). The engine binary
//! is discovered at runtime — a configured path, then the app data dir's
//! `bin/`, then `PATH` — so the app degrades to "unavailable" (rather than
//! failing to build/launch) when it isn't present. Release builds ship a
//! `llama-server` as a bundled resource (fetched in CI); on first use it's
//! unpacked into the app data `bin/` so it's found by the same lookup. Models
//! are GGUF files in the app data dir. Generation only; embeddings stay on the
//! Ollama path.

use std::path::PathBuf;
use std::process::Child;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

/// A running `llama-server` for a specific model. Process-lifetime singleton —
/// like the shared HTTP clients — so successive generate calls reuse it.
struct Server {
    child: Child,
    port: u16,
    model: PathBuf,
}

static SERVER: LazyLock<Mutex<Option<Server>>> = LazyLock::new(|| Mutex::new(None));

/// The bundled llama runtime dir (`$RESOURCE/llama-runtime`), recorded once at
/// startup — resolving it needs an `AppHandle`, which the discovery path lacks.
/// Empty (or absent) in dev/source builds; populated by the release CI fetch.
static BUNDLED_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record the bundled llama runtime dir. Called from the Tauri setup hook with
/// the resolved resource path; a no-op if called more than once.
pub fn set_bundled_dir(dir: PathBuf) {
    let _ = BUNDLED_DIR.set(dir);
}

/// Copy a shipped llama runtime into the writable app-data `bin/` on first use,
/// marking the server executable. We run from app-data rather than straight out
/// of the bundle so it works even when the bundle is a read-only mount (the
/// AppImage squashfs) and so the executable bit is guaranteed regardless of how
/// the bundler copied the resource. Idempotent and cheap: a single stat once the
/// binary is in place. Does nothing when no runtime was bundled.
fn materialize_bundled() {
    let Some(src) = BUNDLED_DIR
        .get()
        .filter(|d| d.join("llama-server").is_file())
    else {
        return;
    };
    let Some(dest) = data_dir().map(|d| d.join("bin")) else {
        return;
    };
    let server = dest.join("llama-server");
    if server.is_file() {
        return; // already materialized
    }
    if std::fs::create_dir_all(&dest).is_err() {
        return;
    }
    // Copy the binary and its co-located shared libraries ($ORIGIN rpath).
    if let Ok(entries) = std::fs::read_dir(src) {
        for e in entries.flatten() {
            let _ = std::fs::copy(e.path(), dest.join(e.file_name()));
        }
    }
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(&server) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&server, perms);
    }
}

fn client() -> reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
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
    // Unpack a bundled runtime into app-data bin/ on first use, so the check
    // below finds it just like a user-installed binary.
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

/// True when both the engine binary and a model file are present — enough to
/// serve generation. (Doesn't spawn the server; that happens lazily.)
pub fn available() -> bool {
    server_binary().is_some() && model_path().is_some()
}

/// Downloaded GGUF models as (filename, size_bytes).
pub fn installed_models() -> Vec<(String, u64)> {
    let Some(dir) = models_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
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

/// Ensure a `llama-server` is running for the configured model; return its base
/// URL. Reuses a live server for the same model, else (re)spawns and waits for
/// `/health` to go green.
async fn ensure_running() -> Result<String, String> {
    let bin = server_binary().ok_or("llama-server binary not found")?;
    let model = model_path().ok_or("no model selected — download one first")?;

    // Reuse an already-running server for the same model.
    {
        let guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            if s.model == model {
                return Ok(format!("http://127.0.0.1:{}", s.port));
            }
        }
    }

    let port = free_port().ok_or("no free port for llama-server")?;
    let child = std::process::Command::new(&bin)
        .arg("-m")
        .arg(&model)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn llama-server: {e}"))?;

    // Replace (and reap) any prior server for a different model.
    {
        let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut old) = guard.take() {
            let _ = old.child.kill();
            let _ = old.child.wait();
        }
        *guard = Some(Server { child, port, model });
    }

    // Poll /health until the model has loaded (up to ~30s for a small model).
    let base = format!("http://127.0.0.1:{port}");
    for _ in 0..60 {
        let ok = client()
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if ok {
            return Ok(base);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("llama-server did not become ready in time".into())
}

/// Generate text from `prompt` via the llama.cpp sidecar.
pub async fn generate(prompt: &str) -> Result<String, String> {
    let base = ensure_running().await?;
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        content: String,
    }
    let body = serde_json::json!({
        "prompt": prompt,
        "n_predict": 120,
        "temperature": 0.2,
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

/// Kill the running sidecar (called on app exit so it isn't orphaned).
pub fn shutdown() {
    let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut s) = guard.take() {
        let _ = s.child.kill();
        let _ = s.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_returns_something() {
        assert!(free_port().is_some());
    }

    #[test]
    fn installed_models_lists_only_gguf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.gguf"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"y").unwrap();
        let found: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("gguf"))
                    .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
            })
            .collect();
        assert_eq!(found, vec!["a.gguf"]);
    }
}
