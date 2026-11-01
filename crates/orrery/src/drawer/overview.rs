//! Drawer Overview tab: the row's facts, the async branches / commits /
//! worktrees sections, and "Dispatch agent".

use super::branches::branches_section;
use super::commits::commits_section;
use super::*;

/// Overview: the synchronous `Row` facts up top, then the async git data
/// (branches / recent commits / worktrees) loaded via [`OrreryApp::drawer_load_overview`].
pub(super) fn overview(
    row: &Row,
    t: &Theme,
    data: &DrawerData,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
    tags: &[SharedString],
) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(16.));

    // Description.
    col = col.child(
        div()
            .text_size(px(t.text_small))
            .line_height(px(20.))
            .text_color(rgb(t.fg1))
            .child(row.description.clone()),
    );

    // Project tags: chips with ×, and a field to add one.
    col = col.child(super::tags::tags_editor(row, tags, data, t, app));

    // AI summary, when present — hidden (even if cached) unless AI is ready.
    if ai_ready && !row.ai_summary.is_empty() {
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
    facts = facts.child(seg("clock", row.age(), t.fg2));
    // Default-branch CI state from the central pass (#183); clicking it opens
    // the run's page when the API offered one.
    if let Some(ci) = &data.ci {
        let state = match ci.state.as_str() {
            "failure" => Some(("circle-alert", "CI failing", t.behind)),
            "success" => Some(("circle-check", "CI passing", t.clean)),
            "pending" => Some(("clock", "CI running", t.fg2)),
            _ => None, // "none": no CI on this repo — show nothing.
        };
        if let Some((icon, label, color)) = state {
            facts = facts.child(match &ci.url {
                Some(url) => {
                    let url = url.clone();
                    div()
                        .id("ci-run")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .text_color(rgb(color))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(t.fg0)))
                        .child(lucide(icon, 13., color))
                        .child(SharedString::from(label))
                        .child(lucide("external-link", 11., t.fg3))
                        .on_click(move |_ev, _win, _cx| {
                            let _ = launch::open(&url);
                        })
                        .into_any_element()
                }
                None => seg(icon, SharedString::from(label), color).into_any_element(),
            });
        }
    }
    // Security advisories (Dependabot / local audit), when a report exists.
    if let Some(report) = &data.advisory {
        facts = facts.child(advisory_seg(report, t));
    }
    col = col.child(facts);

    // Async git data.
    col = col.child(branches_section(data, t, app));
    col = col.child(commits_section(data, t));
    col = col.child(worktrees_section(row, data, t, app));
    col.child(dispatch_section(data, t, app))
}

/// "Dispatch agent" (#185): a task-prompt field, a fresh-worktree toggle, and
/// the Dispatch button. Plain dispatch starts the configured agent in the repo
/// with the task appended (`agent_dispatch_args`); the toggle first creates an
/// `agent/…` branch + worktree and starts the agent there.
fn dispatch_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let repo = data.repo.clone();
    let mut s = section(t, "Dispatch agent", None);
    let Some(input) = &data.dispatch_input else {
        return s;
    };

    let fresh = data.dispatch_fresh;
    let toggle = {
        let app = app.clone();
        div()
            .id("dispatch-fresh")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(lucide(
                if fresh { "circle-check" } else { "circle-dot" },
                14.,
                if fresh { t.clean } else { t.fg3 },
            ))
            .child(
                div()
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg1))
                    .child("Fresh worktree"),
            )
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| this.toggle_dispatch_fresh(cx));
            })
    };

    let (app2, repo2, input2) = (app.clone(), repo, input.clone());
    s = s.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mt(px(2.))
            .child(gpui_component::input::Input::new(input))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(toggle)
                    .child(div().flex_1())
                    .child(pr_btn(
                        SharedString::from("dispatch-agent"),
                        "Dispatch",
                        t,
                        move |cx| {
                            let repo = repo2.clone();
                            let prompt = input2.read(cx).value().trim().to_string();
                            if prompt.is_empty() {
                                return;
                            }
                            app2.update(cx, |this, cx| {
                                this.dispatch_agent(repo, prompt, fresh, cx)
                            });
                        },
                    )),
            ),
    );
    s
}

/// The Overview's advisory fact: the breakdown in the badge colour, or a
/// muted "advisories unavailable" with the reason (and the re-connect hint
/// for a token without the Dependabot scope) on hover.
fn advisory_seg(report: &orrery_core::advisories::AdvisoryReport, t: &Theme) -> impl IntoElement {
    use crate::shell::hygiene::{self, AdvisoryBadge, Tier};
    let text = hygiene::describe(report);
    let (label, color) = match hygiene::badge_of(report) {
        Some(AdvisoryBadge::Open(c)) => {
            let (label, tier) = hygiene::badge_label(&c);
            let color = match tier {
                Tier::Severe => t.behind,
                Tier::Moderate => t.dirty,
                Tier::Mild => t.fg2,
            };
            // "3 high · 5 advisories": the loudest tier, then the total.
            let n = c.total();
            let noun = if n == 1 { "advisory" } else { "advisories" };
            (format!("{label} · {n} {noun}"), color)
        }
        Some(AdvisoryBadge::Unavailable) => ("advisories unavailable".to_string(), t.fg3),
        None => ("no known advisories".to_string(), t.clean),
    };
    div()
        .id("advisory-fact")
        .child(seg("shield-alert", SharedString::from(label), color))
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(text.clone()).build(window, cx)
        })
}

/// A small square icon button (a worktree row's IDE / folder launchers).
fn icon_btn(
    id: String,
    icon: &str,
    t: &Theme,
    on: impl Fn(&mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .w(px(22.))
        .h(px(22.))
        .rounded(px(t.r_xs))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.surface_hover)))
        .child(lucide(icon, 12., t.fg2))
        .on_click(move |_ev, _win, cx| on(cx))
}

/// A worktree Orrery leaves to its own tool: agent worktrees under a
/// `.claude` directory, and Orrery's dispatch worktrees (the Agents view
/// tracks those). Hidden from the drawer list, as from the scan.
fn tool_managed(path: &str) -> bool {
    let p = std::path::Path::new(path);
    if p.components().any(|c| c.as_os_str() == ".claude") {
        return true;
    }
    orrery_core::dispatch::worktree_dest("/r", "w")
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .is_some_and(|dir| p.starts_with(dir))
}

fn worktrees_section(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let open = data.worktrees_open;
    let (shown, hidden): (Vec<&WorktreeRow>, usize) = match &data.worktrees {
        Some(list) => {
            let shown: Vec<&WorktreeRow> = list.iter().filter(|w| !tool_managed(&w.path)).collect();
            let hidden = list.len() - shown.len();
            (shown, hidden)
        }
        None => (Vec::new(), 0),
    };
    let count = if data.worktrees.is_some() {
        shown.len()
    } else {
        row.worktrees.len()
    };
    // The scanned state (branch, dirty, ahead/behind) of a listed worktree.
    let scanned = |path: &str| {
        let path = path.trim_end_matches('/');
        row.worktrees
            .iter()
            .find(|w| w.id.trim_end_matches('/') == path)
    };
    let dirty = row.worktrees.iter().filter(|w| w.dirty > 0).count();

    // Clickable header: chevron + "WORKTREES · n", and a dirty summary when
    // collapsed.
    let header = {
        let app = app.clone();
        let mut h = div()
            .id("wt-toggle")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .mb(px(3.))
            .cursor_pointer()
            .child(lucide(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                13.,
                t.fg3,
            ))
            .child(widgets::section_header(
                "Worktrees",
                widgets::Header::Caps { count: Some(count) },
                t,
            ))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| this.toggle_drawer_worktrees(cx));
            });
        if !open && dirty > 0 {
            h = h.child(
                div()
                    .ml(px(6.))
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.dirty))
                    .child(SharedString::from(format!("{dirty} dirty"))),
            );
        }
        h
    };
    let mut s = div().flex().flex_col().gap(px(3.)).child(header);
    if !open {
        return s;
    }
    match &data.worktrees {
        None => s = s.child(placeholder("Loading…", t)),
        Some(_) if shown.is_empty() && hidden == 0 => s = s.child(placeholder("None.", t)),
        Some(_) => {
            for w in shown {
                let remove = {
                    let (app, name) = (app.clone(), w.name.to_string());
                    div()
                        .id(SharedString::from(format!("wt-rm-{}", w.name)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(22.))
                        .h(px(22.))
                        .rounded(px(t.r_xs))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.surface_hover)))
                        .child(lucide("x", 12., t.fg3))
                        .on_click(move |_ev, _win, cx| {
                            let name = name.clone();
                            app.update(cx, |this, cx| this.drawer_remove_worktree(name, cx));
                        })
                };
                // Branch + state from the scan when it has seen this
                // worktree; the git worktree name until the next rescan.
                let info = scanned(&w.path);
                let branch = info.map(|i| i.branch.clone()).unwrap_or(w.name.clone());
                let mut state = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .flex_none();
                if let Some(i) = info {
                    if i.ahead > 0 || i.behind > 0 {
                        let color = if i.behind > 0 { t.behind } else { t.clean };
                        state = state.child(
                            div()
                                .text_color(rgb(color))
                                .child(SharedString::from(format!("↑{} ↓{}", i.ahead, i.behind))),
                        );
                    }
                    state = state.child(if i.dirty > 0 {
                        seg(
                            "circle-dot",
                            SharedString::from(format!("{} dirty", i.dirty)),
                            t.dirty,
                        )
                        .into_any_element()
                    } else {
                        div()
                            .text_color(rgb(t.clean))
                            .child("clean")
                            .into_any_element()
                    });
                }
                let path = w.path.clone();
                let ide = {
                    let (app, path) = (app.clone(), path.clone());
                    icon_btn(format!("wt-ide-{}", w.name), "code", t, move |cx| {
                        let cmd = app.read(cx).config.ide_command.clone();
                        let _ = launch::launch(&cmd, &path);
                    })
                };
                let folder = icon_btn(format!("wt-dir-{}", w.name), "folder-open", t, {
                    let path = path.clone();
                    move |_cx| {
                        let _ = launch::open(&path);
                    }
                });
                s = s.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .py(px(4.))
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(7.))
                                .child(lucide("folder-tree", 13., t.fg2))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_color(rgb(t.fg1))
                                        .child(branch),
                                )
                                .child(state)
                                .child(ide)
                                .child(folder)
                                .child(remove),
                        )
                        .child(
                            div()
                                .pl(px(20.))
                                .truncate()
                                .text_color(rgb(t.fg3))
                                .child(path),
                        ),
                );
            }
            if hidden > 0 {
                s = s.child(placeholder(
                    format!(
                        "+{hidden} tool-managed worktree{} (.claude, agent dispatch) not shown",
                        if hidden == 1 { "" } else { "s" }
                    ),
                    t,
                ));
            }
        }
    }

    // Add a worktree: name field + Add button.
    if let Some(input) = &data.worktree_input {
        let (app, input2) = (app.clone(), input.clone());
        s = s.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .mt(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(gpui_component::input::Input::new(input)),
                )
                .child(pr_btn(SharedString::from("wt-add"), "Add", t, move |cx| {
                    let name = input2.read(cx).value();
                    if name.trim().is_empty() {
                        return;
                    }
                    app.update(cx, |this, cx| {
                        this.drawer_add_worktree(name.trim().to_string(), cx)
                    });
                })),
        );
    }
    s
}
