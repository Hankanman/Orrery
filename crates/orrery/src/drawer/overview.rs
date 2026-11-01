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
    col = col.child(facts);

    // Async git data.
    col = col.child(branches_section(data, t, app));
    col = col.child(commits_section(data, t));
    col = col.child(worktrees_section(data, t, app));
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

fn worktrees_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut s = section(t, "Worktrees", data.worktrees.as_ref().map(|w| w.len()));
    match &data.worktrees {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("None.", t)),
        Some(list) => {
            for w in list {
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
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(rgb(t.fg3))
                                .child(w.path.clone()),
                        )
                        .child(remove),
                );
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
