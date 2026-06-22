//! Command palette (Ctrl+K) — a centered overlay with a query field over a
//! filtered list of actions + repositories. Arrow keys move the selection, Enter
//! runs it, Esc closes. Code/semantic search land on top of this in a follow-up.
//!
//! State lives in `Overlay::Palette(PaletteData)`; the action handlers + executor
//! are methods on `OrreryApp` (shell.rs). This module owns the item model + the
//! rendering.

use gpui::{
    div, px, rgb, rgba, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription,
};

use crate::data::Row;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::text_input::TextInput;
use crate::theme::Theme;

const PANEL_W: f32 = 640.;
/// Cap on repo rows shown, to keep the list quick.
const MAX_REPOS: usize = 40;

/// Live palette state.
pub struct PaletteData {
    pub query: Entity<TextInput>,
    pub selected: usize,
    /// Keeps the query-observation alive (re-renders the app on each keystroke).
    pub _sub: Subscription,
}

/// A standing command (not tied to a repo).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Rescan,
    Settings,
}

impl PaletteAction {
    fn label(self) -> &'static str {
        match self {
            PaletteAction::Rescan => "Rescan repositories",
            PaletteAction::Settings => "Open settings",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            PaletteAction::Rescan => "refresh-cw",
            PaletteAction::Settings => "settings",
        }
    }
}

/// One palette result.
#[derive(Clone)]
pub enum PaletteItem {
    Action(PaletteAction),
    /// Index into `OrreryApp::rows`.
    Repo(usize),
}

const ACTIONS: [PaletteAction; 2] = [PaletteAction::Rescan, PaletteAction::Settings];

/// Build the filtered result list for `query` (actions first, then repos). Must
/// be deterministic — the executor rebuilds it to resolve the selected index.
pub fn items(rows: &[Row], query: &str) -> Vec<PaletteItem> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();

    for a in ACTIONS {
        if q.is_empty() || a.label().to_lowercase().contains(&q) {
            out.push(PaletteItem::Action(a));
        }
    }

    for (i, r) in rows.iter().enumerate() {
        let hit = q.is_empty()
            || r.name.to_lowercase().contains(&q)
            || r.slug.to_lowercase().contains(&q)
            || r.path.to_lowercase().contains(&q);
        if hit {
            out.push(PaletteItem::Repo(i));
            if out.len() >= MAX_REPOS {
                break;
            }
        }
    }
    out
}

/// Render the palette overlay.
pub fn render(
    data: &PaletteData,
    items: &[PaletteItem],
    rows: &[Row],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let selected = data.selected.min(items.len().saturating_sub(1));

    let mut list = div().flex().flex_col().gap(px(1.)).p(px(6.));
    if items.is_empty() {
        list = list.child(
            div()
                .p(px(12.))
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg3))
                .child("No matches."),
        );
    }
    for (i, item) in items.iter().enumerate() {
        list = list.child(row_view(item, i, i == selected, rows, t, app));
    }

    let panel = div()
        .flex()
        .flex_col()
        .w(px(PANEL_W))
        .max_h(px(520.))
        .rounded(px(t.r_md))
        .bg(rgb(t.page))
        .border_1()
        .border_color(rgb(t.border_strong))
        // Query field.
        .child(
            div()
                .p(px(8.))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(data.query.clone()),
        )
        // Results.
        .child(
            div()
                .id("palette-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(list),
        );

    // Backdrop + top-centered panel.
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .occlude()
        .flex()
        .flex_col()
        .items_center()
        .bg(rgba(0x00000066))
        .child(div().h(px(80.)))
        .child(panel)
}

fn row_view(
    item: &PaletteItem,
    idx: usize,
    selected: bool,
    rows: &[Row],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let (icon, primary, secondary) = match item {
        PaletteItem::Action(a) => (
            a.icon(),
            SharedString::from(a.label()),
            SharedString::default(),
        ),
        PaletteItem::Repo(i) => {
            let r = &rows[*i];
            ("box", r.name.clone(), r.slug.clone())
        }
    };

    let item = item.clone();
    let app = app.clone();
    let mut row = div()
        .id(SharedString::from(format!("pal-{idx}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(t.r_sm))
        .cursor_pointer()
        .text_color(rgb(t.fg1))
        .child(lucide(
            icon,
            15.,
            if selected { t.accent_bright } else { t.fg2 },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(t.text_small))
                .child(primary),
        )
        .on_click(move |_ev, window, cx| {
            let fh = app.read(cx).focus.clone();
            let item = item.clone();
            app.update(cx, |this, cx| this.run_palette_item(item, cx));
            window.focus(&fh, cx);
        });
    if !secondary.is_empty() {
        row = row.child(
            div()
                .max_w(px(280.))
                .truncate()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(secondary),
        );
    }
    if selected {
        row = row.bg(rgb(t.accent_wash));
    }
    row
}
