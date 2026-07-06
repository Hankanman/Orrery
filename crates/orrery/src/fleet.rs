//! Fleet operations UI (#100/#184): multi-select on the repo grid + the fleet
//! bar running bulk Fetch/Pull through `orrery_core::fleet`.
//!
//! Selection lives on [`OrreryApp::selected`] as repo ids, so it survives
//! filter changes (and rescans — pruned to repos that still exist). One run at
//! a time: [`OrreryApp::fleet_run`] carries the engine's cancel flag + live
//! counter and gates the bar's buttons while active. The engine fires progress
//! events on its worker threads; they're bridged over an `async-channel`
//! drained by one foreground task (the `live.rs` pattern) that keeps a keyed
//! Progress toast ("Pulling 12/40…") current. The completion resolves that
//! toast to an aggregate summary — Error with per-repo reasons when anything
//! failed, so a 40-repo pull never fails silently — then rescans once so the
//! grid reflects the new state.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

use orrery_core::fleet::{self, FleetReport, Outcome};

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;
use crate::toast::ToastKind;

/// Failed repos shown in the resolution toast's detail before "+N more".
const MAX_FAILURES_SHOWN: usize = 4;
/// Longest per-repo failure reason (chars) before it's clipped.
const MAX_REASON_CHARS: usize = 60;

/// Which bulk operation the fleet bar runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FleetOp {
    Fetch,
    Pull,
}

impl FleetOp {
    /// Present-progressive verb for the progress toast / bar counter.
    fn verb(self) -> &'static str {
        match self {
            FleetOp::Fetch => "Fetching",
            FleetOp::Pull => "Pulling",
        }
    }

    /// Imperative name for the bar buttons and the summary toast.
    fn label(self) -> &'static str {
        match self {
            FleetOp::Fetch => "Fetch",
            FleetOp::Pull => "Pull",
        }
    }
}

/// An in-flight bulk run. `id` guards stale progress events (a late event
/// can't touch a later run's toast); `cancel` is the engine's flag — flipping
/// it lets in-flight git ops finish while everything not yet started skips.
pub struct FleetRun {
    pub id: u64,
    pub op: FleetOp,
    pub cancel: Arc<AtomicBool>,
    pub done: usize,
    pub total: usize,
}

impl OrreryApp {
    /// Toggle a repo in/out of the multi-selection (card checkbox, Ctrl+click).
    pub fn toggle_selected(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        cx.notify();
    }

    /// Clear the multi-selection (fleet bar "Clear", or Esc with no overlay).
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            self.selected.clear();
            cx.notify();
        }
    }

    /// Select every row passing the current filters (the fleet bar's
    /// "Select all"). Adds to the existing selection rather than replacing it,
    /// so a hand-picked repo outside the filter isn't dropped.
    pub fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        for i in self.visible_rows() {
            let id = self.rows[i].id.clone();
            self.selected.insert(id);
        }
        cx.notify();
    }

    /// Drop selected ids that no longer exist. Called after rescans replace
    /// `rows`, so the selection (and the bar's count) never goes stale.
    pub fn prune_selection(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let ids: std::collections::HashSet<&SharedString> =
            self.rows.iter().map(|r| &r.id).collect();
        self.selected.retain(|id| ids.contains(id));
    }

    /// Flip the active run's cancel flag (fleet bar "Cancel"). In-flight git
    /// ops finish; repos not yet started report as skipped.
    pub fn cancel_fleet(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &self.fleet_run {
            run.cancel.store(true, Ordering::SeqCst);
            cx.notify();
        }
    }

    /// Run `op` across the selected repos on the background executor (one bulk
    /// run at a time). Progress marshals onto the foreground via a channel and
    /// keeps a keyed Progress toast current; completion resolves the toast to
    /// the aggregate summary and rescans once.
    pub fn run_fleet(&mut self, op: FleetOp, cx: &mut Context<Self>) {
        if self.fleet_run.is_some() {
            return;
        }
        // Row order (not hash order), so results/failures read like the grid.
        let repos: Vec<String> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.id))
            .map(|r| r.id.to_string())
            .collect();
        if repos.is_empty() {
            return;
        }
        let total = repos.len();
        self.fleet_seq += 1;
        let run_id = self.fleet_seq;
        let cancel = Arc::new(AtomicBool::new(false));
        self.fleet_run = Some(FleetRun {
            id: run_id,
            op,
            cancel: cancel.clone(),
            done: 0,
            total,
        });
        let key = SharedString::from(format!("fleet:{run_id}"));
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            format!("{} 0/{total}…", op.verb()),
            None,
            cx,
        );

        // Progress events fire on the engine's worker threads; bridge them over
        // a channel drained by one foreground task (the live.rs pattern).
        let (tx, rx) = async_channel::unbounded::<fleet::FleetEvent>();
        {
            let key = key.clone();
            cx.spawn(async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    let applied = this.update(cx, |this, cx| {
                        // Only the still-active run updates the toast: a stale
                        // queued event must not overwrite the resolution.
                        let verb = match &mut this.fleet_run {
                            Some(run) if run.id == run_id => {
                                run.done = ev.done;
                                run.op.verb()
                            }
                            _ => return,
                        };
                        this.upsert_toast(
                            key.clone(),
                            ToastKind::Progress,
                            format!("{verb} {}/{}…", ev.done, ev.total),
                            None,
                            cx,
                        );
                    });
                    if applied.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move {
                    let progress = move |ev: fleet::FleetEvent| {
                        let _ = tx.try_send(ev);
                        // `tx` (owned by this closure) drops when the engine
                        // returns, closing the channel and ending the drain.
                    };
                    let workers = fleet::default_workers();
                    match op {
                        FleetOp::Fetch => {
                            fleet::run(&repos, workers, &cancel, progress, fleet::fetch_op())
                        }
                        FleetOp::Pull => {
                            fleet::run(&repos, workers, &cancel, progress, fleet::pull_op())
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.fleet_run = None;
                let (kind, title) = summary(op, &report);
                this.upsert_toast(key, kind, title, failure_detail(&report), cx);
                // One rescan for the whole run (not per repo) so the grid
                // reflects the new ahead/behind/dirty state.
                this.rescan(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The fleet action bar, pinned under the grid. `None` (costing nothing)
    /// until a selection exists or a run is active. Buttons disable while a
    /// run is in flight — one bulk run at a time — replaced by a live counter
    /// and Cancel.
    pub fn fleet_bar(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
        visible: usize,
    ) -> Option<gpui::AnyElement> {
        if self.selected.is_empty() && self.fleet_run.is_none() {
            return None;
        }
        let idle = self.fleet_run.is_none();
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .child(lucide("check", 15., t.accent_bright))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(t.text_small))
                    .text_color(rgb(t.fg0))
                    .child(SharedString::from(format!(
                        "{} selected",
                        self.selected.len()
                    ))),
            );
        if let Some(run) = &self.fleet_run {
            bar = bar
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(SharedString::from(format!(
                            "{} {}/{}…",
                            run.op.verb(),
                            run.done,
                            run.total
                        ))),
                )
                .child(bar_btn(
                    "fleet-cancel",
                    "x",
                    "Cancel",
                    true,
                    true,
                    t,
                    cx.listener(|this, _e, _w, cx| this.cancel_fleet(cx)),
                ));
        }
        let select_all = format!("Select all ({visible})");
        bar = bar
            .child(bar_btn(
                "fleet-select-all",
                "circle-check",
                &select_all,
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.select_all_visible(cx)),
            ))
            .child(bar_btn(
                "fleet-fetch",
                "refresh-cw",
                FleetOp::Fetch.label(),
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.run_fleet(FleetOp::Fetch, cx)),
            ))
            .child(bar_btn(
                "fleet-pull",
                "cloud-download",
                FleetOp::Pull.label(),
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.run_fleet(FleetOp::Pull, cx)),
            ))
            .child(bar_btn(
                "fleet-clear",
                "x",
                "Clear",
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.clear_selection(cx)),
            ));
        Some(bar.into_any_element())
    }
}

/// One flat fleet-bar button. Disabled (`!enabled`) renders dimmed with no
/// click handler; `danger` tints the label/icon for Cancel.
fn bar_btn(
    id: &'static str,
    icon: &'static str,
    label: &str,
    enabled: bool,
    danger: bool,
    t: &Theme,
    on: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let fg = match (enabled, danger) {
        (false, _) => t.fg3,
        (true, true) => t.behind,
        (true, false) => t.fg1,
    };
    let mut b = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_small))
        .text_color(rgb(fg))
        .child(lucide(icon, 14., fg))
        .child(SharedString::from(label.to_string()));
    if enabled {
        let hov = t.border_strong;
        b = b
            .cursor_pointer()
            .hover(move |s| s.border_color(rgb(hov)))
            .on_click(on);
    }
    b.into_any_element()
}

/// Aggregate summary for the resolution toast, e.g. "Pull: 38 ok, 2 failed".
/// Any failure resolves as an Error toast (persists until clicked) so failed
/// repos are never silently swept away; clean runs are a Success.
fn summary(op: FleetOp, report: &FleetReport) -> (ToastKind, String) {
    let (ok, failed, skipped) = (
        report.ok_count(),
        report.failed_count(),
        report.skipped_count(),
    );
    let mut parts = vec![format!("{ok} ok")];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    let cancelled = if report.cancelled { " cancelled" } else { "" };
    let kind = if failed > 0 {
        ToastKind::Error
    } else {
        ToastKind::Success
    };
    (
        kind,
        format!("{}{cancelled}: {}", op.label(), parts.join(", ")),
    )
}

/// Failed repos + reasons for the toast detail, e.g.
/// "orrery: local changes would be overwritten · zed: histories diverged".
/// One flattened text run (GPUI single-line text panics on embedded newlines),
/// each reason clipped, capped at [`MAX_FAILURES_SHOWN`] entries + "+N more".
fn failure_detail(report: &FleetReport) -> Option<SharedString> {
    let failures: Vec<(&str, &str)> = report
        .results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Failed(why) => {
                Some((r.repo.rsplit('/').next().unwrap_or(&r.repo), why.as_str()))
            }
            _ => None,
        })
        .collect();
    if failures.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = failures
        .iter()
        .take(MAX_FAILURES_SHOWN)
        .map(|(name, why)| {
            format!(
                "{name}: {}",
                clip(&crate::data::oneline(why.to_string()), MAX_REASON_CHARS)
            )
        })
        .collect();
    if failures.len() > MAX_FAILURES_SHOWN {
        parts.push(format!("+{} more", failures.len() - MAX_FAILURES_SHOWN));
    }
    Some(parts.join(" · ").into())
}

/// Clip to at most `max` chars (char-boundary safe), ellipsised.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::fleet::RepoResult;

    fn report(results: Vec<(&str, Outcome)>, cancelled: bool) -> FleetReport {
        let n = results.len();
        FleetReport {
            results: results
                .into_iter()
                .map(|(repo, outcome)| RepoResult {
                    repo: repo.to_string(),
                    outcome,
                })
                .collect(),
            started: n,
            completed: n,
            cancelled,
        }
    }

    #[test]
    fn summary_counts_and_kind() {
        let r = report(
            vec![
                ("/x/a", Outcome::Ok("done".into())),
                ("/x/b", Outcome::Failed("boom".into())),
                ("/x/c", Outcome::Skipped("no upstream".into())),
            ],
            false,
        );
        let (kind, title) = summary(FleetOp::Pull, &r);
        assert!(kind == ToastKind::Error);
        assert_eq!(title, "Pull: 1 ok, 1 failed, 1 skipped");

        let clean = report(vec![("/x/a", Outcome::Ok("done".into()))], false);
        let (kind, title) = summary(FleetOp::Fetch, &clean);
        assert!(kind == ToastKind::Success);
        assert_eq!(title, "Fetch: 1 ok");
    }

    #[test]
    fn summary_marks_cancelled_runs() {
        let r = report(
            vec![
                ("/x/a", Outcome::Ok("done".into())),
                ("/x/b", Outcome::Skipped("cancelled".into())),
            ],
            true,
        );
        let (_, title) = summary(FleetOp::Pull, &r);
        assert_eq!(title, "Pull cancelled: 1 ok, 1 skipped");
    }

    #[test]
    fn failure_detail_lists_names_reasons_and_truncates() {
        let ok = ("/r/fine", Outcome::Ok("done".into()));
        let mut results = vec![ok];
        for i in 0..6 {
            results.push((
                ["/r/a", "/r/b", "/r/c", "/r/d", "/r/e", "/r/f"][i],
                Outcome::Failed(format!("reason {i}\nsecond line")),
            ));
        }
        let detail = failure_detail(&report(results, false)).unwrap();
        // Names come from the path tail; newlines flatten; capped at 4 + more.
        assert!(detail.starts_with("a: reason 0 second line · b: reason 1"));
        assert!(detail.ends_with("+2 more"));
        assert!(!detail.contains('\n'));
    }

    #[test]
    fn failure_detail_none_when_nothing_failed() {
        let r = report(vec![("/x/a", Outcome::Ok("done".into()))], false);
        assert!(failure_detail(&r).is_none());
    }

    #[test]
    fn clip_is_char_boundary_safe() {
        assert_eq!(clip("short", 10), "short");
        let clipped = clip("éééééééééé", 5);
        assert_eq!(clipped, "éééé…");
    }
}
