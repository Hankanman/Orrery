//! Faithful GPUI port of `RepoCard` (grid view) from
//! `src/components/RepoCard.tsx` + the `.orr-card*` rules in `src/index.css`.
//! Layout, spacing, type sizes, token colors and now real lucide/host-brand
//! icons match. The language mark is still a color dot (devicon pass is its own
//! follow-up). Static for now — interactivity is a later phase.

use gpui::{div, px, rgb, FontWeight, IntoElement, ParentElement, SharedString, Styled};

use crate::data::Row;
use crate::icon::{brand, langicon, lucide};
use crate::theme::{devicon_stem, lang_color, Theme};

const MONO: &str = "monospace";

/// The language mark: the multicolor devicon when one is bundled, else the
/// brand-color dot (no devicon for this language).
fn lang_mark(language: &str, t: &Theme) -> gpui::AnyElement {
    if let Some(stem) = devicon_stem(language) {
        if crate::assets::has_icon(&format!("devicon/{stem}.svg")) {
            return langicon(stem, 16.).into_any_element();
        }
    }
    div()
        .w(px(9.))
        .h(px(9.))
        .rounded_full()
        .bg(rgb(lang_color(language, t.fg3)))
        .into_any_element()
}

/// One status segment: a lucide icon + label, both in `color`.
fn seg(icon_name: &str, label: SharedString, color: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide(icon_name, 14., color))
        .child(label)
}

/// A launcher button. `wide` ones flex to fill (IDE/Agent); narrow ones are
/// fixed 38px icon slots (Folder/Host). `content` is text or an icon.
fn button(content: impl IntoElement, wide: bool, t: &Theme) -> impl IntoElement {
    let base = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(8.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .font_family(MONO)
        .child(content);
    if wide {
        base.flex_1().min_w(px(0.))
    } else {
        base.w(px(38.))
    }
}

pub fn card(row: &Row, t: &Theme) -> impl IntoElement {
    // ── head: lang dot + name, and the favorite star ──────────────────────
    let head = div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(9.))
                .min_w(px(0.))
                .text_size(px(t.text_h3))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(t.fg0))
                .child(lang_mark(&row.language, t))
                .child(div().min_w(px(0.)).truncate().child(row.name.clone())),
        )
        .child(lucide(
            "star",
            16.,
            if row.favorite { t.star } else { t.fg3 },
        ));

    // ── slug · path ───────────────────────────────────────────────────────
    let slug = div()
        .mt(px(6.))
        .truncate()
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2))
        .child(SharedString::from(format!("{} · {}", row.slug, row.path)));

    // ── description (2-line clamp ≈ 38px) ────────────────────────────────
    let desc = div()
        .mt(px(9.))
        .h(px(38.))
        .overflow_hidden()
        .text_size(px(t.text_small))
        .line_height(px(19.))
        .text_color(rgb(t.fg1))
        .child(row.description.clone());

    // ── git status row ────────────────────────────────────────────────────
    let mut status = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(13.))
        .mt(px(12.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .child(seg("git-branch", row.branch.clone(), t.fg2));
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .text_color(rgb(color))
                .child(lucide("arrow-up", 13., color))
                .child(SharedString::from(row.ahead.to_string()))
                .child(lucide("arrow-down", 13., color))
                .child(SharedString::from(row.behind.to_string())),
        );
    }
    if row.dirty > 0 {
        status = status.child(seg(
            "circle-dot",
            SharedString::from(row.dirty.to_string()),
            t.dirty,
        ));
    }

    // ── host row: private · stars · release · age ────────────────────────
    let mut host = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .mt(px(9.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2));
    if row.private {
        host = host.child(lucide("lock", 13., t.fg3));
    }
    if !row.host.is_empty() {
        host = host.child(seg("star", row.stars.clone(), t.star));
    }
    if !row.release.is_empty() {
        host = host.child(seg("tag", row.release.clone(), t.fg2));
    }
    host = host.child(seg("clock", row.age.clone(), t.fg2));
    if !row.host.is_empty() {
        // host brand mark, pushed to the right edge
        host = host
            .child(div().flex_1())
            .child(brand(&row.host, 14., t.fg2));
    }

    // ── launchers ─────────────────────────────────────────────────────────
    let acts = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .mt(px(14.))
        .child(button(SharedString::from("Open in IDE"), true, t))
        .child(button(SharedString::from("Agent"), true, t))
        .child(button(lucide("folder-open", 15., t.fg1), false, t))
        .child(button(lucide("external-link", 15., t.fg1), false, t));

    // ── card shell ────────────────────────────────────────────────────────
    let mut shell = div()
        .flex()
        .flex_1()
        .flex_col()
        .min_w(px(0.))
        .px(px(15.))
        .py(px(14.))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(t.r_md))
        .overflow_hidden()
        .child(head)
        .child(slug)
        .child(desc);

    // AI summary, when present, sits between description and status.
    if !row.ai_summary.is_empty() {
        shell = shell.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .mt(px(9.))
                .h(px(17.))
                .overflow_hidden()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.ai))
                .child(lucide("sparkles", 13., t.ai))
                .child(row.ai_summary.clone()),
        );
    }

    shell.child(status).child(host).child(acts)
}
