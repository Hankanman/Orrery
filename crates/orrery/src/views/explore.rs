//! Explore view — browse your GitHub-starred repos and one-click clone them into
//! the first configured workspace root (they appear in Mission Control on the
//! next scan). Loaded lazily over the network when the nav item is selected.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::model::Host;
use orrery_core::{inbox, launch};

use super::{ListBody, VList, list_key};
use crate::icon::brand;
use crate::shell::OrreryApp;
use crate::theme::Theme;
use crate::widgets::{self, Tone};

#[derive(Default)]
pub enum ExploreState {
    #[default]
    Idle,
    Loading,
    Ready(Vec<StarRow>),
    Error(SharedString),
}

/// The Explore view's UI state, alongside its loaded [`ExploreState`].
#[derive(Default)]
pub(crate) struct ExploreUi {
    /// Slugs currently being cloned.
    pub(crate) cloning: HashSet<SharedString>,
    /// Clone failures keyed by slug, shown on the card; cleared on retry.
    pub(crate) errors: HashMap<SharedString, SharedString>,
}

/// A render-ready starred repo.
pub struct StarRow {
    pub slug: SharedString,
    pub name: SharedString, // trailing segment of the slug (clone dir)
    pub description: SharedString,
    pub stars: u32,
    pub language: SharedString,
    pub clone_url: SharedString,
    pub url: SharedString,  // host page
    pub host: &'static str, // brand-icon name: "github" | "gitlab"
}

pub fn star_row(r: inbox::RemoteRepo) -> StarRow {
    let name = r.slug.rsplit('/').next().unwrap_or(&r.slug).to_string();
    // Link to the repo on its actual host, not always github.com.
    let (host, base) = match r.host {
        Host::Github => ("github", "https://github.com"),
        Host::Gitlab => ("gitlab", "https://gitlab.com"),
        // Stars only come from GitHub today; no Gitea brand icon yet.
        Host::Gitea => ("", "https://codeberg.org"),
    };
    StarRow {
        url: format!("{base}/{}", r.slug).into(),
        name: name.into(),
        slug: r.slug.into(),
        description: r.description.unwrap_or_default().into(),
        stars: r.stars,
        language: r.language.unwrap_or_default().into(),
        clone_url: r.clone_url.into(),
        host,
    }
}

/// Render the Explore view: the starred repos (filtered by language) as cards
/// in a virtualised list, under a note naming where clones land. `cloned`
/// holds the slugs already in the fleet; the in-flight clones and clone
/// errors are read from [`ExploreUi`].
#[allow(clippy::too_many_arguments)]
pub fn render(
    state: &ExploreState,
    cloned: Rc<HashSet<SharedString>>,
    errors: &HashMap<SharedString, SharedString>,
    filter: Option<&str>,
    root: Option<&str>,
    list: &VList,
    theme: &Rc<Theme>,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let t = theme.as_ref();
    let note = |text: SharedString| ListBody::Note(super::note(text, t).into_any_element());
    let body = match state {
        ExploreState::Idle | ExploreState::Loading => note("Loading…".into()),
        ExploreState::Error(e) => note(e.clone()),
        ExploreState::Ready(rows) if rows.is_empty() => {
            note("No starred repos — star some on GitHub.".into())
        }
        ExploreState::Ready(rows) => {
            let shown: Rc<Vec<usize>> = Rc::new(
                (0..rows.len())
                    .filter(|&i| filter.is_none_or(|lang| rows[i].language.as_ref() == lang))
                    .collect(),
            );
            if shown.is_empty() {
                note("Nothing in this filter.".into())
            } else {
                // Row 0 is the "Clones land in …" note when there's a root.
                let root: Option<SharedString> = root.map(|r| format!("Clones land in {r}").into());
                let lead = usize::from(root.is_some());
                let count = lead + shown.len();
                // A clone error adds a line to its card, so it's part of the key.
                let mut failed: Vec<&SharedString> = errors.keys().collect();
                failed.sort();
                let slugs: Vec<&SharedString> = shown.iter().map(|&i| &rows[i].slug).collect();
                list.sync(count, list_key((filter, &root, slugs, failed)));
                let (app, theme) = (app.clone(), theme.clone());
                ListBody::List(
                    gpui::list(list.state.clone(), move |ix, _window, cx| {
                        let this = app.read(cx);
                        let ExploreState::Ready(rows) = &this.explore else {
                            return div().into_any_element();
                        };
                        let t = theme.as_ref();
                        let item = match (ix.checked_sub(lead), &root) {
                            // Where a click-to-clone lands, so it's not a surprise.
                            (None, Some(root)) => div()
                                .font_family("monospace")
                                .text_size(px(t.text_data_sm))
                                .text_color(rgb(t.fg3))
                                .child(root.clone())
                                .into_any_element(),
                            (Some(n), _) => match shown.get(n).and_then(|&i| rows.get(i)) {
                                Some(r) => star_card(
                                    r,
                                    cloned.contains(&r.slug),
                                    this.explore_ui.cloning.contains(&r.slug),
                                    this.explore_ui.errors.get(&r.slug),
                                    t,
                                    &app,
                                )
                                .into_any_element(),
                                None => div().into_any_element(),
                            },
                            (None, None) => div().into_any_element(),
                        };
                        // The 10px gap that separated the cards.
                        let item = div().when(ix > 0, |d| d.pt(px(10.))).child(item);
                        VList::pad(ix, count, item)
                    })
                    .size_full()
                    .into_any_element(),
                )
            }
        }
    };
    super::list_frame("Explore", t, app, OrreryApp::load_starred, body)
}

fn star_card(
    r: &StarRow,
    is_cloned: bool,
    is_cloning: bool,
    error: Option<&SharedString>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let url = r.url.clone();
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(brand(r.host, 14., t.fg2))
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
    card = card.child(meta);
    // A failed clone (network drop, lost access, …) surfaces here; the button
    // is back to "Clone" so a retry clears it.
    if let Some(e) = error {
        card = card.child(
            div()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.behind))
                .child(e.clone()),
        );
    }
    card
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
    let tone = if enabled {
        Tone::Neutral
    } else {
        Tone::Disabled
    };
    let (app, slug, url, name) = (
        app.clone(),
        r.slug.clone(),
        r.clone_url.clone(),
        r.name.clone(),
    );
    widgets::button(SharedString::from(format!("clone-{}", r.slug)), tone, t)
        .px(px(12.))
        .py(px(6.))
        .child(widgets::label(label, t.text_data_sm, true))
        .on_click(move |_ev, _win, cx| {
            let (slug, url, name) = (slug.clone(), url.clone(), name.clone());
            app.update(cx, |this, cx| this.clone_starred(slug, url, name, cx));
        })
}
