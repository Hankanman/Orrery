//! Best-effort filesystem live-watch (#16). Emits a debounced `repos-changed`
//! event so the UI can rescan when repos change on disk.
//!
//! Rather than watching the roots *recursively* (which would add hundreds of
//! thousands of inotify watches across `node_modules` etc. and take many
//! seconds to establish), we watch a small, targeted set:
//!
//! - each configured **root**, non-recursively → catches new top-level repos;
//! - each discovered **repo root**, non-recursively → top-level file changes;
//! - each repo's **`.git`** dir, non-recursively → `index`/`HEAD` cover the
//!   high-value signals (staging, commits, branch switches, ahead/behind).
//!
//! This is ~2 watches per repo instead of one-per-directory-in-the-tree, so it
//! establishes instantly and stays quiet. Deep uncommitted edits aren't caught
//! until staged — an acceptable trade for a live-watch convenience. Degrades
//! silently if watches can't be established.

use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::Debouncer;
use tauri::{AppHandle, Emitter};

use crate::{config, scan};

fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

fn watch_one(debouncer: &mut Debouncer<RecommendedWatcher>, path: &std::path::Path) -> bool {
    debouncer
        .watcher()
        .watch(path, RecursiveMode::NonRecursive)
        .is_ok()
}

fn watch_targets(debouncer: &mut Debouncer<RecommendedWatcher>) -> usize {
    let cfg = config::load();
    let mut count = 0usize;

    // Configured roots → detect new top-level repos.
    for root in &cfg.roots {
        if watch_one(debouncer, &expand(root)) {
            count += 1;
        }
    }
    // Each repo's working root + .git → file changes and git operations.
    for repo in scan::repo_paths(&cfg.roots, cfg.scan_depth, &cfg.ignore) {
        if watch_one(debouncer, &repo) {
            count += 1;
        }
        let dotgit = repo.join(".git");
        if dotgit.is_dir() && watch_one(debouncer, &dotgit) {
            count += 1;
        }
    }
    count
}

/// Spawn the watcher thread.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut debouncer) = new_debouncer(Duration::from_millis(900), tx) else {
            return;
        };

        let watching = watch_targets(&mut debouncer);
        if watching == 0 {
            return;
        }

        // Keep `debouncer` alive for the life of the thread; emit on each batch.
        while rx.recv().is_ok() {
            let _ = app.emit("repos-changed", ());
        }
    });
}
