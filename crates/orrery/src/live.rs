//! Live wiring — marshal background desktop signals onto the GPUI foreground so
//! the running app reacts without a manual refresh. Several sources, each owning
//! its own thread + runtime in `orrery-platform`:
//!
//! - **filesystem watch** → rescan the roots and reload the grid (a burst of
//!   events coalesces into one single-flight rescan);
//! - **appearance change** → recompute the theme with the new system accent;
//! - **attention poll** → feed the poll's inbox facts into the attention model
//!   (badges, tray summary, urgent notifications — all downstream of
//!   `recompute_attention`; the poller itself only notifies the non-model
//!   PR/CI kinds);
//! - **global shortcut** (Ctrl+Alt+O) → raise the window;
//! - **system tray** → show / rescan / quit / open-a-repo;
//! - **clock** → a once-a-minute repaint for relative times.
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

use crate::shell::{OrreryApp, SnapKind};
use crate::theme::Theme;

/// A desktop signal to apply on the GPUI foreground.
enum Signal {
    /// Repos changed on disk — rescan and reload the grid.
    ReposChanged,
    /// Desktop theme/accent changed — recompute the theme.
    Appearance(Appearance),
    /// Latest attention-poll result: glance lines (the Inbox nav badge's
    /// fallback) + the raw inbox facts for the attention model.
    Attention(Vec<String>, Vec<orrery_core::inbox::InboxItem>),
    /// Raise the main window (tray: left-click / "Show Orrery").
    ShowWindow,
    /// Quit the app (tray: "Quit").
    Quit,
}

/// Start the background watchers and the gpui task that applies their signals.
/// Call once during app construction (inside `cx.new`). Returns the tray
/// handle if the system tray came up — the app stores it to push attention
/// summaries after each recompute, and the window's close-to-tray behaviour is
/// gated on its presence so there's always a way to quit when there's no tray
/// — and the fs-watcher handle, so the app can re-arm the watches when
/// repos/roots are added at runtime (Settings save, New Project, Explore
/// clone).
pub fn spawn(
    cx: &mut Context<OrreryApp>,
) -> (
    Option<orrery_platform::tray::TrayHandle>,
    orrery_platform::watcher::WatcherHandle,
) {
    let (tx, rx) = async_channel::unbounded::<Signal>();

    // Filesystem watch → rescan. Debounced inside the platform watcher.
    let watcher = {
        let tx = tx.clone();
        orrery_platform::watcher::spawn(move || {
            let _ = tx.try_send(Signal::ReposChanged);
        })
    };

    // Desktop appearance (theme/accent) → live theme. Fires once immediately, so
    // the launch accent is reconfirmed (a no-op past the synchronous startup read).
    {
        let tx = tx.clone();
        orrery_platform::appearance::watch(move |appearance| {
            let _ = tx.try_send(Signal::Appearance(appearance));
        });
    }

    // Attention poll → Inbox badge + attention-model inbox facts. The poller
    // still notifies its non-model kinds (new PR / CI alert) itself; urgent
    // model items notify from the app after each recompute.
    {
        let tx = tx.clone();
        orrery_platform::notifier::watch(move |lines, items| {
            let _ = tx.try_send(Signal::Attention(lines, items));
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

    // The single foreground consumer. Holds a weak handle to the app entity; it
    // ends naturally when the entity is dropped (its `update` calls start failing
    // and the channel closes). Keeps a tray-handle clone to push panel-theme
    // updates (the attention summary is pushed by the app itself, from
    // `recompute_attention`, through the clone returned below).
    let tray_for_app = tray.clone();
    cx.spawn(async move |this, cx| {
        // A signal read ahead while coalescing a ReposChanged burst, to be
        // handled next.
        let mut pending: Option<Signal> = None;
        loop {
            let signal = match pending.take() {
                Some(s) => s,
                None => match rx.recv().await {
                    Ok(s) => s,
                    Err(_) => break,
                },
            };
            match signal {
                Signal::ReposChanged => {
                    // Coalesce a burst of change events (and tray rescans)
                    // queued behind this one into a single rescan.
                    while let Ok(next) = rx.try_recv() {
                        if !matches!(next, Signal::ReposChanged) {
                            pending = Some(next);
                            break;
                        }
                    }
                    // The snapshot pass scans off the UI thread, is
                    // single-flight (a request mid-scan becomes one trailing
                    // rescan), and runs the post-snapshot fan-out.
                    let applied = this.update(cx, |app, cx| {
                        app.request_snapshot(SnapKind::Rescan, cx);
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
                Signal::Attention(lines, items) => {
                    if this
                        .update(cx, |app, cx| {
                            app.attention = lines;
                            app.polled_inbox = Some(items);
                            // Fresh host facts → refresh the attention
                            // surfaces (badges, tray, urgent notifications).
                            app.recompute_attention();
                            // Piggyback the central CI pass on the poll's
                            // cadence (180s) — its own TTL (~5 min) makes
                            // most of these ticks a no-op.
                            app.refresh_ci(cx);
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

    // Clock tick → repaint once a minute, so relative times rendered from
    // timestamps (card ages, the sidebar's "Scanned …") stay current while
    // nothing else changes. Ends when the entity is dropped.
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(60))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
    .detach();

    (tray_for_app, watcher)
}
