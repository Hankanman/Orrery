//! The app's desktop identity, and registering it with xdg-desktop-portal.
//!
//! One id ties three things together: the Wayland `xdg_toplevel` app_id (set
//! on the window in `main.rs`), the basename of the installed `.desktop` file
//! (`orrery.desktop`), and the app id the portals see. KDE's portal backend
//! refuses some requests — GlobalShortcuts among them, with
//! `NotAllowed: An app id is required` — from a caller it can't name.
//!
//! A sandboxed (Flatpak/Snap) app gets its id from the sandbox. A host app like
//! ours gets one from its systemd scope (`app-orrery-….scope`) when the desktop
//! launched it from its `.desktop` entry, but not when it's started from a
//! terminal, an AppImage or `cargo run`. So we tell the portal explicitly via
//! `org.freedesktop.host.portal.Registry.Register` (xdg-desktop-portal ≥ 1.19).
//!
//! The registration is per D-Bus connection and must precede any other portal
//! call on that connection, so callers open a fresh connection, register, then
//! use it for the portal they need. The portal only accepts an id that matches
//! an installed `.desktop` file, so a dev build needs `orrery.desktop` in
//! `~/.local/share/applications` (see the getting-started docs).
//!
//! We call the method directly rather than through `ashpd::register_host_app`
//! because ashpd insists on a reverse-DNS id, and the desktop file (in every
//! package, and in existing installs) is plain `orrery`.

use std::collections::HashMap;

use zbus::zvariant::Value;

/// The app id: the `.desktop` basename, the Wayland app_id and the portal id.
pub const APP_ID: &str = "orrery";

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REGISTRY_IFACE: &str = "org.freedesktop.host.portal.Registry";

/// Register [`APP_ID`] for `conn` with the portal's host-app registry. Must run
/// before any other portal call on `conn`. A no-op inside a sandbox, where the
/// portal already knows the id. Logs (with a hint) and returns `false` on
/// failure; callers carry on, since the portal may still infer the id from the
/// systemd scope.
pub async fn register(conn: &zbus::Connection) -> bool {
    if ashpd::is_sandboxed() {
        return true;
    }
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    match conn
        .call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(REGISTRY_IFACE),
            "Register",
            &(APP_ID, options),
        )
        .await
    {
        Ok(_) => {
            eprintln!("[orrery portal] registered app id '{APP_ID}' with xdg-desktop-portal");
            true
        }
        Err(e) => {
            let e = e.to_string();
            eprintln!(
                "[orrery portal] couldn't register app id '{APP_ID}' with xdg-desktop-portal: {e}{}",
                hint(&e)
            );
            false
        }
    }
}

/// A short, actionable hint for a failed `Register` call, keyed on the
/// portal's error text.
fn hint(err: &str) -> &'static str {
    if err.contains("App info not found") {
        " (hint: install orrery.desktop into ~/.local/share/applications — the portal only accepts an id with a matching desktop file)"
    } else if err.contains("UnknownInterface")
        || err.contains("UnknownMethod")
        || err.contains("No such interface")
    {
        " (hint: xdg-desktop-portal is older than 1.19 and has no host app registry; launch Orrery from its desktop entry instead)"
    } else if err.contains("ServiceUnknown") || err.contains("was not provided") {
        " (hint: no xdg-desktop-portal is running in this session)"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_matches_the_desktop_file() {
        // The include path pins the basename; renaming the file breaks this.
        let desktop = include_str!("../../../packaging/orrery.desktop");
        // On X11 gpui sets WM_CLASS to the app id; the entry must match it.
        assert!(desktop.contains(&format!("StartupWMClass={APP_ID}\n")));
    }

    #[test]
    fn hints_name_the_likely_cause() {
        assert!(hint("org.freedesktop.portal.Error.Failed: Could not register app ID: App info not found for 'orrery'").contains("desktop file"));
        assert!(
            hint("org.freedesktop.DBus.Error.UnknownMethod: No such interface").contains("1.19")
        );
        assert!(
            hint("org.freedesktop.DBus.Error.ServiceUnknown: The name was not provided")
                .contains("no xdg-desktop-portal")
        );
        assert_eq!(hint("something else"), "");
    }
}
