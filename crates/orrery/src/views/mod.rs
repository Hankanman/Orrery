//! The non-grid views rendered in the shell's main column (Inbox, Feed, Explore,
//! Agents, Dev Tools, Cleanup, Settings). Each owns its render + any async state,
//! loaded lazily when its nav item is selected. Shared chrome lives here.

use gpui::{
    Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;
use crate::widgets::{self, Header, Tone};

pub mod agents;
pub mod cleanup;
pub mod devtools;
pub mod explore;
pub mod feed;
pub mod inbox;
pub mod newproject;
pub mod settings;

pub use crate::widgets::{note, tag};

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
    let app = app.clone();
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(t.page))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .h(px(52.))
                .px(px(20.))
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
        .child(
            div()
                .id(scroll_id)
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(20.))
                .child(body),
        )
}

/// Muted monospace text (ages, counts, paths).
pub fn muted_mono(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    div()
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

/// A section: icon + title + count header over a column of rows.
pub fn section_header(icon: &str, title: &str, count: usize, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::section_header(
            title,
            Header::Group { icon, count },
            t,
        ))
}
