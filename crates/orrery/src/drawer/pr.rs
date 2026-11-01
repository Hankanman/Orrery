//! Drawer PR tab: open pull requests with check / review / mergeable rollups
//! and inline approve / merge actions.

use super::*;

/// Pick an icon for an individual check state.
fn check_icon(state: &str) -> &'static str {
    match state {
        "success" => "circle-check",
        "failure" => "circle-alert",
        "pending" => "clock",
        _ => "circle-dot",
    }
}

/// Colour a PR state string (checks / mergeable / review) by sentiment.
fn state_color(s: &str, t: &Theme) -> u32 {
    match s {
        "success" | "clean" | "approved" => t.clean,
        "failure" | "conflicting" | "changes_requested" => t.behind,
        "pending" => t.fg2,
        _ => t.fg3,
    }
}

/// PR tab: the open pull requests with checks/review/mergeable rollups and inline
/// approve / merge actions.
pub(super) fn pr_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    match &data.pr {
        PrState::Ready { prs, .. } if prs.is_empty() => {
            placeholder("No open pull requests.", t).into_any_element()
        }
        PrState::Ready { methods, prs } => {
            let slug = row.slug.clone();
            let mut col = div().flex().flex_col().gap(px(10.));
            for pr in prs {
                col = col.child(pr_card(pr, methods, slug.clone(), t, app));
            }
            col.into_any_element()
        }
        PrState::Error(e) => placeholder(e.clone(), t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    }
}

#[allow(clippy::too_many_arguments)]
fn pr_card(
    pr: &PrRow,
    methods: &[SharedString],
    slug: SharedString,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    // Title row (click opens the PR on the host).
    let title = {
        let url = pr.url.clone();
        let mut row = div()
            .id(SharedString::from(format!("pr-{}", pr.number)))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child(SharedString::from(format!("#{}", pr.number))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(t.text_small))
                    .text_color(rgb(t.fg0))
                    .child(pr.title.clone()),
            )
            .on_click(move |_ev, _win, _cx| {
                let _ = launch::open(&url);
            });
        if pr.draft {
            row = row.child(tag("draft", t.fg3, t));
        }
        row
    };

    // State rollups.
    let states = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(8.))
        .child(seg("git-pull-request", pr.refs.clone(), t.fg2))
        .child(seg_str(
            "circle-check",
            &pr.checks,
            state_color(&pr.checks, t),
        ))
        .child(seg_str(
            "git-merge",
            &pr.mergeable,
            state_color(&pr.mergeable, t),
        ))
        .child(seg_str("eye", &pr.review, state_color(&pr.review, t)));

    // Per-check breakdown: one clickable chip per status check / CI context,
    // so you can see which check failed and jump to its run (not just the rollup).
    let check_runs = (!pr.check_runs.is_empty()).then(|| {
        let mut wrap = div().flex().flex_row().flex_wrap().gap(px(6.));
        for c in &pr.check_runs {
            let color = state_color(&c.state, t);
            let chip = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .px(px(7.))
                .py(px(3.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.button_bg))
                .text_color(rgb(color))
                .child(lucide(check_icon(&c.state), 12., color))
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(c.name.clone()),
                );
            // Clickable (jumps to the CI run) only when the check carries a URL.
            let chip = match &c.url {
                Some(url) => {
                    let (url, hov) = (url.clone(), t.surface_hover);
                    chip.id(SharedString::from(format!(
                        "check-{}-{}",
                        pr.number, c.name
                    )))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(hov)))
                    .child(lucide("external-link", 11., t.fg3))
                    .on_click(move |_ev, _win, _cx| {
                        let _ = launch::open(&url);
                    })
                    .into_any_element()
                }
                None => chip.into_any_element(),
            };
            wrap = wrap.child(chip);
        }
        wrap
    });

    // Actions: approve + each allowed merge method.
    let mut actions = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(6.))
        .child(pr_btn(
            SharedString::from(format!("approve-{}", pr.number)),
            "Approve",
            t,
            {
                let (slug, number) = (slug.clone(), pr.number);
                let app = app.clone();
                move |cx: &mut gpui::App| {
                    let slug = slug.clone();
                    app.update(cx, |this, cx| {
                        this.drawer.pr = PrState::Loading;
                        this.drawer_approve_pr(slug, number, cx);
                        cx.notify();
                    });
                }
            },
        ));
    for method in methods {
        let label = capitalize(method);
        actions = actions.child(pr_btn(
            SharedString::from(format!("merge-{}-{method}", pr.number)),
            &label,
            t,
            {
                let (slug, number, method) = (slug.clone(), pr.number, method.to_string());
                let app = app.clone();
                move |cx: &mut gpui::App| {
                    let (slug, method) = (slug.clone(), method.clone());
                    app.update(cx, |this, cx| {
                        this.drawer.pr = PrState::Loading;
                        this.drawer_merge_pr(slug, number, method, cx);
                        cx.notify();
                    });
                }
            },
        ));
    }

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(11.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(title)
        .child(states);
    if let Some(check_runs) = check_runs {
        card = card.child(check_runs);
    }
    if !pr.author.is_empty() {
        card = card.child(
            div()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(format!("by {}", pr.author))),
        );
    }
    card.child(actions)
}

/// Title-case a lowercase merge-method name ("squash" → "Squash").
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Like [`seg`] but takes a `&str` label.
fn seg_str(icon: &str, label: &str, color: u32) -> impl IntoElement {
    seg(icon, SharedString::from(label.to_string()), color)
}
