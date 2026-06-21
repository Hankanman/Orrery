//! Background attention poll. Asks GitHub what needs your attention — new PRs,
//! review requests, CI alerts — and returns the current glance lines plus the
//! *newly-appeared* items to notify (deduped against the previous poll, filtered
//! by the per-type opt-in toggles). UI-agnostic: callers surface the glance (a
//! tray, a nav badge) and fire notifications however they like.

use std::collections::HashSet;
use std::time::Duration;

use orrery_core::model::AppConfig;
use orrery_core::{cache, config, inbox, oauth};

/// Snapshot of the keys seen on the previous poll, for delta detection.
const SEEN_KEY: &str = "attention_seen";

/// How often the background poller checks for attention items.
const POLL_SECS: u64 = 180;

/// Run the attention poller forever on its own thread + async runtime. On each
/// tick (immediately, then every `POLL_SECS`) it polls GitHub, fires a desktop
/// notification for every newly-appeared item via [`crate::notify`], and calls
/// `on_glance` with the current glance lines so the caller can paint a tray or a
/// nav badge. Owns all threading + the runtime, so callers stay synchronous and
/// UI-agnostic. No-ops (the thread exits) if a runtime can't be built.
pub fn watch(on_glance: impl Fn(Vec<String>) + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            loop {
                let result = poll(&config::load()).await;
                on_glance(result.lines);
                for notice in &result.fresh {
                    let _ = crate::notify::send(&notice.title, &notice.body).await;
                }
                tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
            }
        });
    });
}

/// One thing needing attention: a stable key for dedupe, a one-line glance
/// label, and a title/body for the notification.
struct Attention {
    /// "pr" | "review" | "ci" — selects the per-type opt-in toggle.
    kind: &'static str,
    key: String,
    label: String,
    title: String,
    body: String,
}

/// A newly-appeared item to surface as a desktop notification.
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Result of one poll.
pub struct PollResult {
    /// Glance labels for every current attention item (passive readout).
    pub lines: Vec<String>,
    /// Items new since the previous poll and enabled by config — to notify.
    pub fresh: Vec<Notice>,
}

/// Run one attention poll: gather current items, update the dedupe snapshot, and
/// return the glance lines + the fresh notifications. On the very first poll
/// (no snapshot yet) `fresh` is empty — otherwise the whole inbox would notify
/// at once on launch.
pub async fn poll(cfg: &AppConfig) -> PollResult {
    let items = collect().await;
    let lines: Vec<String> = items.iter().map(|a| a.label.clone()).collect();

    let prev: Option<HashSet<String>> =
        cache::get_meta(SEEN_KEY).and_then(|s| serde_json::from_str(&s).ok());
    let current: HashSet<String> = items.iter().map(|a| a.key.clone()).collect();

    let mut fresh = Vec::new();
    if let Some(prev) = prev {
        if cfg.notify_enabled {
            for a in &items {
                if prev.contains(&a.key) || !type_enabled(cfg, a.kind) {
                    continue;
                }
                fresh.push(Notice {
                    title: a.title.clone(),
                    body: a.body.clone(),
                });
            }
        }
    }

    if let Ok(blob) = serde_json::to_string(&current) {
        cache::set_meta(SEEN_KEY, &blob);
    }

    PollResult { lines, fresh }
}

fn type_enabled(cfg: &AppConfig, kind: &str) -> bool {
    match kind {
        "pr" => cfg.notify_new_pr,
        "review" => cfg.notify_review_requested,
        "ci" => cfg.notify_ci_failure,
        _ => false,
    }
}

/// The trailing path segment of an `owner/name` slug, for compact labels.
fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

/// Gather attention items from GitHub. Returns empty (rather than erroring) when
/// there's no token or a source fails — a degraded poll just shows less.
async fn collect() -> Vec<Attention> {
    let mut out = Vec::new();
    if oauth::github_token().is_none() {
        return out;
    }

    if let Ok(items) = inbox::github_inbox().await {
        for it in items {
            let short = short_repo(&it.repo);
            match it.kind.as_str() {
                "pr" => out.push(Attention {
                    kind: "pr",
                    key: format!("pr:{}#{}", it.repo, it.number),
                    label: format!("New PR: {short} #{}", it.number),
                    title: "New pull request".into(),
                    body: format!("{} #{} · {}", it.repo, it.number, it.title),
                }),
                "review" => out.push(Attention {
                    kind: "review",
                    key: format!("review:{}#{}", it.repo, it.number),
                    label: format!("Review requested: {short} #{}", it.number),
                    title: "Review requested".into(),
                    body: format!("{} #{} · {}", it.repo, it.number, it.title),
                }),
                _ => {} // assigned issues aren't an attention-notification type
            }
        }
    }

    // CheckSuite notifications are GitHub's CI alerts (it notifies on your own
    // failed/required runs, not routine passes).
    if let Ok(notes) = inbox::github_notifications().await {
        for n in notes {
            if n.kind == "CheckSuite" {
                out.push(Attention {
                    kind: "ci",
                    key: format!("ci:{}:{}", n.repo, n.title),
                    label: format!("CI: {}", short_repo(&n.repo)),
                    title: "CI alert".into(),
                    body: format!("{}: {}", n.repo, n.title),
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::short_repo;

    #[test]
    fn short_repo_takes_trailing_segment() {
        assert_eq!(short_repo("Hankanman/Orrery"), "Orrery");
        assert_eq!(short_repo("Orrery"), "Orrery");
    }
}
