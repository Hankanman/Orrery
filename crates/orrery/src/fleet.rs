//! Fleet operations UI (#100/#184): multi-select on the repo grid + the fleet
//! bar running bulk Fetch/Pull/Prune (and bulk "Open in IDE") through
//! `orrery_core::fleet`.
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
//!
//! Bulk **Prune** is confirm-gated (branch deletion is irreversible — the #173
//! pattern scaled up): the Prune button first scans the selection for prunable
//! branches on the background executor, then the bar expands into a confirm
//! strip with the per-repo breakdown ("orrery ×3 · zed ×2 · nothing to prune:
//! api"), and only an explicit Confirm click executes. The strip was chosen
//! over reusing the Cleanup view with a preselection because it keeps the
//! confirm adjacent to the button that armed it and adds no cross-view
//! navigation state; the pending plan ([`OrreryApp::fleet_prune`]) is dropped
//! by any selection change, Esc, Cancel, or another run starting, so a stale
//! plan can never execute. Execution re-derives what's prunable per repo
//! (`git_ops::prune_branches` re-checks), so even a racing external prune is
//! safe — the repo just reports "nothing prunable".

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
/// Per-repo entries shown in the prune confirm strip's breakdown before "+N more".
const MAX_BREAKDOWN_SHOWN: usize = 6;
/// Nothing-to-prune repo names listed in the breakdown before "+N more".
const MAX_SKIPPED_SHOWN: usize = 3;
/// Cap on bulk "Open in IDE" — spawning dozens of editor windows at once is a
/// footgun, so larger selections are refused with a toast instead.
pub const MAX_BULK_LAUNCH: usize = 10;

/// Which bulk operation the fleet bar runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FleetOp {
    Fetch,
    Pull,
    /// Only ever started through the confirm strip
    /// ([`OrreryApp::confirm_fleet_prune`]) — never directly from a button.
    Prune,
}

impl FleetOp {
    /// Present-progressive verb for the progress toast / bar counter.
    fn verb(self) -> &'static str {
        match self {
            FleetOp::Fetch => "Fetching",
            FleetOp::Pull => "Pulling",
            FleetOp::Prune => "Pruning",
        }
    }

    /// Imperative name for the bar buttons and the summary toast.
    fn label(self) -> &'static str {
        match self {
            FleetOp::Fetch => "Fetch",
            FleetOp::Pull => "Pull",
            FleetOp::Prune => "Prune",
        }
    }
}

/// A pending bulk-prune confirm on the fleet bar (see the module docs for the
/// full flow). Dropped on any selection change / Esc / Cancel / run start.
pub enum PrunePlan {
    /// The background scan over the selection is in flight.
    Scanning,
    /// Scan done — the strip shows the breakdown and waits for the explicit
    /// Confirm click. `repos` is the full planned id set (grid order), so the
    /// run's aggregate toast accounts for every selected repo — the ones with
    /// nothing prunable report as engine skips.
    Ready {
        repos: Vec<String>,
        /// Per-repo prunable-branch counts (only repos with count > 0).
        entries: Vec<PruneEntry>,
        /// Display names of selected repos with nothing prunable.
        skipped: Vec<SharedString>,
    },
}

/// One row of the prune confirm breakdown: a repo and how many of its
/// branches would go.
pub struct PruneEntry {
    pub name: SharedString,
    pub count: usize,
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
        // Editing the selection invalidates a pending prune confirm.
        self.fleet_prune = None;
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        cx.notify();
    }

    /// Clear the multi-selection (fleet bar "Clear", or Esc with no overlay).
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selected.is_empty() || self.fleet_prune.is_some() {
            self.selected.clear();
            self.fleet_prune = None;
            cx.notify();
        }
    }

    /// Select every row passing the current filters (the fleet bar's
    /// "Select all"). Adds to the existing selection rather than replacing it,
    /// so a hand-picked repo outside the filter isn't dropped.
    pub fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        self.fleet_prune = None;
        for i in self.visible_rows() {
            let id = self.rows[i].id.clone();
            self.selected.insert(id);
        }
        cx.notify();
    }

    /// Replace the selection with the repos matching `pred` — the palette's
    /// select-by-filter verbs ("Select dirty" / "Select behind"). When nothing
    /// matches, the existing selection is kept and an Info toast explains why
    /// (`what` reads as "No repos are {what}.").
    pub fn select_where(
        &mut self,
        pred: impl Fn(&crate::data::Row) -> bool,
        what: &str,
        cx: &mut Context<Self>,
    ) {
        let matched: std::collections::HashSet<SharedString> = self
            .rows
            .iter()
            .filter(|r| pred(r))
            .map(|r| r.id.clone())
            .collect();
        if matched.is_empty() {
            self.push_toast(
                ToastKind::Info,
                "Nothing selected",
                Some(format!("No repos are {what}.").into()),
                cx,
            );
            return;
        }
        self.fleet_prune = None;
        self.selected = matched;
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

    /// Run `op` across the selected repos (see [`Self::run_fleet_repos`]).
    pub fn run_fleet(&mut self, op: FleetOp, cx: &mut Context<Self>) {
        // Row order (not hash order), so results/failures read like the grid.
        let repos: Vec<String> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.id))
            .map(|r| r.id.to_string())
            .collect();
        self.run_fleet_repos(op, repos, cx);
    }

    /// Run `op` across `repos` on the background executor (one bulk run at a
    /// time). Shared by the fleet bar (selection) and the palette's fleet
    /// verbs ("Fetch all", "Pull all behind" — no selection needed). Progress
    /// marshals onto the foreground via a channel and keeps a keyed Progress
    /// toast current; completion resolves the toast to the aggregate summary
    /// and rescans once.
    pub fn run_fleet_repos(&mut self, op: FleetOp, repos: Vec<String>, cx: &mut Context<Self>) {
        if self.fleet_run.is_some() || repos.is_empty() {
            return;
        }
        // Starting any run invalidates a pending prune confirm.
        self.fleet_prune = None;
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
                        FleetOp::Prune => {
                            fleet::run(&repos, workers, &cancel, progress, fleet::prune_op())
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

    /// The fleet bar's Prune button: scan the selected repos for prunable
    /// branches on the background executor (the Cleanup view's scan), then
    /// expand the bar into the confirm strip with the per-repo breakdown.
    /// Nothing is deleted here — only [`Self::confirm_fleet_prune`] executes.
    pub fn start_fleet_prune(&mut self, cx: &mut Context<Self>) {
        if self.fleet_run.is_some() || self.fleet_prune.is_some() {
            return;
        }
        // Grid order, so the breakdown reads like the grid.
        let rows: Vec<crate::data::Row> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.id))
            .cloned()
            .collect();
        if rows.is_empty() {
            return;
        }
        let selected = rows.len();
        self.fleet_prune_seq += 1;
        let seq = self.fleet_prune_seq;
        self.fleet_prune = Some(PrunePlan::Scanning);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let scanned = cx
                .background_executor()
                .spawn(async move {
                    let prunable = crate::views::cleanup::scan(&rows);
                    (rows, prunable)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Apply only if this scan's confirm is still the pending one —
                // not cancelled, superseded, or invalidated by a selection edit.
                if this.fleet_prune_seq != seq
                    || !matches!(this.fleet_prune, Some(PrunePlan::Scanning))
                {
                    return;
                }
                let (rows, prunable) = scanned;
                let entries: Vec<PruneEntry> = prunable
                    .iter()
                    .map(|r| PruneEntry {
                        name: r.name.clone(),
                        count: r.branches.len(),
                    })
                    .collect();
                if entries.is_empty() {
                    this.fleet_prune = None;
                    this.push_toast(
                        ToastKind::Info,
                        "Nothing to prune",
                        Some(
                            format!("No prunable branches across {selected} selected repos.")
                                .into(),
                        ),
                        cx,
                    );
                } else {
                    let has: std::collections::HashSet<&SharedString> =
                        prunable.iter().map(|r| &r.id).collect();
                    let skipped: Vec<SharedString> = rows
                        .iter()
                        .filter(|r| !has.contains(&r.id))
                        .map(|r| r.name.clone())
                        .collect();
                    this.fleet_prune = Some(PrunePlan::Ready {
                        repos: rows.iter().map(|r| r.id.to_string()).collect(),
                        entries,
                        skipped,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The confirm strip's Confirm click — the only path that executes a bulk
    /// prune. Runs over the full planned selection; repos with nothing
    /// prunable report as engine skips in the aggregate toast.
    pub fn confirm_fleet_prune(&mut self, cx: &mut Context<Self>) {
        let Some(PrunePlan::Ready { repos, .. }) = self.fleet_prune.take() else {
            return;
        };
        self.run_fleet_repos(FleetOp::Prune, repos, cx);
    }

    /// Drop the pending prune confirm (strip Cancel, or Esc).
    pub fn cancel_fleet_prune(&mut self, cx: &mut Context<Self>) {
        if self.fleet_prune.take().is_some() {
            cx.notify();
        }
    }

    /// Bulk "Open in IDE": launch the configured editor on every selected
    /// repo (the card's launch path). Selections over [`MAX_BULK_LAUNCH`] are
    /// refused with a toast asking to narrow — spawning dozens of editor
    /// windows at once is a footgun.
    pub fn launch_selected(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.id))
            .map(|r| r.id.to_string())
            .collect();
        if ids.is_empty() {
            return;
        }
        if ids.len() > MAX_BULK_LAUNCH {
            self.push_toast(
                ToastKind::Error,
                "Too many repos to open",
                Some(
                    format!(
                        "{} selected — narrow the selection to {MAX_BULK_LAUNCH} or fewer \
                         before opening in the IDE.",
                        ids.len()
                    )
                    .into(),
                ),
                cx,
            );
            return;
        }
        let total = ids.len();
        let failed = ids
            .iter()
            .filter(|id| orrery_core::launch::launch(&self.config.ide_command, id).is_err())
            .count();
        if failed > 0 {
            self.push_toast(
                ToastKind::Error,
                "Some IDE launches failed",
                Some(format!("{failed} of {total} repos failed to open.").into()),
                cx,
            );
        } else {
            self.push_toast(
                ToastKind::Success,
                format!(
                    "Opened {total} {} in the IDE",
                    if total == 1 { "repo" } else { "repos" }
                ),
                None,
                cx,
            );
        }
    }

    /// The fleet action bar, pinned under the grid. `None` (costing nothing)
    /// until a selection exists or a run is active. Buttons disable while a
    /// run is in flight (one bulk run at a time — replaced by a live counter
    /// and Cancel) and while a prune confirm strip is pending above the bar.
    pub fn fleet_bar(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
        visible: usize,
    ) -> Option<gpui::AnyElement> {
        if self.selected.is_empty() && self.fleet_run.is_none() {
            return None;
        }
        let idle = self.fleet_run.is_none() && self.fleet_prune.is_none();
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
                "fleet-prune",
                "scissors",
                FleetOp::Prune.label(),
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.start_fleet_prune(cx)),
            ))
            .child(bar_btn(
                "fleet-launch",
                "code",
                "Open in IDE",
                idle,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.launch_selected(cx)),
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
        // A pending prune confirm expands the bar into a strip above the
        // buttons row, showing the per-repo breakdown and Confirm/Cancel.
        let Some(plan) = &self.fleet_prune else {
            return Some(bar.into_any_element());
        };
        Some(
            div()
                .flex()
                .flex_col()
                .child(self.prune_strip(plan, t, cx))
                .child(bar)
                .into_any_element(),
        )
    }

    /// The prune confirm strip (two-stage confirm, the #173 pattern scaled
    /// up): summary + per-repo breakdown while `Ready`, a scanning notice
    /// while the background scan runs. Danger-tinted like the Cleanup view's
    /// armed prune button — branch deletion is irreversible.
    fn prune_strip(&self, plan: &PrunePlan, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(8.))
            .border_t_1()
            .border_color(rgb(t.behind))
            .bg(rgb(t.surface))
            .child(lucide("scissors", 15., t.behind));
        match plan {
            PrunePlan::Scanning => {
                strip = strip.child(
                    div()
                        .flex_1()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child("Scanning selection for prunable branches…"),
                );
            }
            PrunePlan::Ready {
                entries, skipped, ..
            } => {
                let total: usize = entries.iter().map(|e| e.count).sum();
                strip = strip
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(t.text_small))
                            .text_color(rgb(t.fg0))
                            .child(SharedString::from(prune_title(total, entries.len()))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family("monospace")
                            .text_size(px(t.text_data_sm))
                            .text_color(rgb(t.fg2))
                            .child(SharedString::from(prune_breakdown(entries, skipped))),
                    )
                    .child(bar_btn(
                        "fleet-prune-confirm",
                        "scissors",
                        &format!("Confirm prune {total}"),
                        true,
                        true,
                        t,
                        cx.listener(|this, _e, _w, cx| this.confirm_fleet_prune(cx)),
                    ));
            }
        }
        strip
            .child(bar_btn(
                "fleet-prune-cancel",
                "x",
                "Cancel",
                true,
                false,
                t,
                cx.listener(|this, _e, _w, cx| this.cancel_fleet_prune(cx)),
            ))
            .into_any_element()
    }
}

/// "Prune 14 branches across 5 repos" (with singular forms where they apply).
fn prune_title(branches: usize, repos: usize) -> String {
    format!(
        "Prune {branches} {} across {repos} {}",
        if branches == 1 { "branch" } else { "branches" },
        if repos == 1 { "repo" } else { "repos" },
    )
}

/// The confirm strip's per-repo breakdown: "orrery ×3 · zed ×2 · +4 more",
/// then the selected repos with nothing prunable ("nothing to prune: api,
/// docs, +2 more") so the pre-confirm view accounts for the whole selection.
fn prune_breakdown(entries: &[PruneEntry], skipped: &[SharedString]) -> String {
    let mut parts: Vec<String> = entries
        .iter()
        .take(MAX_BREAKDOWN_SHOWN)
        .map(|e| format!("{} ×{}", e.name, e.count))
        .collect();
    if entries.len() > MAX_BREAKDOWN_SHOWN {
        parts.push(format!("+{} more", entries.len() - MAX_BREAKDOWN_SHOWN));
    }
    let mut out = parts.join(" · ");
    if !skipped.is_empty() {
        let mut names: Vec<String> = skipped
            .iter()
            .take(MAX_SKIPPED_SHOWN)
            .map(|s| s.to_string())
            .collect();
        if skipped.len() > MAX_SKIPPED_SHOWN {
            names.push(format!("+{} more", skipped.len() - MAX_SKIPPED_SHOWN));
        }
        out.push_str(&format!(" · nothing to prune: {}", names.join(", ")));
    }
    out
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

    #[test]
    fn summary_covers_prune_runs() {
        let r = report(
            vec![
                ("/x/a", Outcome::Ok("pruned 3 branches".into())),
                ("/x/b", Outcome::Skipped("nothing prunable".into())),
            ],
            false,
        );
        let (kind, title) = summary(FleetOp::Prune, &r);
        assert!(kind == ToastKind::Success);
        assert_eq!(title, "Prune: 1 ok, 1 skipped");
    }

    #[test]
    fn prune_title_handles_plurals() {
        assert_eq!(prune_title(14, 5), "Prune 14 branches across 5 repos");
        assert_eq!(prune_title(1, 1), "Prune 1 branch across 1 repo");
    }

    fn entry(name: &str, count: usize) -> PruneEntry {
        PruneEntry {
            name: name.into(),
            count,
        }
    }

    #[test]
    fn prune_breakdown_lists_counts_and_skips() {
        let entries = vec![entry("orrery", 3), entry("zed", 2)];
        assert_eq!(prune_breakdown(&entries, &[]), "orrery ×3 · zed ×2");
        assert_eq!(
            prune_breakdown(&entries, &["api".into(), "docs".into()]),
            "orrery ×3 · zed ×2 · nothing to prune: api, docs"
        );
    }

    #[test]
    fn prune_breakdown_caps_both_lists() {
        let entries: Vec<PruneEntry> = (0..8).map(|i| entry(&format!("r{i}"), 1)).collect();
        let skipped: Vec<SharedString> = (0..5)
            .map(|i| SharedString::from(format!("s{i}")))
            .collect();
        let out = prune_breakdown(&entries, &skipped);
        assert!(out.contains("r5 ×1 · +2 more"), "{out}");
        assert!(
            out.ends_with("nothing to prune: s0, s1, s2, +2 more"),
            "{out}"
        );
        assert!(!out.contains("r6"), "{out}");
    }
}
