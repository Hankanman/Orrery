//! Background attention poll. Asks every connected host (GitHub, GitLab incl.
//! trusted self-hosted, Gitea / Forgejo / Codeberg) what needs your attention
//! — new PRs, review requests, CI alerts — and returns the current glance
//! lines, the raw inbox facts (the attention model's host input, so the app
//! has them without
//! the Inbox view ever opening), and the *newly-appeared* items to notify
//! (deduped against the previous poll, filtered by the per-type opt-in
//! toggles). Review requests and CI alerts are gathered but never notified
//! from here: both are `Urgent` in `orrery_core::attention` (reviews from
//! inbox facts, CI failures from the central CI pass, #183), and the app
//! notifies new urgent items itself after each recompute — notifying here
//! too would double-fire for the same fact. UI-agnostic: callers surface the
//! glance (a tray, a nav badge) and fire notifications however they like.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

use orrery_core::forge::TrustedHosts;
use orrery_core::model::{AppConfig, Host};
use orrery_core::{cache, config, inbox, oauth};

/// Snapshot of the keys seen per source on the previous successful poll of
/// that source, for delta detection. (Renamed from the pre-per-source
/// `attention_seen` flat set so an old blob is never misread — the first poll
/// after upgrading just seeds silently.)
const SEEN_KEY: &str = "attention_seen_v2";

/// Seen keys per attention source. A source is only present once it has
/// succeeded at least once; a failed fetch leaves its entry untouched.
type Seen = BTreeMap<String, BTreeSet<String>>;

/// The GitHub inbox (PRs + review requests) source. GitHub keeps the bare
/// pre-multi-host names so existing seen-sets carry over; other instances
/// get [`source_key`]s and seed silently the first time they succeed.
const SRC_INBOX: &str = "inbox";
/// The GitHub notifications (CheckSuite / CI alerts) source.
const SRC_NOTIFICATIONS: &str = "notifications";

/// The seen-set key for one host instance's `kind` ("inbox" /
/// "notifications") source: the bare kind for GitHub, else
/// `"<kind>:<host>:<domain>"` (e.g. `"inbox:gitlab:gitlab.acme.io"`).
fn source_key(kind: &'static str, host: Host, domain: &str) -> String {
    match host {
        Host::Github => kind.to_string(),
        _ => format!("{kind}:{}:{domain}", host.as_str()),
    }
}

/// Last successfully fetched raw inbox items per source, so one host failing
/// a poll doesn't drop its items from the facts handed to the app.
static LAST_RAW: Mutex<BTreeMap<String, Vec<inbox::InboxItem>>> = Mutex::new(BTreeMap::new());

/// How often the background poller checks for attention items.
const POLL_SECS: u64 = 180;

/// Run the attention poller forever on its own thread + async runtime. On each
/// tick (immediately, then every `POLL_SECS`) it polls every connected host,
/// fires a desktop notification for every newly-appeared item via
/// [`crate::notify`], and calls `on_glance` with the current glance lines + the
/// raw inbox facts so the caller can paint a tray or a nav badge and feed its
/// attention model. A host whose fetch fails this tick contributes its last
/// good items; the facts are `None` only when nothing is known yet, so the
/// caller keeps its last-known items rather than blanking them on a network
/// blip.
/// Owns all threading + the runtime, so callers stay synchronous and
/// UI-agnostic. No-ops (the thread exits) if a runtime can't be built.
pub fn watch(on_glance: impl Fn(Vec<String>, Option<Vec<inbox::InboxItem>>) + Send + 'static) {
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
    /// Covers every connected host (GitHub, GitLab, Gitea), each falling back
    /// to its last good fetch. `None` when every fetch failed and nothing is
    /// known yet (keep the previous facts); empty when no host is connected.
    pub inbox: Option<Vec<inbox::InboxItem>>,
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

/// One source's poll outcome, keyed by its seen-set name (see
/// [`source_key`]): `None` when it couldn't be fetched (no token,
/// network/API error), else its current items (possibly empty).
type SourceResult = (String, Option<Vec<Attention>>);

/// Pure delta step: the items that are new relative to `prev`, and the updated
/// snapshot. Failed sources keep their previous entry and contribute nothing;
/// a source succeeding for the first time is recorded but yields nothing —
/// so a newly connected GitLab or Gitea seeds silently, exactly like GitHub
/// on first sign-in.
fn diff<'a>(prev: &Seen, sources: &'a [SourceResult]) -> (Vec<&'a Attention>, Seen) {
    let mut seen = prev.clone();
    let mut fresh = Vec::new();
    for (source, items) in sources {
        let Some(items) = items else {
            continue;
        };
        if let Some(before) = prev.get(source) {
            fresh.extend(items.iter().filter(|a| !before.contains(&a.key)));
        }
        seen.insert(
            source.clone(),
            items.iter().map(|a| a.key.clone()).collect(),
        );
    }
    (fresh, seen)
}

/// Fold this poll's raw inbox results into the per-source last-known store
/// and return the facts to hand the app. A failed source contributes its
/// last-known items; a source no longer configured (token removed) is
/// dropped. `None` (keep your previous facts) only when every source failed
/// and none has a last-known value; no sources at all is an empty list.
/// Pure over `last`.
fn merge_raw(
    last: &mut BTreeMap<String, Vec<inbox::InboxItem>>,
    results: Vec<(String, Option<Vec<inbox::InboxItem>>)>,
) -> Option<Vec<inbox::InboxItem>> {
    last.retain(|k, _| results.iter().any(|(key, _)| key == k));
    let mut known = results.is_empty();
    let mut keys = Vec::new();
    for (key, items) in results {
        if let Some(items) = items {
            last.insert(key.clone(), items);
        }
        known |= last.contains_key(&key);
        keys.push(key);
    }
    known.then(|| {
        keys.iter()
            .filter_map(|k| last.get(k))
            .flatten()
            .cloned()
            .collect()
    })
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

/// Gather attention items per source from every connected host instance,
/// plus the raw inbox facts they came from. A source is `None` (rather than
/// erroring) when its fetch fails — a degraded poll just shows less, and the
/// dedupe snapshot for that source is left alone. GitHub without a token
/// still reports its two sources as `None`, as before. The raw inbox facts
/// fall back per source to the last good fetch (see [`merge_raw`]).
async fn collect() -> (Vec<SourceResult>, Option<Vec<inbox::InboxItem>>) {
    // Async lookups: the `gh`/`glab` token fallbacks mustn't block the runtime.
    let tokens = oauth::host_tokens_async().await;
    let trusted = TrustedHosts::from_config(&config::load());

    // Gitea notifications carry nothing the attention glance uses (no CI
    // alerts), so only GitHub notifications and GitLab to-dos are polled.
    let (inbox_sources, note_sources) = futures_util::future::join(
        inbox::inbox_by_source(&tokens, &trusted),
        inbox::notifications_by_source(&tokens, &trusted, &[Host::Github, Host::Gitlab]),
    )
    .await;

    let mut sources: Vec<SourceResult> = Vec::new();
    if tokens.github.is_none() {
        sources.push((SRC_INBOX.into(), None));
        sources.push((SRC_NOTIFICATIONS.into(), None));
    }

    let mut raw = Vec::new();
    for s in inbox_sources {
        let key = source_key(SRC_INBOX, s.host, &s.domain);
        match s.result {
            Ok(items) => {
                sources.push((key.clone(), Some(inbox_attention(&items))));
                raw.push((key, Some(items)));
            }
            Err(e) => {
                eprintln!("[orrery notifier] {} inbox fetch failed: {e}", s.domain);
                sources.push((key.clone(), None));
                raw.push((key, None));
            }
        }
    }

    // CI alerts: GitHub's CheckSuite notifications (it notifies on your own
    // failed/required runs, not routine passes) and GitLab's "build_failed"
    // to-dos. Glance-only since #183: the desktop notification for a failing
    // default branch comes from the attention model's `CiFailing` (see
    // `type_enabled`).
    for s in note_sources {
        let key = source_key(SRC_NOTIFICATIONS, s.host, &s.domain);
        match s.result {
            Ok(notes) => sources.push((key, Some(ci_attention(&notes)))),
            Err(e) => {
                eprintln!(
                    "[orrery notifier] {} notifications fetch failed: {e}",
                    s.domain
                );
                sources.push((key, None));
            }
        }
    }

    let raw = merge_raw(&mut LAST_RAW.lock().unwrap_or_else(|e| e.into_inner()), raw);
    (sources, raw)
}

/// CI-alert attention items from host notifications: GitHub `CheckSuite`
/// notifications and GitLab `build_failed` to-dos.
fn ci_attention(notes: &[inbox::Notification]) -> Vec<Attention> {
    notes
        .iter()
        .filter(|n| match n.host {
            Host::Github => n.kind == "CheckSuite",
            Host::Gitlab => n.reason == "build_failed",
            Host::Gitea => false,
        })
        .map(|n| Attention {
            kind: "ci",
            key: format!("ci:{}:{}", n.repo, n.title),
            label: format!("CI: {}", short_repo(&n.repo)),
            title: "CI alert".into(),
            body: format!("{}: {}", n.repo, n.title),
        })
        .collect()
}

/// Attention items (new PRs, review requests) from the raw inbox facts. Keys
/// only need to be unique within one source (each host instance has its own
/// seen-set).
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
    use super::{
        ci_attention, diff, merge_raw, short_repo, source_key, type_enabled, Attention, Seen,
        SourceResult,
    };
    use orrery_core::inbox::{InboxItem, Notification};
    use orrery_core::model::{AppConfig, Host};
    use std::collections::BTreeMap;

    fn item(key: &str) -> Attention {
        Attention {
            kind: "pr",
            key: key.into(),
            label: key.into(),
            title: String::new(),
            body: String::new(),
        }
    }

    fn src(key: &str, items: Option<Vec<Attention>>) -> SourceResult {
        (key.to_string(), items)
    }

    fn keys(fresh: &[&Attention]) -> Vec<String> {
        fresh.iter().map(|a| a.key.clone()).collect()
    }

    #[test]
    fn first_success_seeds_silently() {
        let sources = vec![src("inbox", Some(vec![item("a"), item("b")]))];
        let (fresh, seen) = diff(&Seen::new(), &sources);
        assert!(fresh.is_empty(), "first sign-in must not notify the inbox");
        assert_eq!(seen["inbox"].len(), 2);
    }

    #[test]
    fn new_items_after_a_seed_notify() {
        let (_, seen) = diff(&Seen::new(), &[src("inbox", Some(vec![item("a")]))]);
        let sources = vec![src("inbox", Some(vec![item("a"), item("b")]))];
        let (fresh, _) = diff(&seen, &sources);
        assert_eq!(keys(&fresh), ["b"]);
    }

    #[test]
    fn a_failed_source_keeps_its_snapshot() {
        // Seeded, then an outage (fetch fails / token missing), then recovery:
        // only items that are genuinely new since before the outage notify.
        let (_, seen) = diff(&Seen::new(), &[src("inbox", Some(vec![item("a")]))]);
        let outage = [src("inbox", None)];
        let (fresh, during) = diff(&seen, &outage);
        assert!(fresh.is_empty());
        assert_eq!(during, seen, "outage must not overwrite the snapshot");
        let sources = vec![src("inbox", Some(vec![item("a"), item("c")]))];
        let (fresh, _) = diff(&during, &sources);
        assert_eq!(keys(&fresh), ["c"]);
    }

    #[test]
    fn sources_are_tracked_independently() {
        // The inbox is seeded; notifications fail on the first poll and so
        // seed silently when they first succeed, without affecting the inbox.
        let first = vec![
            src("inbox", Some(vec![item("pr")])),
            src("notifications", None),
        ];
        let (_, seen) = diff(&Seen::new(), &first);
        assert!(!seen.contains_key("notifications"));
        let second = vec![
            src("inbox", Some(vec![item("pr"), item("pr2")])),
            src("notifications", Some(vec![item("ci")])),
        ];
        let (fresh, seen) = diff(&seen, &second);
        assert_eq!(keys(&fresh), ["pr2"]);
        assert!(seen["notifications"].contains("ci"));
    }

    #[test]
    fn github_keeps_its_keys_and_other_hosts_get_their_own() {
        // GitHub's seen-sets predate multi-host, so its keys must not change
        // (or every user would re-seed); each other instance is separate.
        assert_eq!(source_key("inbox", Host::Github, "github.com"), "inbox");
        assert_eq!(
            source_key("notifications", Host::Github, "github.com"),
            "notifications"
        );
        assert_eq!(
            source_key("inbox", Host::Gitlab, "gitlab.acme.io"),
            "inbox:gitlab:gitlab.acme.io"
        );
        assert_eq!(
            source_key("inbox", Host::Gitea, "codeberg.org"),
            "inbox:gitea:codeberg.org"
        );
    }

    #[test]
    fn a_newly_connected_host_seeds_silently_beside_a_seeded_one() {
        // GitHub is seeded; GitLab is connected later with a backlog of open
        // MRs. Its first successful poll must not notify that backlog, while
        // GitHub keeps notifying genuinely new items.
        let (_, seen) = diff(&Seen::new(), &[src("inbox", Some(vec![item("a")]))]);
        let gitlab = source_key("inbox", Host::Gitlab, "gitlab.com");
        let second = vec![
            src("inbox", Some(vec![item("a"), item("b")])),
            src(&gitlab, Some(vec![item("old1"), item("old2")])),
        ];
        let (fresh, seen) = diff(&seen, &second);
        assert_eq!(keys(&fresh), ["b"]);
        assert_eq!(seen[&gitlab].len(), 2);
        let third = vec![
            src("inbox", Some(vec![item("a"), item("b")])),
            src(&gitlab, Some(vec![item("old1"), item("old2"), item("new")])),
        ];
        let (fresh, _) = diff(&seen, &third);
        assert_eq!(keys(&fresh), ["new"]);
    }

    fn raw(url: &str, host: Host) -> InboxItem {
        InboxItem {
            kind: "pr".into(),
            title: "t".into(),
            repo: "o/r".into(),
            url: url.into(),
            number: 1,
            draft: false,
            host,
            domain: String::new(),
            created_at: 0,
            waiting_since: 0,
        }
    }

    fn urls(items: &Option<Vec<InboxItem>>) -> Option<Vec<String>> {
        items
            .as_ref()
            .map(|v| v.iter().map(|i| i.url.clone()).collect())
    }

    #[test]
    fn raw_facts_fall_back_per_source() {
        let mut last = BTreeMap::new();
        // Nothing connected: an empty (known) list.
        assert_eq!(urls(&merge_raw(&mut last, vec![])), Some(vec![]));
        // Everything failed with nothing known yet: keep the caller's facts.
        let failed = vec![("inbox".to_string(), None)];
        assert_eq!(urls(&merge_raw(&mut last, failed)), None);
        // Both hosts answer.
        let both = vec![
            ("inbox".to_string(), Some(vec![raw("gh1", Host::Github)])),
            (
                "inbox:gitlab:gitlab.com".to_string(),
                Some(vec![raw("gl1", Host::Gitlab)]),
            ),
        ];
        assert_eq!(
            urls(&merge_raw(&mut last, both)),
            Some(vec!["gh1".into(), "gl1".into()])
        );
        // GitLab blips: its last good items stay in the facts.
        let blip = vec![
            ("inbox".to_string(), Some(vec![raw("gh2", Host::Github)])),
            ("inbox:gitlab:gitlab.com".to_string(), None),
        ];
        assert_eq!(
            urls(&merge_raw(&mut last, blip)),
            Some(vec!["gh2".into(), "gl1".into()])
        );
        // GitLab disconnected (no longer a source): its items are dropped.
        let gone = vec![("inbox".to_string(), None)];
        assert_eq!(urls(&merge_raw(&mut last, gone)), Some(vec!["gh2".into()]));
    }

    fn note(host: Host, kind: &str, reason: &str) -> Notification {
        Notification {
            title: "build".into(),
            repo: "o/r".into(),
            reason: reason.into(),
            kind: kind.into(),
            url: None,
            host,
            domain: String::new(),
            updated_at: 0,
        }
    }

    #[test]
    fn ci_alerts_come_from_check_suites_and_failed_build_todos() {
        let notes = vec![
            note(Host::Github, "CheckSuite", "ci_activity"),
            note(Host::Github, "PullRequest", "review_requested"),
            note(Host::Gitlab, "MergeRequest", "build_failed"),
            note(Host::Gitlab, "MergeRequest", "mentioned"),
            note(Host::Gitea, "Commit", "open"),
        ];
        let ci = ci_attention(&notes);
        assert_eq!(ci.len(), 2);
        assert!(ci.iter().all(|a| a.kind == "ci"));
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
