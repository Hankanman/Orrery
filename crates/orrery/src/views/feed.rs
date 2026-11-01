//! Feed view — a release/activity radar for repos and people you follow on
//! GitHub, plus releases from your starred GitLab projects: releases (with
//! tags), and starred/created/forked/open-sourced events, each marked with
//! its host.
//! Loaded lazily over the network when the nav item is selected.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

use super::{ListBody, VList, list_key};
use crate::data;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

#[derive(Default)]
pub enum FeedState {
    #[default]
    Idle,
    Loading,
    Ready(Vec<FeedRow>),
    Error(SharedString),
}

/// A render-ready feed entry.
pub struct FeedRow {
    pub icon: &'static str,
    /// Source event kind ("release" / "starred" / "forked" / …) — drives the
    /// sidebar's activity-type filter.
    pub kind: SharedString,
    pub repo: SharedString,
    pub line: SharedString,   // "alice starred" / release title
    pub detail: SharedString, // release notes snippet, etc.
    pub tag: SharedString,    // release tag, or ""
    pub age: SharedString,
    pub prerelease: bool,
    pub url: SharedString,
    /// Newer than the last time the Feed was viewed (drives the "New" filter).
    pub is_new: bool,
    /// Brand-mark name of the host it came from (see `icon::host_brand`).
    pub host: &'static str,
}

fn action(kind: &str) -> &'static str {
    match kind {
        "starred" => "starred this",
        "created" => "created this",
        "forked" => "forked this",
        "public" => "open-sourced this",
        _ => "",
    }
}

pub fn feed_row(f: inbox::FeedItem, now: i64, since: i64) -> FeedRow {
    let icon = match f.kind.as_str() {
        "release" => "tag",
        "starred" => "star",
        "forked" => "git-branch",
        _ => "box",
    };
    let line = if f.kind == "release" {
        if f.title.is_empty() {
            format!("Release {}", f.tag)
        } else {
            f.title.clone()
        }
    } else {
        let actor = f.actor.as_deref().unwrap_or("Someone");
        format!("{actor} {}", action(&f.kind))
    };
    FeedRow {
        host: crate::icon::host_brand(f.host, None),
        icon,
        kind: f.kind.into(),
        repo: f.repo.into(),
        line: data::oneline(line).into(),
        detail: data::oneline(f.detail).into(),
        tag: f.tag.into(),
        age: data::rel_age(f.timestamp, now).into(),
        prerelease: f.prerelease,
        url: f.url.into(),
        is_new: f.timestamp > since,
    }
}

/// Does `row` pass the sidebar's activity-type filter? Each filter key maps to a
/// source event kind; `Some("new")` keeps items newer than the last visit;
/// `None` keeps everything.
fn passes(row: &FeedRow, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some("new") => row.is_new,
        Some(kind) => row.kind.as_ref() == kind,
    }
}

/// Render the Feed view: the filtered rows in a virtualised list.
pub fn render(
    state: &FeedState,
    filter: Option<&str>,
    list: &VList,
    theme: &Rc<Theme>,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let t = theme.as_ref();
    let note = |text: SharedString| ListBody::Note(super::note(text, t).into_any_element());
    let body = match state {
        FeedState::Idle | FeedState::Loading => note("Loading…".into()),
        FeedState::Error(e) => note(e.clone()),
        FeedState::Ready(rows) if rows.is_empty() => note("Nothing new in your feed.".into()),
        FeedState::Ready(rows) => {
            // Indices of the rows passing the filter.
            let shown: Rc<Vec<usize>> = Rc::new(
                (0..rows.len())
                    .filter(|&i| passes(&rows[i], filter))
                    .collect(),
            );
            if shown.is_empty() {
                note("Nothing in this filter.".into())
            } else {
                let urls: Vec<&SharedString> = shown.iter().map(|&i| &rows[i].url).collect();
                list.sync(shown.len(), list_key((filter, urls)));
                let (app, theme) = (app.clone(), theme.clone());
                ListBody::List(
                    gpui::list(list.state.clone(), move |ix, _window, cx| {
                        let FeedState::Ready(rows) = &app.read(cx).feed else {
                            return div().into_any_element();
                        };
                        let Some(row) = shown.get(ix).and_then(|&i| rows.get(i)) else {
                            return div().into_any_element();
                        };
                        // The 4px gap that separated the rows.
                        let item = div()
                            .when(ix > 0, |d| d.pt(px(4.)))
                            .child(feed_item(row, &theme));
                        VList::pad(ix, shown.len(), item)
                    })
                    .size_full()
                    .into_any_element(),
                )
            }
        }
    };

    super::list_frame("Feed", t, app, OrreryApp::load_feed, body)
}

fn feed_item(r: &FeedRow, t: &Theme) -> impl IntoElement {
    let url = r.url.clone();
    let mut top = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide(r.icon, 14., t.fg2))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(r.repo.clone()),
        );
    if !r.tag.is_empty() {
        top = top.child(super::tag(&r.tag, t.fg2, t));
    }
    if r.prerelease {
        top = top.child(super::tag("pre-release", t.behind, t));
    }
    if r.is_new {
        top = top.child(super::tag("new", t.accent_bright, t));
    }
    top = top.child(div().flex_1());
    if !r.host.is_empty() {
        top = top.child(crate::icon::brand(r.host, 12., t.fg3));
    }
    top = top.child(super::muted_mono(r.age.clone(), t));

    let mut col = div()
        .id(r.url.clone())
        .flex()
        .flex_col()
        .gap(px(3.))
        .px(px(10.))
        .py(px(10.))
        .rounded(px(t.r_sm))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.surface_hover)))
        .on_click(move |_ev, _win, _cx| {
            let _ = launch::open(&url);
        })
        .child(top)
        .child(
            div()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg2))
                .child(r.line.clone()),
        );
    if !r.detail.trim().is_empty() {
        col = col.child(
            div()
                .truncate()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(r.detail.clone()),
        );
    }
    col
}
