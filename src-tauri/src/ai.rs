//! Local AI summaries (#23). A `Backend` seam selects the inference engine:
//! the Ollama HTTP path (default, GPU-accelerated) or the bundled llama.cpp
//! sidecar (#21). The public entry points (`available`, `installed_models`,
//! `generate`, `embed`, `pull`) dispatch on the configured backend; everything
//! backend-agnostic (prompts, `pick_model`, `cosine`) stays free-standing.
//!
//! Everything degrades gracefully: if the active backend isn't reachable,
//! summaries are simply unavailable and the UI shows nothing.

use serde::Deserialize;

use crate::model::Repo;

/// The inference engine serving AI features, from `config.ai_backend`.
enum Backend {
    Ollama,
    LlamaCpp,
}

fn active_backend() -> Backend {
    match crate::config::load().ai_backend.as_str() {
        "llamaCpp" | "llama_cpp" | "llamacpp" => Backend::LlamaCpp,
        _ => Backend::Ollama,
    }
}

// ── Backend-dispatching entry points ───────────────────────────────────────
// Each delegates to the active backend. The llama.cpp arms are stubs until #21
// PR2 lands its sidecar implementation — they report "unavailable" so the UI
// degrades exactly as it does when Ollama is down.

/// Is the active backend reachable?
pub async fn available() -> bool {
    match active_backend() {
        Backend::Ollama => ollama_available().await,
        Backend::LlamaCpp => false,
    }
}

/// Installed/available models as (name, size_bytes) for the active backend.
pub async fn installed_models() -> Vec<(String, u64)> {
    match active_backend() {
        Backend::Ollama => ollama_installed_models().await,
        Backend::LlamaCpp => Vec::new(),
    }
}

/// Generate text from `prompt` using `model` on the active backend.
pub async fn generate(model: &str, prompt: &str) -> Result<String, String> {
    match active_backend() {
        Backend::Ollama => ollama_generate(model, prompt).await,
        Backend::LlamaCpp => Err("llama.cpp backend is not available yet".into()),
    }
}

/// Embed `text` with `model` on the active backend.
pub async fn embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    match active_backend() {
        Backend::Ollama => ollama_embed(model, text).await,
        Backend::LlamaCpp => Err("llama.cpp backend is not available yet".into()),
    }
}

/// Download/prepare `model` on the active backend, reporting progress.
pub async fn pull(model: &str, on_progress: impl FnMut(&str, u64, u64)) -> Result<(), String> {
    match active_backend() {
        Backend::Ollama => ollama_pull(model, on_progress).await,
        Backend::LlamaCpp => Err("llama.cpp backend is not available yet".into()),
    }
}

/// Base URL of the Ollama server, from config (default http://localhost:11434).
/// config::load() is cached, so this is cheap to call per request.
fn base() -> String {
    crate::config::load().ollama_host
}

/// Shared HTTP client so the many Ollama calls (status, per-repo summaries and
/// embeddings) reuse one connection pool. reqwest::Client is Arc-backed.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(reqwest::Client::new);
    CLIENT.clone()
}

/// Is a local Ollama server reachable?
async fn ollama_available() -> bool {
    client()
        .get(format!("{}/api/version", base()))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Installed Ollama models as (name, size_bytes).
async fn ollama_installed_models() -> Vec<(String, u64)> {
    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        name: String,
        #[serde(default)]
        size: u64,
    }
    let resp = client().get(format!("{}/api/tags", base())).send().await;
    match resp {
        Ok(r) => match r.json::<Tags>().await {
            Ok(t) => t.models.into_iter().map(|m| (m.name, m.size)).collect(),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

/// Heuristic: is this an embedding-only model? Such models reject `/api/generate`
/// (Ollama 400), so they must never be picked as a chat fallback.
pub fn is_embedding_model(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("embed") || n.contains("minilm") || n.starts_with("bge") || n.contains("/bge")
}

/// Choose the chat model: the preferred one if installed, otherwise the smallest
/// installed model that can actually generate (embedding models are excluded —
/// picking one would 400 on /api/generate). Pure for testing.
pub fn pick_model(preferred: &str, available: &[(String, u64)]) -> Option<String> {
    if available.iter().any(|(name, _)| name == preferred) {
        return Some(preferred.to_string());
    }
    available
        .iter()
        .filter(|(name, _)| !is_embedding_model(name))
        .min_by_key(|(_, size)| *size)
        .map(|(name, _)| name.clone())
}

/// Build the summarization prompt from repo metadata. Pure for testing.
pub fn summary_prompt(repo: &Repo) -> String {
    let git = &repo.git;
    let changes = if git.dirty > 0 {
        format!("{} uncommitted change(s)", git.dirty)
    } else {
        "a clean tree".to_string()
    };
    format!(
        "You summarize a code repository in ONE concise, factual sentence for a developer dashboard. \
No preamble, no markdown, max 24 words.\n\n\
Name: {name}\n\
Language: {lang}\n\
Description: {desc}\n\
State: branch {branch}, {changes}, {ahead} ahead / {behind} behind upstream.\n\n\
Summary:",
        name = repo.display_name,
        lang = repo.language.as_deref().unwrap_or("unknown"),
        desc = repo.description.as_deref().unwrap_or("(none)"),
        branch = git.branch,
        changes = changes,
        ahead = git.ahead,
        behind = git.behind,
    )
}

/// Generate a summary via Ollama.
///
/// Tries normally first. If the model returns an empty response — the signature
/// of a "thinking" model (qwen3, gemma3, …) that spent its whole token budget
/// on hidden reasoning — it retries once with `think:false`. This way the
/// `think` field is only ever sent to a model that actually needs it, so plain
/// models that might reject the field are never hit with it.
async fn ollama_generate(model: &str, prompt: &str) -> Result<String, String> {
    let first = generate_once(model, prompt, false).await?;
    if !first.is_empty() {
        return Ok(first);
    }
    generate_once(model, prompt, true).await
}

async fn generate_once(model: &str, prompt: &str, suppress_think: bool) -> Result<String, String> {
    #[derive(Deserialize)]
    struct GenResp {
        #[serde(default)]
        response: String,
    }
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": { "temperature": 0.2, "num_predict": 120 }
    });
    if suppress_think {
        body["think"] = serde_json::Value::Bool(false);
    }
    let resp = client()
        .post(format!("{}/api/generate", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }
    let parsed: GenResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed.response.trim().to_string())
}

/// Pull a model via Ollama (`/api/pull`), streaming NDJSON progress. `on_progress`
/// is called with (status, completed_bytes, total_bytes) for each update — kept
/// as a callback so this module stays free of Tauri's event machinery.
async fn ollama_pull(model: &str, mut on_progress: impl FnMut(&str, u64, u64)) -> Result<(), String> {
    use futures_util::StreamExt;

    // Ollama's /api/pull needs an explicit tag; default to :latest when none.
    let model = if model.contains(':') { model.to_string() } else { format!("{model}:latest") };
    let model = model.as_str();

    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        status: String,
        #[serde(default)]
        completed: u64,
        #[serde(default)]
        total: u64,
        #[serde(default)]
        error: Option<String>,
    }

    let body = serde_json::json!({ "model": model, "stream": true });
    let resp = client()
        .post(format!("{}/api/pull", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }

    // Ollama streams newline-delimited JSON objects; buffer partial lines.
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let trimmed = &line[..line.len().saturating_sub(1)];
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(l) = serde_json::from_slice::<Line>(trimmed) {
                if let Some(err) = l.error {
                    return Err(err);
                }
                on_progress(&l.status, l.completed, l.total);
            }
        }
    }
    Ok(())
}

/// Build a readable error from a non-2xx Ollama response, surfacing its
/// `{"error": "..."}` body (e.g. "… does not support generate") instead of a
/// bare status code.
async fn ollama_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    if detail.is_empty() {
        format!("Ollama API {status}")
    } else {
        format!("Ollama API {status}: {detail}")
    }
}

/// Embed `text` with an embedding model via Ollama (`/api/embed`).
async fn ollama_embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        embeddings: Vec<Vec<f32>>,
    }
    let body = serde_json::json!({ "model": model, "input": text });
    let resp = client()
        .post(format!("{}/api/embed", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|e| e.to_string())?;
    parsed
        .embeddings
        .into_iter()
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "model returned no embedding".to_string())
}

/// Cosine similarity of two equal-length vectors (0 if mismatched/empty).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Prompt to write a Conventional Commit message from a staged diff.
pub fn commit_prompt(diff: &str) -> String {
    format!(
        "Write a single Conventional Commit message (e.g. `feat(scope): summary`) for these staged \
changes. Output ONLY the message — no code fences, no explanation. Subject under 72 chars; add a \
short body only if it genuinely helps.\n\nDiff:\n{}\n\nCommit message:",
        clamp_chars(diff, 6000)
    )
}

/// Prompt to summarize commits into a changelog / PR description.
pub fn changelog_prompt(commits: &[String]) -> String {
    format!(
        "Summarize these commits into a concise changelog as markdown bullet points, grouping related \
changes. No preamble.\n\nCommits:\n{}\n\nChangelog:",
        commits.join("\n")
    )
}

/// Prompt to catch the user up on what changed in a repo since they last looked.
pub fn resume_prompt(repo_name: &str, commits: &[String]) -> String {
    format!(
        "In 2–3 short sentences, catch me up on what changed in the \"{repo_name}\" repository since I \
last looked, based on these commits (newest first). Be specific and factual, no preamble, no markdown.\n\n\
Commits:\n{}\n\nWhat changed:",
        commits.join("\n")
    )
}

/// Prompt for a short daily briefing across recently-active repos.
pub fn briefing_prompt(lines: &[String]) -> String {
    format!(
        "You are a dev's morning briefing. In 2–4 short sentences, summarize what changed across these \
repositories. Be specific and factual, no preamble.\n\n{}\n\nBriefing:",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};

    fn repo() -> Repo {
        Repo {
            id: "/x".into(),
            display_name: "Orrery".into(),
            slug: Some("Hankanman/Orrery".into()),
            path: "~/dev/Orrery".into(),
            description: Some("repo dashboard".into()),
            language: Some("Rust".into()),
            git: GitStatus { branch: "main".into(), ahead: 2, behind: 0, dirty: 7 },
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: None,
            remote_host: None,
            stars: 0,
            topics: vec![],
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
        }
    }

    #[test]
    fn pick_model_prefers_configured_then_smallest() {
        let avail = vec![("big:70b".to_string(), 40_000), ("small:1b".to_string(), 1_000)];
        assert_eq!(pick_model("big:70b", &avail).as_deref(), Some("big:70b"));
        // preferred absent → smallest
        assert_eq!(pick_model("missing", &avail).as_deref(), Some("small:1b"));
        assert_eq!(pick_model("x", &[]), None);
    }

    #[test]
    fn pick_model_skips_embedding_models_in_fallback() {
        // nomic-embed-text is the smallest, but it can't generate — must be
        // skipped so the chat fallback never 400s on /api/generate.
        let avail = vec![
            ("nomic-embed-text".to_string(), 270),
            ("gemma:2b".to_string(), 1_600),
            ("qwen:9b".to_string(), 9_000),
        ];
        assert_eq!(pick_model("missing", &avail).as_deref(), Some("gemma:2b"));
        // A preferred embedding model is still honoured if explicitly chosen.
        assert_eq!(pick_model("nomic-embed-text", &avail).as_deref(), Some("nomic-embed-text"));
        // Only embedding models installed → no chat model.
        assert_eq!(pick_model("x", &[("all-minilm".to_string(), 50)]), None);
        assert!(is_embedding_model("nomic-embed-text"));
        assert!(!is_embedding_model("gemma:2b"));
    }

    #[test]
    fn summary_prompt_includes_key_facts() {
        let p = summary_prompt(&repo());
        assert!(p.contains("Orrery"));
        assert!(p.contains("Rust"));
        assert!(p.contains("7 uncommitted"));
        assert!(p.contains("branch main"));
    }

    #[test]
    fn cosine_basics() {
        let a = [1.0, 0.0, 0.0];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6); // identical
        assert!(cosine(&a, &[0.0, 1.0, 0.0]).abs() < 1e-6); // orthogonal
        assert_eq!(cosine(&a, &[1.0, 0.0]), 0.0); // mismatched length
        assert_eq!(cosine(&[], &[]), 0.0); // empty
    }

    #[test]
    fn commit_prompt_includes_diff_and_clamps() {
        let p = commit_prompt("diff --git a/x b/x\n+hello");
        assert!(p.contains("Conventional Commit"));
        assert!(p.contains("+hello"));
        // very long diffs are clamped
        let big = "x".repeat(10_000);
        assert!(commit_prompt(&big).len() < 8000);
    }
}
