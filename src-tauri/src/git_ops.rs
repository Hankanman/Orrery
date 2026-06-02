//! Git command-center operations (Phase 5) built on libgit2: fetch, branch
//! listing/switching/pruning, worktrees, recent log, and the working diff.
//! All synchronous; callers run them off the UI thread.

use std::collections::{HashMap, HashSet};

use git2::{
    BranchType, Cred, CredentialType, DiffOptions, FetchOptions, RemoteCallbacks, Repository,
};
use serde::Serialize;

use crate::model::GitStatus;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub upstream: Option<String>,
    /// Upstream was configured but its remote-tracking ref is gone.
    pub gone: bool,
    /// Fully contained in the default branch (safe to prune).
    pub merged: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub id: String,
    pub summary: String,
    pub author: String,
    pub time_unix: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
}

/// Credentials callback: SSH agent for ssh remotes, the git credential helper
/// (or a token via helper) for HTTPS. Best-effort — failures surface as errors.
fn remote_callbacks() -> RemoteCallbacks<'static> {
    // libgit2 re-invokes this after each rejected attempt; without a cap a bad
    // credential (e.g. wrong key in the agent) loops forever and hangs fetch.
    let mut attempts = 0u32;
    let mut cb = RemoteCallbacks::new();
    cb.credentials(move |url, username, allowed| {
        attempts += 1;
        if attempts > 4 {
            return Err(git2::Error::from_str("authentication failed"));
        }
        if allowed.contains(CredentialType::SSH_KEY) {
            return Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let Ok(config) = git2::Config::open_default() {
                if let Ok(cred) = Cred::credential_helper(&config, url, username) {
                    return Ok(cred);
                }
            }
        }
        Cred::default()
    });
    cb
}

/// Ahead/behind of HEAD vs its upstream.
fn ahead_behind(repo: &Repository) -> (u32, u32) {
    (|| {
        let head = repo.head().ok()?;
        if !head.is_branch() {
            return None;
        }
        let local = head.target()?;
        let upstream = git2::Branch::wrap(head).upstream().ok()?;
        let up_oid = upstream.get().target()?;
        let (a, b) = repo.graph_ahead_behind(local, up_oid).ok()?;
        Some((a as u32, b as u32))
    })()
    .unwrap_or((0, 0))
}

fn status_of(repo: &Repository) -> GitStatus {
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
        .unwrap_or_else(|| "HEAD".to_string());
    let (ahead, behind) = ahead_behind(repo);
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false);
    let dirty = repo
        .statuses(Some(&mut opts))
        .map(|s| s.iter().filter(|e| !e.status().is_ignored()).count() as u32)
        .unwrap_or(0);
    GitStatus { branch, ahead, behind, dirty }
}

/// Fetch the `origin` remote, then return refreshed git status.
pub fn fetch(path: &str) -> Result<GitStatus, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    if let Ok(mut remote) = repo.find_remote("origin") {
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(remote_callbacks());
        let refspecs: Vec<String> = remote
            .fetch_refspecs()
            .map(|r| r.iter().flatten().map(String::from).collect())
            .unwrap_or_default();
        remote
            .fetch(&refspecs, Some(&mut opts), None)
            .map_err(|e| e.to_string())?;
    }
    Ok(status_of(&repo))
}

/// Resolve the tip of the default branch (main/master) for merged checks.
fn default_branch_oid(repo: &Repository) -> Option<git2::Oid> {
    for name in ["main", "master"] {
        if let Ok(b) = repo.find_branch(name, BranchType::Local) {
            if let Some(oid) = b.get().target() {
                return Some(oid);
            }
        }
    }
    repo.head().ok().and_then(|h| h.target())
}

pub fn branches(path: &str) -> Result<Vec<BranchInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head_name = repo.head().ok().and_then(|h| h.shorthand().map(String::from));
    let default_oid = default_branch_oid(&repo);

    let mut out = Vec::new();
    let iter = repo.branches(Some(BranchType::Local)).map_err(|e| e.to_string())?;
    for entry in iter {
        let Ok((branch, _)) = entry else { continue };
        let Some(name) = branch.name().ok().flatten().map(String::from) else {
            continue;
        };
        let tip = branch.get().target();

        let upstream = branch.upstream().ok();
        let upstream_name = upstream
            .as_ref()
            .and_then(|u| u.name().ok().flatten().map(String::from));
        // Upstream configured in .git/config but the tracking ref is missing.
        let has_upstream_cfg = repo
            .config()
            .ok()
            .map(|c| c.get_string(&format!("branch.{name}.merge")).is_ok())
            .unwrap_or(false);
        let gone = has_upstream_cfg && upstream.is_none();

        let merged = match (tip, default_oid) {
            (Some(t), Some(d)) if t != d => {
                repo.graph_ahead_behind(t, d).map(|(a, _)| a == 0).unwrap_or(false)
            }
            _ => false,
        };

        out.push(BranchInfo {
            is_head: Some(&name) == head_name.as_ref(),
            name,
            upstream: upstream_name,
            gone,
            merged,
        });
    }
    Ok(out)
}

pub fn switch_branch(path: &str, name: &str) -> Result<(), String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let (object, reference) = repo
        .revparse_ext(name)
        .map_err(|e| format!("branch not found: {e}"))?;
    repo.checkout_tree(&object, None).map_err(|e| e.to_string())?;
    match reference {
        Some(r) => repo.set_head(r.name().ok_or("invalid ref")?),
        None => repo.set_head_detached(object.id()),
    }
    .map_err(|e| e.to_string())
}

/// Delete branches that are merged or whose upstream is gone (never HEAD or
/// the default branch). Returns the names deleted.
pub fn prune_branches(path: &str) -> Result<Vec<String>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let protected: Vec<String> = ["main", "master"]
        .iter()
        .map(|s| s.to_string())
        .chain(repo.head().ok().and_then(|h| h.shorthand().map(String::from)))
        .collect();

    let to_prune: Vec<String> = branches(path)?
        .into_iter()
        .filter(|b| !b.is_head && !protected.contains(&b.name) && (b.merged || b.gone))
        .map(|b| b.name)
        .collect();

    for name in &to_prune {
        if let Ok(mut b) = repo.find_branch(name, BranchType::Local) {
            b.delete().map_err(|e| e.to_string())?;
        }
    }
    Ok(to_prune)
}

pub fn worktrees(path: &str) -> Result<Vec<WorktreeInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let names = repo.worktrees().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for name in names.iter().flatten() {
        if let Ok(wt) = repo.find_worktree(name) {
            out.push(WorktreeInfo {
                name: name.to_string(),
                path: wt.path().to_string_lossy().into_owned(),
            });
        }
    }
    Ok(out)
}

pub fn add_worktree(path: &str, name: &str, dest: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let wt = repo
        .worktree(name, std::path::Path::new(dest), None)
        .map_err(|e| e.to_string())?;
    Ok(wt.path().to_string_lossy().into_owned())
}

pub fn remove_worktree(path: &str, name: &str) -> Result<(), String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let wt = repo.find_worktree(name).map_err(|e| e.to_string())?;
    wt.prune(None).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn recent_log(path: &str, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.flatten().take(limit) {
        if let Ok(commit) = repo.find_commit(oid) {
            out.push(CommitInfo {
                id: oid.to_string()[..7.min(oid.to_string().len())].to_string(),
                summary: commit.summary().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
                time_unix: commit.time().seconds(),
            });
        }
    }
    Ok(out)
}

/// One day's commit count, keyed by epoch day in the author's local timezone
/// (i.e. `floor((commit_time + tz_offset) / 86400)`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    pub day: i64,
    pub count: u32,
}

/// The set of email addresses that count as "the user", lower-cased: the global
/// git identity plus each repo's configured `user.email`. Used to count only the
/// user's own commits — repos they cloned but never committed to contribute 0,
/// which is the correct contribution-graph semantics.
fn my_emails(paths: &[String]) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut add = |cfg: &git2::Config| {
        if let Ok(email) = cfg.get_string("user.email") {
            set.insert(email.to_lowercase());
        }
    };
    if let Ok(cfg) = git2::Config::open_default() {
        add(&cfg);
    }
    for path in paths {
        if let Ok(cfg) = Repository::open(path).and_then(|r| r.config()) {
            add(&cfg);
        }
    }
    set
}

/// Daily commit counts across `paths` for commits on/after `since_day` (epoch
/// days), counting only commits authored by the user. If no git identity can be
/// resolved, counts all commits so the graph is never mysteriously empty.
/// Walks each repo's HEAD line in commit-time order, stopping once it passes the
/// window. Aggregated by author-local day so the calendar matches when the user
/// actually worked.
pub fn contributions(paths: &[String], since_day: i64) -> Vec<DayCount> {
    let emails = my_emails(paths);
    let mut counts: HashMap<i64, u32> = HashMap::new();

    for path in paths {
        let Ok(repo) = Repository::open(path) else { continue };
        let Ok(mut walk) = repo.revwalk() else { continue };
        if walk.set_sorting(git2::Sort::TIME).is_err() || walk.push_head().is_err() {
            continue;
        }
        for oid in walk.flatten() {
            let Ok(commit) = repo.find_commit(oid) else { continue };
            let when = commit.author().when();
            let local = when.seconds() + i64::from(when.offset_minutes()) * 60;
            let day = local.div_euclid(86_400);
            if day < since_day {
                break; // TIME order: everything after this is older still.
            }
            let mine = emails.is_empty()
                || commit
                    .author()
                    .email()
                    .map(|e| emails.contains(&e.to_lowercase()))
                    .unwrap_or(false);
            if mine {
                *counts.entry(day).or_insert(0) += 1;
            }
        }
    }

    let mut out: Vec<DayCount> = counts.into_iter().map(|(day, count)| DayCount { day, count }).collect();
    out.sort_by_key(|d| d.day);
    out
}

/// Clone `url` into `dest` (full destination path). Returns the working dir.
pub fn clone(url: &str, dest: &str) -> Result<String, String> {
    let mut fo = FetchOptions::new();
    fo.remote_callbacks(remote_callbacks());
    let repo = git2::build::RepoBuilder::new()
        .fetch_options(fo)
        .clone(url, std::path::Path::new(dest))
        .map_err(|e| e.to_string())?;
    Ok(repo
        .workdir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| dest.to_string()))
}

fn diff_to_string(diff: &git2::Diff) -> String {
    let mut buf = String::new();
    let _ = diff.print(git2::DiffFormat::Patch, |_, _, line| {
        match line.origin() {
            '+' | '-' | ' ' => buf.push(line.origin()),
            _ => {}
        }
        buf.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    });
    // Cap to keep the IPC payload + UI reasonable (char-boundary safe).
    if buf.len() > 200_000 {
        let mut end = 200_000;
        while !buf.is_char_boundary(end) {
            end -= 1;
        }
        buf.truncate(end);
        buf.push_str("\n… diff truncated …\n");
    }
    buf
}

/// Unified diff of the working tree + index vs HEAD (for the diff peek).
pub fn working_diff(path: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;
    Ok(diff_to_string(&diff))
}

/// Diff of the index vs HEAD — i.e. exactly what a commit would record.
pub fn staged_diff(path: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), None, None)
        .map_err(|e| e.to_string())?;
    Ok(diff_to_string(&diff))
}

/// Commit the currently-staged changes with `message`. Returns the short hash.
pub fn commit(path: &str, message: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;

    // Refuse to commit on a detached HEAD — it would orphan the commit.
    if let Ok(head) = repo.head() {
        if !head.is_branch() {
            return Err("HEAD is detached — check out a branch before committing".into());
        }
    }

    let mut index = repo.index().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    // Nothing staged → the tree equals the parent's; don't create an empty commit.
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Err("no staged changes to commit".into());
        }
    }

    let sig = repo
        .signature()
        .map_err(|_| "set git user.name and user.email first".to_string())?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| e.to_string())?;
    Ok(oid.to_string()[..7.min(oid.to_string().len())].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Init a temp repo with one commit; return (tempdir, path).
    fn init_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "t").unwrap();
            cfg.set_str("user.email", "t@t").unwrap();
        }
        fs::write(dir.path().join("README.md"), "# Test").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        (dir, path)
    }

    #[test]
    fn branches_marks_head_and_log_has_commit() {
        let (_dir, path) = init_repo();
        let branches = branches(&path).unwrap();
        assert_eq!(branches.len(), 1);
        assert!(branches[0].is_head);

        let log = recent_log(&path, 10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "init");
        assert_eq!(log[0].author, "t");
    }

    #[test]
    fn switch_branch_moves_head() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head, false).unwrap();

        switch_branch(&path, "feature").unwrap();
        let head_branch = branches(&path).unwrap().into_iter().find(|b| b.is_head).unwrap();
        assert_eq!(head_branch.name, "feature");
    }

    #[test]
    fn working_diff_reflects_uncommitted_changes() {
        let (dir, path) = init_repo();
        assert!(working_diff(&path).unwrap().is_empty(), "clean tree → empty diff");
        fs::write(dir.path().join("README.md"), "# Test\nchanged").unwrap();
        assert!(working_diff(&path).unwrap().contains("changed"));
    }

    #[test]
    fn staged_diff_then_commit() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join("new.txt"), "hello").unwrap();
        let repo = Repository::open(&path).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("new.txt")).unwrap();
        idx.write().unwrap();

        assert!(staged_diff(&path).unwrap().contains("new.txt"));
        let short = commit(&path, "feat: add new").unwrap();
        assert_eq!(short.len(), 7);
        assert_eq!(recent_log(&path, 5).unwrap()[0].summary, "feat: add new");
        assert!(staged_diff(&path).unwrap().is_empty(), "nothing staged after commit");
    }

    #[test]
    fn contributions_count_the_users_commit_within_window() {
        let (_dir, path) = init_repo();
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let paths = vec![path];

        // The single init commit (authored as the repo's user.email) counts once,
        // landing within the trailing window.
        let graph = contributions(&paths, today - 7);
        assert_eq!(graph.iter().map(|d| d.count).sum::<u32>(), 1);
        assert!(graph.iter().all(|d| d.day >= today - 7));

        // A window that starts in the future excludes everything.
        assert!(contributions(&paths, today + 7).is_empty());
    }
}
