//! Raise a window with an XDG activation token the desktop handed over (a
//! notification click, a launcher), so on Wayland the compositor lets it take
//! focus instead of only flashing it in the taskbar.
//!
//! gpui's own `activate_window` asks the compositor for a fresh token from
//! the app itself, which focus-stealing prevention usually refuses when the
//! app isn't already focused. The token from the user's click is the one that
//! counts. gpui doesn't take an outside token, but it does expose the
//! window's raw `wl_display` / `wl_surface`, so this binds `xdg_activation_v1`
//! on a private event queue over gpui's own libwayland connection and
//! activates the surface with it.
//!
//! On X11 (or with no token) there's nothing to do here; the caller falls
//! back to gpui's activation.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

/// Activate `window` (a gpui `Window`, or anything else handing out raw
/// handles) with `token`. Returns an error, having done nothing, for a
/// non-Wayland window, an empty token, or a compositor without
/// `xdg_activation_v1`; the caller then falls back to its own activation.
///
/// Call it on the window's own thread (gpui windows are `!Send`, so a
/// `&Window` already guarantees that).
pub fn activate_window<W: HasWindowHandle + HasDisplayHandle>(
    window: &W,
    token: &str,
) -> Result<(), String> {
    let display = window.display_handle().map_err(|e| e.to_string())?;
    let handle = window.window_handle().map_err(|e| e.to_string())?;
    // The borrowed handles keep the display and surface alive for the call.
    unsafe { activate_with_token(display.as_raw(), handle.as_raw(), token) }
}

/// Activate the Wayland surface behind `window` with `token`.
///
/// # Safety
///
/// `display` and `window` must be live handles from the same open window,
/// used on the thread that owns that window, while it is open.
unsafe fn activate_with_token(
    display: RawDisplayHandle,
    window: RawWindowHandle,
    token: &str,
) -> Result<(), String> {
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) = (display, window)
    else {
        return Err("not a Wayland window".into());
    };
    let token = token.trim();
    if token.is_empty() {
        return Err("empty activation token".into());
    }
    // A guest backend over gpui's connection: it doesn't own (or close) the
    // display, and our objects live on their own event queue.
    let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
    let qh = queue.handle();
    let activation: XdgActivationV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("no xdg_activation_v1: {e}"))?;
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast()) }
        .map_err(|e| format!("window surface: {e}"))?;
    let surface = WlSurface::from_id(&conn, id).map_err(|e| format!("window surface: {e}"))?;
    activation.activate(token.to_string(), &surface);
    activation.destroy();
    // Make sure the requests reached the compositor before the guest
    // backend goes away.
    queue
        .roundtrip(&mut State)
        .map_err(|e| format!("wayland roundtrip: {e}"))?;
    Ok(())
}

/// Event sink for the private queue; xdg_activation_v1 and the registry
/// listing send nothing we need.
struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: <XdgActivationV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::{XlibDisplayHandle, XlibWindowHandle};

    #[test]
    fn non_wayland_handles_are_refused() {
        let display = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        let window = RawWindowHandle::Xlib(XlibWindowHandle::new(1));
        let err = unsafe { activate_with_token(display, window, "tok") }.unwrap_err();
        assert!(err.contains("not a Wayland"), "{err}");
    }
}
