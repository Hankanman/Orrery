//! Inbox view — "what's waiting on me?" across GitHub: open pull requests,
//! review requests, assigned issues, and notifications, grouped and counted.
//! Loaded lazily (network, via the `task` bridge) when the nav item is selected.

use std::rc::Rc;

use gpui::{
    AnyElement, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

use super::{ListBody, VList, list_key, note, tag};
use crate::shell::OrreryApp;
use crate::theme::Theme;
use crate::widgets::{self, Header};

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
    /// The raw core items behind `items` — kept for the attention model
    /// (`orrery_core::attention::compute`), which needs the host/slug facts
    /// the render rows flatten away.
    pub raw: Vec<inbox::InboxItem>,
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

/// One row of the virtualised Inbox list: a group header, or an item by its
/// index into [`InboxData::items`] / [`InboxData::notifications`].
#[derive(Clone, Copy)]
enum Entry {
    Header {
        icon: &'static str,
        title: &'static str,
        count: usize,
        /// Not the first group — leads with the gap between groups.
        spaced: bool,
    },
    Item(usize),
    Notice(usize),
}

/// The Inbox groups, in display order: (kind, icon, title).
const GROUPS: [(&str, &str, &str); 3] = [
    ("pr", "git-pull-request", "My pull requests"),
    ("review", "eye", "Awaiting your review"),
    ("issue", "circle-dot", "Assigned issues"),
];

/// Flatten the grouped inbox into list rows. The sidebar category filter
/// gates which groups show (None = all); notifications appear in the
/// unfiltered (All) view and under their own filter.
fn entries(d: &InboxData, filter: Option<&str>) -> Vec<Entry> {
    let show = |kind: &str| filter.is_none() || filter == Some(kind);
    let mut out = Vec::new();
    for (kind, icon, title) in GROUPS {
        let ixs: Vec<usize> = (0..d.items.len())
            .filter(|&i| d.items[i].kind.as_ref() == kind)
            .collect();
        if show(kind) && !ixs.is_empty() {
            out.push(Entry::Header {
                icon,
                title,
                count: ixs.len(),
                spaced: !out.is_empty(),
            });
            out.extend(ixs.into_iter().map(Entry::Item));
        }
    }
    if show("notification") && !d.notifications.is_empty() {
        out.push(Entry::Header {
            icon: "inbox",
            title: "Notifications",
            count: d.notifications.len(),
            spaced: !out.is_empty(),
        });
        out.extend((0..d.notifications.len()).map(Entry::Notice));
    }
    out
}

/// Render the Inbox view: its rows in a virtualised list (a busy inbox runs
/// to hundreds of notifications).
pub fn render(
    state: &InboxState,
    filter: Option<&str>,
    list: &VList,
    theme: &Rc<Theme>,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let t = theme.as_ref();
    let body = match state {
        InboxState::Idle | InboxState::Loading => {
            ListBody::Note(note("Loading…", t).into_any_element())
        }
        InboxState::Error(e) => ListBody::Note(note(e.clone(), t).into_any_element()),
        InboxState::Ready(d) if d.items.is_empty() && d.notifications.is_empty() => {
            ListBody::Note(note("Inbox zero — nothing awaiting you.", t).into_any_element())
        }
        InboxState::Ready(d) => {
            let rows = Rc::new(entries(d, filter));
            let urls: Vec<&SharedString> = d.items.iter().map(|i| &i.url).collect();
            list.sync(rows.len(), list_key((filter, urls, d.notifications.len())));
            let (app, theme) = (app.clone(), theme.clone());
            ListBody::List(
                gpui::list(list.state.clone(), move |ix, _window, cx| {
                    let InboxState::Ready(d) = &app.read(cx).inbox else {
                        return div().into_any_element();
                    };
                    let row = match rows.get(ix) {
                        Some(entry) => entry_row(*entry, d, &theme),
                        None => div().into_any_element(),
                    };
                    VList::pad(ix, rows.len(), row)
                })
                .size_full()
                .into_any_element(),
            )
        }
    };
    super::list_frame("Inbox", t, app, OrreryApp::load_inbox, body)
}

fn entry_row(entry: Entry, d: &InboxData, t: &Theme) -> AnyElement {
    match entry {
        Entry::Header {
            icon,
            title,
            count,
            spaced,
        } => {
            let head = widgets::section_header(title, Header::Group { icon, count }, t);
            if spaced {
                head.mt(px(22.)).into_any_element()
            } else {
                head.into_any_element()
            }
        }
        // The 2px gap that separated a group's rows.
        Entry::Item(i) => div()
            .pt(px(2.))
            .child(item_row(&d.items[i], t))
            .into_any_element(),
        Entry::Notice(i) => div()
            .pt(px(2.))
            .child(notice_item(&d.notifications[i], t))
            .into_any_element(),
    }
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

fn notice_item(n: &NoticeRow, t: &Theme) -> impl IntoElement {
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
        )
}
