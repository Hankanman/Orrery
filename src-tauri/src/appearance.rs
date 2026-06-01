//! Native desktop appearance integration.
//!
//! Orrery uses a *hybrid* strategy: it keeps its own branded structure but
//! borrows the desktop's window background, text colour and accent so it
//! harmonises with the user's theme. Sources:
//!
//! - **color scheme** (light/dark) + **accent** come from the XDG Desktop
//!   Portal (`org.freedesktop.appearance`) over D-Bus, which also gives us a
//!   live-change signal.
//! - **window/view/text colours** (and the precise accent) come from KDE's
//!   `kdeglobals` when present — that's what Qt apps actually paint, so it
//!   matches Dolphin/Discover exactly. On non-KDE desktops these are simply
//!   absent and the frontend falls back to its branded palette.
//!
//! Everything degrades gracefully: no bus, no portal, or no kdeglobals just
//! means fewer fields are populated.

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use zbus::zvariant::Value;
use zbus::Connection;

const APPEARANCE_NS: &str = "org.freedesktop.appearance";

/// An 8-bit sRGB colour.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The desktop's current appearance, as far as we can read it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    /// "dark" | "light", or `None` for "no preference" (frontend decides).
    pub color_scheme: Option<String>,
    /// System accent colour.
    pub accent: Option<Rgb>,
    /// Desktop window background (chrome).
    pub window_bg: Option<Rgb>,
    /// Desktop window text colour.
    pub window_fg: Option<Rgb>,
    /// Desktop "view"/content background (lists, panes).
    pub base_bg: Option<Rgb>,
}

// The freedesktop Settings portal. `ReadOne` returns the setting value as a
// variant; `SettingChanged` fires whenever any namespaced setting changes.
#[zbus::proxy(
    interface = "org.freedesktop.portal.Settings",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait Settings {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<zbus::zvariant::OwnedValue>;

    #[zbus(signal)]
    fn setting_changed(&self, namespace: String, key: String, value: zbus::zvariant::OwnedValue)
        -> zbus::Result<()>;
}

/// Parse the portal `accent-color` struct of three doubles in `[0, 1]`.
fn portal_accent(value: &zbus::zvariant::OwnedValue) -> Option<Rgb> {
    if let Value::Structure(s) = &**value {
        let fields = s.fields();
        if fields.len() == 3 {
            let get = |v: &Value| -> Option<f64> {
                if let Value::F64(x) = v {
                    Some(*x)
                } else {
                    None
                }
            };
            let r = get(&fields[0])?;
            let g = get(&fields[1])?;
            let b = get(&fields[2])?;
            if r < 0.0 || g < 0.0 || b < 0.0 {
                return None;
            }
            let to_u8 = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            return Some(Rgb {
                r: to_u8(r),
                g: to_u8(g),
                b: to_u8(b),
            });
        }
    }
    None
}

/// Colours read out of KDE's `kdeglobals`.
#[derive(Default)]
struct KdeColors {
    accent: Option<Rgb>,
    window_bg: Option<Rgb>,
    window_fg: Option<Rgb>,
    base_bg: Option<Rgb>,
}

fn parse_rgb_triplet(s: &str) -> Option<Rgb> {
    let mut it = s.split(',').map(|p| p.trim().parse::<u8>());
    let r = it.next()?.ok()?;
    let g = it.next()?.ok()?;
    let b = it.next()?.ok()?;
    Some(Rgb { r, g, b })
}

/// Read window/view/text/accent colours from `~/.config/kdeglobals`.
/// Returns defaults (all `None`) if the file is missing or unreadable.
fn read_kdeglobals() -> KdeColors {
    let mut out = KdeColors::default();

    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    let Some(path) = base.map(|b| b.join("kdeglobals")) else {
        return out;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return out;
    };

    let mut section = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match (section.as_str(), key) {
            (_, "AccentColor") => out.accent = parse_rgb_triplet(value),
            ("Colors:Window", "BackgroundNormal") => out.window_bg = parse_rgb_triplet(value),
            ("Colors:Window", "ForegroundNormal") => out.window_fg = parse_rgb_triplet(value),
            ("Colors:View", "BackgroundNormal") => out.base_bg = parse_rgb_triplet(value),
            _ => {}
        }
    }
    out
}

async fn read_appearance(proxy: &SettingsProxy<'_>) -> Appearance {
    let color_scheme = match proxy.read_one(APPEARANCE_NS, "color-scheme").await {
        Ok(v) => match &*v {
            // 0 = no preference, 1 = prefer dark, 2 = prefer light
            Value::U32(1) => Some("dark".to_string()),
            Value::U32(2) => Some("light".to_string()),
            _ => None,
        },
        Err(_) => None,
    };

    let portal_accent = match proxy.read_one(APPEARANCE_NS, "accent-color").await {
        Ok(v) => portal_accent(&v),
        Err(_) => None,
    };

    let kde = read_kdeglobals();

    Appearance {
        color_scheme,
        // Prefer the exact KDE accent (what Qt apps paint); fall back to the
        // portal's (slightly tinted) value on other desktops.
        accent: kde.accent.or(portal_accent),
        window_bg: kde.window_bg,
        window_fg: kde.window_fg,
        base_bg: kde.base_bg,
    }
}

/// One-shot read of the current desktop appearance.
#[tauri::command]
pub async fn get_appearance() -> Appearance {
    let Ok(conn) = Connection::session().await else {
        // No session bus — still try kdeglobals so colours work offline-of-bus.
        let kde = read_kdeglobals();
        return Appearance {
            accent: kde.accent,
            window_bg: kde.window_bg,
            window_fg: kde.window_fg,
            base_bg: kde.base_bg,
            ..Default::default()
        };
    };
    let Ok(proxy) = SettingsProxy::new(&conn).await else {
        return Appearance::default();
    };
    read_appearance(&proxy).await
}

/// Spawn a background task that emits `appearance-changed` whenever the desktop
/// theme or accent colour changes. No-ops if the portal is unavailable.
pub fn spawn_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(conn) = Connection::session().await else {
            return;
        };
        let Ok(proxy) = SettingsProxy::new(&conn).await else {
            return;
        };

        // Push the current state immediately so the UI is correct on launch.
        let _ = app.emit("appearance-changed", read_appearance(&proxy).await);

        let Ok(mut changes) = proxy.receive_setting_changed().await else {
            return;
        };
        while changes.next().await.is_some() {
            // Re-read on any settings change; applying identical values is a
            // no-op on the frontend, so we don't filter by namespace. This also
            // re-reads kdeglobals, picking up theme/accent edits.
            let _ = app.emit("appearance-changed", read_appearance(&proxy).await);
        }
    });
}
