//! Native Orrery (rewrite, Phase 1) — the repo grid in GPUI against the real
//! SQLite cache. The Rust core is reused verbatim: `model.rs` and `cache.rs`
//! are pulled in by `#[path]` (so `cache::load_repos()` reads the shipping
//! `~/.local/share/orrery/cache.sqlite` with zero drift). No webview, no IPC.
//!
//! Phase 1 brings the real `--orr-*` design system (`theme.rs`) and a faithful
//! `RepoCard` port (`card.rs`). Branch: `spike/native-gpui`.

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/model.rs"]
mod model;

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/cache.rs"]
mod cache;

mod card;
mod data;
mod theme;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, px, rgb, size, App, AppContext, Application, Bounds, Context, FontWeight, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, WindowBounds, WindowOptions,
};

use card::card;
use data::Row;
use theme::Theme;

const COLS: usize = 4;
const ROW_H: f32 = 232.; // grid row height (card stretches to fill)

struct RepoGrid {
    rows: Rc<Vec<Row>>,
    theme: Rc<Theme>,
}

impl Render for RepoGrid {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let t = self.theme.clone();
        let total = rows.len();
        let grid_rows = total.div_ceil(COLS);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(16.))
            .py(px(13.))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.page))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(15.))
                    .text_color(rgb(t.fg0))
                    .child("Orrery"),
            )
            .child(
                div()
                    .font_family("monospace")
                    .text_size(px(12.))
                    .text_color(rgb(t.fg2))
                    .child(SharedString::from(format!("{total} repos · native"))),
            );

        let list = gpui::uniform_list("repo-grid", grid_rows, move |range, _window, _cx| {
            range
                .map(|gi| {
                    let start = gi * COLS;
                    let end = (start + COLS).min(rows.len());
                    let mut cells: Vec<gpui::AnyElement> =
                        (start..end).map(|i| card(&rows[i], &t).into_any_element()).collect();
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
        .flex_1();

        let theme = self.theme.clone();
        let body: gpui::AnyElement = if total == 0 {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(theme.fg2))
                .child("No cached repos. Run the Tauri app once to populate the cache, then relaunch.")
                .into_any_element()
        } else {
            list.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.page))
            .text_color(rgb(theme.fg1))
            .font_family("sans-serif")
            .child(header)
            .child(body)
    }
}

fn main() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let rows = Rc::new(data::load(now));
    let theme = Rc::new(Theme::dark());
    eprintln!("[native] loaded {} repos from cache", rows.len());

    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(860.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| RepoGrid { rows, theme }),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
