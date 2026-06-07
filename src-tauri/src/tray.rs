//! System tray (#47): an icon with quick actions (show, rescan, quit). The
//! window hides to the tray on close rather than quitting.

use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager, Wry};

use crate::{cache, config, launch};

const TRAY_ID: &str = "orrery-tray";
/// Cap on attention lines / recent repos shown in the menu, to keep it compact.
const MAX_ATTENTION: usize = 8;
const MAX_RECENT: usize = 5;

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

/// The most-recently-active repos (by last commit) for the tray quick-open list,
/// as `(id, display_name)` pairs. The id is the repo's path — what the IDE
/// launcher and `open_in_ide` already expect.
fn recent_repos() -> Vec<(String, String)> {
    let mut repos = cache::load_repos();
    repos.sort_by(|a, b| b.last_commit_unix.cmp(&a.last_commit_unix));
    repos.into_iter().take(MAX_RECENT).map(|r| (r.id, r.display_name)).collect()
}

/// Build the tray menu: an attention header + per-item lines (informational),
/// the recent-repos quick-open list, then the standing show/rescan/quit actions.
/// Tauri 2 has no mutable-menu API, so updating means rebuilding the whole menu.
fn build_menu(app: &AppHandle, attention: &[String]) -> tauri::Result<Menu<Wry>> {
    let header_text = match attention.len() {
        0 => "All clear".to_string(),
        n => format!("{n} need attention"),
    };
    // Disabled (enabled = false) items are non-interactive labels.
    let header = MenuItem::with_id(app, "header", header_text, false, None::<&str>)?;
    let sep_top = PredefinedMenuItem::separator(app)?;
    let attention_items = attention
        .iter()
        .take(MAX_ATTENTION)
        .enumerate()
        .map(|(i, line)| MenuItem::with_id(app, format!("att:{i}"), format!("● {line}"), false, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;

    let recent = recent_repos();
    let recent_label = MenuItem::with_id(app, "recent", "Recent", false, None::<&str>)?;
    let recent_items = recent
        .iter()
        .map(|(id, name)| MenuItem::with_id(app, format!("open:{id}"), format!("  {name}"), true, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let sep_recent = PredefinedMenuItem::separator(app)?;

    let sep_bottom = PredefinedMenuItem::separator(app)?;
    let show = MenuItem::with_id(app, "show", "Show Orrery", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, "rescan", "Rescan repos", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let mut refs: Vec<&dyn IsMenuItem<Wry>> = vec![&header, &sep_top];
    for it in &attention_items {
        refs.push(it);
    }
    if !recent_items.is_empty() {
        refs.push(&sep_recent);
        refs.push(&recent_label);
        for it in &recent_items {
            refs.push(it);
        }
    }
    refs.push(&sep_bottom);
    refs.push(&show);
    refs.push(&rescan);
    refs.push(&quit);
    Menu::with_items(app, &refs)
}

/// Refresh the tray menu + tooltip to reflect the current attention lines.
/// Called from the background poller; a no-op if the tray isn't present yet.
///
/// Menu/tray mutations touch GTK, which on Linux must run on the main thread —
/// so we hop there (the poller calls this from the async runtime).
pub fn update(app: &AppHandle, attention: &[String]) {
    let app = app.clone();
    let attention = attention.to_vec();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        if let Ok(menu) = build_menu(&app, &attention) {
            let _ = tray.set_menu(Some(menu));
        }
        let tooltip = match attention.len() {
            0 => "Orrery".to_string(),
            n => format!("Orrery — {n} need attention"),
        };
        let _ = tray.set_tooltip(Some(&tooltip));
    });
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &[])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Orrery")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            match id {
                "show" => show_main(app),
                "rescan" => {
                    let _ = app.emit("repos-changed", ());
                }
                "quit" => app.exit(0),
                // A recent-repo entry: open it in the configured IDE directly
                // (works even while the window is hidden).
                _ if id.starts_with("open:") => {
                    let path = &id["open:".len()..];
                    let _ = launch::launch(&config::load().ide_command, path);
                }
                _ => {} // disabled header/attention/recent labels
            }
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
