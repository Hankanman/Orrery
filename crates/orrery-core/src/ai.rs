//! Local AI summaries (#23). A `Backend` seam selects the inference engine:
//! the Ollama HTTP path (default, GPU-accelerated) or the bundled llama.cpp
//! sidecar (#21). The public entry points (`available`, `installed_models`,
//! `generate`/`generate_with`, `embed`/`embed_batch`, `pull`) dispatch on the
//! configured backend, sharing one [`GenParams`] budget type; everything
//! backend-agnostic (prompts, `pick_model`) stays free-standing; vector
//! similarity lives in `crate::semantic`.
//!
//! Everything degrades gracefully: if the active backend isn't reachable,
//! summaries are simply unavailable and the UI shows nothing.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::model::Repo;

/// Generation parameters shared by both backends — Ollama maps them onto
/// `options.{num_predict,temperature}`, llama.cpp onto `n_predict` /
/// `temperature` — so a caller's budget means the same thing on either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenParams {
    /// Max tokens to generate. Thinking models spend part of this on hidden
    /// reasoning, so long outputs need real headroom.
    pub max_tokens: u32,
    pub temperature: f32,
}

impl GenParams {
    /// One-line outputs: repo summaries, the Settings ping.
    pub const SHORT: Self = Self {
        max_tokens: 120,
        temperature: 0.2,
    };
    /// A few sentences: commit messages (subject + short body), catch-ups.
    pub const MEDIUM: Self = Self {
        max_tokens: 384,
        temperature: 0.2,
    };
    /// Multi-paragraph markdown: PR bodies, changelogs.
    pub const LONG: Self = Self {
        max_tokens: 1024,
        temperature: 0.2,
    };
}

/// Per-read timeout for the AI HTTP clients. Not an overall request timeout —
/// a long (non-streamed) generation or a model pull must not be cut off — but a
/// server that goes silent this long is wedged, and without it one stuck
/// request would hang `summarize::run` (and the UI's AI busy state) forever.
/// Generous because CPU inference of a [`GenParams::LONG`] budget is slow and a
/// non-streamed response sends nothing until it finishes.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(300);

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
// Each delegates to the active backend. The llama.cpp arms drive the bundled
// `llama-server` sidecar (see `crate::llama`); embeddings stay Ollama-only, and
// model "pull" is a GGUF download from Settings rather than an Ollama registry
// pull. All degrade to "unavailable" when the backend isn't reachable.

/// Is the active backend reachable?
pub async fn available() -> bool {
    match active_backend() {
        Backend::Ollama => ollama_available().await,
        Backend::LlamaCpp => crate::llama::available().await,
    }
}

/// Installed/available models as (name, size_bytes) for the active backend.
pub async fn installed_models() -> Vec<(String, u64)> {
    match active_backend() {
        Backend::Ollama => ollama_installed_models().await,
        Backend::LlamaCpp => crate::llama::installed_models(),
    }
}

/// Generate text from `prompt` using `model` on the active backend, with the
/// short ([`GenParams::SHORT`]) budget. See [`generate_with`].
pub async fn generate(model: &str, prompt: &str) -> Result<String, String> {
    generate_with(model, prompt, GenParams::SHORT).await
}

/// Generate text from `prompt` using `model` on the active backend, within
/// `params`' token budget. The llama.cpp backend serves the configured GGUF,
/// so it ignores `model`.
pub async fn generate_with(model: &str, prompt: &str, params: GenParams) -> Result<String, String> {
    match active_backend() {
        Backend::Ollama => ollama_generate(model, prompt, params).await,
        Backend::LlamaCpp => crate::llama::generate(prompt, params).await,
    }
}

/// Whether the active backend can embed at all. Embeddings are Ollama-only, so
/// semantic indexing/recall stays dormant (not broken) on the llama.cpp
/// backend — callers skip the work instead of erroring per call.
pub fn embeddings_supported() -> bool {
    matches!(active_backend(), Backend::Ollama)
}

/// Embed `text` with `model` on the active backend. Embeddings are Ollama-only
/// for now — semantic search stays hidden on the llama.cpp backend.
pub async fn embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    embed_batch(model, &[text.to_string()])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "model returned no embedding".to_string())
}

/// Embed several texts in one request (Ollama's `/api/embed` takes an array
/// `input`), returning one vector per input, in order. Errors unless every
/// input got a non-empty vector — callers store a whole source or nothing.
pub async fn embed_batch(model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    match active_backend() {
        Backend::Ollama => ollama_embed(model, texts).await,
        Backend::LlamaCpp => Err("embeddings are not supported on the llama.cpp backend".into()),
    }
}

/// Download/prepare `model` on the active backend, reporting progress. The
/// llama.cpp model download has its own command (`download_llama_model`).
pub async fn pull(model: &str, on_progress: impl FnMut(&str, u64, u64)) -> Result<(), String> {
    match active_backend() {
        Backend::Ollama => {
            let res = ollama_pull(model, on_progress).await;
            // A newly pulled model must be pickable right away.
            invalidate_model_cache();
            res
        }
        Backend::LlamaCpp => {
            Err("use the model download in Settings for the llama.cpp backend".into())
        }
    }
}

/// Base URL of the Ollama server, from config (default http://localhost:11434).
/// config::load() is cached, so this is cheap to call per request.
fn base() -> String {
    crate::config::load().ollama_host
}

/// Shared HTTP client so the many Ollama calls (status, per-repo summaries and
/// embeddings) reuse one connection pool. reqwest::Client is Arc-backed.
///
/// A `connect_timeout` makes a dead/unreachable host fail fast and
/// [`READ_TIMEOUT`] catches a wedged one, but there's no overall request
/// timeout, because generation and model pulls legitimately run for minutes
/// and must not be cut off.
fn client() -> reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_default()
    });
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

/// Models seen returning an empty response until retried with `think:false`.
/// Remembered for the process lifetime so later calls go straight to the
/// request that works instead of paying for a wasted generation every time.
static NEEDS_NO_THINK: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Generate text via Ollama.
///
/// Tries normally first. If the model returns an empty response — the signature
/// of a "thinking" model (qwen3, gemma3, …) that spent its whole token budget
/// on hidden reasoning — it retries once with `think:false` and remembers that
/// the model needs it. This way the `think` field is only ever sent to a model
/// that actually needs it, so plain models that might reject the field are
/// never hit with it.
async fn ollama_generate(model: &str, prompt: &str, params: GenParams) -> Result<String, String> {
    let known = NEEDS_NO_THINK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(model);
    if known {
        return generate_once(model, prompt, params, true).await;
    }
    let first = generate_once(model, prompt, params, false).await?;
    if !first.is_empty() {
        return Ok(first);
    }
    let second = generate_once(model, prompt, params, true).await?;
    if !second.is_empty() {
        NEEDS_NO_THINK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(model.to_string());
    }
    Ok(second)
}

async fn generate_once(
    model: &str,
    prompt: &str,
    params: GenParams,
    suppress_think: bool,
) -> Result<String, String> {
    #[derive(Deserialize)]
    struct GenResp {
        #[serde(default)]
        response: String,
    }
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": { "temperature": params.temperature, "num_predict": params.max_tokens }
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
/// as a callback so this module stays free of any UI/event machinery.
async fn ollama_pull(
    model: &str,
    mut on_progress: impl FnMut(&str, u64, u64),
) -> Result<(), String> {
    use futures_util::StreamExt;

    // Ollama's /api/pull needs an explicit tag; default to :latest when none.
    let model = if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    };
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

/// Embed `texts` with an embedding model via Ollama (`/api/embed`), batched
/// into one request.
async fn ollama_embed(model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        embeddings: Vec<Vec<f32>>,
    }
    let body = serde_json::json!({ "model": model, "input": texts });
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
    if parsed.embeddings.len() != texts.len() || parsed.embeddings.iter().any(|v| v.is_empty()) {
        return Err("model returned no embedding".to_string());
    }
    Ok(parsed.embeddings)
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Prompt to write a Conventional Commit message from a staged diff. The
/// output shape (subject, blank line, optional body) is parsed back apart by
/// [`split_commit_message`].
pub fn commit_prompt(diff: &str) -> String {
    format!(
        "Write a single Conventional Commit message (e.g. `feat(scope): summary`) for these staged \
changes. Output ONLY the message — no code fences, no explanation. Subject under 72 chars on the \
first line; if a short body genuinely helps, add it after one blank line.\n\nDiff:\n{}\n\nCommit message:",
        clamp_chars(diff, 6000)
    )
}

/// Split an AI commit message into (subject, body): the first non-empty line
/// (stripped of wrapping quotes/backticks) is the subject, everything after it
/// is the body. Both come back trimmed; either may be empty.
pub fn split_commit_message(text: &str) -> (String, String) {
    let mut lines = text.trim().lines();
    let subject = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(['"', '`'])
        .to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (subject, body)
}

/// Prompt to summarize commits into a changelog / PR description.
pub fn changelog_prompt(commits: &[String]) -> String {
    format!(
        "Summarize these commits into a concise changelog as markdown bullet points, grouping related \
changes. No preamble.\n\nCommits:\n{}\n\nChangelog:",
        commits.join("\n")
    )
}

/// Prompt to draft a pull-request title + body from a branch's commit range.
/// The first output line is the title; the rest (after a blank line) is the
/// markdown body — [`split_pr_draft`] parses that shape back apart.
pub fn pr_prompt(branch: &str, commits: &[String]) -> String {
    format!(
        "Draft a pull request for branch \"{branch}\" from these commits (newest first). \
Output the PR title on the FIRST line (imperative, under 72 chars, no quotes), then a blank \
line, then a concise markdown body summarizing the changes as bullet points. No preamble, \
no code fences.\n\nCommits:\n{}\n\nPull request:",
        commits.join("\n")
    )
}

/// Split an AI PR draft into (title, body): first non-empty line is the
/// title, the rest is the body. `None` when there's no usable title.
pub fn split_pr_draft(text: &str) -> Option<(String, String)> {
    let mut lines = text.trim().lines();
    let title = lines.next()?.trim().trim_matches('"').to_string();
    if title.is_empty() {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((title, body))
}

/// Non-AI PR draft from a commit range (newest first): the latest commit's
/// subject as the title, the commit list as the body. Works with an empty
/// range too, so "Open PR" never depends on AI being reachable.
pub fn fallback_pr_draft(branch: &str, commits: &[String]) -> (String, String) {
    let title = commits
        .first()
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| branch.to_string());
    let body = commits
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    (title, body)
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

/// How long an installed-models listing is reused before re-querying the
/// backend. Short: just enough that a burst of AI actions (or a summary pass)
/// doesn't hit `/api/tags` before every single generation.
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);

/// Cached installed-models listing: (fetched at, backend key, models). The key
/// is (backend, Ollama host), so a config change never serves a stale list.
type ModelCache = Option<(Instant, String, Vec<(String, u64)>)>;
static MODEL_CACHE: LazyLock<Mutex<ModelCache>> = LazyLock::new(|| Mutex::new(None));

fn model_cache_key() -> String {
    match active_backend() {
        Backend::Ollama => format!("ollama\u{1f}{}", base()),
        Backend::LlamaCpp => "llama".to_string(),
    }
}

fn invalidate_model_cache() {
    *MODEL_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// [`installed_models`], reused for [`MODEL_CACHE_TTL`]. Empty listings are
/// never cached, so a just-started server or just-installed model is picked up
/// on the next call.
async fn installed_models_cached() -> Vec<(String, u64)> {
    let key = model_cache_key();
    {
        let guard = MODEL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, k, models)) = guard.as_ref() {
            if *k == key && at.elapsed() < MODEL_CACHE_TTL {
                return models.clone();
            }
        }
    }
    let models = installed_models().await;
    if !models.is_empty() {
        *MODEL_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), key, models.clone()));
    }
    models
}

/// The chat model to use on the active backend: the configured one if
/// installed, else the smallest non-embedding model ([`pick_model`]). `None`
/// when AI is unreachable or no usable model is installed.
pub async fn chat_model() -> Option<String> {
    let cfg = crate::config::load();
    pick_model(&cfg.ai_model, &installed_models_cached().await)
}

/// Run `prompt` through the configured chat model on the active backend,
/// within `params`' budget. Returns an error when AI is unreachable or no model
/// is installed, so callers degrade gracefully (the `aiReady` contract).
async fn generate_with_model(prompt: &str, params: GenParams) -> Result<String, String> {
    let model = chat_model().await.ok_or("no AI model installed")?;
    generate_with(&model, prompt, params).await
}

/// Generate a Conventional Commit message for a staged diff.
pub async fn commit_message(diff: &str) -> Result<String, String> {
    generate_with_model(&commit_prompt(diff), GenParams::MEDIUM).await
}

/// Summarize commit subjects into a markdown changelog.
pub async fn changelog(commits: &[String]) -> Result<String, String> {
    generate_with_model(&changelog_prompt(commits), GenParams::LONG).await
}

/// A 2–3 sentence catch-up on what changed in a repo since the last look.
pub async fn resume(repo_name: &str, commits: &[String]) -> Result<String, String> {
    generate_with_model(&resume_prompt(repo_name, commits), GenParams::MEDIUM).await
}

/// Draft a PR title + body from a branch's commit range (first line = title —
/// see [`pr_prompt`] / [`split_pr_draft`]). Callers fall back to
/// [`fallback_pr_draft`] when AI is unavailable (the `aiReady` contract).
pub async fn pr_description(branch: &str, commits: &[String]) -> Result<String, String> {
    generate_with_model(&pr_prompt(branch, commits), GenParams::LONG).await
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
            git: GitStatus {
                branch: "main".into(),
                ahead: 2,
                behind: 0,
                dirty: 7,
            },
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
        let avail = vec![
            ("big:70b".to_string(), 40_000),
            ("small:1b".to_string(), 1_000),
        ];
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
        assert_eq!(
            pick_model("nomic-embed-text", &avail).as_deref(),
            Some("nomic-embed-text")
        );
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
    fn pr_prompt_and_draft_split_round_trip() {
        let commits = vec!["feat: two".to_string(), "feat: one".to_string()];
        let p = pr_prompt("feat/x", &commits);
        assert!(p.contains("feat/x"));
        assert!(p.contains("feat: two"));
        assert!(p.contains("FIRST line"));

        // A well-formed draft splits into title + body.
        let (title, body) = split_pr_draft("feat: add x\n\n- one\n- two").unwrap();
        assert_eq!(title, "feat: add x");
        assert_eq!(body, "- one\n- two");
        // Title-only drafts still work; empty drafts don't.
        assert_eq!(
            split_pr_draft("\"feat: quoted\"").unwrap(),
            ("feat: quoted".to_string(), String::new())
        );
        assert!(split_pr_draft("   \n").is_none());
    }

    #[test]
    fn split_commit_message_separates_subject_and_body() {
        let (s, b) = split_commit_message("feat(x): add y\n\nLonger reasoning.\nSecond line.");
        assert_eq!(s, "feat(x): add y");
        assert_eq!(b, "Longer reasoning.\nSecond line.");
        // Subject-only messages give an empty body; wrapping quotes/backticks drop.
        assert_eq!(
            split_commit_message("`fix: subject only`\n"),
            ("fix: subject only".to_string(), String::new())
        );
        // A body directly after the subject (no blank line) still splits.
        let (s, b) = split_commit_message("fix: a\nbody line");
        assert_eq!((s.as_str(), b.as_str()), ("fix: a", "body line"));
        assert_eq!(split_commit_message("  "), (String::new(), String::new()));
    }

    #[test]
    fn fallback_pr_draft_uses_latest_subject_and_lists_commits() {
        let commits = vec!["feat: two".to_string(), "feat: one".to_string()];
        let (title, body) = fallback_pr_draft("feat/x", &commits);
        assert_eq!(title, "feat: two");
        assert_eq!(body, "- feat: two\n- feat: one");
        // No commits (shouldn't happen, but): the branch name still titles it.
        let (title, body) = fallback_pr_draft("feat/x", &[]);
        assert_eq!(title, "feat/x");
        assert!(body.is_empty());
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
