mod ai;
mod appearance;
mod cache;
mod commands;
mod config;
mod forge;
mod git_ops;
mod inbox;
mod launch;
mod model;
mod oauth;
mod scan;
mod watcher;

/// Configure display/rendering environment on Linux before GTK/WebKit init.
/// Both vars are only set if the user hasn't already set them, so anyone can
/// override the behavior from the environment.
#[cfg(target_os = "linux")]
fn configure_linux_env() {
    // WebKitGTK's DMABUF renderer is broken on many drivers (notably NVIDIA),
    // producing blank/garbled webviews. Disable it by default.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // KWin (KDE) only draws server-side window decorations for X11/XWayland
    // windows; GTK refuses SSD on native Wayland, so a Wayland window gets a
    // foreign-looking client-side titlebar. Force XWayland on KDE+Wayland so
    // the window gets the native KWin decoration. Other desktops (GNOME,
    // wlroots) keep native Wayland, where CSD is the expected convention.
    if std::env::var_os("GDK_BACKEND").is_none() {
        let is_kde = std::env::var("XDG_CURRENT_DESKTOP")
            .map(|d| d.to_ascii_uppercase().contains("KDE"))
            .unwrap_or(false);
        let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE")
                .map(|t| t.eq_ignore_ascii_case("wayland"))
                .unwrap_or(false);
        if is_kde && is_wayland {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    configure_linux_env();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            appearance::spawn_watcher(app.handle().clone());
            watcher::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            appearance::get_appearance,
            commands::get_config,
            commands::set_config,
            commands::cached_repos,
            commands::scan_repos,
            commands::set_favorite,
            commands::open_in_ide,
            commands::open_agent,
            commands::enrich_repo,
            commands::github_login_start,
            commands::github_login_poll,
            commands::github_auth_status,
            commands::github_sign_out,
            commands::ai_status,
            commands::summarize_repo,
            commands::fetch_all,
            commands::fetch_repo,
            commands::list_branches,
            commands::switch_branch,
            commands::prune_branches,
            commands::list_worktrees,
            commands::add_worktree,
            commands::remove_worktree,
            commands::repo_log,
            commands::repo_diff,
            commands::repo_staged_diff,
            commands::repo_readme,
            commands::generate_commit_message,
            commands::commit_staged,
            commands::generate_changelog,
            commands::index_repos,
            commands::semantic_search,
            commands::daily_briefing,
            commands::get_inbox,
            commands::get_notifications,
            commands::ci_status,
            commands::list_starred,
            commands::clone_repo
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
