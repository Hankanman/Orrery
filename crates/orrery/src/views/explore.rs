//! Explore view — browse your GitHub-starred repos and one-click clone them into
//! the first configured workspace root (they appear in Mission Control on the
//! next scan). Loaded lazily over the network when the nav item is selected.

use std::collections::HashSet;

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{inbox, launch};

use crate::shell::OrreryApp;
use crate::theme::Theme;

#[derive(Default)]
pub enum ExploreState {
    #[default]
    Idle,
    Loading,
    Ready(Vec<StarRow>),
    Error(SharedString),
}

/// A render-ready starred repo.
pub struct StarRow {
    pub slug: SharedString,
    pub name: SharedString, // trailing segment of the slug (clone dir)
    pub description: SharedString,
    pub stars: u32,
    pub language: SharedString,
    pub clone_url: SharedString,
    pub url: SharedString, // host page
}

pub fn star_row(r: inbox::RemoteRepo) -> StarRow {
    let name = r.slug.rsplit('/').next().unwrap_or(&r.slug).to_string();
    StarRow {
        url: format!("https://github.com/{}", r.slug).into(),
        name: name.into(),
        slug: r.slug.into(),
        description: r.description.unwrap_or_default().into(),
        stars: r.stars,
        language: r.language.unwrap_or_default().into(),
        clone_url: r.clone_url.into(),
    }
}

pub fn render(
    state: &ExploreState,
    cloned: &HashSet<SharedString>,
    cloning: &HashSet<SharedString>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let body = match state {
        ExploreState::Idle | ExploreState::Loading => super::note("Loading…", t).into_any_element(),
        ExploreState::Error(e) => super::note(e.clone(), t).into_any_element(),
        ExploreState::Ready(rows) if rows.is_empty() => {
            super::note("No starred repos — star some on GitHub.", t).into_any_element()
        }
        ExploreState::Ready(rows) => {
            let mut col = div().flex().flex_col().gap(px(10.));
            for r in rows {
                col = col.child(star_card(
                    r,
                    cloned.contains(&r.slug),
                    cloning.contains(&r.slug),
                    t,
                    app,
                ));
            }
            col.into_any_element()
        }
    };
    super::frame(
        "Explore",
        t,
        app,
        OrreryApp::load_starred,
        "explore-scroll",
        body,
    )
}

fn star_card(
    r: &StarRow,
    is_cloned: bool,
    is_cloning: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let url = r.url.clone();
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .id(r.slug.clone())
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .hover(|s| s.text_color(rgb(t.accent_bright)))
                .on_click(move |_ev, _win, _cx| {
                    let _ = launch::open(&url);
                })
                .child(r.slug.clone()),
        )
        .child(div().flex_1())
        .child(clone_button(r, is_cloned, is_cloning, t, app));

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(12.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(head);
    if !r.description.trim().is_empty() {
        card = card.child(
            div()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg2))
                .child(r.description.clone()),
        );
    }
    // Meta: language + stars.
    let mut meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3));
    if !r.language.is_empty() {
        meta = meta.child(div().text_color(rgb(t.fg2)).child(r.language.clone()));
    }
    meta = meta.child(SharedString::from(format!("★ {}", r.stars)));
    card.child(meta)
}

fn clone_button(
    r: &StarRow,
    is_cloned: bool,
    is_cloning: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let (label, enabled) = if is_cloned {
        ("Cloned", false)
    } else if is_cloning {
        ("Cloning…", false)
    } else {
        ("Clone", true)
    };
    let fg = if enabled { t.fg1 } else { t.fg3 };
    let mut btn = div()
        .id(SharedString::from(format!("clone-{}", r.slug)))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(SharedString::from(label));
    if enabled {
        let (app, slug, url, name) = (
            app.clone(),
            r.slug.clone(),
            r.clone_url.clone(),
            r.name.clone(),
        );
        btn = btn
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(t.border_strong)).text_color(rgb(t.fg0)))
            .on_click(move |_ev, _win, cx| {
                let (slug, url, name) = (slug.clone(), url.clone(), name.clone());
                app.update(cx, |this, cx| this.clone_starred(slug, url, name, cx));
            });
    }
    btn
}
