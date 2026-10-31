//! Best-effort filesystem live-watch. Calls `on_change` (debounced) with the
//! set of repos that changed on disk, so the UI rescans only those.
//!
//! What's watched, per discovered repo:
//!
//! - its **working tree**, one non-recursive watch per directory, skipping
//!   heavy build/dependency dirs ([`HEAVY_DIRS`]), gitignored dirs and nested
//!   repos. inotify is per-directory either way, so registering each kept
//!   directory ourselves (rather than `RecursiveMode::Recursive`, which would
//!   also descend into `target/` and `node_modules/`) keeps the watch count to
//!   the directories that hold tracked or trackable files. A new directory is
//!   picked up when it appears. Capped at [`MAX_DIRS_PER_REPO`] per repo;
//! - its **git dir**, non-recursively (`HEAD`, `index`, `packed-refs`), plus
//!   `refs/` recursively (branch, tag and remote-tracking updates). A `.git`
//!   *file* (linked worktree, submodule) is resolved through its `gitdir:`
//!   line, and a separate `commondir` (where a worktree's refs live) is
//!   watched too.
//!
//! Each configured **root** is also watched non-recursively, so a repo
//! appearing directly under it triggers repo discovery.
//!
//! Events are mapped back to the repo that owns them ([`classify`]) and
//! filtered: gitignored paths, heavy dirs and git's own bookkeeping
//! (`FETCH_HEAD`, `ORIG_HEAD`, `*.lock`, `gc.log` — see [`is_git_noise`]) never
//! cause a rescan, so Orrery's own background fetches stay quiet unless they
//! actually move a remote-tracking ref.
//!
//! The watch set is computed from the config at spawn and again on every
//! [`WatcherHandle::rearm`], so repos/roots added while the app runs (Settings
//! save, New Project, Explore clone) get live events without a restart.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{Error, ErrorKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, DebouncedEvent, Debouncer};
use orrery_core::config;
use orrery_core::model::AppConfig;
use orrery_core::scan::{self, expand};

/// Working-tree directories never watched (nor their events acted on): build
/// output and dependency trees that churn without changing anything a card
/// shows, and `.git` (watched separately, by name).
const HEAVY_DIRS: &[&str] = &["target", "node_modules", ".venv", "dist", "build", ".git"];

/// Upper bound on working-tree directories watched per repo, so one huge
/// checkout can't exhaust the inotify budget for everything else. Edits
/// deeper than the cap are still seen at the next git operation or rescan.
const MAX_DIRS_PER_REPO: usize = 2048;

/// The repos a burst of filesystem events touched.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// Something appeared directly under a configured root: re-run repo
    /// discovery (a full rescan).
    pub discover: bool,
    /// Changed repos by working-tree path; `true` when the working tree
    /// changed (not just git metadata), which also invalidates the repo's
    /// cached language.
    pub repos: BTreeMap<PathBuf, bool>,
}

impl ChangeSet {
    /// A full rescan request with no specific repos (the tray's "Rescan").
    pub fn discover() -> Self {
        ChangeSet {
            discover: true,
            repos: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.discover && self.repos.is_empty()
    }

    /// Fold `other` into `self` (coalescing a burst).
    pub fn merge(&mut self, other: ChangeSet) {
        self.discover |= other.discover;
        for (repo, worktree) in other.repos {
            self.touch(repo, worktree);
        }
    }

    fn touch(&mut self, repo: PathBuf, worktree: bool) {
        *self.repos.entry(repo).or_default() |= worktree;
    }
}

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

// ── What to watch ──────────────────────────────────────────────────────────

/// Where a repo's git metadata lives: its own git dir (`HEAD`, `index`) and
/// the common dir its refs live in (the same dir except for linked worktrees).
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitDirs {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

/// Resolve `repo`'s git dirs: `.git` itself when it's a directory, else the
/// `gitdir:` target of a `.git` file (linked worktree / submodule), with a
/// `commondir` file pointing at the shared refs. `None` if neither resolves.
fn resolve_git_dirs(repo: &Path) -> Option<GitDirs> {
    let dotgit = repo.join(".git");
    let git_dir = if dotgit.is_dir() {
        dotgit
    } else {
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        canonical(repo.join(target)) // `join` keeps an absolute target as-is
    };
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => canonical(git_dir.join(text.trim())),
        Err(_) => git_dir.clone(),
    };
    Some(GitDirs {
        git_dir,
        common_dir,
    })
}

/// Normalise a resolved path (drop `..` segments) so prefix matching against
/// event paths works; falls back to the path as given.
fn canonical(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// Which part of a repo an owned path belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    WorkTree,
    GitDir,
}

/// The watch plan for a config: what to register, and how to map an event
/// path back to its repo.
#[derive(Debug, Default)]
struct Plan {
    /// Configured roots (non-recursive; new top-level repos).
    roots: Vec<PathBuf>,
    /// Discovered repo working trees (walked into per-directory watches).
    repos: Vec<PathBuf>,
    /// Git-metadata watches.
    git_watches: Vec<(PathBuf, RecursiveMode)>,
    /// `(prefix, repo, area)`, longest prefix first, so a repo's `.git` (or a
    /// nested repo) wins over the working tree that contains it.
    owners: Vec<(PathBuf, PathBuf, Area)>,
}

fn plan(cfg: &AppConfig) -> Plan {
    let mut plan = Plan {
        roots: cfg.roots.iter().map(|root| expand(root)).collect(),
        ..Plan::default()
    };
    for repo in scan::repo_paths(&cfg.roots, cfg.scan_depth, &cfg.ignore) {
        plan.owners
            .push((repo.clone(), repo.clone(), Area::WorkTree));
        if let Some(dirs) = resolve_git_dirs(&repo) {
            plan.owners
                .push((dirs.git_dir.clone(), repo.clone(), Area::GitDir));
            plan.git_watches
                .push((dirs.git_dir.clone(), RecursiveMode::NonRecursive));
            if dirs.common_dir != dirs.git_dir {
                plan.owners
                    .push((dirs.common_dir.clone(), repo.clone(), Area::GitDir));
                plan.git_watches
                    .push((dirs.common_dir.clone(), RecursiveMode::NonRecursive));
            }
            let refs = dirs.common_dir.join("refs");
            if refs.is_dir() {
                plan.git_watches.push((refs, RecursiveMode::Recursive));
            }
        }
        plan.repos.push(repo);
    }
    plan.owners
        .sort_by_key(|(prefix, ..)| std::cmp::Reverse(prefix.components().count()));
    plan
}

/// Does `rel` (relative to a working tree) pass through a heavy dir?
fn in_heavy_dir(rel: &Path) -> bool {
    rel.components()
        .any(|c| HEAVY_DIRS.iter().any(|h| c.as_os_str() == *h))
}

/// Is `rel` (relative to a git dir) git's own bookkeeping rather than a state
/// change a card shows? `FETCH_HEAD` is rewritten by every fetch (including
/// Orrery's own background ones) even when nothing moved; `ORIG_HEAD`,
/// `*.lock` files and `gc.log` are transient. Reflogs and objects aren't
/// watched, but are filtered too in case a recursive watch reaches them.
fn is_git_noise(rel: &Path) -> bool {
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    matches!(name, "FETCH_HEAD" | "ORIG_HEAD" | "gc.log")
        || name.ends_with(".lock")
        || rel.starts_with("logs")
        || rel.starts_with("objects")
}

/// The repo (and area) that owns `path`, if any: the longest matching prefix.
/// Several repos can share a prefix (a linked worktree's refs live in its main
/// repo's common dir), so all owners of that prefix are returned.
fn owners_of<'a>(path: &Path, plan: &'a Plan) -> Vec<&'a (PathBuf, PathBuf, Area)> {
    let Some((prefix, ..)) = plan.owners.iter().find(|(p, ..)| path.starts_with(p)) else {
        return Vec::new();
    };
    plan.owners.iter().filter(|(p, ..)| p == prefix).collect()
}

/// Map one event path to its effect on `changes`: the owning repo (marking a
/// working-tree change), a discovery request for a new directory directly under
/// a root, or nothing (noise, ignored or heavy paths, files dropped in a root).
/// `is_ignored(repo, rel)` answers gitignore for a working-tree path.
fn classify(
    path: &Path,
    plan: &Plan,
    is_ignored: &mut dyn FnMut(&Path, &Path) -> bool,
    changes: &mut ChangeSet,
) {
    let owners = owners_of(path, plan);
    if owners.is_empty() {
        let under_root = path
            .parent()
            .is_some_and(|p| plan.roots.iter().any(|r| r == p));
        if under_root && path.is_dir() {
            changes.discover = true;
        }
        return;
    }
    for (prefix, repo, area) in owners {
        let Ok(rel) = path.strip_prefix(prefix) else {
            continue;
        };
        match area {
            Area::GitDir if is_git_noise(rel) => {}
            Area::GitDir => changes.touch(repo.clone(), false),
            // The repo dir itself (moved/deleted) or its `.git` entry.
            Area::WorkTree if rel.as_os_str().is_empty() || rel.starts_with(".git") => {
                changes.touch(repo.clone(), false)
            }
            Area::WorkTree if in_heavy_dir(rel) || is_ignored(repo, rel) => {}
            Area::WorkTree => changes.touch(repo.clone(), true),
        }
    }
}

/// The directories under `start` (inclusive) to watch for `repo`'s working
/// tree: skips heavy dirs, gitignored dirs, nested repos (they're their own
/// fleet entries or submodules) and symlinks; stops after `budget` dirs.
fn worktree_dirs(
    repo: &Path,
    start: &Path,
    is_ignored: &mut dyn FnMut(&Path, &Path) -> bool,
    budget: usize,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![start.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= budget {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        out.push(dir);
        for entry in entries.flatten() {
            // `file_type` doesn't follow symlinks, so linked dirs are skipped.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            if in_heavy_dir(rel) || path.join(".git").exists() || is_ignored(repo, rel) {
                continue;
            }
            stack.push(path);
        }
    }
    out
}

/// A per-batch gitignore oracle: opens each repo once and asks libgit2, so
/// nested `.gitignore`s, `info/exclude` and the global excludes all apply.
/// A repo that won't open ignores nothing.
#[derive(Default)]
struct IgnoreCache {
    repos: HashMap<PathBuf, Option<git2::Repository>>,
}

impl IgnoreCache {
    fn is_ignored(&mut self, repo: &Path, rel: &Path) -> bool {
        let handle = self
            .repos
            .entry(repo.to_path_buf())
            .or_insert_with(|| git2::Repository::open(repo).ok());
        handle
            .as_ref()
            .is_some_and(|r| r.is_path_ignored(rel).unwrap_or(false))
    }
}

// ── The watcher thread ─────────────────────────────────────────────────────

/// The live watch state: the plan, plus which working-tree dirs are watched.
struct Watching {
    debouncer: Debouncer<RecommendedWatcher>,
    plan: Plan,
    watched: HashSet<PathBuf>,
    dirs_per_repo: HashMap<PathBuf, usize>,
    /// inotify ran out of watches (ENOSPC); stop trying until a re-arm.
    exhausted: bool,
}

impl Watching {
    /// Start watching with `debouncer` (which holds no watches yet).
    fn new(debouncer: Debouncer<RecommendedWatcher>) -> Self {
        let mut w = Watching {
            debouncer,
            plan: Plan::default(),
            watched: HashSet::new(),
            dirs_per_repo: HashMap::new(),
            exhausted: false,
        };
        w.arm();
        w
    }

    /// Build the plan from the current config and register every watch on
    /// the (fresh) debouncer.
    fn arm(&mut self) {
        self.plan = plan(&config::load());
        self.watched.clear();
        self.dirs_per_repo.clear();
        self.exhausted = false;
        for root in self.plan.roots.clone() {
            self.watch(&root, RecursiveMode::NonRecursive);
        }
        for (path, mode) in self.plan.git_watches.clone() {
            self.watch(&path, mode);
        }
        let mut ignore = IgnoreCache::default();
        for repo in self.plan.repos.clone() {
            self.watch_worktree(&repo, &repo, &mut ignore);
        }
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> bool {
        if self.exhausted {
            return false;
        }
        match self.debouncer.watcher().watch(path, mode) {
            Ok(()) => true,
            Err(e) => {
                log_error(&format!("can't watch {}", path.display()), &e);
                // Out of inotify watches: every further attempt would fail
                // (and log) too, so stop until the next re-arm.
                self.exhausted = is_watch_limit(&e);
                false
            }
        }
    }

    /// Watch `start` and its kept subdirectories as part of `repo`'s working
    /// tree, within the repo's directory budget.
    fn watch_worktree(&mut self, repo: &Path, start: &Path, ignore: &mut IgnoreCache) {
        let used = self.dirs_per_repo.get(repo).copied().unwrap_or(0);
        let budget = MAX_DIRS_PER_REPO.saturating_sub(used);
        if budget == 0 || self.exhausted {
            return;
        }
        let dirs = worktree_dirs(repo, start, &mut |r, p| ignore.is_ignored(r, p), budget);
        if used + dirs.len() >= MAX_DIRS_PER_REPO {
            eprintln!(
                "[orrery watcher] {}: watching the first {MAX_DIRS_PER_REPO} directories only",
                repo.display()
            );
        }
        let mut added = 0;
        for dir in dirs {
            if self.watched.contains(&dir) {
                continue;
            }
            if self.watch(&dir, RecursiveMode::NonRecursive) {
                self.watched.insert(dir);
                added += 1;
            }
        }
        *self.dirs_per_repo.entry(repo.to_path_buf()).or_default() += added;
    }

    /// Map a debounced batch to a [`ChangeSet`], extending the watch set to
    /// directories created in a working tree (and forgetting deleted ones).
    fn handle(&mut self, events: &[DebouncedEvent]) -> ChangeSet {
        let mut ignore = IgnoreCache::default();
        let mut changes = ChangeSet::default();
        for event in events {
            let path = &event.path;
            classify(
                path,
                &self.plan,
                &mut |r, p| ignore.is_ignored(r, p),
                &mut changes,
            );
            if !path.exists() {
                // inotify drops a deleted dir's watch itself; forget it so a
                // re-created dir of the same name is watched again.
                self.watched.retain(|w| !w.starts_with(path));
                continue;
            }
            if !path.is_dir() || self.watched.contains(path) {
                continue;
            }
            let owner = owners_of(path, &self.plan)
                .into_iter()
                .find(|(_, _, area)| *area == Area::WorkTree)
                .map(|(_, repo, _)| repo.clone());
            if let Some(repo) = owner {
                let rel = path.strip_prefix(&repo).unwrap_or(path);
                if !in_heavy_dir(rel) && !ignore.is_ignored(&repo, rel) {
                    self.watch_worktree(&repo, path, &mut ignore);
                }
            }
        }
        changes
    }
}

/// Spawn the watcher thread. `on_change` fires on each debounced change batch
/// that touched a repo (the app rescans the repos in the [`ChangeSet`]). The
/// returned handle re-arms the watch set on demand; the thread stays alive
/// even with zero established watches so a later re-arm (e.g. the first root
/// added in Settings) can bring the watch up.
pub fn spawn(on_change: impl Fn(ChangeSet) + Send + 'static) -> WatcherHandle {
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
        let mut watching = match make_debouncer() {
            Ok(d) => Watching::new(d),
            Err(e) => {
                log_error("disabled", &e);
                return;
            }
        };

        // Keep the debouncer alive for the life of the thread; fire only on
        // batches that touch a repo (`Msg::Fs` also carries notify errors,
        // which we log but never rescan on, so a degraded watch can't spam
        // rescans). On re-arm, drop the debouncer — and with it every
        // existing watch — and register a fresh set from the current config:
        // recreating is simpler than diffing paths, and re-arms are rare
        // (explicit user actions). Exits on disconnect.
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Fs(Ok(events)) => {
                    let changes = watching.handle(&events);
                    if !changes.is_empty() {
                        on_change(changes);
                    }
                }
                Msg::Fs(Err(e)) => log_error("watch error", &e),
                Msg::Rearm => match make_debouncer() {
                    Ok(fresh) => {
                        // Assigning drops the old debouncer (and its watches)
                        // first, so a re-arm never needs twice the inotify
                        // budget.
                        watching.debouncer = fresh;
                        watching.arm();
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
    use std::fs;

    fn cfg_for(root: &Path) -> AppConfig {
        AppConfig {
            roots: vec![root.to_string_lossy().into_owned()],
            ..AppConfig::default()
        }
    }

    fn never_ignored(_: &Path, _: &Path) -> bool {
        false
    }

    /// Classify `paths` against `plan`, nothing gitignored.
    fn changes_for(plan: &Plan, paths: &[PathBuf]) -> ChangeSet {
        let mut changes = ChangeSet::default();
        for p in paths {
            classify(p, plan, &mut never_ignored, &mut changes);
        }
        changes
    }

    #[test]
    fn plan_covers_roots_repos_git_dirs_and_refs() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("proj");
        fs::create_dir_all(repo.join(".git/refs/heads")).unwrap();
        // A plain directory (no .git) is not a repo, so it gets no watch.
        fs::create_dir_all(root.path().join("notes")).unwrap();

        let plan = plan(&cfg_for(root.path()));
        assert_eq!(plan.roots, vec![root.path().to_path_buf()]);
        assert_eq!(plan.repos, vec![repo.clone()]);
        let watches: Vec<_> = plan.git_watches.iter().map(|(p, _)| p.clone()).collect();
        assert!(watches.contains(&repo.join(".git")));
        assert!(plan
            .git_watches
            .contains(&(repo.join(".git/refs"), RecursiveMode::Recursive)));
        assert!(!plan.repos.contains(&root.path().join("notes")));
    }

    #[test]
    fn plan_empty_config_yields_no_targets() {
        let cfg = AppConfig {
            roots: Vec::new(),
            ..AppConfig::default()
        };
        let plan = plan(&cfg);
        assert!(plan.roots.is_empty() && plan.repos.is_empty() && plan.owners.is_empty());
    }

    #[test]
    fn resolves_linked_worktree_git_file() {
        let tmp = tempfile::tempdir().unwrap();
        let main_git = tmp.path().join("main/.git");
        let wt_git = main_git.join("worktrees/feature");
        fs::create_dir_all(&wt_git).unwrap();
        fs::create_dir_all(main_git.join("refs")).unwrap();
        fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("feature");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();

        let dirs = resolve_git_dirs(&wt).unwrap();
        assert_eq!(dirs.git_dir, canonical(wt_git.clone()));
        assert_eq!(dirs.common_dir, canonical(main_git.clone()));

        // A relative `gitdir:` resolves against the worktree.
        fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/feature\n").unwrap();
        assert_eq!(resolve_git_dirs(&wt).unwrap().git_dir, canonical(wt_git));

        // A plain repo is its own common dir.
        let plain = resolve_git_dirs(&tmp.path().join("main")).unwrap();
        assert_eq!(plain.git_dir, main_git);
        assert_eq!(plain.common_dir, main_git);
        // No `.git` at all → nothing to watch.
        assert!(resolve_git_dirs(tmp.path()).is_none());
    }

    #[test]
    fn classify_maps_events_to_their_repo() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        fs::create_dir_all(a.join(".git/refs/heads")).unwrap();
        fs::create_dir_all(b.join(".git")).unwrap();
        let plan = plan(&cfg_for(root.path()));

        // Working-tree edit → that repo, flagged as a working-tree change.
        let c = changes_for(&plan, &[a.join("src/main.rs")]);
        assert_eq!(c.repos.get(&a), Some(&true));
        assert!(!c.discover && c.repos.len() == 1);

        // HEAD / index / ref updates → git-only change for the right repo.
        let c = changes_for(
            &plan,
            &[
                b.join(".git/HEAD"),
                b.join(".git/index"),
                a.join(".git/refs/heads/main"),
            ],
        );
        assert_eq!(c.repos.get(&a), Some(&false));
        assert_eq!(c.repos.get(&b), Some(&false));

        // A git-only and a working-tree change to one repo coalesce to "worktree".
        let c = changes_for(&plan, &[a.join(".git/index"), a.join("README.md")]);
        assert_eq!(c.repos.get(&a), Some(&true));
    }

    #[test]
    fn classify_drops_git_noise_and_heavy_dirs() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        fs::create_dir_all(a.join(".git/refs")).unwrap();
        let plan = plan(&cfg_for(root.path()));
        let noise = [
            a.join(".git/FETCH_HEAD"),
            a.join(".git/ORIG_HEAD"),
            a.join(".git/index.lock"),
            a.join(".git/gc.log"),
            a.join(".git/refs/remotes/origin/main.lock"),
            a.join("target/debug/app"),
            a.join("node_modules/x/index.js"),
            a.join("web/node_modules/y.js"),
            a.join(".venv/lib/site.py"),
            a.join("dist/bundle.js"),
            a.join("build/out.o"),
        ];
        assert!(changes_for(&plan, &noise).is_empty());
    }

    #[test]
    fn classify_honours_gitignore_oracle() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        fs::create_dir_all(a.join(".git")).unwrap();
        let plan = plan(&cfg_for(root.path()));
        let mut changes = ChangeSet::default();
        let mut ignore_logs = |_: &Path, rel: &Path| rel.extension().is_some_and(|e| e == "log");
        classify(&a.join("debug.log"), &plan, &mut ignore_logs, &mut changes);
        assert!(changes.is_empty());
        classify(&a.join("lib.rs"), &plan, &mut ignore_logs, &mut changes);
        assert_eq!(changes.repos.get(&a), Some(&true));
    }

    #[test]
    fn classify_discovers_new_dirs_under_a_root_only() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("a/.git")).unwrap();
        let plan = plan(&cfg_for(root.path()));
        // A file dropped in the root isn't a repo.
        let file = root.path().join("notes.txt");
        fs::write(&file, "").unwrap();
        assert!(changes_for(&plan, &[file]).is_empty());
        // A new directory might be one (a clone in progress) → rediscover.
        let fresh = root.path().join("fresh");
        fs::create_dir_all(&fresh).unwrap();
        assert!(changes_for(&plan, &[fresh]).discover);
        // Outside every root and repo → nothing.
        assert!(changes_for(&plan, &[PathBuf::from("/elsewhere/x")]).is_empty());
    }

    #[test]
    fn classify_prefers_the_innermost_repo() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("outer");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let mut plan = plan(&cfg_for(root.path()));
        // A repo nested in another's working tree (e.g. listed separately).
        let inner = outer.join("vendor/inner");
        plan.owners
            .push((inner.clone(), inner.clone(), Area::WorkTree));
        plan.owners
            .sort_by_key(|(p, ..)| std::cmp::Reverse(p.components().count()));
        let c = changes_for(&plan, &[inner.join("x.rs")]);
        assert_eq!(c.repos.keys().collect::<Vec<_>>(), vec![&inner]);
    }

    #[test]
    fn git_noise_filter() {
        for noisy in [
            "FETCH_HEAD",
            "ORIG_HEAD",
            "gc.log",
            "index.lock",
            "HEAD.lock",
            "packed-refs.lock",
            "refs/heads/main.lock",
            "logs/HEAD",
            "objects/ab/cdef",
        ] {
            assert!(is_git_noise(Path::new(noisy)), "{noisy}");
        }
        for real in ["HEAD", "index", "packed-refs", "refs/heads/main", "config"] {
            assert!(!is_git_noise(Path::new(real)), "{real}");
        }
    }

    #[test]
    fn ignore_cache_reads_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git2::Repository::init(repo).unwrap();
        fs::write(repo.join(".gitignore"), "*.log\nout/\n").unwrap();
        fs::create_dir_all(repo.join("out")).unwrap();
        let mut cache = IgnoreCache::default();
        assert!(cache.is_ignored(repo, Path::new("debug.log")));
        assert!(cache.is_ignored(repo, Path::new("out")));
        assert!(!cache.is_ignored(repo, Path::new("src/lib.rs")));
        // Not a repo → nothing is ignored.
        let other = tempfile::tempdir().unwrap();
        assert!(!cache.is_ignored(other.path(), Path::new("debug.log")));
    }

    #[test]
    fn worktree_dirs_skip_heavy_ignored_and_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git2::Repository::init(repo).unwrap();
        fs::write(repo.join(".gitignore"), "out/\n").unwrap();
        for d in [
            "src/a",
            "out/x",
            "target/debug",
            "node_modules/p",
            "vendor/sub",
        ] {
            fs::create_dir_all(repo.join(d)).unwrap();
        }
        fs::create_dir_all(repo.join("vendor/sub/.git")).unwrap();
        let mut cache = IgnoreCache::default();
        let mut dirs = worktree_dirs(repo, repo, &mut |r, p| cache.is_ignored(r, p), 100);
        dirs.sort();
        let rel: Vec<_> = dirs
            .iter()
            .map(|d| d.strip_prefix(repo).unwrap().to_path_buf())
            .collect();
        let want: Vec<PathBuf> = ["", "src", "src/a", "vendor"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(rel, want);
        // The budget caps the walk.
        assert_eq!(worktree_dirs(repo, repo, &mut never_ignored, 2).len(), 2);
    }

    #[test]
    fn change_sets_merge() {
        let mut a = ChangeSet::default();
        a.touch(PathBuf::from("/r1"), false);
        let mut b = ChangeSet::default();
        b.touch(PathBuf::from("/r1"), true);
        b.touch(PathBuf::from("/r2"), false);
        a.merge(b);
        assert_eq!(a.repos.get(Path::new("/r1")), Some(&true));
        assert_eq!(a.repos.get(Path::new("/r2")), Some(&false));
        assert!(!a.discover);
        a.merge(ChangeSet::discover());
        assert!(a.discover && !a.is_empty());
        assert!(ChangeSet::default().is_empty());
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
