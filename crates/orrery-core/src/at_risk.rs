//! At-risk work detection: the local work that exists only on this disk, or
//! has been parked so long it's likely forgotten.
//!
//! Per repo, [`assess`] reports:
//!
//! - local branches never pushed: no upstream and commits on no remote
//!   branch, or ahead of their upstream;
//! - stashes older than a threshold;
//! - a working tree dirty for longer than a threshold (from the first-seen-
//!   dirty timestamp the cache keeps on every snapshot write, see
//!   `cache::dirty_since_all`);
//! - no remote at all;
//! - a detached HEAD whose commits aren't on any branch.
//!
//! [`assess_fleet`] runs it across a snapshot; the reports feed the attention
//! model through `attention::compute_with` (`Hygiene::at_risk`).

use std::collections::HashMap;
use std::path::Path;

use git2::{BranchType, Oid, Repository};
use serde::{Deserialize, Serialize};

use crate::model::Repo;

const DAY: i64 = 24 * 3600;

/// Cap on commits counted per branch walk: enough to say "lots", bounded so a
/// huge history can't make a fleet pass slow.
const WALK_CAP: u32 = 1000;

/// Configurable thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Thresholds {
    /// A stash older than this many days is at risk of being forgotten.
    pub stash_days: u32,
    /// A working tree dirty for this many days is at risk.
    pub dirty_days: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            stash_days: 30,
            dirty_days: 7,
        }
    }
}

/// A local branch with commits that aren't on its remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpushedBranch {
    pub name: String,
    /// The upstream it's ahead of, or `None` if it was never pushed.
    pub upstream: Option<String>,
    /// Commits not on the upstream (or, with no upstream, on no remote
    /// branch). Capped at 1000.
    pub commits: u32,
}

/// A stash past the age threshold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OldStash {
    /// `stash@{index}`.
    pub index: usize,
    pub message: String,
    /// Unix seconds the stash was made.
    pub created_unix: i64,
    pub age_days: u32,
}

/// One repo's at-risk report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtRisk {
    /// `Repo::id`.
    pub repo_id: String,
    pub unpushed_branches: Vec<UnpushedBranch>,
    pub old_stashes: Vec<OldStash>,
    /// When the working tree was first seen dirty, set only when that's past
    /// the threshold.
    pub dirty_since: Option<i64>,
    /// Whole days dirty, alongside `dirty_since`.
    pub dirty_days: Option<u32>,
    /// The repository has no remote: nothing in it is backed up.
    pub no_remote: bool,
    /// Commits reachable from a detached HEAD but from no branch (local or
    /// remote). Capped at 1000.
    pub orphan_commits: u32,
}

impl AtRisk {
    /// Nothing at risk.
    pub fn is_empty(&self) -> bool {
        self.unpushed_branches.is_empty()
            && self.old_stashes.is_empty()
            && self.dirty_since.is_none()
            && !self.no_remote
            && self.orphan_commits == 0
    }
}

/// Assess the repo at `path`. `dirty_since` is the cached first-seen-dirty
/// time (`None` when clean). `shared_state` covers the checks on state a
/// linked worktree shares with its main checkout (branches, stashes,
/// remotes); pass `false` for a worktree so they aren't reported twice.
pub fn assess(
    path: &Path,
    dirty_since: Option<i64>,
    thresholds: &Thresholds,
    shared_state: bool,
    now: i64,
) -> Result<AtRisk, String> {
    let mut repo = Repository::open(path).map_err(|e| e.to_string())?;
    let mut report = AtRisk {
        repo_id: path.to_string_lossy().into_owned(),
        ..AtRisk::default()
    };

    if let Some(since) = dirty_since {
        let days = now.saturating_sub(since) / DAY;
        if days >= i64::from(thresholds.dirty_days) {
            report.dirty_since = Some(since);
            report.dirty_days = Some(days as u32);
        }
    }

    let remote_tips = remote_tips(&repo);
    if repo.head_detached().unwrap_or(false) {
        if let Some(head) = repo.head().ok().and_then(|h| h.target()) {
            let mut hide = remote_tips.clone();
            hide.extend(local_tips(&repo));
            report.orphan_commits = count_unique(&repo, head, &hide);
        }
    }

    if !shared_state {
        return Ok(report);
    }

    let has_remote = repo.remotes().map(|r| !r.is_empty()).unwrap_or(false);
    report.no_remote = !has_remote && repo.head().is_ok();
    if has_remote {
        report.unpushed_branches = unpushed_branches(&repo, &remote_tips);
    }
    report.old_stashes = old_stashes(&mut repo, thresholds, now);
    Ok(report)
}

/// Tips of every remote-tracking branch (symbolic refs like `origin/HEAD`
/// are skipped; their targets are listed anyway).
fn remote_tips(repo: &Repository) -> Vec<Oid> {
    refs_tips(repo, BranchType::Remote)
}

fn local_tips(repo: &Repository) -> Vec<Oid> {
    refs_tips(repo, BranchType::Local)
}

fn refs_tips(repo: &Repository, kind: BranchType) -> Vec<Oid> {
    let Ok(branches) = repo.branches(Some(kind)) else {
        return Vec::new();
    };
    branches
        .flatten()
        .filter_map(|(b, _)| b.get().target())
        .collect()
}

/// Commits reachable from `tip` but from none of `hide`, capped.
fn count_unique(repo: &Repository, tip: Oid, hide: &[Oid]) -> u32 {
    let Ok(mut walk) = repo.revwalk() else {
        return 0;
    };
    if walk.push(tip).is_err() {
        return 0;
    }
    for oid in hide {
        let _ = walk.hide(*oid);
    }
    walk.take(WALK_CAP as usize).filter(|r| r.is_ok()).count() as u32
}

fn unpushed_branches(repo: &Repository, remote_tips: &[Oid]) -> Vec<UnpushedBranch> {
    let Ok(branches) = repo.branches(Some(BranchType::Local)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (branch, _) in branches.flatten() {
        let Some(name) = branch.name().ok().flatten().map(String::from) else {
            continue;
        };
        let Some(tip) = branch.get().target() else {
            continue;
        };
        let entry = match branch.upstream() {
            Ok(up) => {
                let upstream = up.name().ok().flatten().map(String::from);
                let ahead = up
                    .get()
                    .target()
                    .and_then(|u| repo.graph_ahead_behind(tip, u).ok())
                    .map(|(a, _)| (a as u32).min(WALK_CAP))
                    .unwrap_or(0);
                (ahead > 0).then_some(UnpushedBranch {
                    name,
                    upstream,
                    commits: ahead,
                })
            }
            // No upstream (or a gone one): at risk only if some of its
            // commits are on no remote branch at all. A local branch sitting
            // on an already-pushed commit has nothing to lose.
            Err(_) => {
                let commits = count_unique(repo, tip, remote_tips);
                (commits > 0).then_some(UnpushedBranch {
                    name,
                    upstream: None,
                    commits,
                })
            }
        };
        out.extend(entry);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn old_stashes(repo: &mut Repository, thresholds: &Thresholds, now: i64) -> Vec<OldStash> {
    let mut stashes: Vec<(usize, String, Oid)> = Vec::new();
    let _ = repo.stash_foreach(|index, message, oid| {
        stashes.push((index, message.to_string(), *oid));
        true
    });
    stashes
        .into_iter()
        .filter_map(|(index, message, oid)| {
            let created_unix = repo.find_commit(oid).ok()?.time().seconds();
            let days = now.saturating_sub(created_unix) / DAY;
            (days >= i64::from(thresholds.stash_days)).then_some(OldStash {
                index,
                message,
                created_unix,
                age_days: days as u32,
            })
        })
        .collect()
}

/// [`assess`] every repo in `repos` (on scoped worker threads), using the
/// cached first-seen-dirty times. Returns only non-empty reports, sorted by
/// repo id; repos that fail to open are skipped. Linked worktrees
/// (`worktree_of` set) get only the per-checkout checks.
pub fn assess_fleet(repos: &[Repo], thresholds: &Thresholds, now: i64) -> Vec<AtRisk> {
    assess_fleet_with(repos, &crate::cache::dirty_since_all(), thresholds, now)
}

/// [`assess_fleet`] with an explicit dirty-since map (testable without the
/// on-disk cache).
pub fn assess_fleet_with(
    repos: &[Repo],
    dirty_since: &HashMap<String, i64>,
    thresholds: &Thresholds,
    now: i64,
) -> Vec<AtRisk> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let out = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let workers = crate::fleet::default_workers().min(repos.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(r) = repos.get(i) else { break };
                let since = (r.git.dirty > 0)
                    .then(|| dirty_since.get(&r.id).copied())
                    .flatten();
                let shared = r.worktree_of.is_none();
                if let Ok(mut report) = assess(Path::new(&r.id), since, thresholds, shared, now) {
                    if !report.is_empty() {
                        report.repo_id = r.id.clone();
                        out.lock().unwrap_or_else(|e| e.into_inner()).push(report);
                    }
                }
            });
        }
    });
    let mut out = out.into_inner().unwrap_or_else(|e| e.into_inner());
    out.sort_by(|a, b| a.repo_id.cmp(&b.repo_id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::test_support::init_repo;
    use std::fs;

    fn commit(repo: &Repository, file: &str, msg: &str, time: i64) -> Oid {
        let workdir = repo.workdir().unwrap();
        fs::write(workdir.join(file), msg).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::new("t", "t@t", &git2::Time::new(time, 0)).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap()
    }

    /// A repo with an `origin` remote whose `origin/main` is at the first
    /// commit, and `main` tracking it.
    fn with_origin() -> (tempfile::TempDir, String, Repository) {
        let (dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        repo.remote("origin", "https://example.com/o/r.git")
            .unwrap();
        let head = repo.head().unwrap().target().unwrap();
        let main = repo.head().unwrap().shorthand().unwrap().to_string();
        repo.reference(&format!("refs/remotes/origin/{main}"), head, true, "test")
            .unwrap();
        repo.find_branch(&main, BranchType::Local)
            .unwrap()
            .set_upstream(Some(&format!("origin/{main}")))
            .unwrap();
        (dir, path, repo)
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn clean_pushed_repo_is_not_at_risk() {
        let (_dir, path, _repo) = with_origin();
        let r = assess(Path::new(&path), None, &Thresholds::default(), true, NOW).unwrap();
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn no_remote_is_reported() {
        let (_dir, path) = init_repo();
        let r = assess(Path::new(&path), None, &Thresholds::default(), true, NOW).unwrap();
        assert!(r.no_remote);
        // Branch checks need a remote to compare against; `no_remote` covers it.
        assert!(r.unpushed_branches.is_empty());
    }

    #[test]
    fn unpushed_and_ahead_branches_are_reported() {
        let (_dir, path, repo) = with_origin();
        // main: one commit ahead of origin/main.
        commit(&repo, "a.txt", "ahead", NOW);
        let main = repo.head().unwrap().shorthand().unwrap().to_string();
        // feature: never pushed, two commits of its own.
        let tip = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &tip, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        commit(&repo, "b.txt", "f1", NOW);
        commit(&repo, "c.txt", "f2", NOW);
        // parked: no upstream, but sits on an already-pushed commit: safe.
        let pushed = repo
            .find_reference(&format!("refs/remotes/origin/{main}"))
            .unwrap()
            .peel_to_commit()
            .unwrap();
        repo.branch("parked", &pushed, false).unwrap();

        let r = assess(Path::new(&path), None, &Thresholds::default(), true, NOW).unwrap();
        assert_eq!(
            r.unpushed_branches,
            vec![
                UnpushedBranch {
                    name: "feature".into(),
                    upstream: None,
                    // f1, f2 and main's unpushed commit beneath them.
                    commits: 3,
                },
                UnpushedBranch {
                    name: main.clone(),
                    upstream: Some(format!("origin/{main}")),
                    commits: 1,
                },
            ]
        );
        assert!(!r.no_remote);

        // A worktree skips the shared-state checks.
        let wt = assess(Path::new(&path), None, &Thresholds::default(), false, NOW).unwrap();
        assert!(wt.unpushed_branches.is_empty());
    }

    #[test]
    fn old_stashes_respect_the_threshold() {
        let (dir, path, mut repo) = with_origin();
        let old = git2::Signature::new("t", "t@t", &git2::Time::new(NOW - 40 * DAY, 0)).unwrap();
        fs::write(dir.path().join("README.md"), "old wip").unwrap();
        repo.stash_save(&old, "old wip", None).unwrap();
        let fresh = git2::Signature::new("t", "t@t", &git2::Time::new(NOW - DAY, 0)).unwrap();
        fs::write(dir.path().join("README.md"), "new wip").unwrap();
        repo.stash_save(&fresh, "new wip", None).unwrap();

        let r = assess(Path::new(&path), None, &Thresholds::default(), true, NOW).unwrap();
        assert_eq!(r.old_stashes.len(), 1);
        assert_eq!(r.old_stashes[0].index, 1);
        assert!(r.old_stashes[0].message.contains("old wip"));
        assert_eq!(r.old_stashes[0].age_days, 40);

        let strict = Thresholds {
            stash_days: 0,
            ..Thresholds::default()
        };
        let r = assess(Path::new(&path), None, &strict, true, NOW).unwrap();
        assert_eq!(r.old_stashes.len(), 2);
    }

    #[test]
    fn long_dirty_uses_the_threshold() {
        let (_dir, path, _repo) = with_origin();
        let t = Thresholds::default();
        let r = assess(Path::new(&path), Some(NOW - 2 * DAY), &t, true, NOW).unwrap();
        assert_eq!(r.dirty_since, None);
        let r = assess(Path::new(&path), Some(NOW - 9 * DAY), &t, true, NOW).unwrap();
        assert_eq!(r.dirty_since, Some(NOW - 9 * DAY));
        assert_eq!(r.dirty_days, Some(9));
    }

    #[test]
    fn detached_head_orphans_are_counted() {
        let (_dir, path, repo) = with_origin();
        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();
        // Detached on a pushed commit: nothing orphaned.
        let r = assess(Path::new(&path), None, &Thresholds::default(), true, NOW).unwrap();
        assert_eq!(r.orphan_commits, 0);
        // Two commits made while detached: on no branch.
        commit(&repo, "x.txt", "x", NOW);
        commit(&repo, "y.txt", "y", NOW);
        let r = assess(Path::new(&path), None, &Thresholds::default(), false, NOW).unwrap();
        assert_eq!(r.orphan_commits, 2);
    }

    #[test]
    fn assess_fleet_skips_quiet_repos_and_reads_dirty_since() {
        let (_d1, quiet, _r1) = with_origin();
        let (_d2, dirty, _r2) = with_origin();
        let mut repos = vec![
            crate::enrich::test_support::sample(
                &quiet,
                crate::model::Host::Github,
                "github.com",
                "o/q",
            ),
            crate::enrich::test_support::sample(
                &dirty,
                crate::model::Host::Github,
                "github.com",
                "o/d",
            ),
        ];
        repos[1].git.dirty = 3;
        let since = HashMap::from([(dirty.clone(), NOW - 10 * DAY)]);
        let out = assess_fleet_with(&repos, &since, &Thresholds::default(), NOW);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].repo_id, dirty);
        assert_eq!(out[0].dirty_days, Some(10));
    }
}
