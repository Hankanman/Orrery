//! Local AI summaries (#23). An `AiBackend` abstraction with the Ollama path
//! implemented now (talks to the local Ollama HTTP API, GPU-accelerated) and a
//! bundled-llama.cpp path planned behind the same seam (#21, later).
//!
//! Everything degrades gracefully: if Ollama isn't running, summaries are
//! simply unavailable and the UI shows nothing.

use serde::Deserialize;

use crate::model::Repo;

const OLLAMA: &str = "http://localhost:11434";

/// Shared HTTP client so the many Ollama calls (status, per-repo summaries and
/// embeddings) reuse one connection pool. reqwest::Client is Arc-backed.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(reqwest::Client::new);
    CLIENT.clone()
}

/// Is a local Ollama server reachable?
pub async fn available() -> bool {
    client()
        .get(format!("{OLLAMA}/api/version"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Installed models as (name, size_bytes).
pub async fn installed_models() -> Vec<(String, u64)> {
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
    let resp = client().get(format!("{OLLAMA}/api/tags")).send().await;
    match resp {
        Ok(r) => match r.json::<Tags>().await {
            Ok(t) => t.models.into_iter().map(|m| (m.name, m.size)).collect(),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

/// Choose the model to use: the preferred one if installed, otherwise the
/// smallest installed model (fastest for short summaries). Pure for testing.
pub fn pick_model(preferred: &str, available: &[(String, u64)]) -> Option<String> {
    if available.iter().any(|(name, _)| name == preferred) {
        return Some(preferred.to_string());
    }
    available
        .iter()
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
pub async fn generate(model: &str, prompt: &str) -> Result<String, String> {
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
        .post(format!("{OLLAMA}/api/generate"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Ollama API {}", resp.status()));
    }
    let parsed: GenResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed.response.trim().to_string())
}

/// Embed `text` with an embedding model via Ollama (`/api/embed`).
pub async fn embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        embeddings: Vec<Vec<f32>>,
    }
    let body = serde_json::json!({ "model": model, "input": text });
    let resp = client()
        .post(format!("{OLLAMA}/api/embed"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Ollama embed {}", resp.status()));
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
