//! Best-effort filesystem live-watch. Calls `on_change` (debounced) whenever
//! watched repos change on disk, so the UI can rescan.
//!
//! Rather than watching the roots *recursively* (hundreds of thousands of
//! inotify watches across `node_modules` etc.), we watch a small, targeted set:
//!
//! - each configured **root**, non-recursively → new top-level repos;
//! - each discovered **repo root**, non-recursively → top-level file changes;
//! - each repo's **`.git`** dir, non-recursively → `index`/`HEAD` cover the
//!   high-value signals (staging, commits, branch switches, ahead/behind).
//!
//! ~2 watches per repo instead of one-per-directory, so it establishes instantly
//! and stays quiet. Degrades silently if watches can't be established.

use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::Debouncer;
use orrery_core::config;
use orrery_core::scan::{self, expand};

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

/// Spawn the watcher thread. `on_change` fires on each debounced change batch
/// (the Tauri app emits a webview event; the native app rescans). No-ops if no
/// watch could be established.
pub fn spawn(on_change: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut debouncer) = new_debouncer(Duration::from_millis(900), tx) else {
            return;
        };

        if watch_targets(&mut debouncer) == 0 {
            return;
        }

        // Keep `debouncer` alive for the life of the thread; fire only on real
        // change batches (the channel also carries notify errors, which we
        // ignore so a degraded watch can't spam rescans). Exits on disconnect.
        while let Ok(Ok(_events)) = rx.recv() {
            on_change();
        }
    });
}
