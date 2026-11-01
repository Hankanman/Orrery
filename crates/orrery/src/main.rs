//! Orrery — the desktop GPUI app. All logic comes from the `orrery-core` crate
//! (scan/git/forge/inbox/ai/cache/config) and desktop integration from
//! `orrery-platform`; this crate is the UI: the `--orr-*` theme, cards, the
//! app shell (header + sidebar nav + view switching, `shell/`), the repo
//! drawer, and the views.

mod assets;
mod card;
mod data;
mod drawer;
mod fleet;
mod heatmap;
mod icon;
mod live;
mod palette;
mod shell;
mod task;
mod theme;
mod toast;
mod views;
mod widgets;

use std::rc::Rc;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, px,
    size,
};
use gpui_component::Root;

use shell::OrreryApp;
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

/// Work out what this launch asks for (`orrery open <query>`, an
/// `orrery://` link, or nothing), then become the running instance or hand
/// the request to the one already running. Returns the request this process
/// should act on once its window is up; exits when the request was handed
/// over or can't be served.
fn single_instance() -> Option<orrery_platform::request::AppRequest> {
    use orrery_platform::instance::{self, Claim};
    use orrery_platform::open_request;

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--new-instance`: skip the single-instance hand-off (side-by-side dev
    // builds).
    let unclaimed = args.first().is_some_and(|a| a == "--new-instance");
    if unclaimed {
        args.remove(0);
    }
    let target = match open_request::parse_args(&args) {
        Ok(target) => target,
        Err(e) => {
            eprintln!("orrery: {e}");
            std::process::exit(2);
        }
    };
    // The launcher's activation token, so the raised window may take focus.
    let token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    let request = match &target {
        None => None,
        Some(target) => {
            match open_request::resolve(target, &orrery_core::cache::load_repos(), token) {
                Ok(req) => Some(req),
                Err(e) => {
                    eprintln!("orrery: {e}");
                    std::process::exit(1);
                }
            }
        }
    };
    if unclaimed {
        return request;
    }
    match instance::claim(request.clone()) {
        Claim::Primary => request,
        Claim::HandedOff => {
            eprintln!("[orrery] handed over to the running instance");
            std::process::exit(0);
        }
        Claim::Refused => {
            eprintln!("orrery: the running instance refused the request");
            std::process::exit(1);
        }
    }
}

fn main() {
    let request = single_instance();

    // Point the bundled llama.cpp backend at a runtime shipped next to the
    // binary, if any: packages install it to `<prefix>/lib/orrery/llama-runtime`
    // (the AppImage bundles one; deb/rpm stay lean). A no-op in source builds /
    // when nothing is there — `materialize_bundled` only acts if it finds a
    // `llama-server`, so the discovery falls through to Ollama / PATH otherwise.
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|p| p.parent())
    {
        orrery_core::llama::set_bundled_dir(prefix.join("lib/orrery/llama-runtime"));
    }

    let snap = data::load();
    eprintln!(
        "[native] loaded {} repos across {} roots",
        snap.rows.len(),
        snap.roots
    );
    // Borrow the desktop's accent colour (KDE/portal) so the app harmonises
    // with the user's theme — the design system's runtime accent override.
    let appearance = orrery_platform::appearance::read_blocking();
    let reduced_motion = appearance.reduced_motion;
    let accent = appearance.accent.map(|c| (c.r, c.g, c.b));
    if let Some((r, g, b)) = accent {
        eprintln!("[native] system accent #{r:02x}{g:02x}{b:02x}");
    }
    let theme = Rc::new(Theme::dark().with_system_accent(accent));
    let config = orrery_core::config::load();

    let platform = gpui_platform::current_platform(false);
    Application::with_platform(platform)
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            // Initialise gpui-component, then map its theme onto our --orr-* tokens
            // so its components match the rest of the UI.
            gpui_component::init(cx);
            theme::apply_gpui_component_theme(&theme, cx);
            // Kill the llama.cpp sidecar on quit so it (and its loaded model's
            // RAM/VRAM) doesn't outlive the app.
            cx.on_app_quit(|_| async { orrery_core::llama::shutdown() })
                .detach();
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

            let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Wayland app_id / X11 WM_CLASS: lets the compositor match
                    // the window to orrery.desktop (icon, taskbar grouping).
                    app_id: Some(orrery_platform::app_id::APP_ID.to_owned()),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| OrreryApp::new(snap, theme, config, cx));
                    // Close-to-tray: when the tray is up, the window's close
                    // button minimizes to the tray instead of quitting. Without a
                    // tray we leave the default (close quits) so there's a way out.
                    if view.read(cx).tray_active {
                        window.on_window_should_close(cx, |window, _cx| {
                            window.minimize_window();
                            false
                        });
                    }
                    // Probe AI reachability and build the semantic index in the
                    // background, so Ctrl+K can search by meaning. Also kick off a
                    // host-enrichment pass so cards fill in stars/visibility.
                    // The first attention pass, the AI probe, and the same
                    // post-snapshot fan-out a rescan runs — see
                    // `OrreryApp::startup`.
                    view.update(cx, |this, cx| {
                        this.system_reduced_motion = reduced_motion;
                        this.startup(cx);
                    });
                    // Focus the app root so key bindings (Esc) dispatch to it.
                    let focus = view.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    // This launch's own `orrery open` / `orrery://` request.
                    if let Some(req) = request.clone() {
                        view.update(cx, |this, cx| this.apply_request(req, window, cx));
                    }
                    // gpui-component's Root provides the theme + popover/modal/
                    // notification layers its components need.
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
            cx.activate(true);
        });
}
