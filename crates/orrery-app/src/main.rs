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
mod drawer;
mod icon;
mod live;
mod palette;
mod shell;
mod task;
mod text_input;
mod theme;
mod views;

use std::rc::Rc;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, px,
    size,
};
use gpui_component::Root;

use shell::{OrreryApp, View};
use theme::Theme;

actions!(
    orrery,
    [
        CloseOverlay,
        OpenPalette,
        PaletteUp,
        PaletteDown,
        PaletteConfirm
    ]
);

fn main() {
    let now = data::now_unix();
    let (rows, roots) = data::load(now);
    eprintln!(
        "[native] loaded {} repos across {} roots",
        rows.len(),
        roots
    );
    // Borrow the desktop's accent colour (KDE/portal) so the app harmonises
    // with the user's theme — the design system's runtime accent override.
    let accent = orrery_platform::appearance::read_blocking()
        .accent
        .map(|c| (c.r, c.g, c.b));
    if let Some((r, g, b)) = accent {
        eprintln!("[native] system accent #{r:02x}{g:02x}{b:02x}");
    }
    let theme = Rc::new(Theme::dark().with_system_accent(accent));
    let config = orrery_core::config::load();

    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform)
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            // Initialise gpui-component (theme, inputs, markdown, popovers, …).
            gpui_component::init(cx);
            // Esc closes the active overlay (drawer/palette/dialog).
            cx.bind_keys([KeyBinding::new("escape", CloseOverlay, None)]);
            // Command palette: Ctrl/Cmd+K opens from anywhere; arrows/enter are
            // scoped to the "Palette" key-context so they don't shadow a focused
            // text input's cursor/newline keys.
            cx.bind_keys([
                KeyBinding::new("cmd-k", OpenPalette, None),
                KeyBinding::new("ctrl-k", OpenPalette, None),
                KeyBinding::new("up", PaletteUp, Some("Palette")),
                KeyBinding::new("down", PaletteDown, Some("Palette")),
                KeyBinding::new("enter", PaletteConfirm, Some("Palette")),
            ]);
            // Text-input editing key bindings (scoped to focused inputs).
            text_input::bind_keys(cx);

            let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| {
                        // Start the live wiring: filesystem watch, appearance, and
                        // attention poll all marshal back onto this entity.
                        live::spawn(cx);
                        OrreryApp {
                            view: View::Grid,
                            rows,
                            roots,
                            theme,
                            config,
                            attention: Vec::new(),
                            overlay: None,
                            drawer: Default::default(),
                            inbox: Default::default(),
                            feed: Default::default(),
                            explore: Default::default(),
                            cleanup: Default::default(),
                            explore_cloning: Default::default(),
                            settings: None,
                            focus: cx.focus_handle(),
                        }
                    });
                    // Focus the app root so key bindings (Esc) dispatch to it.
                    let focus = view.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    // gpui-component's Root provides the theme + popover/modal/
                    // notification layers its components need.
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
            cx.activate(true);
        });
}
