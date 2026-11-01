//! Fleet hygiene, app side (PR #224's core APIs wired in):
//!
//! - **at-risk work** (`orrery_core::at_risk`), assessed off the UI thread
//!   after every snapshot and fed to the attention model;
//! - **security advisories** (`orrery_core::advisories`): Dependabot counts on
//!   the CI pass's cadence (tokio runtime), plus the opt-in local audits on the
//!   background executor when `local_audits` is on;
//! - the Cleanup view's **disk-usage cleaner** (`orrery_core::janitor`):
//!   cached sizes shown at once, stale repos re-measured off the UI thread,
//!   and the two-step-confirmed purge.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gpui::{Context, SharedString};
use orrery_core::advisories::{self, AdvisoryReport, AdvisorySource, SeverityCounts};
use orrery_core::at_risk::{self, AtRisk};
use orrery_core::janitor::{self, ArtifactKind, DiskUsage, PurgeReport};
use orrery_core::model::{Host, Repo};

use super::OrreryApp;
use super::passes::Pass;
use crate::toast::ToastKind;

/// What a card shows for a repo's advisories. `Copy`, so it rides in
/// `CardState`; the tooltip text is looked up lazily (see
/// [`HygieneState::advisory_tooltip`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvisoryBadge {
    /// At least one open advisory.
    Open(SeverityCounts),
    /// The source can't answer for this repo (no permission, audit failed).
    Unavailable,
}

/// The fleet-hygiene state, grouped out of [`OrreryApp`].
#[derive(Default)]
pub struct HygieneState {
    /// Non-empty at-risk reports from the last assessment, by repo id.
    pub at_risk: Vec<AtRisk>,
    /// Cached advisory reports keyed (source host, key), as in
    /// `advisory_cache`. Seeded from the cache at launch, reloaded after each
    /// pass that wrote something.
    pub advisories: HashMap<(String, String), AdvisoryReport>,
    /// Per-repo card badge + tooltip, rebuilt with the attention model.
    badges: HashMap<SharedString, (AdvisoryBadge, SharedString)>,
    /// The last whole-pass Dependabot failure toasted, so an ongoing outage
    /// toasts once per distinct error (the CI pass's rule).
    advisory_error: Option<String>,
    /// Artefact sizes by repo id; `None` until the Cleanup view first loads
    /// them. Includes stale entries (last known beats nothing).
    pub disk: Option<HashMap<String, DiskUsage>>,
    /// A measuring pass is running (the panel says so).
    pub measuring: bool,
    /// Repo ids with a purge in flight.
    pub purging: HashSet<SharedString>,
    /// Repo ids with an at-risk "Push" in flight.
    pub pushing: HashSet<SharedString>,
    at_risk_pass: Pass,
    advisory_pass: Pass,
    audit_pass: Pass,
    disk_pass: Pass,
}

impl HygieneState {
    /// Launch state: the cached advisory reports (a quick SQLite read, like
    /// the CI states), so badges paint on the first frame.
    pub fn seeded() -> Self {
        HygieneState {
            advisories: orrery_core::cache::all_advisories(),
            ..Default::default()
        }
    }

    /// The cached report that applies to `repo`: Dependabot by (domain,
    /// slug) for a GitHub remote, else its own local audit. Mirrors
    /// `advisories::facts`.
    pub fn advisory_for(&self, repo: &Repo) -> Option<&AdvisoryReport> {
        report_for(&self.advisories, repo)
    }

    /// The card badge for repo `id`, if it has one.
    pub fn badge(&self, id: &str) -> Option<AdvisoryBadge> {
        self.badges.get(id).map(|(b, _)| *b)
    }

    /// The badge's tooltip (breakdown, or why it's unavailable).
    pub fn advisory_tooltip(&self, id: &str) -> Option<SharedString> {
        self.badges.get(id).map(|(_, t)| t.clone())
    }

    /// Rebuild the per-repo badges from the reports. A linked worktree shares
    /// its main checkout's Dependabot answer, so only cards (main checkouts)
    /// matter here.
    fn rebuild_badges(&mut self, repos: &[Repo]) {
        self.badges = repos
            .iter()
            .filter(|r| r.worktree_of.is_none())
            .filter_map(|r| {
                let report = self.advisory_for(r)?;
                let badge = badge_of(report)?;
                Some((r.id.clone().into(), (badge, describe(report).into())))
            })
            .collect();
    }
}

fn report_for<'a>(
    reports: &'a HashMap<(String, String), AdvisoryReport>,
    repo: &Repo,
) -> Option<&'a AdvisoryReport> {
    match (&repo.slug, repo.host) {
        (Some(slug), Some(Host::Github)) => {
            reports.get(&(repo.remote_host.clone().unwrap_or_default(), slug.clone()))
        }
        _ => reports.get(&(advisories::LOCAL_HOST.to_string(), repo.id.clone())),
    }
}

/// The card badge for a report: open advisories, unavailable, or nothing
/// (a clean answer shows no badge on the card).
pub fn badge_of(report: &AdvisoryReport) -> Option<AdvisoryBadge> {
    if !report.available {
        Some(AdvisoryBadge::Unavailable)
    } else if report.counts.total() > 0 {
        Some(AdvisoryBadge::Open(report.counts))
    } else {
        None
    }
}

/// The badge text: the highest tier's count, e.g. "3 high". The full
/// breakdown lives in the tooltip.
pub fn badge_label(c: &SeverityCounts) -> (String, Tier) {
    let (n, word, tier) = if c.critical > 0 {
        (c.critical, "critical", Tier::Severe)
    } else if c.high > 0 {
        (c.high, "high", Tier::Severe)
    } else if c.medium > 0 {
        (c.medium, "medium", Tier::Moderate)
    } else if c.low > 0 {
        (c.low, "low", Tier::Mild)
    } else {
        (c.unknown, "unknown", Tier::Mild)
    };
    (format!("{n} {word}"), tier)
}

/// How loud a badge is (picks its colour token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Severe,
    Moderate,
    Mild,
}

/// One line describing a report: the breakdown and source, or why it's
/// unavailable, with the re-connect hint for a token minted before the
/// Dependabot scope was requested.
pub fn describe(report: &AdvisoryReport) -> String {
    let source = report.source.label();
    if !report.available {
        let reason = report.reason.as_deref().unwrap_or("no answer");
        return if report.source == AdvisorySource::Dependabot && reason.contains("security_events")
        {
            format!("{source} unavailable: re-connect GitHub to enable (Settings › GitHub account)")
        } else {
            format!(
                "{source} unavailable: {}",
                crate::data::oneline(reason.to_string())
            )
        };
    }
    let c = &report.counts;
    if c.total() == 0 {
        return format!("No known vulnerable dependencies ({source})");
    }
    let parts: Vec<String> = [
        (c.critical, "critical"),
        (c.high, "high"),
        (c.medium, "medium"),
        (c.low, "low"),
        (c.unknown, "unknown"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("{n} {w}"))
    .collect();
    format!("{} ({source})", parts.join(", "))
}

/// The toasts for a purge outcome, as (kind, title, detail): bytes freed on
/// success, then each refusal (with its reason) and each failure as errors,
/// so nothing that was left on disk goes unexplained.
pub fn purge_toasts(
    report: &Result<PurgeReport, String>,
) -> Vec<(ToastKind, String, Option<String>)> {
    let r = match report {
        Err(e) => {
            return vec![(
                ToastKind::Error,
                "Purge failed".into(),
                Some(crate::data::oneline(e.clone())),
            )];
        }
        Ok(r) => r,
    };
    let mut out = Vec::new();
    if !r.removed.is_empty() || (r.refused.is_empty() && r.failed.is_empty()) {
        let n = r.removed.len();
        out.push((
            ToastKind::Success,
            format!("Freed {}", janitor::format_bytes(r.freed_bytes)),
            Some(format!(
                "Removed {n} artefact director{}",
                if n == 1 { "y" } else { "ies" }
            )),
        ));
    }
    let list = |items: &[(String, String)]| {
        crate::data::oneline(
            items
                .iter()
                .map(|(p, why)| format!("{}: {why}", short_path(p)))
                .collect::<Vec<_>>()
                .join("; "),
        )
    };
    if !r.refused.is_empty() {
        let n = r.refused.len();
        out.push((
            ToastKind::Error,
            format!("Refused {n} director{}", if n == 1 { "y" } else { "ies" }),
            Some(list(&r.refused)),
        ));
    }
    if !r.failed.is_empty() {
        let n = r.failed.len();
        out.push((
            ToastKind::Error,
            format!(
                "Couldn't delete {n} director{}",
                if n == 1 { "y" } else { "ies" }
            ),
            Some(list(&r.failed)),
        ));
    }
    out
}

/// The last two components of a path, for compact toast details.
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.trim_end_matches('/').rsplitn(3, '/').collect();
    match parts.as_slice() {
        [last, parent, _] => format!("{parent}/{last}"),
        _ => p.to_string(),
    }
}

/// The confirm key for purging `kind` (or every kind, `None`) in `repo`.
pub fn purge_key(repo: &str, kind: Option<ArtifactKind>) -> SharedString {
    let tag = kind.map(|k| k.label()).unwrap_or("all");
    format!("{repo}\u{1f}{tag}").into()
}

impl OrreryApp {
    /// Assess at-risk work across the fleet (git-heavy, on the background
    /// executor), then feed it to the attention model. Single-flight; runs
    /// after every snapshot.
    pub fn assess_at_risk(&mut self, cx: &mut Context<Self>) {
        if !self.hygiene.at_risk_pass.begin() {
            return;
        }
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let reports = cx
                .background_executor()
                .spawn(async move {
                    at_risk::assess_fleet(&repos, &at_risk::Thresholds::default(), now)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.hygiene.at_risk = reports;
                this.recompute_attention();
                if this.hygiene.at_risk_pass.finish() {
                    this.assess_at_risk(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh security advisories: the Dependabot pass on the tokio runtime
    /// (TTL-paced in core, so cheap to call on every CI tick), and the local
    /// audits on the background executor only when `local_audits` is on.
    /// Both are single-flight.
    pub fn refresh_advisories(&mut self, cx: &mut Context<Self>) {
        if self.config.local_audits {
            self.refresh_local_audits(cx);
        }
        if !self.hygiene.advisory_pass.begin() {
            return;
        }
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let outcome =
                crate::task::run(async move { advisories::refresh(&repos, now, false).await })
                    .await;
            let attempted = outcome
                .as_ref()
                .is_some_and(|o| o.updated > 0 || o.errors > 0);
            let reports = if attempted {
                Some(
                    cx.background_executor()
                        .spawn(async { orrery_core::cache::all_advisories() })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(o) = &outcome {
                    this.toast_advisory_outcome(o, cx);
                }
                if let Some(reports) = reports {
                    this.hygiene.advisories = reports;
                    this.recompute_attention();
                    cx.notify();
                }
                if this.hygiene.advisory_pass.finish() {
                    this.refresh_advisories(cx);
                }
            });
        })
        .detach();
    }

    /// One error toast per distinct whole-pass Dependabot failure (expired
    /// token, rate limit): the cache keeps its last answer, so this is how
    /// the problem becomes visible.
    fn toast_advisory_outcome(&mut self, o: &advisories::Outcome, cx: &mut Context<Self>) {
        if o.all_failed() {
            if self.hygiene.advisory_error != o.last_error
                && let Some(err) = &o.last_error
            {
                self.upsert_toast(
                    "advisory-pass",
                    ToastKind::Error,
                    "Security advisory check failed",
                    Some(crate::data::oneline(err.clone()).into()),
                    cx,
                );
            }
            self.hygiene.advisory_error = o.last_error.clone();
        } else {
            self.hygiene.advisory_error = None;
        }
    }

    /// The opt-in local audits (`cargo audit` / `npm audit`) for repos
    /// without a GitHub remote: blocking and time-boxed in core, so they run
    /// on the background executor.
    fn refresh_local_audits(&mut self, cx: &mut Context<Self>) {
        if !self.hygiene.audit_pass.begin() {
            return;
        }
        let repos = self.repos.clone();
        let cfg = self.config.clone();
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let reports = cx
                .background_executor()
                .spawn(async move {
                    let o = advisories::refresh_local(&repos, &cfg, now, false);
                    (o.updated > 0 || o.errors > 0).then(orrery_core::cache::all_advisories)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(reports) = reports {
                    this.hygiene.advisories = reports;
                    this.recompute_attention();
                    cx.notify();
                }
                if this.hygiene.audit_pass.finish() {
                    this.refresh_local_audits(cx);
                }
            });
        })
        .detach();
    }

    /// Fold the hygiene state into the advisory badges (called from
    /// `recompute_attention`, so badges follow every snapshot and pass).
    pub(super) fn rebuild_advisory_badges(&mut self) {
        let repos = std::mem::take(&mut self.repos);
        self.hygiene.rebuild_badges(&repos);
        self.repos = repos;
    }

    /// Load the Cleanup view's disk usage: cached sizes first (shown at
    /// once), then re-measure every repo whose measurement is older than the
    /// janitor TTL (all of them when `force`) off the UI thread. Linked
    /// worktrees are measured too: each has its own `target/` and
    /// `node_modules/`.
    pub fn load_disk_usage(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.hygiene.disk_pass.begin() {
            return;
        }
        self.hygiene.measuring = true;
        cx.notify();
        let ids: Vec<String> = self.repos.iter().map(|r| r.id.clone()).collect();
        cx.spawn(async move |this, cx| {
            let cached = cx
                .background_executor()
                .spawn(async { orrery_core::cache::all_disk_usage() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.hygiene.disk = Some(cached);
                cx.notify();
            });
            let now = crate::data::now_unix();
            let fresh = cx
                .background_executor()
                .spawn(async move {
                    let fresh = if force {
                        HashSet::new()
                    } else {
                        orrery_core::cache::fresh_disk_usage_ids(janitor::TTL_SECS, now)
                    };
                    let stale: Vec<String> =
                        ids.into_iter().filter(|id| !fresh.contains(id)).collect();
                    if !stale.is_empty() {
                        let cancel = std::sync::atomic::AtomicBool::new(false);
                        orrery_core::fleet::run(
                            &stale,
                            orrery_core::fleet::default_workers().min(4),
                            &cancel,
                            |_| {},
                            janitor::measure_op(now),
                        );
                    }
                    orrery_core::cache::all_disk_usage()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.hygiene.disk = Some(fresh);
                this.hygiene.measuring = false;
                if this.hygiene.disk_pass.finish() {
                    this.load_disk_usage(false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// First click on a disk-cleaner Purge button: arm its two-step confirm.
    pub fn arm_purge(&mut self, key: SharedString, cx: &mut Context<Self>) {
        self.arm_confirm(|app| &mut app.cleanup_ui.purge_confirm, key, cx);
    }

    /// Purge `kinds` of build artefacts in `repo` (reached only through the
    /// armed confirm). Core re-validates every directory before deleting it;
    /// the outcome toasts the bytes freed, and every refusal with its reason.
    /// The repo is re-measured afterwards so the panel shows what's left.
    pub fn purge_artifacts(
        &mut self,
        repo: SharedString,
        kinds: Vec<ArtifactKind>,
        cx: &mut Context<Self>,
    ) {
        self.cleanup_ui.purge_confirm.disarm();
        if !self.hygiene.purging.insert(repo.clone()) {
            return;
        }
        cx.notify();
        let key = format!("purge:{repo}");
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            format!("Purging build artefacts in {name}…"),
            None,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let (report, usage) = cx
                .background_executor()
                .spawn(async move {
                    let path = Path::new(&id);
                    let report = janitor::purge(path, &kinds);
                    let now = crate::data::now_unix();
                    let usage = janitor::measure(path, janitor::Limits::default(), now).ok();
                    if let Some(u) = &usage {
                        let _ = orrery_core::cache::store_disk_usage(u);
                    }
                    (report, usage)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.hygiene.purging.remove(&repo);
                if let Some(disk) = &mut this.hygiene.disk {
                    match usage {
                        Some(u) => {
                            disk.insert(repo.to_string(), u);
                        }
                        None => {
                            disk.remove(repo.as_ref());
                        }
                    }
                }
                let mut toasts = purge_toasts(&report).into_iter();
                if let Some((kind, title, detail)) = toasts.next() {
                    this.upsert_toast(key, kind, title, detail.map(Into::into), cx);
                }
                for (kind, title, detail) in toasts {
                    this.push_toast(kind, title, detail.map(Into::into), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// At-risk "Push": push the checked-out branch (the same `git_ops::push`
    /// as the drawer's Push), then rescan the repo so the at-risk entry and
    /// attention update.
    pub fn push_at_risk(&mut self, repo: SharedString, cx: &mut Context<Self>) {
        if !self.hygiene.pushing.insert(repo.clone()) {
            return;
        }
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        let key = format!("push:{repo}");
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            format!("Pushing {name}…"),
            None,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let result = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::push(&id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.hygiene.pushing.remove(&repo);
                match result {
                    Ok(msg) => {
                        this.upsert_toast(key, ToastKind::Success, "Pushed", Some(msg.into()), cx);
                    }
                    Err(e) => {
                        this.upsert_toast(key, ToastKind::Error, "Push failed", Some(e.into()), cx);
                    }
                }
                let mut changes = orrery_platform::watcher::ChangeSet::default();
                changes
                    .repos
                    .insert(std::path::PathBuf::from(repo.as_ref()), false);
                this.request_changes(changes, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(available: bool, counts: SeverityCounts, reason: Option<&str>) -> AdvisoryReport {
        AdvisoryReport {
            source: AdvisorySource::Dependabot,
            available,
            counts,
            reason: reason.map(String::from),
            checked_at: 0,
        }
    }

    #[test]
    fn badge_shows_the_highest_tier_and_hides_clean_repos() {
        let c = SeverityCounts {
            high: 3,
            low: 2,
            ..Default::default()
        };
        assert_eq!(badge_label(&c), ("3 high".to_string(), Tier::Severe));
        let c = SeverityCounts {
            medium: 1,
            ..Default::default()
        };
        assert_eq!(badge_label(&c), ("1 medium".to_string(), Tier::Moderate));
        assert_eq!(
            badge_of(&report(true, SeverityCounts::default(), None)),
            None,
            "a clean answer shows no badge"
        );
        assert_eq!(
            badge_of(&report(false, SeverityCounts::default(), Some("x"))),
            Some(AdvisoryBadge::Unavailable)
        );
    }

    #[test]
    fn missing_scope_suggests_reconnecting_github() {
        let r = report(
            false,
            SeverityCounts::default(),
            Some("no access to Dependabot alerts (the token needs the security_events scope)"),
        );
        assert!(describe(&r).contains("re-connect GitHub"));
        let r = report(
            false,
            SeverityCounts::default(),
            Some("alerts are disabled"),
        );
        assert_eq!(describe(&r), "Dependabot unavailable: alerts are disabled");
        let r = report(
            true,
            SeverityCounts {
                critical: 1,
                medium: 2,
                ..Default::default()
            },
            None,
        );
        assert_eq!(describe(&r), "1 critical, 2 medium (Dependabot)");
    }

    #[test]
    fn purge_toasts_report_bytes_and_every_refusal() {
        let ok = Ok(PurgeReport {
            freed_bytes: 1_400_000_000,
            removed: vec!["/r/target".into()],
            refused: vec![(
                "/r/app/node_modules".into(),
                "contains tracked files".into(),
            )],
            failed: Vec::new(),
        });
        let t = purge_toasts(&ok);
        assert_eq!(t.len(), 2);
        assert!(t[0].0 == ToastKind::Success);
        assert_eq!(t[0].1, "Freed 1.4 GB");
        assert_eq!(t[0].2.as_deref(), Some("Removed 1 artefact directory"));
        assert!(t[1].0 == ToastKind::Error);
        assert_eq!(t[1].1, "Refused 1 directory");
        assert_eq!(
            t[1].2.as_deref(),
            Some("app/node_modules: contains tracked files")
        );

        // Everything refused: no success toast claiming bytes were freed.
        let refused = Ok(PurgeReport {
            refused: vec![("/r/x".into(), "is a symlink".into())],
            ..PurgeReport::default()
        });
        let t = purge_toasts(&refused);
        assert_eq!(t.len(), 1);
        assert!(t[0].0 == ToastKind::Error);

        let t = purge_toasts(&Err("not a repo".into()));
        assert_eq!(t[0].1, "Purge failed");
    }

    #[test]
    fn purge_keys_are_per_repo_and_kind() {
        assert_ne!(
            purge_key("/a", Some(ArtifactKind::NodeModules)),
            purge_key("/a", None)
        );
        assert_ne!(purge_key("/a", None), purge_key("/b", None));
    }
}
