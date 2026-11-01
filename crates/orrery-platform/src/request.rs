//! Requests from desktop integrations that only the running app can carry out:
//! raising the window, focusing a repo, opening a search. Notification buttons
//! and the GNOME Shell search provider produce these.
//!
//! The services that raise them start before (and independently of) the UI,
//! so instead of a callback per `spawn` the app registers one handler here
//! with [`set_handler`]. The handler runs on the service's thread; like the
//! tray callback it should just push onto the app's background→foreground
//! channel (see `live.rs`). Until a handler is registered, integrations hide
//! affordances that need one (the "Show in Orrery" notification button) and
//! drop requests they can't serve.

use std::sync::{Arc, Mutex};

/// Something a desktop integration asks the app to do. Each carries the XDG
/// activation token the desktop handed over with the user's click, when there
/// is one: on Wayland the app passes it to `xdg_activation_v1` so the
/// compositor lets the window take focus instead of just flashing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequest {
    /// Raise the main window.
    Show { activation_token: Option<String> },
    /// Raise the main window on the repo with this id (its path).
    ShowRepo {
        id: String,
        activation_token: Option<String>,
    },
    /// Raise the main window with this query in the command palette (GNOME
    /// Shell's "more results" click).
    Search {
        query: String,
        activation_token: Option<String>,
    },
}

type Handler = Arc<dyn Fn(AppRequest) + Send + Sync>;

static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);

/// Register the app's handler for [`AppRequest`]s, replacing any earlier one.
pub fn set_handler(handler: impl Fn(AppRequest) + Send + Sync + 'static) {
    if let Ok(mut slot) = HANDLER.lock() {
        *slot = Some(Arc::new(handler));
    }
}

/// Whether an app handler is registered, so a request would be served.
pub fn has_handler() -> bool {
    HANDLER.lock().map(|h| h.is_some()).unwrap_or(false)
}

/// Hand `request` to the app. Returns `false` (and drops it) when no handler
/// is registered.
pub(crate) fn dispatch(request: AppRequest) -> bool {
    // Clone the handler out so it runs without the lock held.
    let handler = HANDLER.lock().ok().and_then(|h| h.clone());
    match handler {
        Some(handler) => {
            handler(request);
            true
        }
        None => false,
    }
}
