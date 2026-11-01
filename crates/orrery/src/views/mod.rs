//! The non-grid views rendered in the shell's main column (Inbox, Feed, Explore,
//! Agents, Dev Tools, Cleanup, Settings). Each owns its render + any async state,
//! loaded lazily when its nav item is selected. Shared chrome lives here.

use std::cell::Cell;

use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    px, rgb,
};

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;
use crate::widgets::{self, Tone};

pub mod agents;
pub mod cleanup;
pub mod devtools;
pub mod explore;
pub mod feed;
pub mod inbox;
pub mod newproject;
pub mod settings;

pub use crate::widgets::{note, tag};

/// The padding around a view's body.
const BODY_PAD: f32 = 20.;

/// A small bordered text button that runs `on` (no app-state capture needed).
pub fn button(label: &str, t: &Theme, on: impl Fn(&mut gpui::App) + 'static) -> impl IntoElement {
    widgets::button(
        SharedString::from(format!("vbtn-{label}")),
        Tone::Neutral,
        t,
    )
    .px(px(12.))
    .py(px(6.))
    .child(widgets::label(label.to_string(), t.text_data_sm, false))
    .on_click(move |_ev, _win, cx| on(cx))
}

/// A view's chrome: a 52px header (title + refresh) over a scrolling body.
pub fn frame(
    title: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    refresh: fn(&mut OrreryApp, &mut Context<OrreryApp>),
    scroll_id: &'static str,
    body: impl IntoElement,
) -> impl IntoElement {
    chrome(title, t, app, refresh).child(
        div()
            .id(scroll_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p(px(BODY_PAD))
            .child(body),
    )
}

/// A view's chrome over a virtualised body: `body` is either a [`VList`]'s
/// list (which scrolls itself; its rows carry the body padding, see
/// [`VList::pad`]) or a padded note for the loading / empty states.
pub fn list_frame(
    title: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    refresh: fn(&mut OrreryApp, &mut Context<OrreryApp>),
    body: ListBody,
) -> impl IntoElement {
    let body = match body {
        ListBody::List(list) => list,
        ListBody::Note(note) => div().p(px(BODY_PAD)).child(note).into_any_element(),
    };
    chrome(title, t, app, refresh).child(div().flex().flex_col().flex_1().min_h(px(0.)).child(body))
}

/// What a [`list_frame`] shows.
pub enum ListBody {
    List(AnyElement),
    Note(AnyElement),
}

/// The 52px header (title + refresh) over an empty body column.
fn chrome(
    title: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    refresh: fn(&mut OrreryApp, &mut Context<OrreryApp>),
) -> gpui::Div {
    let app = app.clone();
    div().flex().flex_col().size_full().bg(rgb(t.page)).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .h(px(52.))
            .px(px(BODY_PAD))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(t.text_h3))
                    .text_color(rgb(t.fg0))
                    .child(SharedString::from(title.to_string())),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("view-refresh")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded(px(t.r_sm))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.surface_hover)))
                    .child(lucide("refresh-cw", 16., t.fg1))
                    .on_click(move |_ev, _win, cx| {
                        app.update(cx, refresh);
                    }),
            ),
    )
}

/// The virtualised views' list states (see [`VList`]).
#[derive(Default)]
pub struct ViewLists {
    pub inbox: VList,
    pub feed: VList,
    pub explore: VList,
}

/// A virtualised view body: a variable-height `gpui::list` whose
/// [`ListState`] lives on the app, so the scroll position survives
/// re-renders. Only the rows in view are built each frame.
pub struct VList {
    pub state: ListState,
    /// Identifies the rows the list was last measured for (see [`Self::sync`]).
    key: Cell<Option<u64>>,
}

impl Default for VList {
    fn default() -> Self {
        VList {
            state: ListState::new(0, ListAlignment::Top, px(400.)),
            key: Cell::new(None),
        }
    }
}

impl VList {
    /// Point the list at `count` rows, identified by `key` (a hash of what
    /// shapes them: the data, the filter, per-row state that changes a row's
    /// height). The list keeps measured row heights, so when the key changes
    /// it re-measures, holding the scroll position where it can.
    pub fn sync(&self, count: usize, key: u64) {
        if self.key.get() == Some(key) && self.state.item_count() == count {
            return;
        }
        let top = self.state.logical_scroll_top();
        self.state.reset(count);
        if count > 0 {
            self.state.scroll_to(ListOffset {
                item_ix: top.item_ix.min(count - 1),
                offset_in_item: if top.item_ix < count {
                    top.offset_in_item
                } else {
                    px(0.)
                },
            });
        }
        self.key.set(Some(key));
    }

    /// Wrap row `ix` of `count` in the view body's padding: the side padding
    /// on every row, plus the top padding on the first and the bottom on the
    /// last, so the list reads like the padded column it replaces.
    pub fn pad(ix: usize, count: usize, row: impl IntoElement) -> AnyElement {
        let mut d = div().px(px(BODY_PAD));
        if ix == 0 {
            d = d.pt(px(BODY_PAD));
        }
        if ix + 1 == count {
            d = d.pb(px(BODY_PAD));
        }
        d.child(row).into_any_element()
    }
}

/// A hash of `parts`, for [`VList::sync`] keys.
pub fn list_key(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(parts)
}

/// Muted monospace text (ages, counts, paths).
pub fn muted_mono(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    div()
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_key_tracks_its_parts() {
        assert_eq!(
            list_key((1u64, Some("rust"))),
            list_key((1u64, Some("rust")))
        );
        assert_ne!(
            list_key((1u64, Some("rust"))),
            list_key((2u64, Some("rust")))
        );
        assert_ne!(
            list_key((1u64, Some("rust"))),
            list_key((1u64, None::<&str>))
        );
    }
}
