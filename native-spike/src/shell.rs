//! App shell — the chrome that wraps every view: the 52px header (brand,
//! roots·repos, search, new/rescan), the 236px left rail with the 8 primary nav
//! items, and the main column. Ported from `AppShell.tsx` + `Sidebar.tsx` +
//! the `.orr-header`/`.orr-sidebar`/`.orr-sb-*` rules in `index.css`.
//!
//! The nav is live: clicking an item switches the active `View`. Most views are
//! Phase-2 scaffolds; Grid renders the real card grid.

use std::rc::Rc;

use gpui::{
    div, px, rgb, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::card::card;
use crate::data::Row;
use crate::theme::Theme;

const COLS: usize = 4;
const ROW_H: f32 = 232.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Grid,
    Inbox,
    Feed,
    Explore,
    Agents,
    Tools,
    Janitor,
    Settings,
}

/// (view, glyph, label) — labels match the real sidebar (route ≠ label).
/// Glyphs are unicode stand-ins; real lucide icons land with the icon pass.
const NAV: [(View, &str, &str); 8] = [
    (View::Grid, "▦", "Mission Control"),
    (View::Inbox, "✉", "Inbox"),
    (View::Feed, "≋", "Feed"),
    (View::Explore, "◎", "Explore"),
    (View::Agents, "❯", "Agents"),
    (View::Tools, "⚒", "Dev Tools"),
    (View::Janitor, "✄", "Cleanup"),
    (View::Settings, "⚙", "Settings"),
];

pub struct OrreryApp {
    pub view: View,
    pub rows: Rc<Vec<Row>>,
    pub roots: usize,
    pub theme: Rc<Theme>,
}

impl OrreryApp {
    fn header(&self, t: &Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.))
            .h(px(52.))
            .px(px(16.))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.page))
            // brand
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(9.))
                    .child(div().text_size(px(18.)).text_color(rgb(t.primary)).child("⊚"))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(15.))
                            .text_color(rgb(t.fg0))
                            .child("Orrery"),
                    ),
            )
            // roots · repos
            .child(
                div()
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg2))
                    .child(SharedString::from(format!(
                        "⊙ {} roots · {} repos",
                        self.roots,
                        self.rows.len()
                    ))),
            )
            // spacer (ml-auto)
            .child(div().flex_1())
            // search box
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(9.))
                    .w(px(380.))
                    .px(px(11.))
                    .py(px(7.))
                    .rounded(px(t.r_sm))
                    .bg(rgb(t.button_bg))
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_color(rgb(t.fg2))
                    .child(div().child("⌕"))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(t.text_small))
                            .child("Search repos, run a command…"),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .rounded(px(t.r_xs))
                            .border_1()
                            .border_color(rgb(t.border))
                            .font_family("monospace")
                            .text_size(px(t.text_data_sm))
                            .child("⌘K"),
                    ),
            )
            .child(icon_btn("＋", t))
            .child(icon_btn("⟳", t))
    }

    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().flex().flex_col().gap(px(4.));
        for (view, glyph, label) in NAV {
            let active = self.view == view;
            let fg = if active { t.accent_bright } else { t.fg1 };
            let mut item = div()
                .id(label)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .px(px(9.))
                .py(px(7.))
                .rounded(px(t.r_sm))
                .text_size(px(t.text_small))
                .text_color(rgb(fg))
                .hover(|s| s.bg(rgb(t.surface_hover)))
                .on_click(cx.listener(move |this, _ev, _win, cx| {
                    this.view = view;
                    cx.notify();
                }))
                .child(div().w(px(16.)).child(SharedString::from(glyph.to_string())))
                .child(SharedString::from(label.to_string()));
            if active {
                item = item.bg(rgb(t.accent_wash));
            }
            nav = nav.child(item);
        }

        div()
            .flex()
            .flex_col()
            .w(px(236.))
            .h_full()
            .px(px(12.))
            .py(px(16.))
            .gap(px(16.))
            .border_r_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.page))
            .child(nav)
            // footer pushed to bottom
            .child(
                div()
                    .mt_auto()
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child("▭ Scanned just now"),
            )
    }

    fn main_view(&self, t: &Theme) -> gpui::AnyElement {
        match self.view {
            View::Grid => self.grid(t).into_any_element(),
            other => placeholder(other, t).into_any_element(),
        }
    }

    fn grid(&self, t: &Theme) -> impl IntoElement {
        let rows = self.rows.clone();
        let theme = self.theme.clone();
        let total = rows.len();
        let grid_rows = total.div_ceil(COLS);

        gpui::uniform_list("repo-grid", grid_rows, move |range, _win, _cx| {
            range
                .map(|gi| {
                    let start = gi * COLS;
                    let end = (start + COLS).min(rows.len());
                    let mut cells: Vec<gpui::AnyElement> = (start..end)
                        .map(|i| card(&rows[i], &theme).into_any_element())
                        .collect();
                    while cells.len() < COLS {
                        cells.push(div().flex_1().min_w(px(0.)).into_any_element());
                    }
                    div()
                        .flex()
                        .flex_row()
                        .items_stretch()
                        .h(px(ROW_H))
                        .gap(px(12.))
                        .px(px(16.))
                        .py(px(8.))
                        .children(cells)
                        .into_any_element()
                })
                .collect()
        })
        .flex_1()
        .size_full()
        .bg(rgb(t.page))
    }
}

impl Render for OrreryApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.page))
            .text_color(rgb(t.fg1))
            .font_family("sans-serif")
            .child(self.header(&t))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar(&t, cx))
                    .child(div().flex().flex_1().min_w(px(0.)).child(self.main_view(&t))),
            )
    }
}

fn icon_btn(glyph: &str, t: &Theme) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("icon-{glyph}")))
        .flex()
        .items_center()
        .justify_center()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(t.r_sm))
        .text_color(rgb(t.fg1))
        .hover(|s| s.bg(rgb(t.surface_hover)))
        .child(SharedString::from(glyph.to_string()))
}

/// Scaffold for a not-yet-ported view: centered title + note.
fn placeholder(view: View, t: &Theme) -> impl IntoElement {
    let (title, sub): (&str, &str) = match view {
        View::Inbox => ("Inbox", "Review queue — PRs & notifications awaiting you"),
        View::Feed => ("Feed", "Activity stream across your repos"),
        View::Explore => ("Explore", "Discover & search across hosts"),
        View::Agents => ("Agents", "Running terminal coding-agent sessions"),
        View::Tools => ("Dev Tools", "Utilities & quick actions"),
        View::Janitor => ("Cleanup", "Prunable branches & worktrees"),
        View::Settings => ("Settings", "Roots, AI, launchers, appearance"),
        View::Grid => ("Mission Control", ""),
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap(px(8.))
        .bg(rgb(t.page))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(22.))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg2))
                .child(SharedString::from(sub.to_string())),
        )
        .child(
            div()
                .mt(px(6.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(t.r_xs))
                .border_1()
                .border_color(rgb(t.border))
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child("Phase 2 scaffold"),
        )
}
