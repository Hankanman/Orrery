//! The grid-view repo card: layout, spacing, token colors and real
//! lucide/devicon/host icons, with live launchers + favorite toggle.
//!
//! Cards render inside `uniform_list` (a `'static` closure), so every stored
//! handler/hover closure captures owned values — never a borrow of `&Theme`.

use gpui::{
    App, ClickEvent, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use gpui_component::checkbox::Checkbox;
use gpui_component::tooltip::Tooltip;
use orrery_core::launch;

use crate::data::Row;
use crate::icon::{brand, langicon, lucide};
use crate::shell::OrreryApp;
use crate::shell::hygiene::{self, AdvisoryBadge, Tier};
use crate::theme::{Theme, devicon_stem, lang_color};
use crate::widgets::{self, Tone};

const MONO: &str = "monospace";

/// Extra card height for the tag-chip line (the grid adds it to its row
/// height while any visible card has tags).
pub const TAGS_LINE_H: f32 = 26.;

/// Most tag chips on a card or list row; the rest show as "+n".
const MAX_CHIPS: usize = 4;

/// A repo's project tags as a single line of chips (clipped, never wrapped,
/// so the card height stays fixed).
fn tag_chips(tags: &[SharedString], t: &Theme) -> gpui::Div {
    let mut line = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .overflow_hidden();
    for tag in tags.iter().take(MAX_CHIPS) {
        line = line.child(widgets::tag(tag, t.accent_bright, t).flex_none());
    }
    if tags.len() > MAX_CHIPS {
        line = line.child(
            div()
                .flex_none()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(format!("+{}", tags.len() - MAX_CHIPS))),
        );
    }
    line
}

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
    /// AI is ready — gates the (cached) AI summary line, per the `aiReady`
    /// contract: with AI off, every AI affordance is hidden.
    pub show_ai: bool,
    /// The security-advisory badge, when the repo has open advisories or
    /// its source can't answer (see `shell::hygiene`).
    pub advisory: Option<AdvisoryBadge>,
}

/// The "N worktrees" affordance: linked worktrees are grouped under their
/// main checkout's card, and this opens the drawer with them listed.
fn worktree_chip(
    id: SharedString,
    row: &Row,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let n = row.worktrees.len();
    let dirty = row.worktrees.iter().filter(|w| w.dirty > 0).count();
    let color = if dirty > 0 { t.dirty } else { t.fg2 };
    let (app, repo) = (app.clone(), row.id.clone());
    let hov = t.border_strong;
    let tip = if dirty > 0 {
        format!(
            "{n} linked worktree{}, {dirty} dirty",
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!("{n} linked worktree{}", if n == 1 { "" } else { "s" })
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(5.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(t.border))
        .text_color(rgb(color))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov)))
        .child(lucide("folder-tree", 12., color))
        .child(SharedString::from(format!(
            "{n} worktree{}",
            if n == 1 { "" } else { "s" }
        )))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_ev, window, cx| {
            // Don't also run the card's open-drawer click.
            cx.stop_propagation();
            let repo = repo.clone();
            app.update(cx, |this, cx| this.open_worktrees(repo, window, cx));
        })
}

/// The security-advisory badge: the highest tier's count ("3 high") in its
/// colour, or a muted shield when the source can't answer. The tooltip (the
/// breakdown, or why it's unavailable) is looked up when shown.
fn advisory_badge(
    id: SharedString,
    row_id: SharedString,
    badge: AdvisoryBadge,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let (label, color) = match badge {
        AdvisoryBadge::Open(c) => {
            let (label, tier) = hygiene::badge_label(&c);
            let color = match tier {
                Tier::Severe => t.behind,
                Tier::Moderate => t.dirty,
                Tier::Mild => t.fg2,
            };
            (Some(label), color)
        }
        AdvisoryBadge::Unavailable => (None, t.fg3),
    };
    let app = app.clone();
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide("shield-alert", 13., color))
        .children(label.map(SharedString::from))
        .tooltip(move |window, cx| {
            let text = app
                .read(cx)
                .hygiene
                .advisory_tooltip(&row_id)
                .unwrap_or_default();
            Tooltip::new(text).build(window, cx)
        })
}

/// The fleet multi-select checkbox (gpui-component [`Checkbox`]): checked
/// when selected; otherwise invisible (keeping its layout slot, so nothing
/// shifts) until the card's hover group is hovered or any selection exists.
/// Clicking toggles the repo in the selection without opening the drawer.
fn select_box(
    id: SharedString,
    group: SharedString,
    row_id: SharedString,
    state: CardState,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let b = Checkbox::new(id)
        .checked(state.selected)
        .tab_stop(false)
        .flex_none()
        .cursor_pointer()
        .on_click(move |_checked, _win, cx| {
            // Selection toggles in place — don't also open the drawer.
            cx.stop_propagation();
            let row_id = row_id.clone();
            app.update(cx, |this, cx| this.toggle_selected(row_id, cx));
        });
    if !state.selected && !state.selecting {
        b.invisible().group_hover(group, |s| s.visible())
    } else {
        b
    }
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
    widgets::seg(icon_name, label, 14., color)
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
    let b = widgets::button(id, Tone::Neutral, t)
        .py(px(8.))
        .px(px(0.))
        .on_click(move |_ev, _win, cx| on(cx))
        .child(
            div()
                .text_size(px(t.text_data_sm))
                .font_family(MONO)
                .child(content),
        );
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(38.))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn card(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
    state: CardState,
    tags: &[SharedString],
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("cardg-{idx}"));
    // ── head: select box + language mark + name, and the favorite star ─────
    let fav_star = {
        let app = app.clone();
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
                app.update(cx, |this, cx| this.set_favorite(idx, !fav, cx));
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
    // One line, never wrapping (the card has a fixed height in the grid's
    // uniform list): the branch name truncates to make room.
    let mut status = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(13.))
        .mt(px(12.))
        .overflow_hidden()
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    // Urgent attention (failing CI / review request) leads the status row.
    if state.urgent {
        status = status.child(urgent_dot(t));
    }
    status = status.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .min_w(px(0.))
            .text_color(rgb(t.fg2))
            .child(lucide("git-branch", 14., t.fg2))
            .child(div().min_w(px(0.)).truncate().child(row.branch.clone())),
    );
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .flex_none()
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
        status = status.child(
            widgets::seg(
                "circle-dot",
                SharedString::from(row.dirty.to_string()),
                14.,
                t.dirty,
            )
            .flex_none(),
        );
    }
    // Fleet hygiene, right-aligned: the worktree group and advisories.
    if !row.worktrees.is_empty() || state.advisory.is_some() {
        status = status.child(div().flex_1());
    }
    if !row.worktrees.is_empty() {
        status = status.child(worktree_chip(
            SharedString::from(format!("wt-{idx}")),
            row,
            t,
            app,
        ));
    }
    if let Some(badge) = state.advisory {
        status = status.child(advisory_badge(
            SharedString::from(format!("adv-{idx}")),
            row.id.clone(),
            badge,
            t,
            app,
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
    host = host.child(seg("clock", row.age(), t.fg2));
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

    // AI summary, when present and AI is ready, sits between description and
    // status (a cached summary stays hidden with AI off — the aiReady contract).
    if state.show_ai && !row.ai_summary.is_empty() {
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
    // Project tags, when the repo has any (the grid grows its rows to fit).
    if !tags.is_empty() {
        body = body.child(tag_chips(tags, t).mt(px(9.)).h(px(TAGS_LINE_H - 9.)));
    }

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
#[allow(clippy::too_many_arguments)]
pub(crate) fn list_item(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
    state: CardState,
    tags: &[SharedString],
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("listg-{idx}"));
    let select = select_box(
        SharedString::from(format!("lsel-{idx}")),
        group.clone(),
        row.id.clone(),
        state,
        app,
    );
    let fav_star = {
        let app = app.clone();
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
                app.update(cx, |this, cx| this.set_favorite(idx, !fav, cx));
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
    if !row.worktrees.is_empty() {
        status = status.child(worktree_chip(
            SharedString::from(format!("lwt-{idx}")),
            row,
            t,
            app,
        ));
    }
    if let Some(badge) = state.advisory {
        status = status.child(advisory_badge(
            SharedString::from(format!("ladv-{idx}")),
            row.id.clone(),
            badge,
            t,
            app,
        ));
    }
    if !row.host.is_empty() {
        status = status.child(seg("star", row.stars.clone(), t.star));
    }
    status = status.child(seg("clock", row.age(), t.fg2));

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
        .children((!tags.is_empty()).then(|| tag_chips(tags, t).flex_shrink(1.).max_w(px(260.))))
        .child(status)
        .child(acts);
    if state.selected {
        shell = shell.bg(rgb(t.accent_wash));
    }
    shell
}
