//! Native Orrery (rewrite) — the desktop GPUI app. All logic comes from the
//! `orrery-core` crate (scan/git/forge/inbox/ai/cache/config); this crate is
//! purely the UI: theme, cards, shell, views. No webview, no IPC. Reading the
//! shipping `~/.local/share/orrery/cache.sqlite` is `orrery_core::cache`.
//!
//! Phase 1: real `--orr-*` theme + faithful RepoCard. Phase 2: the app shell —
//! header + sidebar nav + view switching (`shell.rs`).

mod assets;
mod card;
mod data;
mod icon;
mod shell;
mod theme;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};

use shell::{OrreryApp, View};
use theme::Theme;

fn main() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let (rows, roots) = data::load(now);
    eprintln!(
        "[native] loaded {} repos across {} roots",
        rows.len(),
        roots
    );
    let rows = Rc::new(rows);
    let theme = Rc::new(Theme::dark());

    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform)
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
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
