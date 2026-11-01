//! Drawer Changes tab: the commit composer, push / open-PR, the Unstaged /
//! Staged file lists and the selected file's diff.

use super::*;

/// Changes tab: a commit composer acting on the index, a push / open-PR row
/// closing the commit → push → PR loop, the Unstaged/Staged file lists with
/// per-file stage/unstage, then the selected file's diff.
pub(super) fn changes_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(12.));
    let has_staged = data
        .changes
        .as_ref()
        .is_some_and(|list| list.iter().any(|c| c.staged));

    // Commit composer (the fields exist once the tab has been opened): a
    // single-line subject + an optional multi-line body, joined by a blank
    // line into the commit message.
    if let (Some(input), Some(body_input)) = (&data.commit_input, &data.commit_body_input) {
        let mut actions = div().flex().flex_row().items_center().gap(px(6.));
        // AI: suggest a commit message for the staged diff (gated on aiReady).
        if ai_ready {
            let app3 = app.clone();
            actions = actions.child(pr_btn(
                SharedString::from("gen-commit"),
                "Generate message",
                t,
                move |cx: &mut gpui::App| {
                    app3.update(cx, |this, cx| this.drawer_generate_commit(cx));
                },
            ));
        }
        actions = actions.child(div().flex_1());
        if has_staged {
            let app2 = app.clone();
            let input2 = input.clone();
            let body2 = body_input.clone();
            actions = actions.child(pr_btn(
                SharedString::from("commit"),
                "Commit",
                t,
                move |cx: &mut gpui::App| {
                    let subject = input2.read(cx).value();
                    if subject.trim().is_empty() {
                        return;
                    }
                    let msg = full_commit_message(&subject, &body2.read(cx).value());
                    app2.update(cx, |this, cx| this.drawer_commit_staged(msg, cx));
                },
            ));
        } else {
            // Nothing in the index — commit would be a no-op, so say why (once
            // the lists have actually loaded) instead of offering a live button.
            if data.changes.is_some() {
                actions = actions.child(
                    div()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg3))
                        .child(SharedString::from("Nothing staged")),
                );
            }
            actions = actions.child(disabled_btn("Commit", t));
        }
        col = col
            .child(gpui_component::input::Input::new(input))
            .child(gpui_component::input::Input::new(body_input))
            .child(actions);
    }

    // The AI commit-message suggestion (subject + optional body), with a
    // one-click commit using both.
    if let Some(suggestion) = &data.commit_suggestion {
        let app4 = app.clone();
        let (subject2, body2) = (suggestion.subject.clone(), suggestion.body.clone());
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(t.r_sm))
            .bg(rgb(t.surface))
            .border_1()
            .border_color(rgb(t.border))
            .child(seg("sparkles", suggestion.subject.clone(), t.fg1));
        if !suggestion.body.is_empty() {
            // Multi-line body → markdown, pre-reflowed (the renderer panics on
            // a run's embedded newlines, see data::unwrap_soft_breaks).
            card = card.child(
                div()
                    .text_size(px(t.text_small))
                    .text_color(rgb(t.fg2))
                    .child(gpui_component::text::markdown(suggestion.body_md.clone())),
            );
        }
        col = col.child(
            card.child(div().flex().flex_row().justify_end().child(pr_btn(
                SharedString::from("commit-ai"),
                "Commit this",
                t,
                move |cx: &mut gpui::App| {
                    if subject2.trim().is_empty() {
                        return;
                    }
                    let msg = full_commit_message(&subject2, &body2);
                    app4.update(cx, |this, cx| this.drawer_commit_staged(msg, cx));
                },
            ))),
        );
    }

    // Push / Open PR — the last mile after committing. Push shows when there's
    // something to push (ahead, or a commit made in this drawer session while
    // the row's ahead count may be stale). Open PR shows only when it can work:
    // a GitHub remote, a GitHub token, a known default branch, and the current
    // branch isn't it — hidden otherwise, never broken (the aiReady philosophy;
    // only the AI *drafting* inside the flow is gated on aiReady).
    let show_push = row.ahead > 0 || data.committed;
    let show_pr = github_authed
        && github_slug(row).is_some()
        && data
            .default_branch
            .as_ref()
            .is_some_and(|d| *d != row.branch);
    if show_push || show_pr || data.pr_url.is_some() {
        let mut sync = div().flex().flex_row().items_center().gap(px(6.));
        if show_push {
            if data.push_busy {
                sync = sync.child(disabled_btn("Pushing…", t));
            } else {
                let app6 = app.clone();
                sync = sync.child(pr_btn(
                    SharedString::from("push"),
                    "Push",
                    t,
                    move |cx: &mut gpui::App| {
                        app6.update(cx, |this, cx| this.drawer_push(cx));
                    },
                ));
            }
        }
        if show_pr {
            if data.pr_busy {
                sync = sync.child(disabled_btn("Opening PR…", t));
            } else {
                let app7 = app.clone();
                sync = sync.child(pr_btn(
                    SharedString::from("open-pr"),
                    "Open PR",
                    t,
                    move |cx: &mut gpui::App| {
                        app7.update(cx, |this, cx| this.drawer_open_pr(cx));
                    },
                ));
            }
        }
        if let Some(url) = &data.pr_url {
            let url = url.clone();
            sync = sync.child(
                div()
                    .id("view-pr")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(t.r_sm))
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg1))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.surface_hover)))
                    .child(lucide("external-link", 12., t.fg2))
                    .child(SharedString::from("View PR"))
                    .on_click(move |_ev, _win, _cx| {
                        let _ = launch::open(&url);
                    }),
            );
        }
        col = col.child(sync);
    }

    // Unstaged / Staged file lists, then the selected file's diff.
    col = col.child(changes_section(data, false, t, app));
    col = col.child(changes_section(data, true, t, app));
    col = col.child(diff_pane(data, t, app));

    // AI changelog from recent commits (gated on aiReady).
    if ai_ready {
        let app5 = app.clone();
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(pr_btn(
                    SharedString::from("gen-changelog"),
                    "Generate changelog",
                    t,
                    move |cx: &mut gpui::App| {
                        app5.update(cx, |this, cx| this.drawer_generate_changelog(cx));
                    },
                )),
        );
    }
    if let Some(log) = &data.changelog {
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(seg("sparkles", "Changelog".into(), t.fg3))
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(
                            // The prompt asks for markdown bullets, so render as
                            // markdown — already reflowed when stored (like the Readme), since
                            // the renderer panics on a run's embedded newlines.
                            gpui_component::text::markdown(log.clone()),
                        ),
                ),
        );
    }
    col
}

/// The kind marker for a change row: git's one-letter convention, coloured by
/// sentiment (M/A/D/R and "?" for untracked — each kind visibly distinct).
fn kind_badge(kind: git_ops::ChangeKind, t: &Theme) -> (&'static str, u32) {
    match kind {
        git_ops::ChangeKind::Modified => ("M", t.dirty),
        git_ops::ChangeKind::Added => ("A", t.clean),
        git_ops::ChangeKind::Deleted => ("D", t.behind),
        git_ops::ChangeKind::Renamed => ("R", t.accent_bright),
        git_ops::ChangeKind::Untracked => ("?", t.fg2),
    }
}

/// One of the Changes tab's file lists (`staged` picks which). Header: label +
/// count + a "Stage all"/"Unstage all" action; body: one row per file.
fn changes_section(data: &DrawerData, staged: bool, t: &Theme, app: &Entity<OrreryApp>) -> Div {
    let title = if staged { "STAGED" } else { "UNSTAGED" };
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(title));

    let mut col = div().flex().flex_col().gap(px(2.));
    match &data.changes {
        None => col = col.child(placeholder("Loading…", t)),
        Some(list) => {
            let files: Vec<&ChangeRow> = list.iter().filter(|c| c.staged == staged).collect();
            head = head.child(SharedString::from(format!("· {}", files.len())));
            if files.is_empty() {
                let empty = if staged {
                    "Nothing staged."
                } else {
                    "No unstaged changes."
                };
                col = col.child(placeholder(empty, t));
            } else {
                // Stage all / Unstage all on the section header.
                let label = if staged { "Unstage all" } else { "Stage all" };
                let all: Vec<String> = files.iter().map(|c| c.path.to_string()).collect();
                let app2 = app.clone();
                head = head.child(div().flex_1()).child(pr_btn(
                    SharedString::from(format!("stage-all-{staged}")),
                    label,
                    t,
                    move |cx: &mut gpui::App| {
                        let all = all.clone();
                        app2.update(cx, |this, cx| this.drawer_set_staged(all, !staged, cx));
                    },
                ));
                for c in files {
                    col = col.child(change_item(c, data, t, app));
                }
            }
        }
    }
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(head.mb(px(3.)))
        .child(col)
}

/// One file row: kind letter + path + a stage/unstage icon button. Clicking
/// the row selects it for the diff pane; the trailing button moves it between
/// the index and the working tree ("+" stages, "x" unstages — the same
/// remove-from-list affordance as the worktree rows).
fn change_item(
    c: &ChangeRow,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let selected = data
        .change_sel
        .as_ref()
        .is_some_and(|(p, s)| *p == c.path && *s == c.staged);
    let (letter, color) = kind_badge(c.kind, t);

    let action = {
        let (app, path) = (app.clone(), c.path.to_string());
        let (staged, icon) = (c.staged, if c.staged { "x" } else { "plus" });
        div()
            .id(SharedString::from(format!(
                "chg-act-{}-{}",
                c.staged, c.path
            )))
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(t.r_xs))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide(icon, 12., t.fg3))
            .on_click(move |_ev, _win, cx| {
                // Don't also select the row underneath.
                cx.stop_propagation();
                let path = path.clone();
                app.update(cx, |this, cx| {
                    this.drawer_set_staged(vec![path], !staged, cx)
                });
            })
    };

    let row = div()
        .id(SharedString::from(format!("chg-{}-{}", c.staged, c.path)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(t.r_sm))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .cursor_pointer()
        .child(
            div()
                .w(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(color))
                .child(SharedString::from(letter)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(rgb(if selected { t.fg0 } else { t.fg1 }))
                .child(c.path.clone()),
        )
        .child(action);
    let row = if selected {
        row.bg(rgb(t.accent_wash))
    } else {
        row.hover(|s| s.bg(rgb(t.surface_hover)))
    };
    let (app, path, staged) = (app.clone(), c.path.clone(), c.staged);
    row.on_click(move |_ev, _win, cx| {
        let path = path.clone();
        app.update(cx, |this, cx| this.drawer_select_change(path, staged, cx));
    })
}

/// The selected file's diff, under a header naming the file and its side.
fn diff_pane(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> Div {
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from("DIFF"));
    if let Some((path, staged)) = &data.change_sel {
        let side = if *staged { "staged" } else { "unstaged" };
        head = head.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(SharedString::from(format!("· {path} ({side})"))),
        );
    }

    let body = match (&data.changes, &data.diff) {
        (None, _) => placeholder("Loading…", t).into_any_element(),
        (Some(list), _) if list.is_empty() => {
            placeholder("Working tree clean — nothing to commit.", t).into_any_element()
        }
        (_, DiffState::Ready(hunks)) if hunks.is_empty() => {
            placeholder("No diff for this file.", t).into_any_element()
        }
        (_, DiffState::Ready(hunks)) => match &data.change_sel {
            Some((path, staged)) => {
                diff_block(path.clone(), *staged, hunks, t, app).into_any_element()
            }
            None => placeholder("No diff for this file.", t).into_any_element(),
        },
        _ => placeholder("Loading…", t).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(head.mb(px(3.)))
        .child(body)
}

/// Cap on rendered diff *content* lines. The whole app re-renders on any
/// `cx.notify()` (agents poll, attention poll, appearance signals), so an
/// unbounded diff would rebuild thousands of elements on every background
/// tick. Hunk headers don't count against the cap.
const DIFF_MAX_LINES: usize = 500;

/// Render the selected file's hunks: each gets a header row — the `@@` line
/// plus a "Stage hunk"/"Unstage hunk" affordance matching the pane's side —
/// then its sentiment-coloured body lines. Content is truncated to
/// [`DIFF_MAX_LINES`] total with a muted "… n more" footer.
fn diff_block(
    path: SharedString,
    staged: bool,
    hunks: &[HunkRow],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut block = div()
        .flex()
        .flex_col()
        .p(px(10.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    let mut budget = DIFF_MAX_LINES;
    for (ix, hunk) in hunks.iter().enumerate() {
        if budget == 0 {
            // Cap hit: one muted footer counting everything left.
            let lines: usize = hunks[ix..].iter().map(|h| h.lines.len()).sum();
            let hunks_left = hunks.len() - ix;
            block = block.child(
                div()
                    .pt(px(6.))
                    .text_color(rgb(t.fg3))
                    .child(SharedString::from(format!(
                        "… {lines} more lines in {hunks_left} more hunks"
                    ))),
            );
            break;
        }

        // Hunk header: the @@ line + the stage/unstage-this-hunk affordance.
        let action = {
            let (app, path) = (app.clone(), path.clone());
            let label = if staged { "Unstage hunk" } else { "Stage hunk" };
            div()
                .id(SharedString::from(format!("hunk-act-{staged}-{ix}")))
                .px(px(6.))
                .py(px(1.))
                .rounded(px(t.r_xs))
                .text_color(rgb(t.fg3))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.surface_hover)).text_color(rgb(t.fg1)))
                .child(SharedString::from(label))
                .on_click(move |_ev, _win, cx| {
                    let path = path.clone();
                    app.update(cx, |this, cx| this.drawer_hunk_op(path, staged, ix, cx));
                })
        };
        let mut header_row = div().flex().flex_row().items_center().gap(px(6.));
        if ix > 0 {
            header_row = header_row.mt(px(6.));
        }
        block = block.child(
            header_row
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(rgb(t.accent_bright))
                        .child(hunk.header.clone()),
                )
                .child(action),
        );

        for line in hunk.lines.iter().take(budget) {
            let color = match line.as_bytes().first() {
                Some(b'+') => t.clean,
                Some(b'-') => t.behind,
                _ => t.fg2,
            };
            block = block.child(div().text_color(rgb(color)).child(line.clone()));
        }
        if hunk.lines.len() > budget {
            let hidden = hunk.lines.len() - budget;
            block = block.child(
                div()
                    .pt(px(6.))
                    .text_color(rgb(t.fg3))
                    .child(SharedString::from(format!("… {hidden} more lines"))),
            );
        }
        budget = budget.saturating_sub(hunk.lines.len());
    }
    block
}
