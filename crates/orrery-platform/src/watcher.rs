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
//! and stays quiet. Degrades gracefully (logging to stderr) if watches can't be
//! established.
//!
//! The watch set is computed from the config at spawn and again on every
//! [`WatcherHandle::rearm`], so repos/roots added while the app runs (Settings
//! save, New Project, Explore clone) get live events without a restart.

use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{Error, ErrorKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer};
use orrery_core::config;
use orrery_core::model::AppConfig;
use orrery_core::scan::{self, expand};

/// What the watcher thread reacts to: a debounced fs change batch from the
/// debouncer, or a re-arm request from the app.
enum Msg {
    Fs(DebounceEventResult),
    Rearm,
}

/// Handle to the running watcher thread. Cheap to clone.
#[derive(Clone)]
pub struct WatcherHandle {
    tx: std::sync::mpsc::Sender<Msg>,
}

impl WatcherHandle {
    /// Ask the watcher thread to reload the config, recompute its targets and
    /// re-establish its watches. Call after anything that adds repos or roots
    /// at runtime. No-op if the thread is gone.
    pub fn rearm(&self) {
        let _ = self.tx.send(Msg::Rearm);
    }
}

fn watch_one(debouncer: &mut Debouncer<RecommendedWatcher>, path: &std::path::Path) -> bool {
    match debouncer.watcher().watch(path, RecursiveMode::NonRecursive) {
        Ok(()) => true,
        Err(e) => {
            log_error(&format!("can't watch {}", path.display()), &e);
            false
        }
    }
}

/// True when a notify error is inotify running out of watches (ENOSPC), which
/// notify reports as `MaxFilesWatch` (or, from some paths, a raw io error).
fn is_watch_limit(e: &Error) -> bool {
    match &e.kind {
        ErrorKind::MaxFilesWatch => true,
        ErrorKind::Io(io) => io.raw_os_error() == Some(28), // ENOSPC
        _ => false,
    }
}

/// Log a watch error to stderr, with a remediation hint for the inotify limit.
fn log_error(context: &str, e: &Error) {
    if is_watch_limit(e) {
        eprintln!(
            "[orrery watcher] {context}: inotify watch limit reached (ENOSPC); \
             raise it with `sysctl fs.inotify.max_user_watches=524288` \
             (persist in /etc/sysctl.d/)"
        );
    } else {
        eprintln!("[orrery watcher] {context}: {e}");
    }
}

/// The paths the watcher registers for a config: each configured root, each
/// discovered repo root, and each repo's `.git` dir (all non-recursive).
fn target_paths(cfg: &AppConfig) -> Vec<PathBuf> {
    // Configured roots → detect new top-level repos.
    let mut paths: Vec<PathBuf> = cfg.roots.iter().map(|root| expand(root)).collect();
    // Each repo's working root + .git → file changes and git operations.
    for repo in scan::repo_paths(&cfg.roots, cfg.scan_depth, &cfg.ignore) {
        let dotgit = repo.join(".git");
        paths.push(repo);
        if dotgit.is_dir() {
            paths.push(dotgit);
        }
    }
    paths
}

fn watch_targets(debouncer: &mut Debouncer<RecommendedWatcher>) -> usize {
    target_paths(&config::load())
        .iter()
        .filter(|path| watch_one(debouncer, path))
        .count()
}

/// Spawn the watcher thread. `on_change` fires on each debounced change batch
/// (the app rescans). The returned handle re-arms the watch set on demand; the
/// thread stays alive even with zero established watches so a later re-arm
/// (e.g. the first root added in Settings) can bring the watch up.
pub fn spawn(on_change: impl Fn() + Send + 'static) -> WatcherHandle {
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let handle = WatcherHandle { tx: tx.clone() };
    std::thread::spawn(move || {
        // The debouncer forwards fs batches onto the same channel re-arm
        // requests arrive on, so one recv loop serves both.
        let make_debouncer = || {
            let tx = tx.clone();
            new_debouncer(
                Duration::from_millis(900),
                move |res: DebounceEventResult| {
                    let _ = tx.send(Msg::Fs(res));
                },
            )
        };
        let mut debouncer = match make_debouncer() {
            Ok(d) => d,
            Err(e) => {
                log_error("disabled", &e);
                return;
            }
        };
        watch_targets(&mut debouncer);

        // Keep `debouncer` alive for the life of the thread; fire only on real
        // change batches (`Msg::Fs` also carries notify errors, which we log
        // but never rescan on, so a degraded watch can't spam rescans). On re-arm, drop the debouncer
        // — and with it every existing watch — and register a fresh set from
        // the current config: recreating is simpler than diffing paths, and
        // re-arms are rare (explicit user actions). Exits on disconnect.
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Fs(Ok(_events)) => on_change(),
                Msg::Fs(Err(e)) => log_error("watch error", &e),
                Msg::Rearm => match make_debouncer() {
                    Ok(fresh) => {
                        debouncer = fresh;
                        watch_targets(&mut debouncer);
                    }
                    Err(e) => log_error("re-arm failed", &e),
                },
            }
        }
    });
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_paths_cover_roots_repos_and_dotgit() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // A plain directory (no .git) is not a repo, so it gets no watch.
        std::fs::create_dir_all(root.path().join("notes")).unwrap();

        let cfg = AppConfig {
            roots: vec![root.path().to_string_lossy().into_owned()],
            ..AppConfig::default()
        };
        let paths = target_paths(&cfg);
        assert!(paths.contains(&root.path().to_path_buf()));
        assert!(paths.contains(&repo));
        assert!(paths.contains(&repo.join(".git")));
        assert!(!paths.contains(&root.path().join("notes")));
    }

    #[test]
    fn target_paths_empty_config_yields_no_targets() {
        let cfg = AppConfig {
            roots: Vec::new(),
            ..AppConfig::default()
        };
        assert!(target_paths(&cfg).is_empty());
    }

    #[test]
    fn watch_limit_detects_enospc() {
        assert!(is_watch_limit(&Error::new(ErrorKind::MaxFilesWatch)));
        assert!(is_watch_limit(&Error::io(
            std::io::Error::from_raw_os_error(28)
        )));
        assert!(!is_watch_limit(&Error::generic("other")));
        assert!(!is_watch_limit(&Error::io(
            std::io::Error::from_raw_os_error(2)
        )));
    }
}
