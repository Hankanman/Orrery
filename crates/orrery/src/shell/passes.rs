//! The background passes behind the grid: the single-flight guards
//! ([`Pass`], [`SnapshotPass`]), the snapshot pipeline (rescan / scoped
//! refresh / cache reload) and the post-snapshot fan-out (semantic index,
//! host enrichment, CI, contribution graph, agent sessions).

use gpui::{Context, SharedString};
use orrery_platform::watcher::ChangeSet;

use super::OrreryApp;
use crate::toast::ToastKind;

/// Single-flight state for one background pass: at most one run in flight;
/// requests arriving meanwhile coalesce into one trailing run, which starts
/// after the current one lands and so sees the latest app state. This keeps an
/// older result from overwriting a newer one, and stops two overlapping agent
/// scans from both reporting the same session as finished.
#[derive(Default)]
pub struct Pass {
    running: bool,
    queued: bool,
}

impl Pass {
    /// Claim the pass. `false` when a run is already in flight — the request
    /// is then queued as the trailing run instead.
    pub(super) fn begin(&mut self) -> bool {
        if self.running {
            self.queued = true;
            false
        } else {
            self.running = true;
            true
        }
    }

    /// Release the pass; `true` when a trailing run was requested meanwhile
    /// (the caller starts it).
    pub(super) fn finish(&mut self) -> bool {
        self.running = false;
        std::mem::take(&mut self.queued)
    }
}

/// What a snapshot pass produces: a cheap reload of the cached snapshot (after
/// enrichment / summaries / cache edits), a rescan of just the repos the fs
/// watcher reported, or a full disk rescan. Each subsumes the ones before it,
/// so a queued request is upgraded rather than run twice.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SnapKind {
    Reload,
    /// Rescan the repos in [`SnapshotPass::changes`] only.
    Refresh,
    Rescan,
}

/// The snapshot pass: every producer of `rows` (rescan, cache reload) runs
/// through it, one at a time, so snapshots apply in the order they were taken.
#[derive(Default)]
pub struct SnapshotPass {
    /// The run in flight, if any.
    current: Option<SnapKind>,
    queued: Option<SnapKind>,
    /// Explore slugs whose clone finished and that should stop showing as
    /// "cloning" once a rescan started after the clone has landed.
    pub(super) settle_clones: Vec<SharedString>,
    /// Watcher-reported repo changes not yet picked up by a pass; the next
    /// Refresh/Rescan to start takes them.
    changes: ChangeSet,
}

impl SnapshotPass {
    /// Claim the pass for `kind`; `false` (and queued) when one is running.
    fn begin(&mut self, kind: SnapKind) -> bool {
        if self.current.is_some() {
            self.queued = self.queued.max(Some(kind));
            false
        } else {
            self.current = Some(kind);
            true
        }
    }

    /// Release the pass, returning the coalesced trailing request, if any.
    fn finish(&mut self) -> Option<SnapKind> {
        self.current = None;
        self.queued.take()
    }

    /// Is a disk rescan in flight or queued (the footer's "Scanning…")?
    pub(super) fn scanning(&self) -> bool {
        self.current == Some(SnapKind::Rescan) || self.queued == Some(SnapKind::Rescan)
    }
}

/// Single-flight guards + stale-result counters for the app's background
/// passes, grouped out of [`OrreryApp`].
#[derive(Default)]
pub struct Passes {
    pub snapshot: SnapshotPass,
    pub enrich: Pass,
    pub ci: Pass,
    pub activity: Pass,
    pub agents: Pass,
    pub index: Pass,
    /// Latest-issued load id for each network view; a result whose id isn't
    /// the latest (a refresh superseded it) is dropped.
    pub inbox_seq: u64,
    pub feed_seq: u64,
    pub explore_seq: u64,
    /// The persisted already-notified attention set has been read (see
    /// `OrreryApp::load_attention_seen`); urgent notifications wait for it.
    pub attention_seeded: bool,
}

impl OrreryApp {
    /// Install a fresh fleet snapshot (rows + raw repos + root count) and
    /// recompute the attention model from it. Every path that reloads `rows`
    /// goes through here, so the attention surfaces (nav badges, grid filter,
    /// card dots) never go stale relative to the grid.
    pub fn apply_snapshot(&mut self, snap: crate::data::Snapshot) {
        self.rows = snap.rows;
        self.roots = snap.roots;
        self.repos = snap.repos;
        if snap.scanned_at.is_some() {
            self.scanned_at = snap.scanned_at;
        }
        self.derived.rows_changed();
        self.recompute_attention();
    }

    /// Produce and apply a fresh snapshot of `kind` through the single-flight
    /// snapshot pass: a disk rescan or a cache reload, off the UI thread. A
    /// request while one is running coalesces into one trailing run (a rescan
    /// wins over a reload), so overlapping requests never apply out of order.
    /// A rescan is followed by the post-snapshot fan-out
    /// ([`Self::after_snapshot`]); a reload only repaints.
    pub fn request_snapshot(&mut self, kind: SnapKind, cx: &mut Context<Self>) {
        if !self.passes.snapshot.begin(kind) {
            return;
        }
        // Clones that finished before this rescan started are settled by it.
        let settle = if kind == SnapKind::Rescan {
            std::mem::take(&mut self.passes.snapshot.settle_clones)
        } else {
            Vec::new()
        };
        // A rescan of either kind takes every pending watcher change (a full
        // one still needs their working-tree flags for language invalidation).
        let changes = if kind == SnapKind::Reload {
            ChangeSet::default()
        } else {
            std::mem::take(&mut self.passes.snapshot.changes)
        };
        // A scoped rescan starts from the current fleet; the pass is serial,
        // so this is the latest applied snapshot.
        let base = (kind == SnapKind::Refresh).then(|| self.repos.clone());
        cx.notify(); // the footer shows "Scanning…" while a rescan runs
        cx.spawn(async move |this, cx| {
            let snap = cx
                .background_executor()
                .spawn(async move {
                    match (kind, base) {
                        (SnapKind::Refresh, Some(repos)) => {
                            crate::data::rescan_repos(repos, &changes)
                        }
                        (SnapKind::Reload, _) => crate::data::load(),
                        _ => crate::data::rescan(&changes),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_snapshot(snap);
                for slug in &settle {
                    this.explore_ui.cloning.remove(slug);
                }
                let next = this.passes.snapshot.finish();
                if kind != SnapKind::Reload {
                    this.after_snapshot(cx);
                }
                if let Some(next) = next {
                    this.request_snapshot(next, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Act on a watcher change set: rescan just the touched repos, or run
    /// full discovery when something appeared under a root. Changes arriving
    /// while a pass runs accumulate and are taken by the trailing pass.
    pub fn request_changes(&mut self, changes: ChangeSet, cx: &mut Context<Self>) {
        if changes.is_empty() {
            return;
        }
        let kind = if changes.discover {
            SnapKind::Rescan
        } else {
            SnapKind::Refresh
        };
        self.passes.snapshot.changes.merge(changes);
        self.request_snapshot(kind, cx);
    }

    /// The fan-out after a fresh scan (launch, watcher, explicit rescan): drop
    /// vanished selections, then refresh everything derived from the fleet —
    /// semantic index (incremental; a no-op unless AI is ready), host
    /// enrichment and CI (TTL-paced, so usually no-ops), the contribution
    /// graph, and live agent sessions (the card indicator). Each is
    /// single-flight, so calling this often is safe.
    pub fn after_snapshot(&mut self, cx: &mut Context<Self>) {
        self.prune_selection();
        self.index_semantic(cx);
        self.enrich_hosts(cx);
        self.refresh_ci(cx);
        self.load_activity(cx);
        self.scan_agents(false, cx);
        cx.notify();
    }

    /// One-shot launch work, after the window is up: the first attention pass
    /// from the launch snapshot's local git facts (so badges and card dots are
    /// right on the first paint; host/inbox/agent facts refine it as their
    /// sources load), the AI probe, and the post-snapshot fan-out — whose
    /// agents scan also detects dispatched sessions that ended while Orrery was
    /// closed and resumes supervising live ones (#185).
    pub fn startup(&mut self, cx: &mut Context<Self>) {
        self.recompute_attention();
        self.load_attention_seen(cx);
        self.ai_startup(cx);
        self.after_snapshot(cx);
    }

    /// Re-scan the roots from disk (off the UI thread) and reload the grid,
    /// then run the post-snapshot fan-out (see [`Self::request_snapshot`]).
    pub(crate) fn rescan(&mut self, cx: &mut Context<Self>) {
        // Explicit rescans follow repo/root additions (Settings save, New
        // Project, header/palette refresh) — re-arm the fs watcher so the new
        // paths get live change events too. The watcher-driven rescan
        // (`live.rs`) requests the snapshot directly, so routine fs events
        // never churn the watches.
        self.watcher.rearm();
        self.request_snapshot(SnapKind::Rescan, cx);
    }

    /// Recompute the contribution graph (commits/day across all repos) on the
    /// background pool — git history walking is slow — then store it. Cheap to
    /// call on rescan; the revwalk stops past the one-year window.
    /// Single-flight, so an older pass can't overwrite a newer graph.
    pub fn load_activity(&mut self, cx: &mut Context<Self>) {
        if !self.passes.activity.begin() {
            return;
        }
        let paths: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
        cx.spawn(async move |this, cx| {
            let activity = cx
                .background_executor()
                .spawn(async move { orrery_core::activity::compute(&paths) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.grid.activity = Some(activity);
                if this.passes.activity.finish() {
                    this.load_activity(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh host enrichment (stars/topics/issues/release/visibility) from
    /// GitHub/GitLab on the tokio runtime, then reload the grid from the freshly
    /// written cache. A no-op when every repo's cache is still within the TTL
    /// (so repeated rescans cost nothing) or when offline. Network failures are
    /// silent by design — stale enrichment simply persists. Single-flight.
    pub fn enrich_hosts(&mut self, cx: &mut Context<Self>) {
        if !self.passes.enrich.begin() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let updated =
                crate::task::run(async move { orrery_core::enrich::refresh_cached(now).await })
                    .await
                    .unwrap_or(0);
            let _ = this.update(cx, |this, cx| {
                // Rebuild rows from the enriched cache (through the snapshot
                // pass, so it can't race a rescan).
                if updated > 0 {
                    this.request_snapshot(SnapKind::Reload, cx);
                }
                if this.passes.enrich.finish() {
                    this.enrich_hosts(cx);
                }
            });
        })
        .detach();
    }

    /// Run the central CI-status pass (`orrery_core::ci`, #183) on the tokio
    /// runtime, then reload `ci_states` from the cache and recompute
    /// attention — failing default-branch CI flows to badges, tray, and
    /// notifications through the model. TTL-paced in core (~5 min), so
    /// calling this on every rescan and attention poll is cheap. Single-flight.
    ///
    /// A pass where *everything* failed (expired token, rate limit, outage)
    /// surfaces one error toast per distinct error — the cached states are
    /// deliberately left untouched (see `orrery_core::ci`), so this toast is
    /// the only way an auth problem becomes visible instead of CI silently
    /// going stale.
    pub fn refresh_ci(&mut self, cx: &mut Context<Self>) {
        if !self.passes.ci.begin() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let outcome =
                crate::task::run(async move { orrery_core::ci::refresh_cached(now).await }).await;
            // Nothing attempted (all fresh / no token / no repos), or the pass
            // itself failed — nothing to reload.
            let attempted = outcome
                .as_ref()
                .is_some_and(|o| o.updated > 0 || o.errors > 0);
            let states = if attempted {
                Some(
                    cx.background_executor()
                        .spawn(async { orrery_core::cache::all_ci_states() })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                if let (Some(outcome), Some(states)) = (outcome, states) {
                    this.apply_ci_outcome(outcome, states, cx);
                }
                if this.passes.ci.finish() {
                    this.refresh_ci(cx);
                }
            });
        })
        .detach();
    }

    /// Fold a finished CI pass into the app: the error toast (once per
    /// distinct whole-pass failure), the fresh states, and attention.
    fn apply_ci_outcome(
        &mut self,
        outcome: orrery_core::ci::Outcome,
        states: std::collections::HashMap<(String, String), orrery_core::cache::CiEntry>,
        cx: &mut Context<Self>,
    ) {
        if outcome.all_failed() {
            if self.ci_last_error != outcome.last_error
                && let Some(err) = &outcome.last_error
            {
                self.upsert_toast(
                    "ci-pass",
                    ToastKind::Error,
                    "CI status check failed",
                    Some(err.clone().into()),
                    cx,
                );
            }
            self.ci_last_error = outcome.last_error;
        } else {
            self.ci_last_error = None;
        }
        self.ci_states = states;
        self.recompute_attention();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_is_single_flight_with_one_trailing_run() {
        let mut p = Pass::default();
        assert!(p.begin());
        // Requests while running coalesce into one queued run.
        assert!(!p.begin());
        assert!(!p.begin());
        assert!(p.finish(), "a trailing run was requested");
        assert!(p.begin());
        assert!(!p.finish(), "nothing queued this time");
        assert!(p.begin(), "free again after finishing");
    }

    #[test]
    fn snapshot_pass_upgrades_a_queued_reload_to_a_rescan() {
        let mut p = SnapshotPass::default();
        assert!(p.begin(SnapKind::Reload));
        assert!(!p.scanning());
        assert!(!p.begin(SnapKind::Rescan));
        assert!(!p.begin(SnapKind::Reload));
        assert!(p.scanning(), "a queued rescan counts as scanning");
        assert_eq!(p.finish(), Some(SnapKind::Rescan));
        assert!(p.begin(SnapKind::Rescan));
        assert!(p.scanning());
        assert_eq!(p.finish(), None);
    }
}
