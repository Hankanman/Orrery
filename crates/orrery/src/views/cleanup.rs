//! Cleanup view — the branch janitor: prunable (merged / upstream-gone) branches
//! across all repos, grouped by repo, with bulk delete behind a two-stage
//! confirm (deleting branches is irreversible). Sync git, loaded off the
//! UI thread when the nav item is selected. Never touches the current/default
//! branch (the core `prunable` already excludes those).

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::git_ops;

use crate::data::Row;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

#[derive(Default)]
pub enum CleanupState {
    #[default]
    Idle,
    Loading,
    Ready(Vec<CleanupRepo>),
}

/// A repo with prunable branches.
pub struct CleanupRepo {
    pub id: SharedString,
    pub name: SharedString,
    pub branches: Vec<BranchRow>,
}

pub struct BranchRow {
    pub name: SharedString,
    /// "merged" or "gone".
    pub why: &'static str,
}

/// Scan every repo for prunable branches (sync git — runs off the UI thread).
pub fn scan(rows: &[Row]) -> Vec<CleanupRepo> {
    rows.iter()
        .filter_map(|r| {
            let branches: Vec<BranchRow> = git_ops::prunable(&r.id)
                .unwrap_or_default()
                .into_iter()
                .map(|b| BranchRow {
                    name: b.name.into(),
                    why: if b.gone { "gone" } else { "merged" },
                })
                .collect();
            (!branches.is_empty()).then(|| CleanupRepo {
                id: r.id.clone(),
                name: r.name.clone(),
                branches,
            })
        })
        .collect()
}

/// Does a branch's prune reason pass the sidebar filter (None = all)?
fn passes(why: &str, filter: Option<&str>) -> bool {
    filter.is_none() || filter == Some(why)
}

pub fn render(
    state: &CleanupState,
    filter: Option<&str>,
    confirm: Option<&str>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let body = match state {
        CleanupState::Idle | CleanupState::Loading => super::note("Loading…", t).into_any_element(),
        CleanupState::Ready(repos) if repos.is_empty() => {
            super::note("Nothing to clean up — no prunable branches.", t).into_any_element()
        }
        CleanupState::Ready(repos) => {
            // Only repos with at least one branch matching the filter.
            let shown: Vec<&CleanupRepo> = repos
                .iter()
                .filter(|r| r.branches.iter().any(|b| passes(b.why, filter)))
                .collect();
            if shown.is_empty() {
                super::note("Nothing in this filter.", t).into_any_element()
            } else {
                let mut col = div().flex().flex_col().gap(px(12.));
                for r in shown {
                    let armed = confirm == Some(r.id.as_ref());
                    col = col.child(repo_card(r, filter, armed, t, app));
                }
                col.into_any_element()
            }
        }
    };
    super::frame(
        "Cleanup",
        t,
        app,
        OrreryApp::load_cleanup,
        "cleanup-scroll",
        body,
    )
}

fn repo_card(
    r: &CleanupRepo,
    filter: Option<&str>,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let n = r.branches.len();
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide("scissors", 14., t.fg2))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(r.name.clone()),
        )
        .child(div().flex_1())
        .child(prune_button(r.id.clone(), n, armed, t, app));

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .p(px(12.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(head);
    for b in r.branches.iter().filter(|b| passes(b.why, filter)) {
        card = card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .child(lucide("git-branch", 12., t.fg3))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(rgb(t.fg1))
                        .child(b.name.clone()),
                )
                .child(super::tag(
                    b.why,
                    if b.why == "gone" { t.behind } else { t.fg3 },
                    t,
                )),
        );
    }
    card
}

/// The per-repo prune button. Two-stage: the first click arms a confirm (the
/// button flips to a danger-styled "Confirm prune {n}?"), the second actually
/// deletes — branch deletion is irreversible. The armed state reverts on a
/// timeout or when another repo's button is armed (see `OrreryApp::arm_prune`).
fn prune_button(
    id: SharedString,
    n: usize,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let label = if armed {
        format!("Confirm prune {n}?")
    } else {
        format!("Prune {n}")
    };
    let btn = div()
        .id(SharedString::from(format!("prune-{id}")))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .cursor_pointer();
    let btn = if armed {
        btn.border_1()
            .border_color(rgb(t.behind))
            .text_color(rgb(t.behind))
    } else {
        btn.border_1()
            .border_color(rgb(t.border))
            .text_color(rgb(t.fg1))
            .hover(|s| s.border_color(rgb(t.behind)).text_color(rgb(t.behind)))
    };
    btn.child(SharedString::from(label))
        .on_click(move |_ev, _win, cx| {
            let id = id.clone();
            app.update(cx, |this, cx| {
                if armed {
                    this.prune_repo(id, cx)
                } else {
                    this.arm_prune(id, cx)
                }
            });
        })
}
