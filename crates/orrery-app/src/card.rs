//! Faithful GPUI port of `RepoCard` (grid view) from
//! `src/components/RepoCard.tsx` + the `.orr-card*` rules in `src/index.css`.
//! Layout, spacing, token colors and real lucide/devicon/host icons match, and
//! the launchers + favorite toggle are live.
//!
//! Cards render inside `uniform_list` (a `'static` closure), so every stored
//! handler/hover closure captures owned values — never a borrow of `&Theme`.

use gpui::{
    App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::{cache, launch};

use crate::data::Row;
use crate::icon::{brand, langicon, lucide};
use crate::shell::OrreryApp;
use crate::theme::{Theme, devicon_stem, lang_color};

const MONO: &str = "monospace";

/// The language mark: the multicolor devicon when one is bundled, else the
/// brand-color dot (no devicon for this language).
fn lang_mark(language: &str, t: &Theme) -> gpui::AnyElement {
    if let Some(stem) = devicon_stem(language)
        && crate::assets::has_icon(&format!("devicon/{stem}.svg"))
    {
        return langicon(stem, 16.).into_any_element();
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

/// A clickable launcher button. `wide` ones flex to fill (IDE/Agent); narrow
/// ones are fixed 38px icon slots (Folder/Host). `on` fires on click.
fn button(
    id: SharedString,
    content: impl IntoElement,
    wide: bool,
    t: &Theme,
    on: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let (hov_border, hov_fg) = (t.border_strong, t.fg0);
    let b = div()
        .id(id)
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
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov_border)).text_color(rgb(hov_fg)))
        .on_click(move |_ev, _win, cx| on(cx))
        .child(content);
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(38.))
    }
}

pub fn card(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
) -> impl IntoElement {
    // ── head: language mark + name, and the (clickable) favorite star ──────
    let fav_star = {
        let app = app.clone();
        let id = row.id.clone();
        let fav = row.favorite;
        div()
            .id(SharedString::from(format!("fav-{idx}")))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(lucide("star", 16., if fav { t.star } else { t.fg3 }))
            .on_click(move |_ev, _win, cx| {
                // Don't let the star toggle also open the drawer.
                cx.stop_propagation();
                let next = !fav;
                let _ = cache::set_favorite(&id, next);
                app.update(cx, |this, cx| {
                    if let Some(r) = this.rows.get_mut(idx) {
                        r.favorite = next;
                    }
                    cx.notify();
                });
            })
    };

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
        .child(fav_star);

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

    // ── host row: private · stars · release · age · host brand ───────────
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
        host = host
            .child(div().flex_1())
            .child(brand(&row.host, 14., t.fg2));
    }

    // ── launchers (live) ─────────────────────────────────────────────────
    let id_ide = SharedString::from(format!("ide-{idx}"));
    let id_agent = SharedString::from(format!("agent-{idx}"));
    let id_folder = SharedString::from(format!("folder-{idx}"));
    let id_host = SharedString::from(format!("host-{idx}"));

    let ide_action = {
        let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::launch(&cmd, &path);
        }
    };
    let agent_action = {
        let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::spawn(&cmd, &path);
        }
    };
    let folder_action = {
        let path = row.id.clone();
        move |_cx: &mut App| {
            let _ = launch::open(&path);
        }
    };

    let mut acts = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .mt(px(14.))
        .child(button(
            id_ide,
            SharedString::from("Open in IDE"),
            true,
            t,
            ide_action,
        ))
        .child(button(
            id_agent,
            SharedString::from("Agent"),
            true,
            t,
            agent_action,
        ))
        .child(button(
            id_folder,
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            folder_action,
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        acts = acts.child(button(
            id_host,
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move |_cx: &mut App| {
                let _ = launch::open(&url);
            },
        ));
    }

    // ── clickable content region → opens the repo drawer ──────────────────
    // Everything except the launcher row opens the drawer on click; the
    // launchers (and the favorite star, which stops propagation) act in place.
    let mut body = {
        let app = app.clone();
        let id = row.id.clone();
        div()
            .id(SharedString::from(format!("open-{idx}")))
            .flex()
            .flex_col()
            .cursor_pointer()
            .on_click(move |_ev, window, cx| {
                let id = id.clone();
                app.update(cx, |this, cx| this.open_drawer(id, window, cx));
            })
            .child(head)
            .child(slug)
            .child(desc)
    };

    // AI summary, when present, sits between description and status.
    if !row.ai_summary.is_empty() {
        body = body.child(
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
    body = body.child(status).child(host);

    // ── card shell (hover lift via border/bg) ─────────────────────────────
    let (hov_border, hov_bg) = (t.border_accent, t.surface_hover);
    div()
        .id(SharedString::from(format!("card-{idx}")))
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
        .hover(move |s| s.border_color(rgb(hov_border)).bg(rgb(hov_bg)))
        .child(body)
        .child(acts)
}
