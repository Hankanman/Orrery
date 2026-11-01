//! KDE KRunner integration (#49 / #138): exposes repos over the `org.kde.krunner1`
//! D-Bus interface so they're searchable straight from KRunner / the Plasma
//! launcher. Matching is shared with the GNOME Shell search provider (see
//! [`crate::repo_search`]). A match opens in the IDE by default; its action
//! buttons open the terminal agent or the folder, or copy the path. Also
//! writes a dbusplugin `.desktop` so KRunner discovers the service.
//! Best-effort — no-ops without a session bus, like the rest of platform.
//!
//! The service runs on its own thread + runtime, mirroring `appearance::watch`
//! / `notifier::watch`. The same connection also serves
//! [`crate::gnome_search`], so one `spawn` covers both launchers.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

use crate::gnome_search;
use crate::repo_search::{self, RepoAction};

const BUS_NAME: &str = "com.orrery.krunner";
const OBJECT_PATH: &str = "/krunner";

/// Most matches returned per query.
const MAX_MATCHES: usize = 10;

/// `QueryMatch::Type` for our matches (between Possible and Exact).
const MATCH_TYPE: i32 = 60;

/// The secondary actions, as `(id, text, icon)`. The empty id is the default
/// (clicking or Enter on the match): open in the IDE.
const ACTIONS: [(&str, &str, &str); 3] = [
    ("agent", "Open terminal agent", "utilities-terminal"),
    ("folder", "Open folder", "folder-open"),
    ("copy", "Copy path", "edit-copy"),
];

/// The repo action an action id names; the empty id (and any id we don't
/// know) is the default, open in the IDE.
fn action_for(id: &str) -> RepoAction {
    match id {
        "agent" => RepoAction::Agent,
        "folder" => RepoAction::Folder,
        "copy" => RepoAction::CopyPath,
        _ => RepoAction::Ide,
    }
}

struct Runner;

#[zbus::interface(name = "org.kde.krunner1")]
impl Runner {
    /// Returns matches as `(id, text, iconName, type, relevance, properties)`.
    /// The `id` is the repo's path (the app keys repos by path), which `Run`
    /// validates before acting on.
    #[zbus(name = "Match")]
    async fn do_match(
        &self,
        query: &str,
    ) -> Vec<(
        String,
        String,
        String,
        i32,
        f64,
        HashMap<String, OwnedValue>,
    )> {
        let terms = repo_search::query_terms(query);
        repo_search::search(&repo_search::repos(), &terms, MAX_MATCHES)
            .into_iter()
            .map(|(r, score)| {
                (
                    r.id.clone(),
                    format!("{}  —  {}", r.display_name, repo_search::subtitle(r)),
                    "folder-development".to_string(),
                    MATCH_TYPE,
                    score,
                    HashMap::new(),
                )
            })
            .collect()
    }

    /// The actions offered on every match (no per-match `actions` property,
    /// so KRunner shows them all).
    #[zbus(name = "Actions")]
    async fn actions(&self) -> Vec<(String, String, String)> {
        ACTIONS
            .iter()
            .map(|(id, text, icon)| (id.to_string(), text.to_string(), icon.to_string()))
            .collect()
    }

    #[zbus(name = "Run")]
    async fn run(&self, match_id: &str, action_id: &str) {
        // `repo_search::run` only acts on a known repo: the session bus is
        // unauthenticated, so never on an arbitrary path.
        if let Err(e) = repo_search::run(action_for(action_id), match_id).await {
            eprintln!("[orrery krunner] '{action_id}' failed for {match_id}: {e}");
        }
    }
}

/// Write the KRunner dbusplugin descriptor so the launcher discovers our service.
/// Best-effort and idempotent (skips if already present).
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
    // Holding the connection in scope keeps the bus names owned and the
    // services registered; zbus drives method dispatch on its own internal
    // executor.
    let _conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Runner)?
        .name(gnome_search::BUS_NAME)?
        .serve_at(gnome_search::OBJECT_PATH, gnome_search::SearchProvider)?
        .build()
        .await?;
    std::future::pending::<()>().await;
    Ok(())
}

/// Start the launcher search services (KRunner and the GNOME Shell search
/// provider) on their own thread + runtime. Degrades silently (the thread
/// exits) when there's no session bus or the services can't be registered —
/// launcher search is then simply unavailable, like every other platform
/// integration. Each desktop only queries its own interface, so serving both
/// everywhere costs nothing.
pub fn spawn() {
    ensure_plugin_file();
    std::thread::spawn(|| {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        if let Err(e) = rt.block_on(serve()) {
            eprintln!("[orrery krunner] disabled: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_map_to_repo_actions() {
        assert_eq!(action_for(""), RepoAction::Ide);
        assert_eq!(action_for("agent"), RepoAction::Agent);
        assert_eq!(action_for("folder"), RepoAction::Folder);
        assert_eq!(action_for("copy"), RepoAction::CopyPath);
        assert_eq!(action_for("bogus"), RepoAction::Ide);
        for (id, _, _) in ACTIONS {
            assert_ne!(action_for(id), RepoAction::Ide, "{id} must not fall back");
        }
    }
}
