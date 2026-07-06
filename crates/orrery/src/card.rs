//! The grid-view repo card: layout, spacing, token colors and real
//! lucide/devicon/host icons, with live launchers + favorite toggle.
//!
//! Cards render inside `uniform_list` (a `'static` closure), so every stored
//! handler/hover closure captures owned values — never a borrow of `&Theme`.

use gpui::{
    App, ClickEvent, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use orrery_core::{cache, launch};

use crate::data::Row;
use crate::icon::{brand, langicon, lucide};
use crate::shell::OrreryApp;
use crate::theme::{Theme, devicon_stem, lang_color};

const MONO: &str = "monospace";

/// Per-card interactive state, resolved by the caller inside the
/// `uniform_list` render closure (cheap per-row lookups by id — no allocation).
#[derive(Clone, Copy)]
pub struct CardState {
    /// A live agent session is running in this repo.
    pub active: bool,
    /// An urgent attention item (`orrery_core::attention::Severity::Urgent`).
    pub urgent: bool,
    /// This repo is in the fleet multi-selection.
    pub selected: bool,
    /// Any selection exists — keeps every card's checkbox visible (not just
    /// hover-revealed) while a selection is being built.
    pub selecting: bool,
}

/// The fleet multi-select checkbox: filled with a check when selected;
/// otherwise invisible (keeping its layout slot, so nothing shifts) until the
/// card's hover group is hovered or any selection exists. Clicking toggles the
/// repo in the selection without opening the drawer.
fn select_box(
    id: SharedString,
    group: SharedString,
    row_id: SharedString,
    state: CardState,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let (border_c, bg_c) = if state.selected {
        (t.primary, t.primary)
    } else {
        (t.border_strong, t.button_bg)
    };
    let hov = t.primary;
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(16.))
        .h(px(16.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(border_c))
        .bg(rgb(bg_c))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov)))
        .on_click(move |_ev, _win, cx| {
            // Selection toggles in place — don't also open the drawer.
            cx.stop_propagation();
            let row_id = row_id.clone();
            app.update(cx, |this, cx| this.toggle_selected(row_id, cx));
        });
    if state.selected {
        b = b.child(lucide("check", 12., t.page));
    } else if !state.selecting {
        b = b.invisible().group_hover(group, |s| s.visible());
    }
    b
}

/// Click handler for a card's open-drawer region: Ctrl/Cmd+click toggles the
/// fleet selection, a plain click opens the drawer.
fn open_or_select(
    app: &Entity<OrreryApp>,
    row_id: &SharedString,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let app = app.clone();
    let id = row_id.clone();
    move |ev, window, cx| {
        let id = id.clone();
        if ev.modifiers().secondary() {
            app.update(cx, |this, cx| this.toggle_selected(id, cx));
        } else {
            app.update(cx, |this, cx| this.open_drawer(id, window, cx));
        }
    }
}

/// The language mark: the multicolor devicon when one is bundled, else the
/// brand-color dot (no devicon for this language). Shared with the sidebar's
/// LANGUAGES list.
pub(crate) fn lang_mark(language: &str, t: &Theme) -> gpui::AnyElement {
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

/// The urgent-attention mark: a small flat dot in the danger token, shown
/// with the git status indicators when the repo has an Urgent item.
fn urgent_dot(t: &Theme) -> impl IntoElement {
    div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(t.behind))
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
    state: CardState,
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("cardg-{idx}"));
    // ── head: select box + language mark + name, and the favorite star ─────
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
                .child(select_box(
                    SharedString::from(format!("sel-{idx}")),
                    group.clone(),
                    row.id.clone(),
                    state,
                    t,
                    app,
                ))
                .child(lang_mark(&row.language, t))
                .child(div().min_w(px(0.)).truncate().child(row.name.clone()))
                // Live agent session running in this repo.
                .children(
                    state
                        .active
                        .then(|| lucide("square-terminal", 13., t.clean)),
                ),
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
        .text_size(px(t.text_data_sm));
    // Urgent attention (failing CI / review request) leads the status row.
    if state.urgent {
        status = status.child(urgent_dot(t));
    }
    status = status.child(seg("git-branch", row.branch.clone(), t.fg2));
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
    // Everything except the launcher row opens the drawer on click (Ctrl+click
    // toggles the fleet selection instead); the launchers (and the favorite
    // star / select box, which stop propagation) act in place.
    let mut body = div()
        .id(SharedString::from(format!("open-{idx}")))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(open_or_select(app, &row.id))
        .child(head)
        .child(slug)
        .child(desc);

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

    // ── card shell (hover lift via border/bg; accent border when selected) ─
    let (hov_border, hov_bg) = (t.border_accent, t.surface_hover);
    div()
        .id(SharedString::from(format!("card-{idx}")))
        .group(group)
        .flex()
        .flex_1()
        .flex_col()
        .min_w(px(0.))
        .px(px(15.))
        .py(px(14.))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(if state.selected {
            t.border_accent
        } else {
            t.border
        }))
        .rounded(px(t.r_md))
        .overflow_hidden()
        .hover(move |s| s.border_color(rgb(hov_border)).bg(rgb(hov_bg)))
        .child(body)
        .child(acts)
}

/// A compact single-row repo entry for the list layout — the same data and
/// launchers as the grid card, laid out horizontally in one fixed-height row.
pub(crate) fn list_item(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
    state: CardState,
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("listg-{idx}"));
    let select = select_box(
        SharedString::from(format!("lsel-{idx}")),
        group.clone(),
        row.id.clone(),
        state,
        t,
        app,
    );
    let fav_star = {
        let app = app.clone();
        let id = row.id.clone();
        let fav = row.favorite;
        div()
            .id(SharedString::from(format!("lfav-{idx}")))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(lucide("star", 15., if fav { t.star } else { t.fg3 }))
            .on_click(move |_ev, _win, cx| {
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

    // Name + slug·path; clicking opens the drawer (Ctrl+click selects).
    let open = {
        div()
            .id(SharedString::from(format!("lopen-{idx}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .flex_1()
            .min_w(px(0.))
            .cursor_pointer()
            .on_click(open_or_select(app, &row.id))
            .child(lang_mark(&row.language, t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(t.text_small))
                            .text_color(rgb(t.fg0))
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(t.text_data_sm))
                            .text_color(rgb(t.fg2))
                            .child(SharedString::from(format!("{} · {}", row.slug, row.path))),
                    ),
            )
            // Live agent session running in this repo.
            .children(
                state
                    .active
                    .then(|| lucide("square-terminal", 13., t.clean)),
            )
    };

    // Status segments (branch / ahead-behind / dirty / stars / age).
    let mut status = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .flex_none()
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    // Urgent attention (failing CI / review request) leads the status row.
    if state.urgent {
        status = status.child(urgent_dot(t));
    }
    status = status.child(seg("git-branch", row.branch.clone(), t.fg2));
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
    if !row.host.is_empty() {
        status = status.child(seg("star", row.stars.clone(), t.star));
    }
    status = status.child(seg("clock", row.age.clone(), t.fg2));

    // Launchers — narrow icon buttons.
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
        .gap(px(6.))
        .flex_none()
        .child(button(
            SharedString::from(format!("lide-{idx}")),
            lucide("code", 15., t.fg1),
            false,
            t,
            ide_action,
        ))
        .child(button(
            SharedString::from(format!("lagent-{idx}")),
            lucide("square-terminal", 15., t.fg1),
            false,
            t,
            agent_action,
        ))
        .child(button(
            SharedString::from(format!("lfolder-{idx}")),
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            folder_action,
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        acts = acts.child(button(
            SharedString::from(format!("lhost-{idx}")),
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move |_cx: &mut App| {
                let _ = launch::open(&url);
            },
        ));
    }

    let hov_bg = t.surface_hover;
    let mut shell = div()
        .id(SharedString::from(format!("lrow-{idx}")))
        .group(group)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .w_full()
        .h(px(60.))
        .px(px(16.))
        .border_b_1()
        .border_color(rgb(t.border))
        .hover(move |s| s.bg(rgb(hov_bg)))
        .child(select)
        .child(fav_star)
        .child(open)
        .child(status)
        .child(acts);
    if state.selected {
        shell = shell.bg(rgb(t.accent_wash));
    }
    shell
}
