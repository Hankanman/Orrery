//! Native desktop appearance integration.
//!
//! Reads the freedesktop "appearance" settings (color scheme + accent colour)
//! from the XDG Desktop Portal over D-Bus and subscribes to live changes so the
//! UI can mirror the user's desktop exactly. Everything here degrades
//! gracefully: if there's no session bus or no portal, we simply report "no
//! preference" and the frontend falls back to the `prefers-color-scheme` media
//! query.

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

const APPEARANCE_NS: &str = "org.freedesktop.appearance";

/// A system accent colour, components in the sRGB `[0, 1]` range.
#[derive(Debug, Clone, Serialize)]
pub struct Accent {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

/// The desktop's current appearance preferences.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    /// "dark" | "light", or `None` for "no preference" (frontend decides).
    pub color_scheme: Option<String>,
    /// The system accent colour, if the portal exposes one.
    pub accent: Option<Accent>,
}

// The freedesktop Settings portal. `ReadOne` returns the setting value as a
// variant; `SettingChanged` fires whenever any namespaced setting changes.
#[zbus::proxy(
    interface = "org.freedesktop.portal.Settings",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait Settings {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

    #[zbus(signal)]
    fn setting_changed(&self, namespace: String, key: String, value: OwnedValue)
        -> zbus::Result<()>;
}

fn parse_accent(value: &OwnedValue) -> Option<Accent> {
    // accent-color is a struct of three doubles (r, g, b). A "no accent"
    // sentinel is signalled with negative components.
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
            return Some(Accent { r, g, b });
        }
    }
    None
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

    let accent = match proxy.read_one(APPEARANCE_NS, "accent-color").await {
        Ok(v) => parse_accent(&v),
        Err(_) => None,
    };

    Appearance {
        color_scheme,
        accent,
    }
}

/// One-shot read of the current desktop appearance.
#[tauri::command]
pub async fn get_appearance() -> Appearance {
    let Ok(conn) = Connection::session().await else {
        return Appearance::default();
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
            // no-op on the frontend, so we don't need to filter by namespace.
            let _ = app.emit("appearance-changed", read_appearance(&proxy).await);
        }
    });
}
