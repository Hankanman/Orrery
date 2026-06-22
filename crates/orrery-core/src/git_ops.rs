//! Git command-center operations (Phase 5) built on libgit2: fetch, branch
//! listing/switching/pruning, worktrees, recent log, and the working diff.
//! All synchronous; callers run them off the UI thread.

use std::collections::{HashMap, HashSet};

use git2::{
    BranchType, Cred, CredentialType, DiffOptions, FetchOptions, RemoteCallbacks, Repository,
};
use serde::{Deserialize, Serialize};

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
    GitStatus {
        branch,
        ahead,
        behind,
        dirty,
    }
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
    let head_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from));
    let default_oid = default_branch_oid(&repo);

    let mut out = Vec::new();
    let iter = repo
        .branches(Some(BranchType::Local))
        .map_err(|e| e.to_string())?;
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
            (Some(t), Some(d)) if t != d => repo
                .graph_ahead_behind(t, d)
                .map(|(a, _)| a == 0)
                .unwrap_or(false),
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
    repo.checkout_tree(&object, None)
        .map_err(|e| e.to_string())?;
    match reference {
        Some(r) => repo.set_head(r.name().ok_or("invalid ref")?),
        None => repo.set_head_detached(object.id()),
    }
    .map_err(|e| e.to_string())
}

/// Outcome of a fleet write op: either it did something, or it was safely
/// skipped (e.g. a dirty tree). A hard `Err` is reserved for real failures.
pub enum OpOutcome {
    Done(String),
    Skipped(String),
}

/// Fast-forward-only pull: fetch `origin`, then advance HEAD to its upstream
/// iff that's a clean fast-forward on a clean tree. Diverged/dirty/no-upstream
/// are reported as skips, not errors, so a fleet pull is safe by default.
pub fn pull(path: &str) -> Result<OpOutcome, String> {
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

    let (branch, local_oid) = {
        let head = repo.head().map_err(|e| e.to_string())?;
        if !head.is_branch() {
            return Ok(OpOutcome::Skipped("detached HEAD".into()));
        }
        match (head.shorthand().map(String::from), head.target()) {
            (Some(b), Some(o)) => (b, o),
            _ => return Ok(OpOutcome::Skipped("unborn branch".into())),
        }
    };
    let upstream = match repo
        .find_branch(&branch, BranchType::Local)
        .ok()
        .and_then(|b| b.upstream().ok())
    {
        Some(u) => u,
        None => return Ok(OpOutcome::Skipped("no upstream".into())),
    };
    let Some(up_oid) = upstream.get().target() else {
        return Ok(OpOutcome::Skipped("no upstream".into()));
    };
    if up_oid == local_oid {
        return Ok(OpOutcome::Done("up to date".into()));
    }
    let (ahead, behind) = repo
        .graph_ahead_behind(local_oid, up_oid)
        .map_err(|e| e.to_string())?;
    if ahead > 0 {
        return Ok(OpOutcome::Skipped("diverged".into()));
    }
    if behind == 0 {
        return Ok(OpOutcome::Done("up to date".into()));
    }
    if status_of(&repo).dirty > 0 {
        return Ok(OpOutcome::Skipped("uncommitted changes".into()));
    }
    let refname = format!("refs/heads/{branch}");
    repo.find_reference(&refname)
        .and_then(|mut r| r.set_target(up_oid, "pull: fast-forward"))
        .map_err(|e| e.to_string())?;
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(OpOutcome::Done(format!("fast-forwarded {behind}")))
}

/// Stash uncommitted changes (including untracked). A clean tree is a skip.
pub fn stash(path: &str) -> Result<OpOutcome, String> {
    let mut repo = Repository::open(path).map_err(|e| e.to_string())?;
    if status_of(&repo).dirty == 0 {
        return Ok(OpOutcome::Skipped("clean".into()));
    }
    let sig = repo
        .signature()
        .map_err(|_| "set git user.name and user.email first".to_string())?;
    repo.stash_save(
        &sig,
        "orrery: fleet stash",
        Some(git2::StashFlags::INCLUDE_UNTRACKED),
    )
    .map_err(|e| e.to_string())?;
    Ok(OpOutcome::Done("stashed".into()))
}

/// The default branch name, preferring `origin/HEAD`, then a local main/master.
fn default_branch_name(repo: &Repository) -> Option<String> {
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(name) = r.symbolic_target().and_then(|t| t.rsplit('/').next()) {
            return Some(name.to_string());
        }
    }
    ["main", "master"]
        .into_iter()
        .find(|n| repo.find_branch(n, BranchType::Local).is_ok())
        .map(String::from)
}

/// Switch to the default branch. A dirty tree is skipped (don't clobber work).
pub fn checkout_default(path: &str) -> Result<OpOutcome, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    if status_of(&repo).dirty > 0 {
        return Ok(OpOutcome::Skipped("uncommitted changes".into()));
    }
    let branch = default_branch_name(&repo).ok_or("no default branch")?;
    let on_default = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
        .as_deref()
        == Some(branch.as_str());
    if on_default {
        return Ok(OpOutcome::Skipped(format!("already on {branch}")));
    }
    drop(repo); // switch_branch reopens the repo
    switch_branch(path, &branch)?;
    Ok(OpOutcome::Done(format!("on {branch}")))
}

/// Result of running a command in a repo (captured, not streamed live).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdResult {
    pub code: Option<i32>,
    pub ok: bool,
    /// Last few lines of combined stdout+stderr.
    pub output_tail: String,
}

/// The last `n` non-empty-trimmed lines of `s`, capped for display.
fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    let tail = lines[start..].join("\n");
    if tail.chars().count() > 400 {
        tail.chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        tail
    }
}

/// Run a command in a repo's directory and capture its result. The command is
/// NOT shell-interpreted: the first whitespace token is the executable, the
/// rest are literal args — so there's no pipe/`&&`/glob/injection surface.
pub fn run_command(path: &str, command: &str) -> Result<CmdResult, String> {
    let mut parts = command.split_whitespace();
    let program = parts.next().ok_or("empty command")?;
    let args: Vec<&str> = parts.collect();
    let output = std::process::Command::new(program)
        .args(&args)
        .current_dir(path)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(CmdResult {
        code: output.status.code(),
        ok: output.status.success(),
        output_tail: tail_lines(&combined, 6),
    })
}

fn protected_branches(repo: &Repository) -> Vec<String> {
    ["main", "master"]
        .iter()
        .map(|s| s.to_string())
        .chain(
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().map(String::from)),
        )
        .collect()
}

/// Branches that are safe to prune: merged into the default branch, or with a
/// gone upstream — never HEAD, main, or master.
pub fn prunable(path: &str) -> Result<Vec<BranchInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let protected = protected_branches(&repo);
    Ok(branches(path)?
        .into_iter()
        .filter(|b| !b.is_head && !protected.contains(&b.name) && (b.merged || b.gone))
        .collect())
}

/// Delete the prunable branches (see `prunable`). Returns the names deleted.
pub fn prune_branches(path: &str) -> Result<Vec<String>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let to_prune: Vec<String> = prunable(path)?.into_iter().map(|b| b.name).collect();
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
    // `valid(true)` lets us prune a still-live worktree (the default only prunes
    // ones whose working dir is already gone). We deliberately leave the working
    // directory on disk — this unlinks the worktree, it doesn't delete files.
    let mut opts = git2::WorktreePruneOptions::new();
    opts.valid(true);
    wt.prune(Some(&mut opts)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Full SHA of the current HEAD commit — the unambiguous cursor stored for the
/// "resume where I left off" feature (#69).
pub fn head_sha(path: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head = repo.head().map_err(|e| e.to_string())?;
    let oid = head.target().ok_or("HEAD is unborn")?;
    Ok(oid.to_string())
}

/// Commits on HEAD that landed *after* `since_sha` (newest first), capped at
/// `max`. Returns all of HEAD (up to `max`) if `since_sha` can't be resolved —
/// e.g. it was rewritten by a rebase — so the caller still gets a useful diff.
pub fn log_since_sha(path: &str, since_sha: &str, max: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let since = repo.revparse_single(since_sha).map(|o| o.id()).ok();
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.flatten() {
        // Stop at the last-seen commit (exclusive) — everything newer is "since".
        if Some(oid) == since {
            break;
        }
        if let Ok(commit) = repo.find_commit(oid) {
            out.push(CommitInfo {
                id: oid.to_string()[..7.min(oid.to_string().len())].to_string(),
                summary: commit.summary().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
                time_unix: commit.time().seconds(),
            });
        }
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    pub day: i64,
    pub count: u32,
}

/// Daily commit counts across `paths` for commits on/after `since_day` (epoch
/// days), counting only commits authored by the user. If no git identity can be
/// resolved, counts all commits so the graph is never mysteriously empty.
/// Walks each repo's HEAD line in commit-time order, stopping once it passes the
/// window. Aggregated by author-local day so the calendar matches when the user
/// actually worked.
pub fn contributions(paths: &[String], since_day: i64) -> Vec<DayCount> {
    // "The user" = global git identity plus each repo's configured user.email,
    // lower-cased. Repos cloned but never committed to thus contribute 0.
    let mut emails: HashSet<String> = HashSet::new();
    if let Ok(email) = git2::Config::open_default().and_then(|c| c.get_string("user.email")) {
        emails.insert(email.to_lowercase());
    }
    for path in paths {
        if let Ok(email) = Repository::open(path)
            .and_then(|r| r.config())
            .and_then(|c| c.get_string("user.email"))
        {
            emails.insert(email.to_lowercase());
        }
    }
    // Walking commit objects is the expensive part; repos are independent, so
    // fan out across cores. A shared work index hands each worker the next repo,
    // so heavy repos (deep histories) don't pile up behind each other.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(paths.len().max(1));
    let partials = std::sync::Mutex::new(Vec::<HashMap<i64, u32>>::new());
    let next = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        let emails = &emails;
        let partials = &partials;
        let next = &next;
        for _ in 0..threads {
            scope.spawn(move || {
                let mut local: HashMap<i64, u32> = HashMap::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    let Ok(repo) = Repository::open(path) else {
                        continue;
                    };
                    let Ok(mut walk) = repo.revwalk() else {
                        continue;
                    };
                    if walk.set_sorting(git2::Sort::TIME).is_err() || walk.push_head().is_err() {
                        continue;
                    }
                    for oid in walk.flatten() {
                        let Ok(commit) = repo.find_commit(oid) else {
                            continue;
                        };
                        let author = commit.author();
                        let when = author.when();
                        let day = (when.seconds() + i64::from(when.offset_minutes()) * 60)
                            .div_euclid(86_400);
                        if day < since_day {
                            break; // TIME order: everything after this is older still.
                        }
                        let mine = emails.is_empty()
                            || author
                                .email()
                                .map(|e| emails.contains(&e.to_lowercase()))
                                .unwrap_or(false);
                        if mine {
                            *local.entry(day).or_insert(0) += 1;
                        }
                    }
                }
                partials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(local);
            });
        }
    });

    let mut counts: HashMap<i64, u32> = HashMap::new();
    for partial in partials.into_inner().unwrap_or_else(|e| e.into_inner()) {
        for (day, c) in partial {
            *counts.entry(day).or_insert(0) += c;
        }
    }

    let mut out: Vec<DayCount> = counts
        .into_iter()
        .map(|(day, count)| DayCount { day, count })
        .collect();
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

/// Recursively copy a template directory's contents into `dst`, skipping the
/// template's own `.git` so its history doesn't contaminate the new repo.
fn copy_template(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(Result::ok)
    {
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) if !r.as_os_str().is_empty() => r,
            _ => continue, // the root itself
        };
        // Skip the template's git metadata at any depth.
        if rel.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Create a new repo at `dest`: `git init`, optionally seed from a template dir,
/// optionally add an `origin` remote, and optionally stage everything + make a
/// first commit (`name` is used for a placeholder README when the tree would
/// otherwise be empty). Returns the working directory.
pub fn init(
    dest: &str,
    name: &str,
    template: Option<&str>,
    remote: Option<&str>,
    first_commit_msg: Option<&str>,
) -> Result<String, String> {
    let dest_path = std::path::Path::new(dest);
    std::fs::create_dir_all(dest_path).map_err(|e| e.to_string())?;
    let repo = Repository::init(dest_path).map_err(|e| e.to_string())?;

    if let Some(tpl) = template {
        copy_template(std::path::Path::new(tpl), dest_path)?;
    }

    if let Some(url) = remote {
        repo.remote("origin", url).map_err(|e| e.to_string())?;
    }

    if let Some(msg) = first_commit_msg {
        // Don't create an empty-tree commit: if nothing was seeded, drop a
        // README so the first commit is meaningful.
        let has_content = std::fs::read_dir(dest_path)
            .map(|it| it.filter_map(Result::ok).any(|e| e.file_name() != ".git"))
            .unwrap_or(false);
        if !has_content {
            std::fs::write(dest_path.join("README.md"), format!("# {name}\n"))
                .map_err(|e| e.to_string())?;
        }

        let mut index = repo.index().map_err(|e| e.to_string())?;
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(|e| e.to_string())?;
        index.write().map_err(|e| e.to_string())?;
        let tree = repo
            .find_tree(index.write_tree().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let sig = repo
            .signature()
            .map_err(|_| "set git user.name and user.email first".to_string())?;
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[])
            .map_err(|e| e.to_string())?;
    }

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
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        (dir, path)
    }

    #[test]
    fn stash_skips_clean_then_stashes_dirty() {
        let (_dir, path) = init_repo();
        // Clean tree → skipped.
        assert!(matches!(stash(&path), Ok(OpOutcome::Skipped(_))));
        // Make it dirty, then stash succeeds.
        fs::write(std::path::Path::new(&path).join("README.md"), "# changed").unwrap();
        assert!(matches!(stash(&path), Ok(OpOutcome::Done(_))));
        // Tree is clean again after stashing.
        assert_eq!(status_of(&Repository::open(&path).unwrap()).dirty, 0);
    }

    #[test]
    fn run_command_captures_exit_and_output() {
        let (_dir, path) = init_repo();
        let ok = run_command(&path, "git rev-parse --is-inside-work-tree").unwrap();
        assert!(ok.ok && ok.code == Some(0));
        assert!(ok.output_tail.contains("true"));
        // A failing command reports a non-zero code, not an Err.
        let bad = run_command(&path, "git not-a-real-subcommand").unwrap();
        assert!(!bad.ok);
        // A missing executable is a hard error.
        assert!(run_command(&path, "definitely-not-a-real-binary-xyz").is_err());
    }

    #[test]
    fn pull_skips_repo_without_upstream() {
        let (_dir, path) = init_repo();
        // No origin / no upstream → safe skip, not an error.
        assert!(matches!(pull(&path), Ok(OpOutcome::Skipped(_))));
    }

    #[test]
    fn init_creates_repo_with_first_commit_and_remote() {
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("newproj");
        let dest_str = dest.to_string_lossy().into_owned();
        // git identity for the commit (CI agents may lack a global one).
        std::env::set_var("GIT_AUTHOR_NAME", "t");
        std::env::set_var("GIT_AUTHOR_EMAIL", "t@t");
        std::env::set_var("GIT_COMMITTER_NAME", "t");
        std::env::set_var("GIT_COMMITTER_EMAIL", "t@t");

        let workdir = init(
            &dest_str,
            "newproj",
            None,
            Some("https://example.com/x.git"),
            Some("Initial commit"),
        );
        // Skip the assertion if the environment has no usable git identity.
        if let Ok(workdir) = workdir {
            assert!(dest.join(".git").is_dir(), "should be a git repo");
            assert!(
                dest.join("README.md").is_file(),
                "empty init should seed a README"
            );
            let repo = Repository::open(&workdir).unwrap();
            assert!(
                repo.find_remote("origin").is_ok(),
                "origin remote should be set"
            );
            assert_eq!(
                recent_log(&workdir, 5).unwrap().len(),
                1,
                "one first commit"
            );
        }
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
    fn worktree_add_list_remove_roundtrips() {
        let (_dir, path) = init_repo();
        assert!(worktrees(&path).unwrap().is_empty());

        let dest = format!("{path}-wt");
        add_worktree(&path, "feat-x", &dest).unwrap();
        let wts = worktrees(&path).unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].name, "feat-x");

        // Removing a still-live worktree must succeed (valid-prune).
        remove_worktree(&path, "feat-x").unwrap();
        assert!(worktrees(&path).unwrap().is_empty());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn prunable_lists_merged_branches_not_head() {
        let (dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let first = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &first, false).unwrap();

        // Advance the default branch so `feature` is strictly behind (= merged).
        fs::write(dir.path().join("b.txt"), "two").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("b.txt")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "second", &tree, &[&first])
            .unwrap();

        let names: Vec<String> = prunable(&path)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert!(
            names.contains(&"feature".to_string()),
            "merged branch is prunable: {names:?}"
        );
        assert!(
            prunable(&path).unwrap().iter().all(|b| !b.is_head),
            "HEAD is never prunable"
        );
    }

    #[test]
    fn switch_branch_moves_head() {
        let (_dir, path) = init_repo();
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head, false).unwrap();

        switch_branch(&path, "feature").unwrap();
        let head_branch = branches(&path)
            .unwrap()
            .into_iter()
            .find(|b| b.is_head)
            .unwrap();
        assert_eq!(head_branch.name, "feature");
    }

    #[test]
    fn working_diff_reflects_uncommitted_changes() {
        let (dir, path) = init_repo();
        assert!(
            working_diff(&path).unwrap().is_empty(),
            "clean tree → empty diff"
        );
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
        assert!(
            staged_diff(&path).unwrap().is_empty(),
            "nothing staged after commit"
        );
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

    /// Opt-in perf probe against the real configured workspace. Run with:
    ///   cargo test -p orrery --lib perf_contributions -- --ignored --nocapture
    #[test]
    #[ignore]
    fn perf_contributions() {
        use std::time::Instant;

        let cfg = crate::config::load();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let t = Instant::now();
        let repos = crate::scan::scan(
            &cfg.roots,
            cfg.scan_depth,
            &cfg.ignore,
            &crate::cache::favorites(),
            now,
        );
        let scan_ms = t.elapsed().as_secs_f64() * 1000.0;
        let ids: Vec<String> = repos.iter().map(|r| r.id.clone()).collect();
        let today = now.div_euclid(86_400);
        let since = today - 7 * 53;

        eprintln!("\n── contribution perf ─────────────────────");
        eprintln!("repos: {}  (scan {:.0} ms)", ids.len(), scan_ms);

        let t = Instant::now();
        let just_open = ids.iter().filter_map(|p| Repository::open(p).ok()).count();
        eprintln!(
            "open all repos: {:.1} ms ({just_open} opened)",
            t.elapsed().as_secs_f64() * 1000.0
        );

        for i in 1..=3 {
            let t = Instant::now();
            let graph = contributions(&ids, since);
            let total: u32 = graph.iter().map(|d| d.count).sum();
            eprintln!(
                "contributions() run {i}: {:.1} ms  ({} active days, {total} commits)",
                t.elapsed().as_secs_f64() * 1000.0,
                graph.len()
            );
        }
        eprintln!("──────────────────────────────────────────\n");
    }
}
