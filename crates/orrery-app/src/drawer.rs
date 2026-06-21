//! RepoDrawer — the right-anchored detail panel (port of `RepoDrawer.tsx`). Opens
//! over the shell when a card is clicked; a scrim backdrop or the close button
//! dismisses it. Tabs: Overview / Changes / PR / Notes / Readme.
//!
//! This is the workhorse primitive — most journeys (catch-up, dive, commit, PR
//! triage) live here. First cut: the shell + the Overview tab rendered from the
//! already-loaded `Row` (no async). The other tabs and the async git data
//! (branches, commits, diff, PRs, readme) fill in next.

use gpui::{
    div, px, rgb, rgba, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use orrery_core::launch;

use crate::data::Row;
use crate::icon::{brand, lucide};
use crate::shell::{DrawerTab, OrreryApp, Overlay};
use crate::theme::Theme;

const MONO: &str = "monospace";
const PANEL_W: f32 = 560.;

const TABS: [(DrawerTab, &str); 5] = [
    (DrawerTab::Overview, "Overview"),
    (DrawerTab::Changes, "Changes"),
    (DrawerTab::Pr, "PR"),
    (DrawerTab::Notes, "Notes"),
    (DrawerTab::Readme, "Readme"),
];

pub fn drawer(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    app: &Entity<OrreryApp>,
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
        .child(body(row, tab, t))
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

fn body(row: &Row, tab: DrawerTab, t: &Theme) -> impl IntoElement {
    let content = match tab {
        DrawerTab::Overview => overview(row, t).into_any_element(),
        other => coming_soon(other, t).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_hidden()
        .p(px(18.))
        .gap(px(16.))
        .child(content)
}

/// Overview, rendered from the already-loaded `Row` (no fetch). Branches,
/// recent commits and worktrees (async git_ops) land in the next pass.
fn overview(row: &Row, t: &Theme) -> impl IntoElement {
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
    col.child(facts)
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
