//! Disk-usage janitor: find the build-artefact directories in a repo
//! (`target/`, `node_modules/`, virtualenvs, …), measure them, and purge the
//! ones the user picks.
//!
//! **Safety is the point of this module.** A directory only counts as an
//! artefact when *all* of these hold:
//!
//! - its name is a known artefact kind, and the kind's marker is present
//!   (`target/` needs a sibling `Cargo.toml`; a virtualenv needs `pyvenv.cfg`);
//! - git ignores it (`Repository::is_path_ignored`);
//! - no tracked file lives under it (a force-added file inside an ignored dir
//!   is still tracked content);
//! - it is a real directory inside the repo's working tree: discovery never
//!   follows symlinks, and [`purge_dirs`] re-checks every path (no symlink,
//!   canonical path inside the work tree, no `..`) before deleting anything.
//!
//! Sizing walks on scoped worker threads (the `scan`/`fleet` idiom), never
//! follows symlinks or crosses filesystems, counts hard-linked files once, and
//! is bounded by an entry budget and a deadline so a pathological tree can't
//! stall it; a cut-short measurement is flagged `truncated`.
//!
//! Results are cached in SQLite with a TTL (`cache::store_disk_usage`), since
//! walking a few gigabytes of `node_modules` is not free.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use git2::Repository;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::fleet::Outcome;

/// How long a cached measurement stays fresh. Artefact dirs grow on every
/// build, but a day-old size is plenty to decide what to purge.
pub const TTL_SECS: i64 = 24 * 3600;

/// How deep discovery descends into a work tree looking for artefact dirs —
/// deep enough for monorepo layouts (`packages/app/node_modules`).
const MAX_DEPTH: usize = 8;

/// A kind of build-artefact directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    /// Cargo's `target/` (only beside a `Cargo.toml`).
    CargoTarget,
    /// npm/pnpm/yarn `node_modules/`.
    NodeModules,
    /// A Python virtualenv: `.venv/` or `venv/` containing `pyvenv.cfg`.
    PythonVenv,
    /// A `dist/` output dir (only when gitignored, like every kind).
    Dist,
    /// A `build/` output dir (only when gitignored, like every kind).
    Build,
    /// Next.js's `.next/` cache.
    NextCache,
    /// Python bytecode `__pycache__/`.
    PyCache,
    /// Gradle's project-local `.gradle/` cache.
    Gradle,
}

impl ArtifactKind {
    pub const ALL: [ArtifactKind; 8] = [
        ArtifactKind::CargoTarget,
        ArtifactKind::NodeModules,
        ArtifactKind::PythonVenv,
        ArtifactKind::Dist,
        ArtifactKind::Build,
        ArtifactKind::NextCache,
        ArtifactKind::PyCache,
        ArtifactKind::Gradle,
    ];

    /// Short display label.
    pub fn label(self) -> &'static str {
        match self {
            ArtifactKind::CargoTarget => "Cargo target",
            ArtifactKind::NodeModules => "node_modules",
            ArtifactKind::PythonVenv => "Python venv",
            ArtifactKind::Dist => "dist",
            ArtifactKind::Build => "build",
            ArtifactKind::NextCache => "Next.js cache",
            ArtifactKind::PyCache => "__pycache__",
            ArtifactKind::Gradle => "Gradle cache",
        }
    }

    /// The kind of the directory at `dir` (named `name`), if its name and
    /// marker files say it's an artefact dir. Says nothing about git status.
    fn classify(dir: &Path, name: &str) -> Option<ArtifactKind> {
        match name {
            "target" => dir
                .parent()
                .is_some_and(|p| p.join("Cargo.toml").is_file())
                .then_some(ArtifactKind::CargoTarget),
            "node_modules" => Some(ArtifactKind::NodeModules),
            ".venv" | "venv" => dir
                .join("pyvenv.cfg")
                .is_file()
                .then_some(ArtifactKind::PythonVenv),
            "dist" => Some(ArtifactKind::Dist),
            "build" => Some(ArtifactKind::Build),
            ".next" => Some(ArtifactKind::NextCache),
            "__pycache__" => Some(ArtifactKind::PyCache),
            ".gradle" => Some(ArtifactKind::Gradle),
            _ => None,
        }
    }
}

/// One measured artefact directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDir {
    pub kind: ArtifactKind,
    /// Absolute path.
    pub path: String,
    /// Path relative to the repo's work tree (display + purge selection).
    pub rel: String,
    /// On-disk size (allocated blocks, hard links counted once).
    pub bytes: u64,
    /// Number of non-directory entries.
    pub files: u64,
    /// The walk hit its entry budget or deadline: `bytes` is a lower bound.
    pub truncated: bool,
}

/// A repo's artefact dirs and their sizes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    /// `Repo::id` (absolute path).
    pub repo_id: String,
    pub dirs: Vec<ArtifactDir>,
    pub total_bytes: u64,
    /// Any dir's walk was cut short (see [`ArtifactDir::truncated`]).
    pub truncated: bool,
    /// Unix seconds the measurement was taken.
    pub measured_at: i64,
}

impl DiskUsage {
    /// Total bytes of the dirs of `kind`.
    pub fn bytes_of(&self, kind: ArtifactKind) -> u64 {
        self.dirs
            .iter()
            .filter(|d| d.kind == kind)
            .map(|d| d.bytes)
            .sum()
    }
}

/// What [`purge`]/[`purge_dirs`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    /// Bytes actually released (measured before deletion; for a dir that
    /// failed part-way, before minus what's left).
    pub freed_bytes: u64,
    /// Absolute paths deleted.
    pub removed: Vec<String>,
    /// `(path, reason)` for each path that failed a safety check and was left
    /// untouched.
    pub refused: Vec<(String, String)>,
    /// `(path, error)` for each path whose deletion failed.
    pub failed: Vec<(String, String)>,
}

/// Bounds on one sizing walk.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Worker threads for the walk.
    pub workers: usize,
    /// Maximum entries visited across all dirs before stopping.
    pub max_entries: u64,
    /// Wall-clock budget for the whole walk.
    pub deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            workers: crate::fleet::default_workers(),
            max_entries: 2_000_000,
            deadline: Duration::from_secs(60),
        }
    }
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// The repo's work tree (canonical) and the sorted tracked paths in its index.
struct RepoView {
    repo: Repository,
    workdir: PathBuf,
    tracked: Vec<Vec<u8>>,
}

impl RepoView {
    fn open(path: &Path) -> Result<Self, String> {
        let repo = Repository::open(path).map_err(|e| e.to_string())?;
        let workdir = repo
            .workdir()
            .ok_or("bare repository has no work tree")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let mut tracked: Vec<Vec<u8>> = repo
            .index()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|e| e.path)
            .collect();
        tracked.sort();
        Ok(RepoView {
            repo,
            workdir,
            tracked,
        })
    }

    /// Whether any tracked file lives under the work-tree-relative dir `rel`.
    fn has_tracked_under(&self, rel: &Path) -> bool {
        let mut prefix = rel.as_os_str().as_encoded_bytes().to_vec();
        prefix.push(b'/');
        let i = self
            .tracked
            .partition_point(|p| p.as_slice() < prefix.as_slice());
        self.tracked.get(i).is_some_and(|p| p.starts_with(&prefix))
    }

    /// Whether git ignores the work-tree-relative dir `rel`.
    fn ignored(&self, rel: &Path) -> bool {
        // A trailing slash tells libgit2 it's a directory, so `dir/` patterns
        // match.
        let mut with_slash = rel.as_os_str().to_owned();
        with_slash.push("/");
        self.repo
            .is_path_ignored(Path::new(&with_slash))
            .unwrap_or(false)
    }

    /// Why the work-tree-relative dir `rel` (absolute `abs`) is *not* a
    /// purgeable artefact dir, or its kind when it is.
    fn check(&self, abs: &Path, rel: &Path) -> Result<ArtifactKind, String> {
        let name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("not a named directory")?;
        let kind =
            ArtifactKind::classify(abs, name).ok_or("not a known build-artefact directory")?;
        if !self.ignored(rel) {
            return Err("not ignored by git".into());
        }
        if self.has_tracked_under(rel) {
            return Err("contains tracked files".into());
        }
        Ok(kind)
    }
}

/// Find the artefact dirs in the repo at `repo_path` (see the module docs for
/// the rules). Never follows symlinks; doesn't descend into `.git`, nested
/// repos, found artefact dirs, or any other ignored directory.
pub fn find_artifacts(repo_path: &Path) -> Result<Vec<(ArtifactKind, PathBuf)>, String> {
    let view = RepoView::open(repo_path)?;
    Ok(find_in(&view))
}

fn find_in(view: &RepoView) -> Vec<(ArtifactKind, PathBuf)> {
    let mut found = Vec::new();
    let mut it = WalkDir::new(&view.workdir)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == ".git" || path.join(".git").exists() {
            // The repo's own git dir, or a nested repo/submodule/worktree.
            it.skip_current_dir();
            continue;
        }
        let Ok(rel) = path.strip_prefix(&view.workdir) else {
            it.skip_current_dir();
            continue;
        };
        match view.check(path, rel) {
            Ok(kind) => {
                found.push((kind, path.to_path_buf()));
                it.skip_current_dir();
            }
            // Some other ignored dir (a cache, a local data dir): not ours to
            // measure, and walking it would be wasted work.
            Err(_) if view.ignored(rel) => it.skip_current_dir(),
            Err(_) => {}
        }
    }
    found.sort();
    found
}

// ── Sizing ───────────────────────────────────────────────────────────────────

/// Size of one directory tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirSize {
    pub bytes: u64,
    pub files: u64,
    pub truncated: bool,
}

/// Measure every dir in `dirs` in parallel within `limits`. Results are in
/// input order. The top level of each dir is split into one task per child
/// directory, so one huge `node_modules` spreads across all workers.
pub fn measure_dirs(dirs: &[PathBuf], limits: Limits) -> Vec<DirSize> {
    let started = Instant::now();
    let bytes: Vec<AtomicU64> = dirs.iter().map(|_| AtomicU64::new(0)).collect();
    let files: Vec<AtomicU64> = dirs.iter().map(|_| AtomicU64::new(0)).collect();
    let truncated: Vec<AtomicBool> = dirs.iter().map(|_| AtomicBool::new(false)).collect();
    let entries = AtomicU64::new(0);
    // (dev, inode) of multiply-linked files already counted.
    let links: Mutex<HashSet<(u64, u64)>> = Mutex::new(HashSet::new());

    let count = |i: usize, md: &std::fs::Metadata| {
        if md.nlink() > 1
            && !md.is_dir()
            && !links
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((md.dev(), md.ino()))
        {
            return;
        }
        bytes[i].fetch_add(md.blocks() * 512, Ordering::Relaxed);
        if !md.is_dir() {
            files[i].fetch_add(1, Ordering::Relaxed);
        }
    };
    let over_budget = || {
        entries.fetch_add(1, Ordering::Relaxed) >= limits.max_entries
            || started.elapsed() > limits.deadline
    };

    // Top level, on this thread: count the dir and its direct entries, and
    // queue each child directory as a task.
    let mut tasks: Vec<(usize, PathBuf)> = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let Ok(md) = std::fs::symlink_metadata(dir) else {
            continue;
        };
        if !md.is_dir() {
            continue;
        }
        count(i, &md);
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for child in read.flatten() {
            if over_budget() {
                truncated[i].store(true, Ordering::Relaxed);
                break;
            }
            let Ok(cmd) = child.metadata() else { continue };
            if cmd.is_dir() && cmd.dev() == md.dev() {
                tasks.push((i, child.path()));
            } else if !cmd.is_dir() {
                count(i, &cmd);
            }
        }
    }

    let next = AtomicUsize::new(0);
    let workers = limits.workers.max(1).min(tasks.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let t = next.fetch_add(1, Ordering::Relaxed);
                let Some((i, path)) = tasks.get(t) else {
                    break;
                };
                for entry in WalkDir::new(path)
                    .follow_links(false)
                    .same_file_system(true)
                {
                    if over_budget() {
                        truncated[*i].store(true, Ordering::Relaxed);
                        break;
                    }
                    let Ok(entry) = entry else { continue };
                    if let Ok(md) = entry.metadata() {
                        count(*i, &md);
                    }
                }
            });
        }
    });

    (0..dirs.len())
        .map(|i| DirSize {
            bytes: bytes[i].load(Ordering::Relaxed),
            files: files[i].load(Ordering::Relaxed),
            truncated: truncated[i].load(Ordering::Relaxed),
        })
        .collect()
}

/// Find and measure the artefact dirs of the repo at `repo_path`.
pub fn measure(repo_path: &Path, limits: Limits, now: i64) -> Result<DiskUsage, String> {
    let view = RepoView::open(repo_path)?;
    let found = find_in(&view);
    let paths: Vec<PathBuf> = found.iter().map(|(_, p)| p.clone()).collect();
    let sizes = measure_dirs(&paths, limits);
    let dirs: Vec<ArtifactDir> = found
        .into_iter()
        .zip(sizes)
        .map(|((kind, path), size)| ArtifactDir {
            kind,
            rel: path
                .strip_prefix(&view.workdir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            path: path.to_string_lossy().into_owned(),
            bytes: size.bytes,
            files: size.files,
            truncated: size.truncated,
        })
        .collect();
    Ok(DiskUsage {
        repo_id: repo_path.to_string_lossy().into_owned(),
        total_bytes: dirs.iter().map(|d| d.bytes).sum(),
        truncated: dirs.iter().any(|d| d.truncated),
        dirs,
        measured_at: now,
    })
}

/// A `fleet::run` op: measure each repo (two walk workers per repo, since the
/// fleet already runs repos in parallel) and cache the result. Pair with
/// `cache::fresh_disk_usage_ids` to skip repos measured within [`TTL_SECS`].
pub fn measure_op(now: i64) -> impl Fn(&str) -> Outcome + Sync {
    move |path| {
        let limits = Limits {
            workers: 2,
            ..Limits::default()
        };
        match measure(Path::new(path), limits, now) {
            Ok(usage) => {
                let summary = format!(
                    "{} in {} artefact dir{}",
                    format_bytes(usage.total_bytes),
                    usage.dirs.len(),
                    if usage.dirs.len() == 1 { "" } else { "s" }
                );
                match crate::cache::store_disk_usage(&usage) {
                    Ok(()) => Outcome::Ok(summary),
                    Err(e) => Outcome::Failed(e),
                }
            }
            Err(e) => Outcome::Failed(e),
        }
    }
}

// ── Purge ────────────────────────────────────────────────────────────────────

/// Delete every artefact dir of the given `kinds` in the repo at `repo_path`,
/// returning what was freed. Each dir is re-validated right before deletion
/// (see [`purge_dirs`]).
pub fn purge(repo_path: &Path, kinds: &[ArtifactKind]) -> Result<PurgeReport, String> {
    let view = RepoView::open(repo_path)?;
    let paths: Vec<PathBuf> = find_in(&view)
        .into_iter()
        .filter(|(k, _)| kinds.contains(k))
        .map(|(_, p)| p)
        .collect();
    let report = purge_in(&view, &paths);
    crate::cache::clear_disk_usage(&repo_path.to_string_lossy());
    Ok(report)
}

/// Delete specific artefact dirs (absolute, or relative to the work tree) of
/// the repo at `repo_path`. Every path must pass all the safety checks — a
/// real directory (not a symlink) whose canonical path is inside the work
/// tree, a known artefact kind with its marker, ignored by git, with no
/// tracked files under it — or it is refused and left untouched.
pub fn purge_dirs(repo_path: &Path, paths: &[String]) -> Result<PurgeReport, String> {
    let view = RepoView::open(repo_path)?;
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let report = purge_in(&view, &paths);
    crate::cache::clear_disk_usage(&repo_path.to_string_lossy());
    Ok(report)
}

fn purge_in(view: &RepoView, paths: &[PathBuf]) -> PurgeReport {
    let mut report = PurgeReport::default();
    for path in paths {
        let shown = path.to_string_lossy().into_owned();
        let abs = match validate(view, path) {
            Ok(abs) => abs,
            Err(reason) => {
                report.refused.push((shown, reason));
                continue;
            }
        };
        let limits = Limits {
            max_entries: u64::MAX,
            deadline: Duration::from_secs(600),
            ..Limits::default()
        };
        let before = measure_dirs(std::slice::from_ref(&abs), limits)[0].bytes;
        match std::fs::remove_dir_all(&abs) {
            Ok(()) => {
                report.freed_bytes += before;
                report.removed.push(abs.to_string_lossy().into_owned());
            }
            Err(e) => {
                let left = measure_dirs(std::slice::from_ref(&abs), limits)[0].bytes;
                report.freed_bytes += before.saturating_sub(left);
                report
                    .failed
                    .push((abs.to_string_lossy().into_owned(), e.to_string()));
            }
        }
    }
    report
}

/// The safety gate in front of every deletion: the canonical absolute path of
/// `path` if it's a purgeable artefact dir of this repo, else why not.
fn validate(view: &RepoView, path: &Path) -> Result<PathBuf, String> {
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("path contains `.` or `..`".into());
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        view.workdir.join(path)
    };
    let md = std::fs::symlink_metadata(&joined).map_err(|e| e.to_string())?;
    if md.file_type().is_symlink() {
        return Err("is a symlink".into());
    }
    if !md.is_dir() {
        return Err("not a directory".into());
    }
    // Canonicalising resolves any symlinked *ancestor*, so a path that only
    // looks like it's in the repo is caught here.
    let abs = joined.canonicalize().map_err(|e| e.to_string())?;
    let rel = match abs.strip_prefix(&view.workdir) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => return Err("outside the repository".into()),
    };
    if rel.components().any(|c| c.as_os_str() == ".git") {
        return Err("inside the git directory".into());
    }
    view.check(&abs, &rel)?;
    Ok(abs)
}

/// "1.4 GB" / "512 KB" / "12 B", base-1000 like most file managers.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::test_support::init_repo;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn commit_all(path: &str, files: &[&str]) {
        let repo = Repository::open(path).unwrap();
        let mut index = repo.index().unwrap();
        for f in files {
            index.add_path(Path::new(f)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add", &tree, &[&parent])
            .unwrap();
    }

    /// A repo with a Cargo `target/`, a `node_modules/`, a venv, a
    /// `__pycache__`, all gitignored, plus unignored `build/` source.
    fn fixture() -> (tempfile::TempDir, String) {
        let (dir, path) = init_repo();
        let root = dir.path();
        fs::write(
            root.join(".gitignore"),
            "target/\nnode_modules/\n.venv/\n__pycache__/\n",
        )
        .unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        commit_all(&path, &[".gitignore", "Cargo.toml"]);
        write(&root.join("target/debug/app"), 5000);
        write(&root.join("node_modules/pkg/index.js"), 3000);
        write(&root.join("node_modules/pkg/lib/a.js"), 3000);
        write(&root.join(".venv/pyvenv.cfg"), 10);
        write(&root.join("src/__pycache__/m.pyc"), 100);
        // Not ignored: source, never an artefact.
        write(&root.join("build/script.sh"), 100);
        (dir, path)
    }

    #[test]
    fn finds_only_ignored_artifact_dirs_with_markers() {
        let (dir, path) = fixture();
        // A `target/` without Cargo.toml beside it isn't Cargo's.
        write(&dir.path().join("docs/target/x"), 10);
        let found = find_artifacts(Path::new(&path)).unwrap();
        let rels: Vec<(ArtifactKind, String)> = found
            .iter()
            .map(|(k, p)| {
                let rel = p.strip_prefix(dir.path().canonicalize().unwrap()).unwrap();
                (*k, rel.to_string_lossy().into_owned())
            })
            .collect();
        assert_eq!(
            rels,
            vec![
                (ArtifactKind::CargoTarget, "target".into()),
                (ArtifactKind::NodeModules, "node_modules".into()),
                (ArtifactKind::PythonVenv, ".venv".into()),
                (ArtifactKind::PyCache, "src/__pycache__".into()),
            ]
        );
    }

    #[test]
    fn measure_sums_sizes_per_dir() {
        let (_dir, path) = fixture();
        let usage = measure(Path::new(&path), Limits::default(), 42).unwrap();
        assert_eq!(usage.measured_at, 42);
        assert_eq!(usage.dirs.len(), 4);
        assert!(!usage.truncated);
        let nm = usage
            .dirs
            .iter()
            .find(|d| d.kind == ArtifactKind::NodeModules)
            .unwrap();
        assert_eq!(nm.rel, "node_modules");
        assert_eq!(nm.files, 2);
        assert!(nm.bytes >= 6000, "{}", nm.bytes);
        assert_eq!(
            usage.total_bytes,
            usage.dirs.iter().map(|d| d.bytes).sum::<u64>()
        );
    }

    #[test]
    fn measure_is_bounded_by_the_entry_budget() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            write(&dir.path().join(format!("d{}/f{i}", i % 5)), 10);
        }
        let limits = Limits {
            max_entries: 10,
            ..Limits::default()
        };
        let size = measure_dirs(&[dir.path().to_path_buf()], limits)[0];
        assert!(size.truncated);
        assert!(size.files < 50);
        let full = measure_dirs(&[dir.path().to_path_buf()], Limits::default())[0];
        assert!(!full.truncated);
        assert_eq!(full.files, 50);
    }

    #[test]
    fn measure_counts_hard_links_once_and_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("big"), 100_000);
        write(&dir.path().join("a/one"), 4096);
        fs::hard_link(dir.path().join("a/one"), dir.path().join("a/two")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("a/link")).unwrap();
        let size = measure_dirs(&[dir.path().to_path_buf()], Limits::default())[0];
        assert!(size.bytes < 100_000, "followed a symlink: {}", size.bytes);
        // `one` + the symlink itself; `two` is the same inode as `one`.
        assert_eq!(size.files, 2);
        let single = measure_dirs(&[dir.path().join("a/one")], Limits::default());
        // `a/one` is a file, not a dir: nothing measured.
        assert_eq!(single[0], DirSize::default());
    }

    #[test]
    fn purge_deletes_selected_kinds_and_reports_bytes() {
        let (dir, path) = fixture();
        let report = purge(
            Path::new(&path),
            &[ArtifactKind::NodeModules, ArtifactKind::PyCache],
        )
        .unwrap();
        assert_eq!(report.removed.len(), 2, "{report:?}");
        assert!(report.refused.is_empty() && report.failed.is_empty());
        assert!(report.freed_bytes >= 6100, "{}", report.freed_bytes);
        assert!(!dir.path().join("node_modules").exists());
        assert!(!dir.path().join("src/__pycache__").exists());
        // Everything else is untouched.
        assert!(dir.path().join("target/debug/app").exists());
        assert!(dir.path().join("src").exists());
        assert!(dir.path().join("build/script.sh").exists());
    }

    #[test]
    fn purge_refuses_tracked_dir() {
        let (dir, path) = init_repo();
        // `dist/` is ignored, but a file in it was force-added: tracked content.
        fs::write(dir.path().join(".gitignore"), "dist/\n").unwrap();
        write(&dir.path().join("dist/keep.txt"), 10);
        commit_all(&path, &[".gitignore", "dist/keep.txt"]);
        write(&dir.path().join("dist/junk.js"), 10);

        assert!(find_artifacts(Path::new(&path)).unwrap().is_empty());
        let report = purge_dirs(Path::new(&path), &["dist".into()]).unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.refused.len(), 1);
        assert_eq!(report.refused[0].1, "contains tracked files");
        assert!(dir.path().join("dist/keep.txt").exists());
    }

    #[test]
    fn purge_refuses_unignored_dir() {
        let (dir, path) = init_repo();
        write(&dir.path().join("node_modules/x.js"), 10);
        let report = purge_dirs(Path::new(&path), &["node_modules".into()]).unwrap();
        assert_eq!(report.refused[0].1, "not ignored by git");
        assert!(dir.path().join("node_modules/x.js").exists());
    }

    #[test]
    fn purge_refuses_symlink_escape() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join(".gitignore"), "node_modules\nvendor/\n").unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("precious.txt"), 10);
        write(&outside.path().join("node_modules/precious.js"), 10);

        // `node_modules` itself is a symlink pointing outside the repo.
        std::os::unix::fs::symlink(outside.path(), dir.path().join("node_modules")).unwrap();
        // A symlinked *ancestor* (`vendor -> outside`) reaching a real dir.
        std::os::unix::fs::symlink(outside.path(), dir.path().join("vendor")).unwrap();

        assert!(find_artifacts(Path::new(&path)).unwrap().is_empty());
        let report = purge(Path::new(&path), &ArtifactKind::ALL).unwrap();
        assert!(report.removed.is_empty(), "{report:?}");

        let report = purge_dirs(
            Path::new(&path),
            &[
                "node_modules".into(),
                "vendor/node_modules".into(),
                outside.path().join("node_modules").to_string_lossy().into(),
                "../outside/node_modules".into(),
            ],
        )
        .unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        let reasons: Vec<&str> = report.refused.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            reasons,
            vec![
                "is a symlink",
                "outside the repository",
                "outside the repository",
                "path contains `.` or `..`",
            ]
        );
        assert!(outside.path().join("precious.txt").exists());
        assert!(outside.path().join("node_modules/precious.js").exists());
    }

    #[test]
    fn purge_refuses_unknown_dirs_and_the_repo_itself() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join(".gitignore"), "secrets/\n").unwrap();
        write(&dir.path().join("secrets/key"), 10);
        let report = purge_dirs(Path::new(&path), &["secrets".into(), path.clone()]).unwrap();
        let reasons: Vec<&str> = report.refused.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            reasons,
            vec![
                "not a known build-artefact directory",
                "outside the repository"
            ]
        );
        assert!(dir.path().join("secrets/key").exists());
    }

    #[test]
    fn format_bytes_reads_naturally() {
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(1_400_000_000), "1.4 GB");
        assert_eq!(format_bytes(512_000), "512 KB");
    }
}
