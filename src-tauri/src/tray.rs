//! System tray (#47): an icon with quick actions (show, rescan, quit). The
//! window hides to the tray on close rather than quitting.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager};

const TRAY_ID: &str = "orrery-tray";

// Monochrome symbolic glyphs (transparent background), so the tray icon reads
// like the other symbolic icons in the panel rather than a full-colour badge.
// We can't hand the SNI host a themed icon name through Tauri's pixmap API, so
// we ship both tints and pick by the desktop colour-scheme: a light glyph for
// dark panels, a dark glyph for light ones. Source: docs/design-system/assets.
const TRAY_LIGHT: &[u8] = include_bytes!("../icons/tray-light.png");
const TRAY_DARK: &[u8] = include_bytes!("../icons/tray-dark.png");

/// The symbolic glyph tinted for a dark (`true`) or light (`false`) panel.
fn tray_glyph(panel_is_dark: bool) -> Option<Image<'static>> {
    Image::from_bytes(if panel_is_dark { TRAY_LIGHT } else { TRAY_DARK }).ok()
}

/// Whether the panel is dark, from an `appearance-changed` payload. Defaults to
/// dark (the common panel) when the scheme is absent or unparseable.
fn panel_is_dark(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("colorScheme").and_then(|s| s.as_str()).map(|s| s != "light"))
        .unwrap_or(true)
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Orrery", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, "rescan", "Rescan repos", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &rescan, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Orrery")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "rescan" => {
                let _ = app.emit("repos-changed", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { .. } = event {
                show_main(tray.app_handle());
            }
        });

    // Start with the dark-panel glyph; the appearance watcher emits the current
    // scheme on launch (handled below), correcting it immediately if wrong.
    if let Some(icon) = tray_glyph(true) {
        builder = builder.icon(icon);
    }
    builder.build(app)?;

    // Re-tint the glyph whenever the desktop theme flips, so it keeps matching
    // the panel. `appearance-changed` is emitted by appearance::spawn_watcher.
    let handle = app.clone();
    app.listen("appearance-changed", move |event| {
        if let (Some(tray), Some(icon)) =
            (handle.tray_by_id(TRAY_ID), tray_glyph(panel_is_dark(event.payload())))
        {
            let _ = tray.set_icon(Some(icon));
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::panel_is_dark;

    #[test]
    fn reads_color_scheme_from_payload() {
        assert!(panel_is_dark(r#"{"colorScheme":"dark"}"#));
        assert!(!panel_is_dark(r#"{"colorScheme":"light"}"#));
    }

    #[test]
    fn defaults_to_dark_when_unknown() {
        // "no preference", missing field, or garbage all fall back to dark.
        assert!(panel_is_dark(r#"{"colorScheme":null}"#));
        assert!(panel_is_dark(r#"{"accent":null}"#));
        assert!(panel_is_dark("not json"));
    }

    #[test]
    fn glyph_bytes_decode() {
        assert!(super::tray_glyph(true).is_some());
        assert!(super::tray_glyph(false).is_some());
    }
}
