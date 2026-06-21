//! RepoDrawer — the right-anchored detail panel (port of `RepoDrawer.tsx`). Opens
//! over the shell when a card is clicked; a scrim backdrop or the close button
//! dismisses it. Tabs: Overview / Changes / PR / Notes / Readme.
//!
//! This is the workhorse primitive — most journeys (catch-up, dive, commit, PR
//! triage) live here. The Overview tab is complete: the synchronous `Row` facts
//! plus async-loaded branches (switchable), recent commits, and worktrees. The
//! Changes / PR / Notes / Readme tabs are scaffolds, filled in next.

use gpui::{
    div, px, rgb, rgba, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
};
use orrery_core::{git_ops, launch};

use crate::data::{self, Row};
use crate::icon::{brand, lucide};
use crate::shell::{DrawerTab, OrreryApp, Overlay};
use crate::theme::Theme;

const MONO: &str = "monospace";
const PANEL_W: f32 = 560.;
/// Recent commits shown in the Overview.
const LOG_LIMIT: usize = 8;

const TABS: [(DrawerTab, &str); 5] = [
    (DrawerTab::Overview, "Overview"),
    (DrawerTab::Changes, "Changes"),
    (DrawerTab::Pr, "PR"),
    (DrawerTab::Notes, "Notes"),
    (DrawerTab::Readme, "Readme"),
];

// ── async per-repo data ────────────────────────────────────────────────────
// The drawer's git data is loaded off the UI thread and marshalled back onto the
// foreground (the live-wiring pattern). `None` = still loading; `Some(vec)` =
// loaded (possibly empty). `repo` guards against a result landing after the
// drawer moved to a different repo or closed.

pub struct BranchRow {
    pub name: SharedString,
    pub current: bool,
    pub gone: bool,
    pub merged: bool,
}

pub struct CommitRow {
    pub summary: SharedString,
    pub author: SharedString,
    pub age: SharedString,
}

pub struct WorktreeRow {
    pub name: SharedString,
    pub path: SharedString,
}

/// Async-loaded Overview data for the currently open repo.
#[derive(Default)]
pub struct DrawerData {
    pub repo: SharedString,
    pub branches: Option<Vec<BranchRow>>,
    pub commits: Option<Vec<CommitRow>>,
    pub worktrees: Option<Vec<WorktreeRow>>,
}

impl DrawerData {
    /// Fresh, all-loading state for a newly opened repo.
    pub fn loading(repo: SharedString) -> Self {
        DrawerData {
            repo,
            ..Default::default()
        }
    }
}

type Loaded = (
    Vec<git_ops::BranchInfo>,
    Vec<git_ops::CommitInfo>,
    Vec<git_ops::WorktreeInfo>,
);

/// Read branches + recent log + worktrees for `id` (all git-heavy — runs on the
/// background pool).
fn read_overview(id: &str) -> Loaded {
    (
        git_ops::branches(id).unwrap_or_default(),
        git_ops::recent_log(id, LOG_LIMIT).unwrap_or_default(),
        git_ops::worktrees(id).unwrap_or_default(),
    )
}

/// Apply a finished Overview load to the app, but only if the drawer still shows
/// the same repo (else the user moved on and this is stale).
fn store_overview(
    this: &WeakEntity<OrreryApp>,
    cx: &mut AsyncApp,
    repo: &SharedString,
    loaded: Loaded,
    now: i64,
) {
    let (branches, commits, worktrees) = loaded;
    let _ = this.update(cx, |this, cx| {
        if &this.drawer.repo != repo {
            return;
        }
        this.drawer.branches = Some(branches.into_iter().map(branch_row).collect());
        this.drawer.commits = Some(commits.into_iter().map(|c| commit_row(c, now)).collect());
        this.drawer.worktrees = Some(worktrees.into_iter().map(worktree_row).collect());
        cx.notify();
    });
}

/// Kick off the Overview load for `repo` (branches/commits/worktrees).
pub fn load_overview(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move { read_overview(&id) })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

/// Switch `repo` to `name`, then refresh the Overview. Spawn-only (the caller,
/// already holding `&mut OrreryApp`, sets the loading state). The `.git/HEAD`
/// change also trips the filesystem watcher, so the card row refreshes on its own.
pub fn switch_branch(repo: SharedString, name: SharedString, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let (id, branch) = (repo.to_string(), name.to_string());
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move {
                let _ = git_ops::switch_branch(&id, &branch);
                read_overview(&id)
            })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

fn branch_row(b: git_ops::BranchInfo) -> BranchRow {
    BranchRow {
        name: b.name.into(),
        current: b.is_head,
        gone: b.gone,
        merged: b.merged,
    }
}

fn commit_row(c: git_ops::CommitInfo, now: i64) -> CommitRow {
    CommitRow {
        summary: c.summary.into(),
        author: c.author.into(),
        age: data::rel_age(c.time_unix, now).into(),
    }
}

fn worktree_row(w: git_ops::WorktreeInfo) -> WorktreeRow {
    WorktreeRow {
        name: w.name.into(),
        path: w.path.into(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn drawer(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    app: &Entity<OrreryApp>,
    data: &DrawerData,
    ide_cmd: &str,
    agent_cmd: &str,
) -> impl IntoElement {
    // Scrim: click anywhere outside the panel to dismiss.
    let backdrop = {
        let app = app.clone();
        div()
            .id("drawer-backdrop")
            .flex_1()
            .h_full()
            .bg(rgba(0x00000066))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let panel = div()
        .flex()
        .flex_col()
        .w(px(PANEL_W))
        .h_full()
        .bg(rgb(t.page))
        .border_l_1()
        .border_color(rgb(t.border))
        .child(header(row, t, app))
        .child(tab_bar(tab, t, app))
        .child(body(row, tab, t, data, app))
        .child(footer(row, t, ide_cmd, agent_cmd));

    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .flex()
        .flex_row()
        .child(backdrop)
        .child(panel)
}

fn header(row: &Row, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let close = {
        let app = app.clone();
        div()
            .id("drawer-close")
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(30.))
            .rounded(px(t.r_sm))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide("x", 17., t.fg1))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let mut title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .text_size(px(t.text_h3))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.fg0))
        .child(div().min_w(px(0.)).truncate().child(row.name.clone()));
    if !row.host.is_empty() {
        title = title.child(brand(&row.host, 15., t.fg2));
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .px(px(18.))
        .py(px(15.))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(4.))
                .child(title)
                .child(
                    div()
                        .truncate()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(SharedString::from(format!("{} · {}", row.slug, row.path))),
                ),
        )
        .child(close)
}

fn tab_bar(active: DrawerTab, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .gap(px(2.))
        .px(px(12.))
        .border_b_1()
        .border_color(rgb(t.border));

    for (tab, label) in TABS {
        let is_active = tab == active;
        let fg = if is_active { t.fg0 } else { t.fg2 };
        let app = app.clone();
        // 1px underline on the active tab; page-coloured (invisible) otherwise so
        // the row height stays constant.
        let underline = if is_active { t.accent_bright } else { t.page };
        let item = div()
            .id(label)
            .px(px(11.))
            .py(px(10.))
            .text_size(px(t.text_small))
            .text_color(rgb(fg))
            .cursor_pointer()
            .border_b_1()
            .border_color(rgb(underline))
            .hover(|s| s.text_color(rgb(t.fg0)))
            .child(SharedString::from(label))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    if let Some(Overlay::Drawer { tab: cur, .. }) = &mut this.overlay {
                        *cur = tab;
                    }
                    cx.notify();
                });
            });
        bar = bar.child(item);
    }
    bar
}

fn body(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    data: &DrawerData,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let content = match tab {
        DrawerTab::Overview => overview(row, t, data, app).into_any_element(),
        other => coming_soon(other, t).into_any_element(),
    };
    div()
        .id("drawer-body")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(px(18.))
        .gap(px(16.))
        .child(content)
}

/// Overview: the synchronous `Row` facts up top, then the async git data
/// (branches / recent commits / worktrees) loaded via [`load_overview`].
fn overview(row: &Row, t: &Theme, data: &DrawerData, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(16.));

    // Description.
    col = col.child(
        div()
            .text_size(px(t.text_small))
            .line_height(px(20.))
            .text_color(rgb(t.fg1))
            .child(row.description.clone()),
    );

    // AI summary, when present.
    if !row.ai_summary.is_empty() {
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(7.))
                .p(px(11.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(lucide("sparkles", 14., t.ai))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(t.text_data_sm))
                        .line_height(px(18.))
                        .text_color(rgb(t.ai))
                        .child(row.ai_summary.clone()),
                ),
        );
    }

    // Git status block.
    let mut status = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(16.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .child(seg("git-branch", row.branch.clone(), t.fg1));
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .text_color(rgb(color))
                .child(lucide("arrow-up", 13., color))
                .child(SharedString::from(row.ahead.to_string()))
                .child(lucide("arrow-down", 13., color))
                .child(SharedString::from(row.behind.to_string())),
        );
    }
    if row.dirty > 0 {
        status = status.child(seg(
            "circle-dot",
            SharedString::from(format!("{} dirty", row.dirty)),
            t.dirty,
        ));
    }
    col = col.child(status);

    // Host facts.
    let mut facts = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(16.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2));
    if row.private {
        facts = facts.child(seg("lock", SharedString::from("private"), t.fg3));
    }
    if !row.host.is_empty() {
        facts = facts.child(seg("star", row.stars.clone(), t.star));
    }
    if !row.release.is_empty() {
        facts = facts.child(seg("tag", row.release.clone(), t.fg2));
    }
    facts = facts.child(seg("clock", row.age.clone(), t.fg2));
    col = col.child(facts);

    // Async git data.
    col = col.child(branches_section(data, t, app));
    col = col.child(commits_section(data, t));
    col.child(worktrees_section(data, t))
}

/// Section wrapper: an uppercase label (+ optional count) over a list.
fn section(t: &Theme, title: &str, count: Option<usize>) -> Div {
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(title.to_uppercase()));
    if let Some(n) = count {
        head = head.child(SharedString::from(format!("· {n}")));
    }
    div().flex().flex_col().gap(px(3.)).child(head.mb(px(3.)))
}

/// A muted "Loading…" / empty placeholder line.
fn placeholder(text: &str, t: &Theme) -> impl IntoElement {
    div()
        .py(px(3.))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(text.to_string()))
}

/// A small bordered pill tag (merged / gone).
fn tag(text: &str, color: u32, t: &Theme) -> impl IntoElement {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(color))
        .child(SharedString::from(text.to_string()))
}

fn branches_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut s = section(t, "Branches", data.branches.as_ref().map(|b| b.len()));
    match &data.branches {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No branches.", t)),
        Some(list) => {
            for b in list {
                s = s.child(branch_item(b, data.repo.clone(), t, app));
            }
        }
    }
    s
}

fn branch_item(
    b: &BranchRow,
    repo: SharedString,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let fg = if b.current { t.accent_bright } else { t.fg1 };
    let icon = if b.current { "check" } else { "git-branch" };
    let mut item = div()
        .id(SharedString::from(format!("br-{}", b.name)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(lucide(icon, 13., fg))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(b.name.clone()),
        );
    if b.merged {
        item = item.child(tag("merged", t.fg3, t));
    }
    if b.gone {
        item = item.child(tag("gone", t.behind, t));
    }
    // Only non-current branches are switchable.
    if !b.current {
        let name = b.name.clone();
        let app = app.clone();
        item = item
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .on_click(move |_ev, _win, cx| {
                let (repo, name) = (repo.clone(), name.clone());
                app.update(cx, |this, cx| {
                    this.drawer.branches = None; // optimistic loading state
                    switch_branch(repo, name, cx);
                    cx.notify();
                });
            });
    }
    item
}

fn commits_section(data: &DrawerData, t: &Theme) -> impl IntoElement {
    let mut s = section(t, "Recent commits", data.commits.as_ref().map(|c| c.len()));
    match &data.commits {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No commits.", t)),
        Some(list) => {
            for c in list {
                s = s.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .py(px(4.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(t.text_small))
                                .text_color(rgb(t.fg1))
                                .child(c.summary.clone()),
                        )
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(t.text_data_sm))
                                .text_color(rgb(t.fg3))
                                .child(SharedString::from(format!("{} · {}", c.author, c.age))),
                        ),
                );
            }
        }
    }
    s
}

fn worktrees_section(data: &DrawerData, t: &Theme) -> impl IntoElement {
    let mut s = section(t, "Worktrees", data.worktrees.as_ref().map(|w| w.len()));
    match &data.worktrees {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("None.", t)),
        Some(list) => {
            for w in list {
                s = s.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(7.))
                        .py(px(4.))
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .child(lucide("folder-tree", 13., t.fg2))
                        .child(div().text_color(rgb(t.fg1)).child(w.name.clone()))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(rgb(t.fg3))
                                .child(w.path.clone()),
                        ),
                );
            }
        }
    }
    s
}

/// Placeholder for a tab not yet ported — clearly labelled so it reads as
/// scaffold, not breakage.
fn coming_soon(tab: DrawerTab, t: &Theme) -> impl IntoElement {
    let label = match tab {
        DrawerTab::Changes => "Staged diff + AI commit message",
        DrawerTab::Pr => "Open PRs · checks · review · merge",
        DrawerTab::Notes => "Catch-up summary + scratchpad",
        DrawerTab::Readme => "Rendered README",
        DrawerTab::Overview => "",
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .flex_1()
        .gap(px(6.))
        .text_color(rgb(t.fg3))
        .child(lucide("hammer", 20., t.fg3))
        .child(
            div()
                .text_size(px(t.text_data_sm))
                .child(SharedString::from(label)),
        )
}

fn footer(row: &Row, t: &Theme, ide_cmd: &str, agent_cmd: &str) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .px(px(18.))
        .py(px(14.))
        .border_t_1()
        .border_color(rgb(t.border))
        .child(launch_btn(
            "drawer-ide",
            SharedString::from("Open in IDE"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
                move || {
                    let _ = launch::launch(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-agent",
            SharedString::from("Agent"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
                move || {
                    let _ = launch::spawn(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-folder",
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            {
                let path = row.id.clone();
                move || {
                    let _ = launch::open(&path);
                }
            },
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        bar = bar.child(launch_btn(
            "drawer-host",
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move || {
                let _ = launch::open(&url);
            },
        ));
    }
    bar
}

/// A drawer launcher button. `on` runs a side-effecting launch (no app state).
fn launch_btn(
    id: &'static str,
    content: impl IntoElement,
    wide: bool,
    t: &Theme,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let (hov_border, hov_fg) = (t.border_strong, t.fg0);
    let b = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(9.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .font_family(MONO)
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov_border)).text_color(rgb(hov_fg)))
        .on_click(move |_ev, _win, _cx| on())
        .child(content);
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(40.))
    }
}

/// Inline icon+label segment (shared shape with the card's status segs).
fn seg(icon: &str, label: SharedString, color: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide(icon, 13., color))
        .child(label)
}
