//! Cleanup: scan the fleet for prunable branches, and the two-stage
//! (arm-then-confirm) per-repo prune.

use gpui::{Context, SharedString};

use crate::shell::OrreryApp;
use crate::toast::ToastKind;

/// The error toast (title, detail) for a branch-prune outcome, if anything
/// went wrong: the whole prune failing, or individual branches that couldn't
/// be deleted. `None` when every prunable branch went.
fn prune_failure(
    report: &Result<orrery_core::git_ops::PruneReport, String>,
) -> Option<(String, String)> {
    match report {
        Err(e) => Some(("Prune failed".into(), crate::data::oneline(e.clone()))),
        Ok(r) if r.failed.is_empty() => None,
        Ok(r) => {
            let n = r.failed.len();
            let title = format!(
                "Couldn't delete {n} branch{}",
                if n == 1 { "" } else { "es" }
            );
            let detail = r
                .failed
                .iter()
                .map(|(branch, err)| format!("{branch}: {err}"))
                .collect::<Vec<_>>()
                .join("; ");
            Some((title, crate::data::oneline(detail)))
        }
    }
}

impl OrreryApp {
    /// Scan all repos for prunable branches (sync git, off-thread).
    pub fn load_cleanup(&mut self, cx: &mut Context<Self>) {
        use crate::views::cleanup::CleanupState;
        self.cleanup = CleanupState::Loading;
        self.cleanup_confirm = None;
        cx.notify();
        let rows = self.rows.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = CleanupState::Ready(result);
                // Fresh prunable counts → refresh the attention surfaces.
                this.recompute_attention();
                cx.notify();
            });
        })
        .detach();
    }

    /// First click on a Cleanup "Prune" button: arm a two-stage confirm for that
    /// repo (the button flips to "Confirm prune {n}?"). Deleting branches is
    /// irreversible, so it only runs on the confirming second click. The armed
    /// state reverts after a few seconds, and arming another repo's button
    /// replaces it.
    pub fn arm_prune(&mut self, id: SharedString, cx: &mut Context<Self>) {
        self.cleanup_confirm = Some(id);
        self.cleanup_confirm_gen += 1;
        let generation = self.cleanup_confirm_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            // Revert only if this timer's arm is still the active one.
            let _ = this.update(cx, |this, cx| {
                if this.cleanup_confirm_gen == generation && this.cleanup_confirm.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Prune the given repo's stale branches, then refresh the Cleanup list.
    /// Only reached via the two-stage confirm in the Cleanup view (`arm_prune`).
    pub fn prune_repo(&mut self, id: SharedString, cx: &mut Context<Self>) {
        self.cleanup_confirm = None;
        cx.notify();
        let path = id.to_string();
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::prune_branches(&path) })
                .await;
            let Ok(rows) = this.update(cx, |this, cx| {
                // Surface what couldn't be pruned (a branch checked out in a
                // worktree, a permissions error, …) instead of dropping it.
                if let Some((title, detail)) = prune_failure(&report) {
                    this.push_toast(ToastKind::Error, title, Some(detail.into()), cx);
                }
                this.rows.clone()
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = crate::views::cleanup::CleanupState::Ready(result);
                // Pruning changed the prunable counts → refresh attention.
                this.recompute_attention();
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_failure_reports_only_failures() {
        use orrery_core::git_ops::PruneReport;
        let clean = Ok(PruneReport {
            deleted: vec!["old".into()],
            failed: Vec::new(),
        });
        assert!(prune_failure(&clean).is_none());
        let partial = Ok(PruneReport {
            deleted: Vec::new(),
            failed: vec![("wip".into(), "checked out\nin a worktree".into())],
        });
        let (title, detail) = prune_failure(&partial).unwrap();
        assert_eq!(title, "Couldn't delete 1 branch");
        assert_eq!(detail, "wip: checked out in a worktree");
        let (title, _) = prune_failure(&Err("not a repo".into())).unwrap();
        assert_eq!(title, "Prune failed");
    }
}
