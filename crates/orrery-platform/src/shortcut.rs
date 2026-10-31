//! Global shortcut to summon the app, via the XDG **GlobalShortcuts portal**.
//!
//! On Wayland there's no silent global key-grab; the portal is the sanctioned
//! mechanism. The app registers an "activate" shortcut with a preferred
//! Ctrl+Alt+O trigger — on a desktop that implements the portal (KDE Plasma 6,
//! GNOME) the user can rebind it in their global-shortcuts settings. `on_activate`
//! fires whenever the shortcut is triggered. Degrades gracefully (logs, and the
//! thread exits) if the portal is unavailable, like the rest of platform.
//! Registers the app id with the portal first; see [`crate::app_id`].

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

/// Stable id for our one shortcut, matched on the `Activated` signal.
const SHORTCUT_ID: &str = "activate";
/// Preferred accelerator (portal syntax). The portal/desktop may override it and
/// let the user pick — `preferred_trigger` is only a hint.
const PREFERRED_TRIGGER: &str = "CTRL+ALT+o";

/// Spawn the portal listener on its own thread + runtime. `on_activate` is called
/// (on that thread) each time the shortcut fires.
pub fn spawn(on_activate: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        // Best-effort: a missing/!implemented portal just means no global hotkey.
        if let Err(e) = rt.block_on(run(on_activate)) {
            eprintln!(
                "[orrery shortcut] global shortcut unavailable (GlobalShortcuts portal): {e}"
            );
        }
    });
}

async fn run(on_activate: impl Fn()) -> ashpd::Result<()> {
    // KDE's portal rejects a GlobalShortcuts session from a caller it can't put
    // an app id to ("NotAllowed: An app id is required"). Register ours on a
    // connection of our own: registration must come before any other portal
    // call on the connection, and ashpd's shared one may already be in use by
    // gpui. A failure is logged with a hint; we still try, because a launch from
    // the desktop entry lets the portal infer the id from the systemd scope.
    let conn = zbus::Connection::session().await?;
    crate::app_id::register(&conn).await;
    let shortcuts = GlobalShortcuts::with_connection(conn).await?;
    let session = shortcuts.create_session(Default::default()).await?;

    let activate =
        NewShortcut::new(SHORTCUT_ID, "Summon Orrery").preferred_trigger(Some(PREFERRED_TRIGGER));
    // Await the bind's response so the registration completes (or its refusal
    // surfaces) before we listen. The desktop owns the final trigger, which may
    // differ from the preferred one; log it so "the hotkey does nothing" is
    // diagnosable.
    let bound = shortcuts
        .bind_shortcuts(&session, &[activate], None, Default::default())
        .await?
        .response()?;
    for s in bound.shortcuts() {
        let trigger = match s.trigger_description() {
            "" => "unassigned",
            t => t,
        };
        eprintln!("[orrery shortcut] bound '{}' → {trigger}", s.id());
    }

    // The stream borrows nothing of `session`, but `session` must outlive it:
    // dropping the session ends the registration. Both stay in scope here.
    let mut activated = shortcuts.receive_activated().await?;
    while let Some(activation) = activated.next().await {
        if activation.shortcut_id() == SHORTCUT_ID {
            on_activate();
        }
    }
    Ok(())
}
