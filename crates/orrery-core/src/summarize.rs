//! Generate local-AI one-line repo summaries and persist them to the cache.
//!
//! The producer side of the AI-summary pipeline; `cache::apply_summaries` (the
//! consumer) overlays them onto repos at load so the cards show them. Gated on
//! `ai_enabled` + a reachable backend — when AI is unavailable this is a no-op
//! and the UI simply shows no summaries (the `aiReady` contract).
//!
//! Summaries are generated **sequentially**: local inference is heavy and the
//! single Ollama/llama.cpp server would thrash under concurrent requests.

use crate::model::Repo;
use crate::{ai, cache, config};

/// Generate + cache a summary for every repo whose cached summary is missing or
/// stale (the repo committed since it was last summarized). Returns the number
/// of summaries (re)generated — `0` when AI is disabled/unreachable, no model is
/// installed, or everything is already current.
pub async fn run(repos: &[Repo]) -> usize {
    let cfg = config::load();
    if !cfg.ai_enabled {
        return 0;
    }
    // Pick the chat model: configured if installed, else the smallest non-embed.
    let models = ai::installed_models().await;
    let Some(model) = ai::pick_model(&cfg.ai_model, &models) else {
        return 0;
    };

    let mut done = 0;
    for r in repos {
        if cache::cached_summary(&r.id, r.last_commit_unix).is_some() {
            continue;
        }
        if let Ok(summary) = ai::generate(&model, &ai::summary_prompt(r)).await {
            let summary = summary.trim();
            if !summary.is_empty() {
                cache::store_summary(&r.id, summary, r.last_commit_unix);
                done += 1;
            }
        }
    }
    done
}

/// Summarize the current cached repo snapshot. Convenience for the app, which
/// holds render rows rather than `Repo`s.
pub async fn run_cached() -> usize {
    let repos = cache::load_repos();
    run(&repos).await
}
