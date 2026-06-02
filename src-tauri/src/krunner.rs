//! KDE KRunner integration (#49): exposes repos over the `org.kde.krunner1`
//! D-Bus interface so they're searchable from KRunner / the desktop launcher.
//! Matches read the cached repo list (populated by the app's scans), so there's
//! no filesystem walk per keystroke. Also writes a dbusplugin `.desktop` so
//! KRunner discovers the service. Best-effort — no-ops without a session bus.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

use crate::{cache, config, launch};

const BUS_NAME: &str = "com.orrery.krunner";
const OBJECT_PATH: &str = "/krunner";

struct Runner;

#[zbus::interface(name = "org.kde.krunner1")]
impl Runner {
    /// Returns matches as `(id, text, iconName, type, relevance, properties)`.
    #[zbus(name = "Match")]
    async fn do_match(
        &self,
        query: &str,
    ) -> Vec<(String, String, String, i32, f64, HashMap<String, OwnedValue>)> {
        let q = query.trim().to_lowercase();
        if q.len() < 2 {
            return Vec::new();
        }
        cache::load_repos()
            .into_iter()
            .filter(|r| {
                r.display_name.to_lowercase().contains(&q)
                    || r.slug.as_deref().unwrap_or("").to_lowercase().contains(&q)
                    || r.path.to_lowercase().contains(&q)
            })
            .take(10)
            .map(|r| {
                let subtitle = r.slug.clone().unwrap_or_else(|| r.path.clone());
                (
                    r.id,
                    format!("{}  —  {}", r.display_name, subtitle),
                    "folder-development".to_string(),
                    60,
                    0.8,
                    HashMap::new(),
                )
            })
            .collect()
    }

    #[zbus(name = "Actions")]
    async fn actions(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    #[zbus(name = "Run")]
    async fn run(&self, match_id: &str, _action_id: &str) {
        // `match_id` is the repo's absolute path (its id) — open it in the IDE.
        let _ = launch::launch(&config::load().ide_command, match_id);
    }
}

fn ensure_plugin_file() {
    let Some(dir) = dirs::data_dir().map(|d| d.join("krunner").join("dbusplugins")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("orrery.desktop");
    if path.exists() {
        return;
    }
    let _ = std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Service\nName=Orrery\nComment=Open your git repos\n\
             X-KDE-ServiceTypes=Plasma/Runner\nX-Plasma-API=DBus\n\
             X-Plasma-DBusRunner-Service={BUS_NAME}\nX-Plasma-DBusRunner-Path={OBJECT_PATH}\n"
        ),
    );
}

async fn serve() -> zbus::Result<()> {
    let _conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Runner)?
        .build()
        .await?;
    std::future::pending::<()>().await;
    Ok(())
}

/// Start the KRunner service in the background.
pub fn spawn() {
    ensure_plugin_file();
    tauri::async_runtime::spawn(async {
        if let Err(e) = serve().await {
            eprintln!("[orrery krunner] disabled: {e}");
        }
    });
}
