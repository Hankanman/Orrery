//! Tauri IPC commands exposed to the frontend.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::git_ops::{self, BranchInfo, CommitInfo, WorktreeInfo};
use crate::inbox::{self, CiStatus, InboxItem, Notification, RemoteRepo};
use crate::model::{AppConfig, GitStatus, Host, HostInfo, Repo};
use crate::{ai, cache, config, forge, launch, oauth, scan};

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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    pub available: bool,
    pub model: Option<String>,
    pub models: Vec<String>,
}

/// Whether local AI is available and which model would be used.
#[tauri::command]
pub async fn ai_status() -> AiStatus {
    let cfg = config::load();
    if !cfg.ai_enabled || !ai::available().await {
        return AiStatus { available: false, model: None, models: Vec::new() };
    }
    let installed = ai::installed_models().await;
    let model = ai::pick_model(&cfg.ai_model, &installed);
    AiStatus {
        available: true,
        model,
        models: installed.into_iter().map(|(name, _)| name).collect(),
    }
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

async fn resolve_ai_model() -> Result<String, String> {
    let cfg = config::load();
    if !cfg.ai_enabled {
        return Err("AI is disabled".into());
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
                match ai::embed(&model, &text).await {
                    Ok(vec) => {
                        cache::store_embedding(&id, &vec);
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
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
