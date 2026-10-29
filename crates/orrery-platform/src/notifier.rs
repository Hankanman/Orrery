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

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use orrery_core::model::AppConfig;
use orrery_core::{cache, config, inbox, oauth};

/// Snapshot of the keys seen per source on the previous successful poll of
/// that source, for delta detection. (Renamed from the pre-per-source
/// `attention_seen` flat set so an old blob is never misread — the first poll
/// after upgrading just seeds silently.)
const SEEN_KEY: &str = "attention_seen_v2";

/// Seen keys per attention source. A source is only present once it has
/// succeeded at least once; a failed fetch leaves its entry untouched.
type Seen = BTreeMap<String, BTreeSet<String>>;

/// The inbox (PRs + review requests) source.
const SRC_INBOX: &str = "inbox";
/// The notifications (CheckSuite / CI alerts) source.
const SRC_NOTIFICATIONS: &str = "notifications";

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
                    if let Err(e) = crate::notify::send(&notice.title, &notice.body).await {
                        eprintln!("[orrery notifier] notification failed: {e}");
                    }
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
/// return the glance lines + the fresh notifications. The snapshot is kept per
/// source and only updated for sources that fetched successfully: a missing
/// token or a failed fetch must not wipe it, or the whole inbox would notify at
/// once when the source recovers. The first time a source succeeds (no
/// snapshot for it yet — first launch, first sign-in) it seeds silently.
pub async fn poll(cfg: &AppConfig) -> PollResult {
    let (sources, inbox) = collect().await;

    let prev: Seen = cache::get_meta(SEEN_KEY)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let (new_items, seen) = diff(&prev, &sources);

    let fresh = if cfg.notify_enabled {
        new_items
            .into_iter()
            .filter(|a| type_enabled(cfg, a.kind))
            .map(|a| Notice {
                title: a.title.clone(),
                body: a.body.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    if seen != prev {
        if let Ok(blob) = serde_json::to_string(&seen) {
            cache::set_meta(SEEN_KEY, &blob);
        }
    }

    let lines = sources
        .iter()
        .flat_map(|(_, items)| items.iter().flatten())
        .map(|a| a.label.clone())
        .collect();

    PollResult {
        lines,
        fresh,
        inbox,
    }
}

/// One source's poll outcome: `None` when it couldn't be fetched (no token,
/// network/API error), else its current items (possibly empty).
type SourceResult = (&'static str, Option<Vec<Attention>>);

/// Pure delta step: the items that are new relative to `prev`, and the updated
/// snapshot. Failed sources keep their previous entry and contribute nothing;
/// a source succeeding for the first time is recorded but yields nothing.
fn diff<'a>(prev: &Seen, sources: &'a [SourceResult]) -> (Vec<&'a Attention>, Seen) {
    let mut seen = prev.clone();
    let mut fresh = Vec::new();
    for (source, items) in sources {
        let Some(items) = items else {
            continue;
        };
        if let Some(before) = prev.get(*source) {
            fresh.extend(items.iter().filter(|a| !before.contains(&a.key)));
        }
        seen.insert(
            source.to_string(),
            items.iter().map(|a| a.key.clone()).collect(),
        );
    }
    (fresh, seen)
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

/// Gather attention items from GitHub per source, plus the raw inbox facts
/// they came from. A source is `None` (rather than erroring) when there's no
/// token or its fetch fails — a degraded poll just shows less, and the
/// dedupe snapshot for that source is left alone.
async fn collect() -> (Vec<SourceResult>, Vec<inbox::InboxItem>) {
    let mut raw = Vec::new();
    if oauth::github_token().is_none() {
        return (vec![(SRC_INBOX, None), (SRC_NOTIFICATIONS, None)], raw);
    }

    let inbox_items = match inbox::github_inbox().await {
        Ok(items) => {
            let out = inbox_attention(&items);
            raw = items;
            Some(out)
        }
        Err(e) => {
            eprintln!("[orrery notifier] inbox fetch failed: {e}");
            None
        }
    };

    // CheckSuite notifications are GitHub's CI alerts (it notifies on your own
    // failed/required runs, not routine passes). Glance-only since #183: the
    // desktop notification for a failing default branch comes from the
    // attention model's `CiFailing` (see `type_enabled`).
    let ci_items = match inbox::github_notifications().await {
        Ok(notes) => Some(
            notes
                .into_iter()
                .filter(|n| n.kind == "CheckSuite")
                .map(|n| Attention {
                    kind: "ci",
                    key: format!("ci:{}:{}", n.repo, n.title),
                    label: format!("CI: {}", short_repo(&n.repo)),
                    title: "CI alert".into(),
                    body: format!("{}: {}", n.repo, n.title),
                })
                .collect(),
        ),
        Err(e) => {
            eprintln!("[orrery notifier] notifications fetch failed: {e}");
            None
        }
    };

    (
        vec![(SRC_INBOX, inbox_items), (SRC_NOTIFICATIONS, ci_items)],
        raw,
    )
}

/// Attention items (new PRs, review requests) from the raw inbox facts.
fn inbox_attention(items: &[inbox::InboxItem]) -> Vec<Attention> {
    let mut out = Vec::new();
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
    out
}

#[cfg(test)]
mod tests {
    use super::{diff, short_repo, type_enabled, Attention, Seen, SourceResult};
    use orrery_core::model::AppConfig;

    fn item(key: &str) -> Attention {
        Attention {
            kind: "pr",
            key: key.into(),
            label: key.into(),
            title: String::new(),
            body: String::new(),
        }
    }

    fn keys(fresh: &[&Attention]) -> Vec<String> {
        fresh.iter().map(|a| a.key.clone()).collect()
    }

    #[test]
    fn first_success_seeds_silently() {
        let sources: Vec<SourceResult> = vec![("inbox", Some(vec![item("a"), item("b")]))];
        let (fresh, seen) = diff(&Seen::new(), &sources);
        assert!(fresh.is_empty(), "first sign-in must not notify the inbox");
        assert_eq!(seen["inbox"].len(), 2);
    }

    #[test]
    fn new_items_after_a_seed_notify() {
        let (_, seen) = diff(&Seen::new(), &[("inbox", Some(vec![item("a")]))]);
        let sources: Vec<SourceResult> = vec![("inbox", Some(vec![item("a"), item("b")]))];
        let (fresh, _) = diff(&seen, &sources);
        assert_eq!(keys(&fresh), ["b"]);
    }

    #[test]
    fn a_failed_source_keeps_its_snapshot() {
        // Seeded, then an outage (fetch fails / token missing), then recovery:
        // only items that are genuinely new since before the outage notify.
        let (_, seen) = diff(&Seen::new(), &[("inbox", Some(vec![item("a")]))]);
        let (fresh, during) = diff(&seen, &[("inbox", None)]);
        assert!(fresh.is_empty());
        assert_eq!(during, seen, "outage must not overwrite the snapshot");
        let sources: Vec<SourceResult> = vec![("inbox", Some(vec![item("a"), item("c")]))];
        let (fresh, _) = diff(&during, &sources);
        assert_eq!(keys(&fresh), ["c"]);
    }

    #[test]
    fn sources_are_tracked_independently() {
        // The inbox is seeded; notifications fail on the first poll and so
        // seed silently when they first succeed, without affecting the inbox.
        let first: Vec<SourceResult> =
            vec![("inbox", Some(vec![item("pr")])), ("notifications", None)];
        let (_, seen) = diff(&Seen::new(), &first);
        assert!(!seen.contains_key("notifications"));
        let second: Vec<SourceResult> = vec![
            ("inbox", Some(vec![item("pr"), item("pr2")])),
            ("notifications", Some(vec![item("ci")])),
        ];
        let (fresh, seen) = diff(&seen, &second);
        assert_eq!(keys(&fresh), ["pr2"]);
        assert!(seen["notifications"].contains("ci"));
    }

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
