mod ai;
mod appearance;
mod cache;
mod cli;
mod commands;
mod config;
mod forge;
mod git_ops;
mod inbox;
mod krunner;
mod launch;
mod model;
mod notifier;
mod oauth;
mod scan;
mod search;
mod tray;
mod watcher;

/// Configure display/rendering environment on Linux before GTK/WebKit init.
/// Both vars are only set if the user hasn't already set them, so anyone can
/// override the behavior from the environment.
#[cfg(target_os = "linux")]
fn configure_linux_env() {
    // WebKitGTK's DMABUF renderer historically rendered blank/garbled on NVIDIA
    // (proprietary driver + older WebKitGTK), so we disable it by default — but
    // that also gives up GPU-accelerated compositing, which makes the UI judder.
    //
    // On modern stacks (NVIDIA *open* kernel module + recent WebKitGTK, with a
    // non-transparent window) the DMABUF renderer often works AND is far
    // smoother. Set ORRERY_WEBKIT_ACCEL=1 to keep it enabled and test that on
    // your machine; if the window renders fine, it's the real fix.
    let force_accel = std::env::var("ORRERY_WEBKIT_ACCEL").map(|v| v == "1").unwrap_or(false);
    if !force_accel && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
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
    // Headless CLI subcommands (orrery list/open/…) — handle and exit before
    // touching GUI env vars (so e.g. GDK_BACKEND doesn't leak to a CLI-spawned editor).
    if cli::maybe_run() {
        return;
    }

    #[cfg(target_os = "linux")]
    configure_linux_env();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        if let Some(w) = tauri::Manager::get_webview_window(app, "main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(),
        )
        .manage(commands::AgentSessions::default())
        .manage(commands::BulkCancel::default())
        .setup(|app| {
            appearance::spawn_watcher(app.handle().clone());
            watcher::spawn(app.handle().clone());
            krunner::spawn();
            let _ = tray::build(app.handle());
            // Poll GitHub for attention events and keep the tray glance fresh.
            notifier::spawn(app.handle().clone());

            // Close hides to the tray instead of quitting, so the background
            // poller keeps surfacing notifications. Quit via the tray menu.
            if let Some(window) = tauri::Manager::get_webview_window(app, "main") {
                let hide_target = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                });
            }
            // Global hotkey to summon Orrery (best-effort — may be unavailable
            // on native Wayland without a portal).
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            let _ = app.global_shortcut().register("CommandOrControl+Alt+O");

            // Keep WebKitGTK's accelerated compositor always on. By default it's
            // on-demand and tears down when no layer needs it, which on NVIDIA
            // shows as judder that vanishes the moment you open the inspector
            // (tauri-apps/tauri#10566). Forcing ALWAYS keeps it engaged.
            #[cfg(target_os = "linux")]
            if let Some(window) = tauri::Manager::get_webview_window(app, "main") {
                let _ = window.with_webview(|webview| {
                    use webkit2gtk::{HardwareAccelerationPolicy, SettingsExt, WebViewExt};
                    let wv = webview.inner();
                    if let Some(settings) = WebViewExt::settings(&wv) {
                        settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
                    }
                });
            }
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
            commands::open_folder,
            commands::enrich_repo,
            commands::github_login_start,
            commands::github_login_poll,
            commands::github_auth_status,
            commands::github_sign_out,
            commands::ai_status,
            commands::ai_test,
            commands::pull_model,
            commands::clear_ai_cache,
            commands::summarize_repo,
            commands::fetch_all,
            commands::fetch_repo,
            commands::list_branches,
            commands::switch_branch,
            commands::prune_branches,
            commands::prunable_branches,
            commands::list_worktrees,
            commands::add_worktree,
            commands::remove_worktree,
            commands::repo_log,
            commands::contribution_graph,
            commands::repo_diff,
            commands::repo_staged_diff,
            commands::repo_readme,
            commands::generate_commit_message,
            commands::commit_staged,
            commands::generate_changelog,
            commands::get_note,
            commands::set_note,
            commands::mark_seen,
            commands::resume_summary,
            commands::index_repos,
            commands::semantic_search,
            commands::daily_briefing,
            commands::get_inbox,
            commands::get_notifications,
            commands::ci_status,
            commands::list_starred,
            commands::pr_panel,
            commands::merge_pr,
            commands::approve_pr,
            commands::get_feed,
            commands::clone_repo,
            commands::init_repo,
            commands::active_agents,
            commands::list_agent_sessions,
            commands::kill_agent,
            commands::bulk_op,
            commands::cancel_bulk,
            commands::search_code,
            commands::notify
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
