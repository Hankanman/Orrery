//! Native Orrery (rewrite) — the desktop app in GPUI against the real SQLite
//! cache. The Rust core is reused verbatim: `model.rs` and `cache.rs` are pulled
//! in by `#[path]` (so `cache::load_repos()` reads the shipping
//! `~/.local/share/orrery/cache.sqlite` with zero drift). No webview, no IPC.
//!
//! Phase 1: real `--orr-*` theme + faithful RepoCard. Phase 2: the app shell —
//! header + sidebar nav + view switching (`shell.rs`). Branch: spike/native-gpui.

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/model.rs"]
mod model;

#[allow(dead_code, clippy::all)]
#[path = "../../src-tauri/src/cache.rs"]
mod cache;

mod card;
mod data;
mod shell;
mod theme;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions,
};

use shell::{OrreryApp, View};
use theme::Theme;

fn main() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let (rows, roots) = data::load(now);
    eprintln!("[native] loaded {} repos across {} roots", rows.len(), roots);
    let rows = Rc::new(rows);
    let theme = Rc::new(Theme::dark());

    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| OrreryApp {
                    view: View::Grid,
                    rows,
                    roots,
                    theme,
                })
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
