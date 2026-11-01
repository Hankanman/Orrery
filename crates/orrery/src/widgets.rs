//! Shared UI primitives, built on gpui-component and styled from the
//! `--orr-*` tokens (`theme.rs`). One home for the bordered button, the pill
//! tag, the icon + label status segment, the muted note and the section
//! header, which used to be re-implemented per view.
//!
//! Colours that vary by interaction state (a button's hover and press) come
//! from gpui-component's theme, which [`crate::theme::apply_gpui_component_theme`]
//! maps onto our tokens once at startup. The per-call-site styling here only
//! sets the resting state and the geometry.

use gpui::{Div, ElementId, FontWeight, ParentElement, SharedString, Styled, div, px, rgb};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::tag::Tag;
use gpui_component::{Disableable as _, Selectable as _};

use crate::icon::lucide;
use crate::theme::Theme;

pub const MONO: &str = "monospace";

/// What a [`button`] is for, which picks its resting colours and hover.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The plain bordered button.
    Neutral,
    /// A toggled-on toolbar button (accent wash + accent border).
    Active,
    /// A destructive action: neutral at rest, danger-tinted on hover.
    Danger,
    /// A destructive action waiting for its confirming second click.
    Armed,
    /// Inert and muted.
    Disabled,
}

/// The app's bordered button: a gpui-component [`Button`] with the resting
/// state on our tokens. Callers add the geometry (`px`/`py`, width) and the
/// content via `.child(...)`; see [`label`] for the usual text content.
pub fn button(id: impl Into<ElementId>, tone: Tone, t: &Theme) -> Button {
    let b = Button::new(id)
        .rounded(px(t.r_sm))
        .h_auto()
        .border_1()
        .bg(rgb(t.button_bg))
        .border_color(rgb(t.border));
    match tone {
        Tone::Neutral => b.cursor_pointer().text_color(rgb(t.fg1)),
        Tone::Active => b
            .selected(true)
            .cursor_pointer()
            .bg(rgb(t.accent_wash))
            .border_color(rgb(t.border_accent))
            .text_color(rgb(t.accent_bright)),
        Tone::Danger => b.danger().cursor_pointer().text_color(rgb(t.fg1)),
        Tone::Armed => b
            .danger()
            .cursor_pointer()
            .border_color(rgb(t.behind))
            .text_color(rgb(t.behind)),
        Tone::Disabled => b.disabled(true).text_color(rgb(t.fg3)),
    }
}

/// Button text content: `text` at `size`, in the monospace face when `mono`.
/// It inherits the button's colour, so the hover state recolours it.
pub fn label(text: impl Into<SharedString>, size: f32, mono: bool) -> Div {
    let d = div().text_size(px(size)).child(text.into());
    if mono { d.font_family(MONO) } else { d }
}

/// A small bordered pill tag in `color` (draft / merged / pid …): a
/// gpui-component [`Tag`] at our size and radius.
pub fn tag(text: &str, color: u32, t: &Theme) -> Tag {
    Tag::custom(rgb(t.page).into(), rgb(color).into(), rgb(t.border).into())
        .outline()
        .rounded(px(t.r_xs))
        .px(px(5.))
        .py(px(1.))
        .line_height(gpui::phi())
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .child(SharedString::from(text.to_string()))
}

/// An inline status segment: a lucide icon + label, both in `color`.
pub fn seg(icon: &str, label: impl Into<SharedString>, icon_size: f32, color: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide(icon, icon_size, color))
        .child(label.into())
}

/// A muted note line (loading / empty / error states), in the faint text
/// colour at the small text size. Callers narrow the size or pad it as
/// their surface needs.
pub fn note(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .text_size(px(t.text_small))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

/// The look of a [`section_header`].
#[derive(Clone, Copy)]
pub enum Header<'a> {
    /// A view's group header: lucide icon + medium-weight title + muted
    /// count (Inbox groups).
    Group { icon: &'a str, count: usize },
    /// The drawer's uppercase monospace label, with an optional "· n" count.
    Caps { count: Option<usize> },
    /// The sidebar's uppercase semibold label.
    Rail,
}

/// A section header: one component for the view groups, the drawer sections
/// and the sidebar panels, each keeping its own look via [`Header`].
pub fn section_header(title: &str, kind: Header, t: &Theme) -> Div {
    match kind {
        Header::Group { icon, count } => div()
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
            .child(
                div()
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child(SharedString::from(count.to_string())),
            ),
        Header::Caps { count } => {
            let head = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(title.to_uppercase()));
            match count {
                Some(n) => head.child(SharedString::from(format!("· {n}"))),
                None => head,
            }
        }
        Header::Rail => div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(t.text_data_sm))
            .text_color(rgb(t.fg3))
            .child(SharedString::from(title.to_string())),
    }
}
