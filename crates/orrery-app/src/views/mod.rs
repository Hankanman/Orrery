//! The non-grid views rendered in the shell's main column (Inbox, Feed, Explore,
//! Agents, Dev Tools, Cleanup, Settings). Each owns its render + any async state,
//! loaded lazily when its nav item is selected. Shared chrome lives here.

use gpui::{
    div, px, rgb, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

pub mod cleanup;
pub mod explore;
pub mod feed;
pub mod inbox;

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

/// A muted "Loading…" / empty / error note.
pub fn note(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    div()
        .text_size(px(t.text_small))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

/// A small bordered pill tag.
pub fn tag(text: &str, color: u32, t: &Theme) -> impl IntoElement {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(t.border))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(color))
        .child(SharedString::from(text.to_string()))
}

/// Muted monospace text (ages, counts, paths).
pub fn muted_mono(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    div()
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

/// A section header: icon + title + count.
pub fn section_header(icon: &str, title: &str, count: usize, t: &Theme) -> gpui::Div {
    div().flex().flex_col().gap(px(2.)).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .mb(px(6.))
            .text_color(rgb(t.fg2))
            .child(lucide(icon, 15., t.fg2))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(t.text_small))
                    .child(SharedString::from(title.to_string())),
            )
            .child(muted_mono(format!("{count}"), t)),
    )
}
