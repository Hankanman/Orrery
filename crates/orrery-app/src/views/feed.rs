//! Feed view — a release/activity radar for repos and people you follow on
//! GitHub: releases (with tags), and starred/created/forked/open-sourced events.
//! Loaded lazily over the network when the nav item is selected.

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

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
    pub repo: SharedString,
    pub line: SharedString,   // "alice starred" / release title
    pub detail: SharedString, // release notes snippet, etc.
    pub tag: SharedString,    // release tag, or ""
    pub age: SharedString,
    pub prerelease: bool,
    pub url: SharedString,
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

pub fn feed_row(f: inbox::FeedItem, now: i64) -> FeedRow {
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
        icon,
        repo: f.repo.into(),
        line: line.into(),
        detail: f.detail.into(),
        tag: f.tag.into(),
        age: data::rel_age(f.timestamp, now).into(),
        prerelease: f.prerelease,
        url: f.url.into(),
    }
}

pub fn render(state: &FeedState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let body = match state {
        FeedState::Idle | FeedState::Loading => super::note("Loading…", t).into_any_element(),
        FeedState::Error(e) => super::note(e.clone(), t).into_any_element(),
        FeedState::Ready(rows) if rows.is_empty() => {
            super::note("Nothing new in your feed.", t).into_any_element()
        }
        FeedState::Ready(rows) => {
            let mut col = div().flex().flex_col().gap(px(4.));
            for r in rows {
                col = col.child(feed_item(r, t));
            }
            col.into_any_element()
        }
    };

    super::frame("Feed", t, app, OrreryApp::load_feed, "feed-scroll", body)
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
    top = top
        .child(div().flex_1())
        .child(super::muted_mono(r.age.clone(), t));

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
