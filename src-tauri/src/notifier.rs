//! Background attention poller (#70). On an interval it asks GitHub what needs
//! your attention — new PRs, review requests, CI/check alerts — fires a native
//! notification for each *newly-appeared* item (opt-in per type, deduped
//! against the previous poll), and refreshes the tray quick-glance so the count
//! and recent repos stay current even while the window is hidden.

use std::collections::HashSet;
use std::time::Duration;

use tauri::AppHandle;

use crate::{cache, config, inbox, oauth, tray};

const POLL_SECS: u64 = 180;
/// Snapshot of the keys seen on the previous poll, for delta detection.
const SEEN_KEY: &str = "attention_seen";

/// One thing needing attention: a stable key for dedupe, a one-line tray label,
/// and a title/body for the notification.
struct Attention {
    /// "pr" | "review" | "ci" — selects the per-type opt-in toggle.
    kind: &'static str,
    key: String,
    label: String,
    title: String,
    body: String,
}

/// Spawn the poll loop on the async runtime. Runs once immediately (to seed the
/// dedupe snapshot and paint the tray), then every `POLL_SECS`.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            run_once(&app).await;
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        }
    });
}

async fn run_once(app: &AppHandle) {
    let cfg = config::load();
    let items = collect().await;

    // The tray glance always reflects current state, regardless of the
    // notification toggles — it's a passive readout, not a notification.
    let lines: Vec<String> = items.iter().map(|a| a.label.clone()).collect();
    tray::update(app, &lines);

    // Delta detection: only items absent from the previous snapshot are "new".
    // On the very first run (no snapshot yet) we seed silently — otherwise the
    // entire current inbox would notify at once on launch.
    let prev: Option<HashSet<String>> =
        cache::get_meta(SEEN_KEY).and_then(|s| serde_json::from_str(&s).ok());
    let current: HashSet<String> = items.iter().map(|a| a.key.clone()).collect();

    if let Some(prev) = prev {
        if cfg.notify_enabled {
            for a in &items {
                if prev.contains(&a.key) || !type_enabled(&cfg, a.kind) {
                    continue;
                }
                fire(app, a);
            }
        }
    }

    if let Ok(blob) = serde_json::to_string(&current) {
        cache::set_meta(SEEN_KEY, &blob);
    }
}

fn type_enabled(cfg: &crate::model::AppConfig, kind: &str) -> bool {
    match kind {
        "pr" => cfg.notify_new_pr,
        "review" => cfg.notify_review_requested,
        "ci" => cfg.notify_ci_failure,
        _ => false,
    }
}

fn fire(app: &AppHandle, a: &Attention) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(&a.title).body(&a.body).show();
}

/// The trailing path segment of an `owner/name` slug, for compact tray labels.
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
