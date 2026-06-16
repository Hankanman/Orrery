//! Phase 0 native spike — render Orrery's repo grid in GPUI (Zed's GPU-native
//! toolkit) against the REAL SQLite cache, to measure CPU on the NVIDIA path
//! vs. the WebKitGTK/Tauri build.
//!
//! It reuses the core verbatim: `model.rs` and `cache.rs` are pulled in by
//! `#[path]` so there's zero drift from the shipping cache format. The only
//! entry point we touch is `cache::load_repos()`, which reads
//! `~/.local/share/orrery/cache.sqlite`.
//!
//! This is throwaway code on the `spike/native-gpui` branch. It is NOT wired
//! into the Tauri app and intentionally duplicates a little styling rather than
//! sharing the CSS design system (which can't cross the native boundary).

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/model.rs"]
mod model;

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/cache.rs"]
mod cache;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, px, rgb, size, App, AppContext, Application, Bounds, Context, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, WindowBounds, WindowOptions,
};

// Dark tokens approximating the `--orr-*` design system so the spike looks like
// Orrery rather than a debug window.
const BG: u32 = 0x0d1117; // page
const CARD: u32 = 0x161b22; // card surface
const BORDER: u32 = 0x2a313c; // elevation border
const TEXT: u32 = 0xe6edf3; // primary text
const MUTED: u32 = 0x8b949e; // secondary text
const ACCENT: u32 = 0x58a6ff; // links / accent
const GREEN: u32 = 0x3fb950; // clean / ahead
const YELLOW: u32 = 0xd29922; // dirty
const RED: u32 = 0xf85149; // behind
const STAR: u32 = 0xe3b341; // stars

const COLS: usize = 4;

/// Flattened, render-ready view of a `model::Repo` — everything the card needs,
/// nothing it doesn't.
struct Row {
    name: SharedString,
    slug: SharedString,
    language: SharedString,
    branch: SharedString,
    age: SharedString,
    dirty: u32,
    ahead: u32,
    behind: u32,
    stars: u32,
    favorite: bool,
}

fn rel_age(last_commit_unix: i64, now: i64) -> String {
    if last_commit_unix <= 0 {
        return "—".into();
    }
    let secs = (now - last_commit_unix).max(0);
    let days = secs / 86_400;
    if days >= 365 {
        format!("{}y", days / 365)
    } else if days >= 1 {
        format!("{days}d")
    } else {
        let hours = secs / 3_600;
        if hours >= 1 {
            format!("{hours}h")
        } else {
            format!("{}m", (secs / 60).max(1))
        }
    }
}

fn to_rows(repos: Vec<model::Repo>, now: i64) -> Vec<Row> {
    repos
        .into_iter()
        .map(|r| Row {
            name: r.display_name.into(),
            slug: r.slug.unwrap_or_default().into(),
            language: r.language.unwrap_or_default().into(),
            branch: r.git.branch.into(),
            age: rel_age(r.last_commit_unix, now).into(),
            dirty: r.git.dirty,
            ahead: r.git.ahead,
            behind: r.git.behind,
            stars: r.stars,
            favorite: r.favorite,
        })
        .collect()
}

/// A small rounded status chip, e.g. "●3" dirty or "↑2" ahead.
fn chip(label: String, color: u32) -> impl IntoElement {
    div()
        .px_1p5()
        .py_0p5()
        .rounded_md()
        .bg(rgb(0x0d1117))
        .border_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .text_color(rgb(color))
        .child(SharedString::from(label))
}

fn card(row: &Row) -> impl IntoElement {
    // Meta chips, conditionally present.
    let mut chips: Vec<gpui::AnyElement> = Vec::new();
    if !row.branch.is_empty() {
        chips.push(chip(format!("⎇ {}", row.branch), MUTED).into_any_element());
    }
    if row.dirty > 0 {
        chips.push(chip(format!("● {}", row.dirty), YELLOW).into_any_element());
    } else {
        chips.push(chip("✓".into(), GREEN).into_any_element());
    }
    if row.ahead > 0 {
        chips.push(chip(format!("↑{}", row.ahead), GREEN).into_any_element());
    }
    if row.behind > 0 {
        chips.push(chip(format!("↓{}", row.behind), RED).into_any_element());
    }
    if row.stars > 0 {
        chips.push(chip(format!("★ {}", row.stars), STAR).into_any_element());
    }

    let title = if row.favorite {
        format!("★ {}", row.name)
    } else {
        row.name.to_string()
    };

    div()
        .flex()
        .flex_1()
        .flex_col()
        .gap_2()
        .p_3()
        .min_w(px(0.))
        .h(px(132.))
        .bg(rgb(CARD))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded_lg()
        .child(
            div()
                .text_color(rgb(TEXT))
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_color(rgb(ACCENT))
                .text_xs()
                .child(row.slug.clone()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1p5()
                .children(chips),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .mt_auto()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(row.language.clone())
                .child(SharedString::from(format!("{} ago", row.age))),
        )
}

struct RepoGrid {
    rows: Rc<Vec<Row>>,
}

impl Render for RepoGrid {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let total = rows.len();
        let grid_rows = total.div_ceil(COLS);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .child(
                div()
                    .text_color(rgb(TEXT))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Orrery · GPUI native spike"),
            )
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .text_sm()
                    .child(SharedString::from(format!("{total} repos · GPU-rendered"))),
            );

        let list = gpui::uniform_list("repo-grid", grid_rows, move |range, _window, _cx| {
            range
                .map(|grid_ix| {
                    let start = grid_ix * COLS;
                    let end = (start + COLS).min(rows.len());
                    let mut cards: Vec<gpui::AnyElement> = (start..end)
                        .map(|i| card(&rows[i]).into_any_element())
                        .collect();
                    // Pad the last row so cards keep their column width.
                    while cards.len() < COLS {
                        cards.push(div().flex_1().min_w(px(0.)).into_any_element());
                    }
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .children(cards)
                        .into_any_element()
                })
                .collect()
        })
        .flex_1();

        let body: gpui::AnyElement = if total == 0 {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("No cached repos. Run the Tauri app once to populate ~/.local/share/orrery/cache.sqlite, then relaunch.")
                .into_any_element()
        } else {
            list.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
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

    let repos = cache::load_repos();
    let count = repos.len();
    let rows = Rc::new(to_rows(repos, now));
    eprintln!("[spike] loaded {count} repos from cache; opening GPUI window");

    // v1.6.3 split platform construction into the gpui_platform crate.
    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(860.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| RepoGrid { rows }),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
