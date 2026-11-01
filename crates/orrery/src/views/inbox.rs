//! Inbox view — "what's waiting on me?" across GitHub, GitLab and Gitea: open
//! pull requests, review requests, assigned issues, and notifications, grouped
//! and counted, each marked with its host and, when it has waited too long, a
//! stale badge. Loaded lazily (network, via the `task` bridge) when the nav
//! item is selected.

use std::rc::Rc;

use gpui::{
    AnyElement, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

use super::{ListBody, VList, list_key, note, tag};
use crate::icon::{brand, host_brand};
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
    /// Brand-mark name of the host it lives on (see `icon::host_brand`).
    pub host: &'static str,
}

pub struct NoticeRow {
    pub title: SharedString,
    pub repo: SharedString,
    pub reason: SharedString,
    /// Brand-mark name of the host it came from.
    pub host: &'static str,
    /// Web page for the subject, when the host gives one (GitLab, Gitea).
    pub url: Option<SharedString>,
}

/// Trailing `name` of an `owner/name` slug, for compact labels.
fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

pub fn inbox_row(i: inbox::InboxItem) -> InboxRow {
    InboxRow {
        tag: format!("{} #{}", short_repo(&i.repo), i.number).into(),
        host: host_brand(i.host, Some(&i.domain)),
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
        host: host_brand(n.host, Some(&n.domain)),
        url: n.url.filter(|u| !u.is_empty()).map(Into::into),
    }
}

/// The badge for a stale item at `now`: "waiting 3d" for a review request,
/// "stale 9d" for one of your PRs; `None` when it isn't stale.
pub fn stale_badge(
    item: &inbox::InboxItem,
    now: i64,
    thresholds: &inbox::StaleThresholds,
) -> Option<SharedString> {
    if !item.is_stale(now, thresholds) {
        return None;
    }
    let days = item.age_secs(now).unwrap_or(0) / 86_400;
    let word = if item.kind == "review" {
        "waiting"
    } else {
        "stale"
    };
    Some(format!("{word} {days}d").into())
}

/// The sidebar filter key for one host (`"host:gitlab"`), and back.
pub fn host_filter_key(brand: &str) -> SharedString {
    format!("host:{brand}").into()
}

fn filter_host(filter: Option<&str>) -> Option<&str> {
    filter.and_then(|f| f.strip_prefix("host:"))
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

/// Flatten the grouped inbox into list rows. The sidebar filter gates what
/// shows (None = all): a category key shows that group; `"stale"` shows only
/// stale items (`stale[i]`), across groups; `"host:<brand>"` shows one host's
/// items and notifications. Notifications appear in the unfiltered (All)
/// view, under their own filter, and under a host filter.
fn entries(d: &InboxData, filter: Option<&str>, stale: &[bool]) -> Vec<Entry> {
    let host = filter_host(filter);
    let across_groups = filter.is_none() || filter == Some("stale") || host.is_some();
    let show = |kind: &str| across_groups || filter == Some(kind);
    let keep_item = |i: usize| match filter {
        Some("stale") => stale.get(i).copied().unwrap_or(false),
        _ => host.is_none_or(|h| d.items[i].host == h),
    };
    let mut out = Vec::new();
    for (kind, icon, title) in GROUPS {
        let ixs: Vec<usize> = (0..d.items.len())
            .filter(|&i| d.items[i].kind.as_ref() == kind && keep_item(i))
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
    let notes: Vec<usize> = (0..d.notifications.len())
        .filter(|&i| host.is_none_or(|h| d.notifications[i].host == h))
        .collect();
    let notes_shown = filter.is_none() || filter == Some("notification") || host.is_some();
    if notes_shown && !notes.is_empty() {
        out.push(Entry::Header {
            icon: "inbox",
            title: "Notifications",
            count: notes.len(),
            spaced: !out.is_empty(),
        });
        out.extend(notes.into_iter().map(Entry::Notice));
    }
    out
}

/// Render the Inbox view: its rows in a virtualised list (a busy inbox runs
/// to hundreds of notifications).
pub fn render(
    state: &InboxState,
    filter: Option<&str>,
    thresholds: &inbox::StaleThresholds,
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
            // Stale badges, per item (`raw` runs parallel to `items`).
            let now = crate::data::now_unix();
            let badges: Rc<Vec<Option<SharedString>>> = Rc::new(
                d.raw
                    .iter()
                    .map(|i| stale_badge(i, now, thresholds))
                    .collect(),
            );
            let stale: Vec<bool> = badges.iter().map(Option::is_some).collect();
            let rows = Rc::new(entries(d, filter, &stale));
            let urls: Vec<&SharedString> = d.items.iter().map(|i| &i.url).collect();
            list.sync(
                rows.len(),
                list_key((filter, urls, d.notifications.len(), stale)),
            );
            let (app, theme) = (app.clone(), theme.clone());
            ListBody::List(
                gpui::list(list.state.clone(), move |ix, _window, cx| {
                    let InboxState::Ready(d) = &app.read(cx).inbox else {
                        return div().into_any_element();
                    };
                    let row = match rows.get(ix) {
                        Some(entry) => entry_row(*entry, d, &badges, &theme),
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

fn entry_row(
    entry: Entry,
    d: &InboxData,
    badges: &[Option<SharedString>],
    t: &Theme,
) -> AnyElement {
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
            .child(item_row(
                &d.items[i],
                badges.get(i).and_then(Option::as_ref),
                t,
            ))
            .into_any_element(),
        Entry::Notice(i) => div()
            .pt(px(2.))
            .child(notice_item(&d.notifications[i], i, t))
            .into_any_element(),
    }
}

/// The host's brand mark for a row, or nothing when the host is unknown.
fn host_mark(host: &'static str, t: &Theme) -> Option<impl IntoElement> {
    (!host.is_empty()).then(|| brand(host, 13., t.fg3))
}

fn item_row(it: &InboxRow, stale: Option<&SharedString>, t: &Theme) -> impl IntoElement {
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
        .children(host_mark(it.host, t))
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
    if let Some(label) = stale {
        // Amber "needs attention", not the danger red used for failures.
        row = row.child(tag(label, t.dirty, t));
    }
    row.child(
        div()
            .font_family("monospace")
            .text_size(px(t.text_data_sm))
            .text_color(rgb(t.fg3))
            .child(it.tag.clone()),
    )
}

fn notice_item(n: &NoticeRow, ix: usize, t: &Theme) -> impl IntoElement {
    let mut row = div()
        .id(SharedString::from(format!("inbox-notice-{ix}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(9.))
        .rounded(px(t.r_sm));
    // GitLab and Gitea notifications link to their subject; GitHub's don't.
    if let Some(url) = n.url.clone() {
        row = row
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .on_click(move |_ev, _win, _cx| {
                let _ = launch::open(&url);
            });
    }
    row.children(host_mark(n.host, t))
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

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::model::Host;

    const DAY: i64 = 86_400;

    fn item(kind: &str, host: Host, domain: &str, waiting_since: i64) -> inbox::InboxItem {
        inbox::InboxItem {
            kind: kind.into(),
            title: "t".into(),
            repo: "o/r".into(),
            url: format!("https://{domain}/{kind}/{waiting_since}"),
            number: 1,
            draft: false,
            host,
            domain: domain.into(),
            created_at: waiting_since,
            waiting_since,
        }
    }

    fn notice(host: Host, domain: &str) -> inbox::Notification {
        inbox::Notification {
            title: "n".into(),
            repo: "o/r".into(),
            reason: "mention".into(),
            kind: "Issue".into(),
            url: None,
            host,
            domain: domain.into(),
            updated_at: 0,
        }
    }

    fn data(raw: Vec<inbox::InboxItem>, notes: Vec<inbox::Notification>) -> InboxData {
        InboxData {
            items: raw.iter().cloned().map(inbox_row).collect(),
            notifications: notes.into_iter().map(notice_row).collect(),
            raw,
        }
    }

    fn shown(entries: &[Entry]) -> (Vec<usize>, Vec<usize>) {
        let mut items = Vec::new();
        let mut notes = Vec::new();
        for e in entries {
            match e {
                Entry::Item(i) => items.push(*i),
                Entry::Notice(i) => notes.push(*i),
                Entry::Header { .. } => {}
            }
        }
        (items, notes)
    }

    #[test]
    fn rows_carry_their_host_mark() {
        let d = data(
            vec![
                item("pr", Host::Github, "github.com", 0),
                item("pr", Host::Gitea, "codeberg.org", 0),
                item("pr", Host::Gitea, "git.acme.io", 0),
            ],
            vec![notice(Host::Gitlab, "gitlab.acme.io")],
        );
        let hosts: Vec<&str> = d.items.iter().map(|i| i.host).collect();
        assert_eq!(hosts, ["github", "codeberg", "gitea"]);
        assert_eq!(d.notifications[0].host, "gitlab");
    }

    #[test]
    fn stale_badge_names_the_wait() {
        let now = 100 * DAY;
        let t = inbox::StaleThresholds::default();
        let review = item("review", Host::Github, "github.com", now - 3 * DAY);
        assert_eq!(stale_badge(&review, now, &t).as_deref(), Some("waiting 3d"));
        let pr = item("pr", Host::Github, "github.com", now - 9 * DAY);
        assert_eq!(stale_badge(&pr, now, &t).as_deref(), Some("stale 9d"));
        let fresh = item("pr", Host::Github, "github.com", now - DAY);
        assert_eq!(stale_badge(&fresh, now, &t), None);
    }

    #[test]
    fn filters_by_stale_and_host() {
        let d = data(
            vec![
                item("pr", Host::Github, "github.com", 0),
                item("review", Host::Gitlab, "gitlab.com", 0),
                item("issue", Host::Gitlab, "gitlab.com", 0),
            ],
            vec![
                notice(Host::Github, "github.com"),
                notice(Host::Gitlab, "gitlab.com"),
            ],
        );
        let stale = [false, true, false];
        assert_eq!(
            shown(&entries(&d, None, &stale)),
            (vec![0, 1, 2], vec![0, 1])
        );
        // Stale: only stale items, across groups, no notifications.
        assert_eq!(
            shown(&entries(&d, Some("stale"), &stale)),
            (vec![1], vec![])
        );
        // A host: its items and notifications.
        let gitlab = host_filter_key("gitlab");
        assert_eq!(
            shown(&entries(&d, Some(&gitlab), &stale)),
            (vec![1, 2], vec![1])
        );
        // A category still shows one group only.
        assert_eq!(shown(&entries(&d, Some("pr"), &stale)), (vec![0], vec![]));
        assert_eq!(
            shown(&entries(&d, Some("notification"), &stale)),
            (vec![], vec![0, 1])
        );
    }
}
