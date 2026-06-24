//! Live wiring — marshal background desktop signals onto the GPUI foreground so
//! the running app reacts without a manual refresh. Several sources, each owning
//! its own thread + runtime in `orrery-platform`:
//!
//! - **filesystem watch** → rescan the roots and reload the grid;
//! - **appearance change** → recompute the theme with the new system accent;
//! - **attention poll** → update the Inbox nav badge (notifications fire inside
//!   the platform poller);
//! - **global shortcut** (Ctrl+Alt+O) → raise the window;
//! - **system tray** → show / rescan / quit / open-a-repo.
//!
//! GPUI is single-threaded: entity mutation needs `&mut App`, which only exists
//! on the foreground. So each background callback just pushes a [`Signal`] onto
//! an `async-channel`, and one gpui task drains it, updating the entity (with
//! `cx.notify()`) on the foreground. Heavy work (the rescan) is handed to the
//! background executor so it never blocks the UI.

use std::rc::Rc;

use gpui::Context;
use orrery_platform::appearance::Appearance;
use orrery_platform::tray::TrayAction;

use crate::data;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// A desktop signal to apply on the GPUI foreground.
enum Signal {
    /// Repos changed on disk — rescan and reload the grid.
    ReposChanged,
    /// Desktop theme/accent changed — recompute the theme.
    Appearance(Appearance),
    /// Latest attention glance lines — for the Inbox nav badge.
    Attention(Vec<String>),
    /// Raise the main window (tray: left-click / "Show Orrery").
    ShowWindow,
    /// Quit the app (tray: "Quit").
    Quit,
}

/// Start the background watchers and the gpui task that applies their signals.
/// Call once during app construction (inside `cx.new`). Returns whether the
/// system tray came up — the window's close-to-tray behaviour is gated on it, so
/// there's always a way to quit when there's no tray.
pub fn spawn(cx: &mut Context<OrreryApp>) -> bool {
    let (tx, rx) = async_channel::unbounded::<Signal>();

    // Filesystem watch → rescan. Debounced inside the platform watcher.
    {
        let tx = tx.clone();
        orrery_platform::watcher::spawn(move || {
            let _ = tx.try_send(Signal::ReposChanged);
        });
    }

    // Desktop appearance (theme/accent) → live theme. Fires once immediately, so
    // the launch accent is reconfirmed (a no-op past the synchronous startup read).
    {
        let tx = tx.clone();
        orrery_platform::appearance::watch(move |appearance| {
            let _ = tx.try_send(Signal::Appearance(appearance));
        });
    }

    // Attention poll → Inbox badge. Notifications fire inside the poller itself.
    {
        let tx = tx.clone();
        orrery_platform::notifier::watch(move |lines| {
            let _ = tx.try_send(Signal::Attention(lines));
        });
    }

    // Global shortcut (Ctrl+Alt+O) → raise the window, via the portal.
    {
        let tx = tx.clone();
        orrery_platform::shortcut::spawn(move || {
            let _ = tx.try_send(Signal::ShowWindow);
        });
    }

    // KDE KRunner service. Self-contained — it reads the cached repo list and
    // opens a match in the IDE directly, so it needs no channel back to the app.
    orrery_platform::krunner::spawn();

    // System tray. Menu activations come back on the tray thread; forward the
    // app-level ones onto the channel (Open is handled inside the tray itself).
    let tray = {
        let tx = tx.clone();
        orrery_platform::tray::spawn(move |action| {
            let signal = match action {
                TrayAction::Show => Signal::ShowWindow,
                TrayAction::Rescan => Signal::ReposChanged,
                TrayAction::Quit => Signal::Quit,
                TrayAction::Open(_) => return, // handled in the tray
            };
            let _ = tx.try_send(signal);
        })
    };
    let tray_active = tray.is_some();

    // The single foreground consumer. Holds a weak handle to the app entity; it
    // ends naturally when the entity is dropped (its `update` calls start failing
    // and the channel closes). Owns the tray handle so it can push glance +
    // panel-theme updates to it.
    cx.spawn(async move |this, cx| {
        while let Ok(signal) = rx.recv().await {
            match signal {
                Signal::ReposChanged => {
                    // The git scan is slow; run it on the background pool and
                    // only touch the entity with the finished rows.
                    let (rows, roots) = cx
                        .background_executor()
                        .spawn(async { data::rescan() })
                        .await;
                    let applied = this.update(cx, |app, cx| {
                        app.rows = rows;
                        app.roots = roots;
                        // New/changed repos → refresh the semantic index (cheap
                        // when unchanged; a no-op unless AI is ready) and host
                        // enrichment (skips repos still within the cache TTL).
                        app.index_semantic();
                        app.enrich_hosts(cx);
                        app.load_activity(cx);
                        cx.notify();
                    });
                    if applied.is_err() {
                        break; // entity gone — stop draining
                    }
                }
                Signal::Appearance(appearance) => {
                    // Keep the tray glyph matching the panel (dark unless the
                    // scheme is explicitly light).
                    if let Some(tray) = &tray {
                        tray.set_panel_dark(appearance.color_scheme.as_deref() != Some("light"));
                    }
                    let accent = appearance.accent.map(|c| (c.r, c.g, c.b));
                    if this
                        .update(cx, |app, cx| {
                            app.theme = Rc::new(Theme::dark().with_system_accent(accent));
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Signal::Attention(lines) => {
                    if let Some(tray) = &tray {
                        tray.set_glance(lines.clone());
                    }
                    if this
                        .update(cx, |app, cx| {
                            app.attention = lines;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Signal::ShowWindow => cx.update(|cx| cx.activate(true)),
                Signal::Quit => cx.update(|cx| cx.quit()),
            }
        }
    })
    .detach();

    tray_active
}
