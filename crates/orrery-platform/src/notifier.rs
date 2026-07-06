//! Background attention poll. Asks GitHub what needs your attention — new PRs,
//! review requests, CI alerts — and returns the current glance lines, the raw
//! inbox facts (the attention model's host input, so the app has them without
//! the Inbox view ever opening), and the *newly-appeared* items to notify
//! (deduped against the previous poll, filtered by the per-type opt-in
//! toggles). Review requests and CI alerts are gathered but never notified
//! from here: both are `Urgent` in `orrery_core::attention` (reviews from
//! inbox facts, CI failures from the central CI pass, #183), and the app
//! notifies new urgent items itself after each recompute — notifying here
//! too would double-fire for the same fact. UI-agnostic: callers surface the
//! glance (a tray, a nav badge) and fire notifications however they like.

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
/// `on_glance` with the current glance lines + the raw inbox facts so the
/// caller can paint a tray or a nav badge and feed its attention model. Owns
/// all threading + the runtime, so callers stay synchronous and UI-agnostic.
/// No-ops (the thread exits) if a runtime can't be built.
pub fn watch(on_glance: impl Fn(Vec<String>, Vec<inbox::InboxItem>) + Send + 'static) {
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
                on_glance(result.lines, result.inbox);
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
    /// The raw inbox facts behind the glance — the attention model's host
    /// input, kept fresh by this poll even when the Inbox view never loads.
    pub inbox: Vec<inbox::InboxItem>,
}

/// Run one attention poll: gather current items, update the dedupe snapshot, and
/// return the glance lines + the fresh notifications. On the very first poll
/// (no snapshot yet) `fresh` is empty — otherwise the whole inbox would notify
/// at once on launch.
pub async fn poll(cfg: &AppConfig) -> PollResult {
    let (items, inbox) = collect().await;
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

    PollResult {
        lines,
        fresh,
        inbox,
    }
}

fn type_enabled(cfg: &AppConfig, kind: &str) -> bool {
    match kind {
        "pr" => cfg.notify_new_pr,
        // Review requests and CI failures notify through the attention model
        // (they're Urgent there and the app dedupes + fires after each
        // recompute — CI facts come from the central pass in
        // `orrery_core::ci`, #183); notifying from this poll too would
        // double-fire for the same fact. Both still feed the glance lines
        // and the seen-set above.
        "review" | "ci" => false,
        _ => false,
    }
}

/// The trailing path segment of an `owner/name` slug, for compact labels.
fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

/// Gather attention items from GitHub, plus the raw inbox facts they came
/// from. Returns empty (rather than erroring) when there's no token or a
/// source fails — a degraded poll just shows less.
async fn collect() -> (Vec<Attention>, Vec<inbox::InboxItem>) {
    let mut out = Vec::new();
    let mut raw = Vec::new();
    if oauth::github_token().is_none() {
        return (out, raw);
    }

    if let Ok(items) = inbox::github_inbox().await {
        for it in &items {
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
        raw = items;
    }

    // CheckSuite notifications are GitHub's CI alerts (it notifies on your own
    // failed/required runs, not routine passes). Glance-only since #183: the
    // desktop notification for a failing default branch comes from the
    // attention model's `CiFailing` (see `type_enabled`).
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

    (out, raw)
}

#[cfg(test)]
mod tests {
    use super::{short_repo, type_enabled};
    use orrery_core::model::AppConfig;

    #[test]
    fn short_repo_takes_trailing_segment() {
        assert_eq!(short_repo("Hankanman/Orrery"), "Orrery");
        assert_eq!(short_repo("Orrery"), "Orrery");
    }

    #[test]
    fn reviews_and_ci_never_notify_from_the_poll() {
        // Review requests and CI failures are Urgent in the attention model;
        // the app notifies them after each recompute (CI facts from the
        // central pass, #183). The poll must not double-fire, whatever the
        // (still-honored-by-the-model) per-type toggles say.
        let cfg = AppConfig::default();
        assert!(cfg.notify_review_requested);
        assert!(cfg.notify_ci_failure);
        assert!(!type_enabled(&cfg, "review"));
        assert!(!type_enabled(&cfg, "ci"));
        assert!(type_enabled(&cfg, "pr"));
    }
}
