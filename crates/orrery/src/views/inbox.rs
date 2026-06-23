//! Inbox view — "what's waiting on me?" across GitHub: open pull requests,
//! review requests, assigned issues, and notifications, grouped and counted.
//! Loaded lazily (network, via the `task` bridge) when the nav item is selected.

use gpui::{
    Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

use super::{note, section_header, tag};
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Lazy-loaded inbox state.
#[derive(Default)]
pub enum InboxState {
    #[default]
    Idle,
    Loading,
    Ready(InboxData),
    Error(SharedString),
}

pub struct InboxData {
    pub items: Vec<InboxRow>,
    pub notifications: Vec<NoticeRow>,
}

/// A render-ready attention item (PR / review request / assigned issue).
pub struct InboxRow {
    pub kind: SharedString, // "pr" | "review" | "issue"
    pub title: SharedString,
    pub tag: SharedString, // "repo #123"
    pub url: SharedString,
    pub draft: bool,
}

pub struct NoticeRow {
    pub title: SharedString,
    pub repo: SharedString,
    pub reason: SharedString,
}

/// Trailing `name` of an `owner/name` slug, for compact labels.
fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

pub fn inbox_row(i: inbox::InboxItem) -> InboxRow {
    InboxRow {
        tag: format!("{} #{}", short_repo(&i.repo), i.number).into(),
        kind: i.kind.into(),
        title: crate::data::oneline(i.title).into(),
        url: i.url.into(),
        draft: i.draft,
    }
}

pub fn notice_row(n: inbox::Notification) -> NoticeRow {
    NoticeRow {
        title: crate::data::oneline(n.title).into(),
        repo: short_repo(&n.repo).to_string().into(),
        reason: n.reason.into(),
    }
}

/// Render the Inbox view.
pub fn render(
    state: &InboxState,
    filter: Option<&str>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let body = match state {
        InboxState::Idle | InboxState::Loading => note("Loading…", t).into_any_element(),
        InboxState::Error(e) => note(e.clone(), t).into_any_element(),
        InboxState::Ready(d) if d.items.is_empty() && d.notifications.is_empty() => {
            note("Inbox zero — nothing awaiting you.", t).into_any_element()
        }
        InboxState::Ready(d) => ready(d, filter, t).into_any_element(),
    };
    super::frame("Inbox", t, app, OrreryApp::load_inbox, "inbox-scroll", body)
}

fn ready(d: &InboxData, filter: Option<&str>, t: &Theme) -> impl IntoElement {
    // The sidebar category filter gates which groups show (None = all).
    let show = |kind: &str| filter.is_none() || filter == Some(kind);
    let of_kind = |kind: &str| -> Vec<&InboxRow> {
        d.items.iter().filter(|i| i.kind.as_ref() == kind).collect()
    };
    let prs = of_kind("pr");
    let reviews = of_kind("review");
    let issues = of_kind("issue");

    let mut col = div().flex().flex_col().gap(px(22.));
    if show("pr") && !prs.is_empty() {
        col = col.child(group("git-pull-request", "My pull requests", &prs, t));
    }
    if show("review") && !reviews.is_empty() {
        col = col.child(group("eye", "Awaiting your review", &reviews, t));
    }
    if show("issue") && !issues.is_empty() {
        col = col.child(group("circle-dot", "Assigned issues", &issues, t));
    }
    // Notifications only appear in the unfiltered (All) view.
    if filter.is_none() && !d.notifications.is_empty() {
        col = col.child(notifications(&d.notifications, t));
    }
    col
}

fn group(icon: &str, title: &str, items: &[&InboxRow], t: &Theme) -> impl IntoElement {
    let mut col = section_header(icon, title, items.len(), t);
    for it in items {
        col = col.child(item_row(it, t));
    }
    col
}

fn item_row(it: &InboxRow, t: &Theme) -> impl IntoElement {
    let url = it.url.clone();
    let mut row = div()
        .id(it.url.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(9.))
        .rounded(px(t.r_sm))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.surface_hover)))
        .on_click(move |_ev, _win, _cx| {
            let _ = launch::open(&url);
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(it.title.clone()),
        );
    if it.draft {
        row = row.child(tag("draft", t.fg3, t));
    }
    row.child(
        div()
            .font_family("monospace")
            .text_size(px(t.text_data_sm))
            .text_color(rgb(t.fg3))
            .child(it.tag.clone()),
    )
}

fn notifications(notes: &[NoticeRow], t: &Theme) -> impl IntoElement {
    let mut col = section_header("inbox", "Notifications", notes.len(), t);
    for n in notes {
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(9.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(n.title.clone()),
                )
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg3))
                        .child(SharedString::from(format!("{} · {}", n.repo, n.reason))),
                ),
        );
    }
    col
}
