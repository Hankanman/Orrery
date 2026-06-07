//! Tauri IPC commands exposed to the frontend.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::git_ops::{self, BranchInfo, CommitInfo, WorktreeInfo};
use crate::inbox::{self, CiStatus, InboxItem, Notification, RemoteRepo};
use crate::model::{AppConfig, GitStatus, Host, HostInfo, Repo};
use crate::{ai, cache, config, forge, launch, llama, oauth, scan};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutcome {
    pub id: String,
    pub status: Option<GitStatus>,
    pub error: Option<String>,
}

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
    let mut repos = cache::load_repos();
    cache::apply_host_info(&mut repos);
    repos
}

/// Scan the configured roots for repos (runs off the UI thread), refresh the
/// cache, and return the results.
#[tauri::command]
pub async fn scan_repos() -> Result<Vec<Repo>, String> {
    let mut repos = tauri::async_runtime::spawn_blocking(|| {
        let cfg = config::load();
        let favorites = cache::favorites();
        scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &favorites, now_unix())
    })
    .await
    .map_err(|e| e.to_string())?;

    // Carry over persisted host enrichment so a fresh scan keeps last-known
    // visibility/stars until the frontend's enrich pass re-confirms them.
    cache::apply_host_info(&mut repos);
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

/// Reveal the repo's directory in the system file manager.
#[tauri::command]
pub fn open_folder(app: tauri::AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(id, None::<&str>).map_err(|e| e.to_string())
}

/// A live terminal-agent process plus the metadata the dashboard shows.
pub struct AgentEntry {
    pub child: std::process::Child,
    /// The command template it was launched with (e.g. the agent_command).
    pub command: String,
    /// Unix seconds when it was launched.
    pub started_at: i64,
}

/// Tracks live terminal-agent child processes, keyed by repo id (#51).
#[derive(Default)]
pub struct AgentSessions(pub std::sync::Mutex<std::collections::HashMap<String, AgentEntry>>);

/// A running session, serialized for the agent dashboard (#68).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionInfo {
    /// Repo id (absolute path) the agent runs in.
    pub id: String,
    /// OS process id of the spawned terminal.
    pub pid: u32,
    pub command: String,
    pub started_at: i64,
}

/// Drop exited children, calling `try_wait` (and a blocking `wait` on error) so
/// they don't linger as zombies. Shared by the listing commands — listing is
/// also the GC pass.
fn reap(map: &mut std::collections::HashMap<String, AgentEntry>) {
    map.retain(|_, e| match e.child.try_wait() {
        Ok(None) => true,        // still running
        Ok(Some(_)) => false,    // exited, reaped by try_wait
        Err(_) => {
            let _ = e.child.wait(); // best-effort reap before drop
            false
        }
    });
}

#[tauri::command]
pub fn open_agent(id: String, sessions: tauri::State<AgentSessions>) -> Result<(), String> {
    let command = config::load().agent_command;
    let child = launch::spawn(&command, &id)?;
    let entry = AgentEntry { child, command, started_at: now_unix() };
    let mut map = sessions.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut old) = map.insert(id, entry) {
        let _ = old.child.try_wait(); // reap a prior (possibly exited) session
    }
    Ok(())
}

/// Repo ids with a currently-running agent session (reaps exited ones).
#[tauri::command]
pub fn active_agents(sessions: tauri::State<AgentSessions>) -> Vec<String> {
    let mut map = sessions.0.lock().unwrap_or_else(|e| e.into_inner());
    reap(&mut map);
    map.keys().cloned().collect()
}

/// Full session list for the agent dashboard (reaps exited), newest first.
#[tauri::command]
pub fn list_agent_sessions(sessions: tauri::State<AgentSessions>) -> Vec<AgentSessionInfo> {
    let mut map = sessions.0.lock().unwrap_or_else(|e| e.into_inner());
    reap(&mut map);
    let mut out: Vec<AgentSessionInfo> = map
        .iter()
        .map(|(id, e)| AgentSessionInfo {
            id: id.clone(),
            pid: e.child.id(),
            command: e.command.clone(),
            started_at: e.started_at,
        })
        .collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out
}

/// Terminate a repo's agent session (kills the spawned terminal + its agent).
#[tauri::command]
pub fn kill_agent(id: String, sessions: tauri::State<AgentSessions>) -> Result<(), String> {
    let mut map = sessions.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut entry) = map.remove(&id) {
        let _ = entry.child.kill();
        let _ = entry.child.wait(); // reap so no zombie remains
    }
    Ok(())
}

// ── Fleet bulk actions (#63) ───────────────────────────────────────────────

/// Cooperative cancel flag for the in-flight bulk run, checked between repos.
#[derive(Default)]
pub struct BulkCancel(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

/// A batch operation to run across selected repos. Serde-tagged so the
/// frontend sends `{ kind: "runCommand", command: "pnpm install" }` etc.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BulkOp {
    Fetch,
    Pull,
    Stash,
    CheckoutDefault,
    RunCommand { command: String },
}

/// Per-repo result, emitted as a `bulk-progress` event as each repo finishes.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BulkProgress {
    run_id: String,
    id: String,
    /// "ok" | "skipped" | "error"
    status: String,
    detail: String,
}

fn outcome_to_progress(res: Result<git_ops::OpOutcome, String>) -> (String, String) {
    match res {
        Ok(git_ops::OpOutcome::Done(d)) => ("ok".into(), d),
        Ok(git_ops::OpOutcome::Skipped(r)) => ("skipped".into(), r),
        Err(e) => ("error".into(), e),
    }
}

fn run_bulk_one(op: &BulkOp, id: &str) -> (String, String) {
    match op {
        BulkOp::Fetch => match git_ops::fetch(id) {
            Ok(s) if s.behind > 0 => ("ok".into(), format!("{} behind", s.behind)),
            Ok(_) => ("ok".into(), "up to date".into()),
            Err(e) => ("error".into(), e),
        },
        BulkOp::Pull => outcome_to_progress(git_ops::pull(id)),
        BulkOp::Stash => outcome_to_progress(git_ops::stash(id)),
        BulkOp::CheckoutDefault => outcome_to_progress(git_ops::checkout_default(id)),
        BulkOp::RunCommand { command } => match git_ops::run_command(id, command) {
            Ok(r) if r.ok => ("ok".into(), r.output_tail),
            Ok(r) => ("error".into(), format!("exit {}: {}", r.code.unwrap_or(-1), r.output_tail)),
            Err(e) => ("error".into(), e),
        },
    }
}

/// Run `op` across `ids` concurrently, streaming a `bulk-progress` event per
/// repo and a final `bulk-done`. Cooperatively cancellable via `cancel_bulk`.
#[tauri::command]
pub async fn bulk_op(
    app: tauri::AppHandle,
    run_id: String,
    ids: Vec<String>,
    op: BulkOp,
    cancel: tauri::State<'_, BulkCancel>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tauri::Emitter;

    let flag = cancel.0.clone();
    flag.store(false, Ordering::Relaxed); // fresh run

    tauri::async_runtime::spawn_blocking(move || {
        // Repos are independent working trees, so bulk ops parallelize safely;
        // cap concurrency so e.g. several `pnpm install`s don't swamp the box.
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(6)
            .min(ids.len().max(1));
        let next = AtomicUsize::new(0);
        let (op, ids, app, flag, next) = (&op, &ids, &app, &flag, &next);
        let run_id = run_id.as_str();

        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(move || loop {
                    if flag.load(Ordering::Relaxed) {
                        break; // cancelled — stop claiming work
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(id) = ids.get(i) else { break };
                    let (status, detail) = run_bulk_one(op, id);
                    let _ = app.emit(
                        "bulk-progress",
                        BulkProgress { run_id: run_id.to_string(), id: id.clone(), status, detail },
                    );
                });
            }
        });

        let cancelled = flag.load(Ordering::Relaxed);
        let _ = app.emit("bulk-done", serde_json::json!({ "runId": run_id, "cancelled": cancelled }));
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Signal the in-flight bulk run to stop (checked between repos).
#[tauri::command]
pub fn cancel_bulk(cancel: tauri::State<BulkCancel>) {
    cancel.0.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Send a native desktop notification (#50).
#[tauri::command]
pub fn notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

/// Fetch host enrichment (stars/topics/issues/release/visibility) for a repo,
/// cached for 6h. `refresh` bypasses the cache (used by an explicit Rescan) to
/// force a fresh fetch. On network failure, falls back to any stale cache.
#[tauri::command]
pub async fn enrich_repo(host: Host, domain: String, slug: String, refresh: bool) -> Result<HostInfo, String> {
    let now = now_unix();
    if !refresh {
        if let Some(fresh) = cache::cached_host_info(&slug, 6 * 3600, now) {
            return Ok(fresh);
        }
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
    oauth::device_start(&oauth::github_client_id()).await
}

#[tauri::command]
pub async fn github_login_poll(device_code: String) -> Result<oauth::PollResult, String> {
    oauth::device_poll(&oauth::github_client_id(), &device_code).await
}

#[tauri::command]
pub fn github_auth_status() -> bool {
    oauth::github_authed()
}

#[tauri::command]
pub fn github_sign_out() {
    oauth::sign_out();
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    /// Ollama server is reachable at `endpoint`.
    pub reachable: bool,
    /// AI summaries are turned on in config.
    pub enabled: bool,
    /// The Ollama base URL in use.
    pub endpoint: String,
    /// Chat model that would actually be used (preferred if installed, else
    /// the smallest installed model).
    pub model: Option<String>,
    /// Configured embedding model.
    pub embed_model: String,
    /// Whether the configured embedding model is installed.
    pub embed_installed: bool,
    /// Installed model names.
    pub models: Vec<String>,
    /// Reason it's not usable (unreachable / no models), for the UI.
    pub error: Option<String>,
}

/// Connection + model status for the AI settings panel. Reports reachability
/// independently of the summaries toggle, so the user can verify Ollama is
/// connected even with summaries off.
#[tauri::command]
pub async fn ai_status() -> AiStatus {
    let cfg = config::load();

    // The llama.cpp backend reports against the bundled sidecar, not Ollama.
    if is_llama_backend(&cfg) {
        let models: Vec<String> = llama::installed_models().into_iter().map(|(n, _)| n).collect();
        let reachable = ai::available().await; // binary + model both present
        let model = (!cfg.llama_model_path.is_empty())
            .then(|| std::path::Path::new(&cfg.llama_model_path).file_name().map(|f| f.to_string_lossy().into_owned()))
            .flatten();
        let error = if reachable {
            None
        } else if model.is_none() {
            Some("No model downloaded — fetch one in Settings.".into())
        } else {
            Some("llama-server binary not found (config path, app data bin/, or PATH).".into())
        };
        return AiStatus {
            reachable,
            enabled: cfg.ai_enabled,
            endpoint: "llama.cpp (bundled)".into(),
            model,
            embed_model: cfg.embed_model,
            embed_installed: false, // embeddings stay on the Ollama backend
            models,
            error,
        };
    }

    let endpoint = cfg.ollama_host.clone();
    if !ai::available().await {
        return AiStatus {
            reachable: false,
            enabled: cfg.ai_enabled,
            endpoint: endpoint.clone(),
            model: None,
            embed_model: cfg.embed_model,
            embed_installed: false,
            models: Vec::new(),
            error: Some(format!("Ollama not reachable at {endpoint}")),
        };
    }
    let installed = ai::installed_models().await;
    let names: Vec<String> = installed.iter().map(|(name, _)| name.clone()).collect();
    let model = ai::pick_model(&cfg.ai_model, &installed);
    let embed_installed = names.iter().any(|n| n == &cfg.embed_model);
    let error = if names.is_empty() {
        Some("Connected, but no models are installed (run `ollama pull …`)".into())
    } else {
        None
    };
    AiStatus {
        reachable: true,
        enabled: cfg.ai_enabled,
        endpoint,
        model,
        embed_model: cfg.embed_model,
        embed_installed,
        models: names,
        error,
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearResult {
    pub summaries: usize,
    pub embeddings: usize,
}

/// Clear cached AI summaries and embeddings from the SQLite cache.
#[tauri::command]
pub fn clear_ai_cache() -> Result<ClearResult, String> {
    let (summaries, embeddings) = cache::clear_ai()?;
    Ok(ClearResult { summaries, embeddings })
}

/// Pull an Ollama model, emitting `pull-progress` events ({model, status,
/// percent}) so the UI can show a progress bar. Resolves when the pull finishes.
#[tauri::command]
pub async fn pull_model(app: tauri::AppHandle, model: String) -> Result<(), String> {
    use tauri::Emitter;
    let name = model.clone();
    ai::pull(&model, move |status, completed, total| {
        let percent = if total > 0 { (completed.saturating_mul(100) / total) as u32 } else { 0 };
        let _ = app.emit(
            "pull-progress",
            serde_json::json!({ "model": name, "status": status, "percent": percent }),
        );
    })
    .await
}

/// Default GGUF for the llama.cpp backend: tiny, instruction-tuned, ~400 MB.
const DEFAULT_LLAMA_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf";

/// Download a GGUF model for the llama.cpp backend into the app data dir,
/// emitting `llama-download-progress` ({completed, total}) events, then point
/// config at it. Empty `url` uses the default tiny model (#21).
#[tauri::command]
pub async fn download_llama_model(app: tauri::AppHandle, url: String) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::io::Write;
    use tauri::Emitter;

    let url = if url.trim().is_empty() { DEFAULT_LLAMA_MODEL_URL.to_string() } else { url.trim().to_string() };
    let filename = url
        .rsplit('/')
        .next()
        .filter(|s| s.ends_with(".gguf"))
        .ok_or("URL must point to a .gguf file")?
        .to_string();
    let dir = llama::models_dir().ok_or("no data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&filename);
    // Download to a .part file so an interrupted download can't masquerade as a
    // complete model.
    let tmp = dir.join(format!("{filename}.part"));

    let resp = reqwest::Client::new().get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        let _ = app.emit("llama-download-progress", serde_json::json!({ "completed": downloaded, "total": total }));
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;

    let mut cfg = config::load();
    cfg.llama_model_path = dest.to_string_lossy().into_owned();
    config::save(&cfg)?;
    Ok(cfg.llama_model_path)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTest {
    pub chat_ok: bool,
    pub embed_ok: bool,
    pub ms: u64,
    pub error: Option<String>,
}

/// End-to-end check: actually run a tiny generation and an embedding against
/// the configured models, so the user can confirm AI is *working*, not just
/// reachable. Returns which legs passed and the round-trip time.
#[tauri::command]
pub async fn ai_test() -> AiTest {
    let cfg = config::load();
    let start = std::time::Instant::now();

    let installed = ai::installed_models().await;
    if installed.is_empty() {
        return AiTest {
            chat_ok: false,
            embed_ok: false,
            ms: 0,
            error: Some(format!("Ollama not reachable at {} (or no models)", cfg.ollama_host)),
        };
    }

    let mut error = None;
    let chat_ok = match ai::pick_model(&cfg.ai_model, &installed) {
        Some(model) => match ai::generate(&model, "Reply with the single word: ok").await {
            Ok(_) => true,
            Err(e) => {
                error = Some(format!("chat: {e}"));
                false
            }
        },
        None => false,
    };
    // Embeddings are intentionally Ollama-only; on the llama.cpp backend the
    // embed leg is N/A, not a failure — skip it so a working chat test reads green.
    let embed_ok = if is_llama_backend(&cfg) {
        false
    } else {
        match ai::embed(&cfg.embed_model, "orrery connectivity test").await {
            Ok(_) => true,
            Err(e) => {
                error.get_or_insert(format!("embed: {e}"));
                false
            }
        }
    };

    AiTest { chat_ok, embed_ok, ms: start.elapsed().as_millis() as u64, error }
}

/// Summarize a repo (cached while its last commit is unchanged).
#[tauri::command]
pub async fn summarize_repo(repo: Repo, refresh: bool) -> Result<String, String> {
    if !refresh {
        if let Some(cached) = cache::cached_summary(&repo.id, repo.last_commit_unix) {
            return Ok(cached);
        }
    }
    let cfg = config::load();
    if !cfg.ai_enabled {
        return Err("AI summaries are disabled".into());
    }
    let installed = ai::installed_models().await;
    let model = ai::pick_model(&cfg.ai_model, &installed).ok_or("no Ollama model available")?;
    let summary = ai::generate(&model, &ai::summary_prompt(&repo)).await?;
    if !summary.is_empty() {
        cache::store_summary(&repo.id, &summary, repo.last_commit_unix);
    }
    Ok(summary)
}

// ── Phase 5: command center ────────────────────────────────────────────────

/// Fetch many repos in parallel batches; returns refreshed status per repo.
#[tauri::command]
pub async fn fetch_all(ids: Vec<String>) -> Vec<FetchOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        let results = std::sync::Mutex::new(Vec::with_capacity(ids.len()));
        for group in ids.chunks(8) {
            std::thread::scope(|scope| {
                for id in group {
                    scope.spawn(|| {
                        let outcome = match git_ops::fetch(id) {
                            Ok(status) => FetchOutcome { id: id.clone(), status: Some(status), error: None },
                            Err(e) => FetchOutcome { id: id.clone(), status: None, error: Some(e) },
                        };
                        results.lock().unwrap_or_else(|e| e.into_inner()).push(outcome);
                    });
                }
            });
        }
        results.into_inner().unwrap_or_else(|e| e.into_inner())
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub async fn fetch_repo(id: String) -> Result<GitStatus, String> {
    tauri::async_runtime::spawn_blocking(move || git_ops::fetch(&id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_branches(id: String) -> Result<Vec<BranchInfo>, String> {
    git_ops::branches(&id)
}

#[tauri::command]
pub fn switch_branch(id: String, name: String) -> Result<(), String> {
    git_ops::switch_branch(&id, &name)
}

#[tauri::command]
pub fn prune_branches(id: String) -> Result<Vec<String>, String> {
    git_ops::prune_branches(&id)
}

/// Prunable branches per repo, across the given paths — for the branch janitor.
/// Repos with nothing to prune are omitted. Runs off the UI thread (#64).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPrunable {
    pub id: String,
    pub branches: Vec<BranchInfo>,
}

#[tauri::command]
pub async fn prunable_branches(paths: Vec<String>) -> Vec<RepoPrunable> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|id| match git_ops::prunable(&id) {
                Ok(branches) if !branches.is_empty() => Some(RepoPrunable { id, branches }),
                _ => None,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub fn list_worktrees(id: String) -> Result<Vec<WorktreeInfo>, String> {
    git_ops::worktrees(&id)
}

#[tauri::command]
pub fn add_worktree(id: String, name: String, dest: String) -> Result<String, String> {
    git_ops::add_worktree(&id, &name, &dest)
}

#[tauri::command]
pub fn remove_worktree(id: String, name: String) -> Result<(), String> {
    git_ops::remove_worktree(&id, &name)
}

#[tauri::command]
pub fn repo_log(id: String, limit: usize) -> Result<Vec<CommitInfo>, String> {
    git_ops::recent_log(&id, limit)
}

/// Cached contribution-graph result. Keyed by the repo set (`sig`) and the day
/// it was computed for, with a short TTL — walking every repo's history is the
/// expensive part, and commits don't change second-to-second.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedContrib {
    computed_at: i64,
    today: i64,
    sig: u64,
    days: Vec<git_ops::DayCount>,
}

/// Order-independent fingerprint of the repo set, so adding/removing a repo
/// invalidates the cache but reordering doesn't.
fn ids_signature(ids: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in sorted {
        id.hash(&mut h);
    }
    h.finish()
}

/// Daily commit counts (the user's own) across the given repos for the trailing
/// ~53 weeks — the data behind Mission Control's contribution graph. Cached for
/// 10 minutes per repo set so revisiting the view is instant.
#[tauri::command]
pub async fn contribution_graph(ids: Vec<String>) -> Vec<git_ops::DayCount> {
    const TTL: i64 = 600;
    let now = now_unix();
    let today = now.div_euclid(86_400);
    let sig = ids_signature(&ids);

    if let Some(c) = cache::get_meta("contrib_graph").and_then(|r| serde_json::from_str::<CachedContrib>(&r).ok()) {
        if c.sig == sig && c.today == today && now - c.computed_at < TTL {
            return c.days;
        }
    }

    let days = tauri::async_runtime::spawn_blocking(move || {
        let since = today - 7 * 53; // a little over a year, week-aligned by the UI
        git_ops::contributions(&ids, since)
    })
    .await
    .unwrap_or_default();

    if let Ok(blob) = serde_json::to_string(&CachedContrib { computed_at: now, today, sig, days: days.clone() }) {
        cache::set_meta("contrib_graph", &blob);
    }
    days
}

#[tauri::command]
pub fn repo_diff(id: String) -> Result<String, String> {
    git_ops::working_diff(&id)
}

/// Staged diff (index vs HEAD) — exactly what a commit would record.
#[tauri::command]
pub fn repo_staged_diff(id: String) -> Result<String, String> {
    git_ops::staged_diff(&id)
}

/// Raw README markdown for the detail drawer.
#[tauri::command]
pub fn repo_readme(id: String) -> Option<String> {
    let candidates = ["README.md", "Readme.md", "readme.md", "README.markdown", "README"];
    candidates
        .iter()
        .find_map(|name| std::fs::read_to_string(std::path::Path::new(&id).join(name)).ok())
}

// ── Phase 6: local-AI superpowers ──────────────────────────────────────────

fn is_llama_backend(cfg: &AppConfig) -> bool {
    matches!(cfg.ai_backend.as_str(), "llamaCpp" | "llama_cpp" | "llamacpp")
}

async fn resolve_ai_model() -> Result<String, String> {
    let cfg = config::load();
    if !cfg.ai_enabled {
        return Err("AI is disabled".into());
    }
    // The llama.cpp backend serves the configured GGUF; `generate` ignores the
    // model string, so any non-empty value just signals "engine is ready".
    if is_llama_backend(&cfg) {
        return if ai::available().await {
            Ok(cfg.llama_model_path)
        } else {
            Err("llama.cpp engine or model not available".into())
        };
    }
    let installed = ai::installed_models().await;
    ai::pick_model(&cfg.ai_model, &installed).ok_or_else(|| "no Ollama model available".to_string())
}

/// Generate a commit message from the staged diff (#39).
#[tauri::command]
pub async fn generate_commit_message(id: String) -> Result<String, String> {
    let id2 = id.clone();
    let diff = tauri::async_runtime::spawn_blocking(move || git_ops::staged_diff(&id2))
        .await
        .map_err(|e| e.to_string())??;
    if diff.trim().is_empty() {
        return Err("Nothing staged — `git add` your changes first.".into());
    }
    let model = resolve_ai_model().await?;
    ai::generate(&model, &ai::commit_prompt(&diff)).await
}

/// Commit the staged changes with the given message (#39).
#[tauri::command]
pub fn commit_staged(id: String, message: String) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("empty commit message".into());
    }
    git_ops::commit(&id, message.trim())
}

/// Generate a changelog / PR description from recent commits (#42).
#[tauri::command]
pub async fn generate_changelog(id: String, limit: usize) -> Result<String, String> {
    let id2 = id.clone();
    let commits = tauri::async_runtime::spawn_blocking(move || git_ops::recent_log(&id2, limit))
        .await
        .map_err(|e| e.to_string())??;
    if commits.is_empty() {
        return Err("no commits to summarize".into());
    }
    let lines: Vec<String> = commits.iter().map(|c| format!("- {} ({})", c.summary, c.id)).collect();
    let model = resolve_ai_model().await?;
    ai::generate(&model, &ai::changelog_prompt(&lines)).await
}

// ── Per-repo notes + "resume where I left off" (#69) ───────────────────────

/// AI-free summary of what landed since the user last looked, returned to the
/// drawer. `text` is the AI catch-up (empty when AI is off or nothing changed);
/// `commitCount` lets the UI show "N commits since you last looked" regardless.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSummary {
    pub text: String,
    pub commit_count: usize,
    /// True the first time a repo is opened (no prior cursor) — nothing to catch
    /// up on yet, so the UI shows a neutral state rather than "0 commits".
    pub first_visit: bool,
}

/// The markdown note pinned to a repo.
#[tauri::command]
pub fn get_note(id: String) -> String {
    cache::note(&id)
}

/// Persist a repo's markdown note.
#[tauri::command]
pub fn set_note(id: String, text: String) -> Result<(), String> {
    cache::set_note(&id, &text)
}

/// Record that the user has caught up to the repo's current HEAD.
#[tauri::command]
pub fn mark_seen(id: String) -> Result<(), String> {
    let sha = git_ops::head_sha(&id)?;
    cache::set_seen(&id, &sha, now_unix())
}

/// Summarize what changed in a repo since the user last looked. Does NOT advance
/// the cursor — the frontend calls `mark_seen` once the user has seen this.
#[tauri::command]
pub async fn resume_summary(id: String) -> Result<ResumeSummary, String> {
    let Some(since) = cache::seen_sha(&id) else {
        return Ok(ResumeSummary { text: String::new(), commit_count: 0, first_visit: true });
    };
    let id2 = id.clone();
    let commits = tauri::async_runtime::spawn_blocking(move || git_ops::log_since_sha(&id2, &since, 50))
        .await
        .map_err(|e| e.to_string())??;
    if commits.is_empty() {
        return Ok(ResumeSummary { text: String::new(), commit_count: 0, first_visit: false });
    }
    let commit_count = commits.len();
    // The catch-up text is best-effort: if AI is off/unreachable, the count
    // alone is still useful, so we don't fail the whole command.
    let text = match resolve_ai_model().await {
        Ok(model) => {
            let name = cache::load_repos()
                .into_iter()
                .find(|r| r.id == id)
                .map(|r| r.display_name)
                .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id).to_string());
            let lines: Vec<String> = commits.iter().map(|c| format!("- {} ({})", c.summary, c.id)).collect();
            ai::generate(&model, &ai::resume_prompt(&name, &lines)).await.unwrap_or_default()
        }
        Err(_) => String::new(),
    };
    Ok(ResumeSummary { text, commit_count, first_visit: false })
}

/// Build/refresh the semantic-search embedding index for the given repos (#41).
#[tauri::command]
pub async fn index_repos(repos: Vec<Repo>) -> usize {
    let model = config::load().embed_model;
    let mut count = 0usize;
    // Embed in small concurrent batches rather than one-at-a-time.
    for chunk in repos.chunks(6) {
        let done = futures_util::future::join_all(chunk.iter().map(|repo| {
            let model = model.clone();
            let id = repo.id.clone();
            let text = format!(
                "{} {} {} {}",
                repo.display_name,
                repo.slug.as_deref().unwrap_or(""),
                repo.language.as_deref().unwrap_or(""),
                repo.description.as_deref().unwrap_or("")
            );
            async move {
                // Embedding only changes when the indexed text does. Skip repos
                // whose text signature is unchanged — this turns rescans (and
                // every file-watch refresh) from N Ollama calls into N cheap
                // meta lookups.
                let key = format!("embed_sig:{id}");
                let sig = text_signature(&text);
                if cache::get_meta(&key).as_deref() == Some(sig.as_str()) {
                    return false;
                }
                match ai::embed(&model, &text).await {
                    Ok(vec) => {
                        cache::store_embedding(&id, &vec);
                        cache::set_meta(&key, &sig);
                        true
                    }
                    Err(_) => false,
                }
            }
        }))
        .await;
        count += done.into_iter().filter(|x| *x).count();
    }
    count
}

/// Stable hex fingerprint of a repo's embedding text, for skip-if-unchanged.
fn text_signature(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:x}", h.finish())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
}

/// Cross-repo content search via ripgrep (#65). Runs off the UI thread.
#[tauri::command]
pub async fn search_code(query: String, paths: Vec<String>) -> Result<Vec<crate::search::SearchHit>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::search::search(&query, &paths, 60))
        .await
        .map_err(|e| e.to_string())?
}

/// Semantic search over the embedding index; returns ranked repo ids (#41).
#[tauri::command]
pub async fn semantic_search(query: String) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let model = config::load().embed_model;
    let q = ai::embed(&model, &query).await?;
    let mut hits: Vec<SearchHit> = cache::load_embeddings()
        .into_iter()
        .map(|(id, v)| SearchHit { id, score: ai::cosine(&q, &v) })
        .filter(|h| h.score > 0.35)
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(8);
    Ok(hits)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub text: String,
    pub repo_count: usize,
}

/// A short AI digest of what changed across repos since the last visit (#40).
#[tauri::command]
pub async fn daily_briefing(repos: Vec<Repo>) -> Result<Briefing, String> {
    let now = now_unix();
    // First run (no stored timestamp): look back a week rather than dumping
    // every repo ever touched.
    let since = cache::get_meta("last_open")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(now - 7 * 24 * 3600);

    let mut recent: Vec<&Repo> = repos.iter().filter(|r| r.last_commit_unix > since).collect();
    recent.sort_by(|a, b| b.last_commit_unix.cmp(&a.last_commit_unix));
    recent.truncate(12);

    if recent.is_empty() {
        cache::set_meta("last_open", &now.to_string());
        return Ok(Briefing { text: "Nothing new since your last visit.".into(), repo_count: 0 });
    }

    let lines: Vec<String> = recent
        .iter()
        .map(|r| {
            format!(
                "- {} ({}): {} uncommitted, {} ahead / {} behind",
                r.display_name,
                r.language.as_deref().unwrap_or("?"),
                r.git.dirty,
                r.git.ahead,
                r.git.behind
            )
        })
        .collect();
    let count = lines.len();
    let model = resolve_ai_model().await?;
    let text = ai::generate(&model, &ai::briefing_prompt(&lines)).await?;
    // Only advance the window once we've actually produced a briefing.
    cache::set_meta("last_open", &now.to_string());
    Ok(Briefing { text, repo_count: count })
}

// ── Phase 7: cross-host dev inbox ──────────────────────────────────────────

/// Open PRs / review requests / assigned issues across hosts (#43, #44).
#[tauri::command]
pub async fn get_inbox() -> Result<Vec<InboxItem>, String> {
    inbox::github_inbox().await
}

/// Host notifications (#46).
#[tauri::command]
pub async fn get_notifications() -> Result<Vec<Notification>, String> {
    inbox::github_notifications().await
}

/// CI status for a repo's default branch (#45).
#[tauri::command]
pub async fn ci_status(slug: String) -> Result<CiStatus, String> {
    inbox::github_ci(&slug).await
}

/// Starred repos to browse (#25).
#[tauri::command]
pub async fn list_starred() -> Result<Vec<RemoteRepo>, String> {
    inbox::github_starred().await
}

/// Open PRs for a repo with checks / review / mergeable + allowed merge
/// methods (#67). Cached briefly since the drawer re-opens often; `refresh`
/// (used after a merge/approve) forces a re-fetch.
#[tauri::command]
pub async fn pr_panel(slug: String, refresh: bool) -> Result<inbox::PrPanel, String> {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Cached {
        at: i64,
        panel: inbox::PrPanel,
    }
    // Short TTL — PR check/review state is fast-moving, but the cache still
    // saves a GraphQL round-trip when flicking the drawer open and shut.
    const TTL: i64 = 60;
    let key = format!("pr:{slug}");
    let now = now_unix();
    if !refresh {
        if let Some(c) = cache::get_meta(&key).and_then(|r| serde_json::from_str::<Cached>(&r).ok()) {
            if now - c.at < TTL {
                return Ok(c.panel);
            }
        }
    }
    let panel = inbox::github_prs(&slug).await?;
    if let Ok(blob) = serde_json::to_string(&Cached { at: now, panel: panel.clone() }) {
        cache::set_meta(&key, &blob);
    }
    Ok(panel)
}

/// Squash/rebase/merge a PR (#67). Branch protection is enforced by GitHub.
#[tauri::command]
pub async fn merge_pr(slug: String, number: u64, method: String) -> Result<(), String> {
    inbox::github_merge_pr(&slug, number, &method).await
}

/// Approve a PR (#67).
#[tauri::command]
pub async fn approve_pr(slug: String, number: u64) -> Result<(), String> {
    inbox::github_approve_pr(&slug, number).await
}

/// Activity feed (starred releases + followed-user activity). Cached 30 min
/// since it hits the GitHub API; `refresh` forces a re-fetch.
#[tauri::command]
pub async fn get_feed(refresh: bool) -> Result<Vec<inbox::FeedItem>, String> {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Cached {
        at: i64,
        items: Vec<inbox::FeedItem>,
    }
    const TTL: i64 = 1800;
    let now = now_unix();
    if !refresh {
        if let Some(c) = cache::get_meta("feed").and_then(|r| serde_json::from_str::<Cached>(&r).ok()) {
            if now - c.at < TTL {
                return Ok(c.items);
            }
        }
    }
    let items = inbox::github_feed().await?;
    if let Ok(blob) = serde_json::to_string(&Cached { at: now, items: items.clone() }) {
        cache::set_meta("feed", &blob);
    }
    Ok(items)
}

/// Reject a directory name that's empty or would escape its root.
fn safe_dir_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err("invalid project name".into());
    }
    Ok(name.to_string())
}

/// Create a new project in a chosen root: `git init` + optional template copy,
/// remote, and first commit (#71). Returns the new working directory.
#[tauri::command]
pub async fn init_repo(
    dest_root: String,
    name: String,
    template: Option<String>,
    remote: Option<String>,
    first_commit: Option<String>,
) -> Result<String, String> {
    let name = safe_dir_name(&name)?;
    let dest = scan::expand(&dest_root).join(&name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let dest_str = dest.to_string_lossy().into_owned();
    // Empty optional strings from the form mean "not set".
    let template = template.filter(|s| !s.trim().is_empty()).map(|s| scan::expand(&s).to_string_lossy().into_owned());
    let remote = remote.filter(|s| !s.trim().is_empty());
    let first_commit = first_commit.filter(|s| !s.trim().is_empty());
    tauri::async_runtime::spawn_blocking(move || {
        git_ops::init(&dest_str, &name, template.as_deref(), remote.as_deref(), first_commit.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Clone a repo into a configured root and return its working dir (#26).
#[tauri::command]
pub async fn clone_repo(url: String, dest_root: String) -> Result<String, String> {
    let name = url
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .to_string();
    // Guard against a name that would escape the root (path traversal).
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err("could not derive a safe directory name from the URL".into());
    }
    let dest = scan::expand(&dest_root).join(&name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let dest_str = dest.to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || git_ops::clone(&url, &dest_str))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Opt-in probe: times index_repos cold (forced re-embed) vs warm (skip).
    ///   cargo test -p orrery --lib perf_index -- --ignored --nocapture
    #[test]
    #[ignore]
    fn perf_index() {
        let cfg = config::load();
        let repos = scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &cache::favorites(), now_unix());
        // Force cold by invalidating signatures for this run.
        for r in &repos {
            cache::set_meta(&format!("embed_sig:{}", r.id), "force-cold");
        }
        eprintln!("\n── index_repos perf ({} repos) ──", repos.len());
        for label in ["cold", "warm"] {
            let r = repos.clone();
            let t = Instant::now();
            let n = tauri::async_runtime::block_on(index_repos(r));
            eprintln!("{label}: {:.1} ms  ({n} embedded)", t.elapsed().as_secs_f64() * 1000.0);
        }
        eprintln!("─────────────────────────────────\n");
    }
}
