//! Generate local-AI one-line repo summaries and persist them to the cache.
//!
//! The producer side of the AI-summary pipeline; `cache::apply_summaries` (the
//! consumer) overlays them onto repos at load so the cards show them. Gated on
//! `ai_enabled` + a reachable backend — when AI is unavailable this is a no-op
//! and the UI simply shows no summaries (the `aiReady` contract).
//!
//! Summaries are generated **sequentially**: local inference is heavy and the
//! single Ollama/llama.cpp server would thrash under concurrent requests.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use crate::model::Repo;
use crate::{ai, cache, config};

/// (repo id, last commit) pairs the model answered with an empty summary.
/// Skipped by later passes until the repo commits again (or the app restarts),
/// so one repo the model can't summarize doesn't cost a generation every pass.
static EMPTY: LazyLock<Mutex<HashSet<(String, i64)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Generate + cache a summary for every repo whose cached summary is missing or
/// stale (the repo committed since it was last summarized). Returns the number
/// of summaries (re)generated — `0` when AI is disabled/unreachable, no model is
/// installed, or everything is already current. A backend error ends the pass
/// early (the server likely went away); the rest are retried next pass.
pub async fn run(repos: &[Repo]) -> usize {
    let cfg = config::load();
    if !cfg.ai_enabled {
        return 0;
    }
    let Some(model) = ai::chat_model().await else {
        return 0;
    };

    let mut done = 0;
    for r in repos {
        if cache::cached_summary(&r.id, r.last_commit_unix).is_some() {
            continue;
        }
        let key = (r.id.clone(), r.last_commit_unix);
        if EMPTY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key)
        {
            continue;
        }
        let Ok(summary) =
            ai::generate_with(&model, &ai::summary_prompt(r), ai::GenParams::SHORT).await
        else {
            break;
        };
        let summary = summary.trim();
        if summary.is_empty() {
            EMPTY.lock().unwrap_or_else(|e| e.into_inner()).insert(key);
        } else {
            cache::store_summary(&r.id, summary, r.last_commit_unix);
            done += 1;
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
