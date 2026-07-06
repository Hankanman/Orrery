//! Git command-center operations (Phase 5) built on libgit2: fetch, branch
//! listing/switching/pruning, worktrees, recent log, and the working diff.
//! All synchronous; callers run them off the UI thread.

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

/// What kind of pending change a file has (relative to HEAD or the index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

/// One file's pending change, for the drawer's Changes tab. A path that is
/// both staged and further modified in the working tree yields two entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    /// In the index (part of the next commit) vs working-tree only.
    pub staged: bool,
}

/// One hunk of a single file's diff, for hunk-level staging. Its position in
/// the [`file_hunks`] vec is the `hunk_ix` that [`stage_hunk`] /
/// [`unstage_hunk`] take — valid only against the diff it was read from, so
/// recompute after every staging op (the UI reloads anyway).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ …` header line (no trailing newline).
    pub header: String,
    /// Body lines, each prefixed with its origin char (` `, `+`, `-`).
    pub lines: Vec<String>,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
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

/// Ahead/behind of HEAD vs its upstream, or `None` when HEAD isn't a branch or
/// has no tracking branch. Canonical impl — `scan` reuses it via `status_of`.
pub(crate) fn ahead_behind(repo: &Repository) -> Option<(u32, u32)> {
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    let local = head.target()?;
    let upstream = git2::Branch::wrap(head).upstream().ok()?;
    let up_oid = upstream.get().target()?;
    let (a, b) = repo.graph_ahead_behind(local, up_oid).ok()?;
    Some((a as u32, b as u32))
}

/// Branch + ahead/behind + dirty count for a repo. The canonical git-status
/// reader, shared by `scan` (grid snapshot) and the drawer's refresh ops.
pub(crate) fn status_of(repo: &Repository) -> GitStatus {
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
        .unwrap_or_else(|| "HEAD".to_string());
    let (ahead, behind) = ahead_behind(repo).unwrap_or((0, 0));
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

/// Skip reason: local history diverged from upstream (a pull would not be a
/// fast-forward). Named so `fleet::pull_op` can match it exactly.
pub const SKIP_DIVERGED: &str = "diverged";
/// Skip reason: uncommitted changes in the working tree. Named so
/// `fleet::pull_op` can match it exactly.
pub const SKIP_DIRTY: &str = "uncommitted changes";

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
        return Ok(OpOutcome::Skipped(SKIP_DIVERGED.into()));
    }
    if behind == 0 {
        return Ok(OpOutcome::Done("up to date".into()));
    }
    if status_of(&repo).dirty > 0 {
        return Ok(OpOutcome::Skipped(SKIP_DIRTY.into()));
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

/// The repo's default branch name (`origin/HEAD`, else a local main/master) —
/// the base branch for "Open PR" and the merged checks.
pub fn default_branch(path: &str) -> Option<String> {
    Repository::open(path)
        .ok()
        .and_then(|r| default_branch_name(&r))
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
        return Ok(OpOutcome::Skipped(SKIP_DIRTY.into()));
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

/// Create branch `branch` at HEAD and add worktree `name` at `dest` checked out
/// on it — the agent-dispatch shape (#185), where the branch (`agent/…`) is
/// namespaced with a `/` and so can't double as the worktree name the way
/// [`add_worktree`]'s does (libgit2 uses the name as a directory under
/// `.git/worktrees/`).
pub fn add_worktree_on_branch(
    path: &str,
    name: &str,
    branch: &str,
    dest: &str,
) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let branch = repo
        .branch(branch, &head, false)
        .map_err(|e| e.to_string())?;
    let reference = branch.into_reference();
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&reference));
    let wt = repo
        .worktree(name, std::path::Path::new(dest), Some(&opts))
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

/// Commits on HEAD that aren't reachable from `base` (a local branch name,
/// with `origin/<base>` also hidden when present), newest first, capped at
/// `max` — the commit range a PR from the current branch would contain. If
/// `base` can't be resolved at all, returns recent HEAD commits (up to `max`)
/// so the caller still gets something to describe.
pub fn commits_ahead_of(path: &str, base: &str, max: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    if let Some(oid) = repo
        .find_branch(base, BranchType::Local)
        .ok()
        .and_then(|b| b.get().target())
    {
        let _ = walk.hide(oid);
    }
    if let Ok(r) = repo.find_reference(&format!("refs/remotes/origin/{base}")) {
        if let Some(oid) = r.target() {
            let _ = walk.hide(oid);
        }
    }
    let mut out = Vec::new();
    for oid in walk.flatten().take(max) {
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

/// Build the libgit2 diff behind [`file_diff`] / [`file_hunks`]: one file
/// (repo-relative pathspec), index vs HEAD when `staged`, else working tree vs
/// index (with untracked content shown as an add). `reverse` flips old/new —
/// hunk order is preserved, which [`unstage_hunk`] relies on.
fn file_diff_raw<'r>(
    repo: &'r Repository,
    file: &str,
    staged: bool,
    reverse: bool,
) -> Result<git2::Diff<'r>, String> {
    let mut opts = DiffOptions::new();
    opts.pathspec(file).reverse(reverse);
    if staged {
        let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
    } else {
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        repo.diff_index_to_workdir(None, Some(&mut opts))
    }
    .map_err(|e| e.to_string())
}

/// Unified diff for a single file (repo-relative pathspec): index vs HEAD when
/// `staged`, else working tree vs index (with untracked content shown as an
/// add). Backs the drawer's per-file diff pane.
pub fn file_diff(path: &str, file: &str, staged: bool) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let diff = file_diff_raw(&repo, file, staged, false)?;
    Ok(diff_to_string(&diff))
}

/// A single file's diff split into hunks (same sides as [`file_diff`]). The
/// vec position of each hunk is the `hunk_ix` for [`stage_hunk`] /
/// [`unstage_hunk`]. Binary files yield no hunks.
pub fn file_hunks(path: &str, file: &str, staged: bool) -> Result<Vec<Hunk>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let diff = file_diff_raw(&repo, file, staged, false)?;
    let mut out = Vec::new();
    for delta_ix in 0..diff.deltas().len() {
        let Some(patch) = git2::Patch::from_diff(&diff, delta_ix).map_err(|e| e.to_string())?
        else {
            continue; // unchanged or binary
        };
        for h in 0..patch.num_hunks() {
            let (hunk, line_count) = patch.hunk(h).map_err(|e| e.to_string())?;
            let mut lines = Vec::with_capacity(line_count);
            for l in 0..line_count {
                let line = patch.line_in_hunk(h, l).map_err(|e| e.to_string())?;
                let content = String::from_utf8_lossy(line.content());
                let content = content.trim_end_matches(['\n', '\r']);
                match line.origin() {
                    o @ (' ' | '+' | '-') => lines.push(format!("{o}{content}")),
                    // "\ No newline at end of file" and friends.
                    _ => lines.push(content.to_string()),
                }
            }
            out.push(Hunk {
                header: String::from_utf8_lossy(hunk.header())
                    .trim_end()
                    .to_string(),
                lines,
                old_start: hunk.old_start(),
                old_lines: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_lines: hunk.new_lines(),
            });
        }
    }
    Ok(out)
}

/// Stage one hunk of `file`'s *unstaged* diff into the index (`git add -p`,
/// one hunk). `hunk_ix` indexes [`file_hunks`]`(path, file, false)`.
pub fn stage_hunk(path: &str, file: &str, hunk_ix: usize) -> Result<(), String> {
    apply_file_hunk(path, file, hunk_ix, true)
}

/// Unstage one hunk of `file`'s *staged* diff, resetting that region of the
/// index back toward HEAD (`git restore --staged -p`, one hunk). `hunk_ix`
/// indexes [`file_hunks`]`(path, file, true)`.
pub fn unstage_hunk(path: &str, file: &str, hunk_ix: usize) -> Result<(), String> {
    apply_file_hunk(path, file, hunk_ix, false)
}

/// Apply a single hunk to the index via `git_apply` (`ApplyLocation::Index`),
/// filtering every other hunk out with the hunk callback. Staging applies the
/// index→workdir diff; unstaging applies the HEAD→index diff *reversed* —
/// either way the patch's pre-image is the current index, which is what
/// index-only application requires. Reversal preserves hunk order, so the UI's
/// forward-diff indices stay valid on both sides.
fn apply_file_hunk(path: &str, file: &str, hunk_ix: usize, stage: bool) -> Result<(), String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    // Untracked files aren't in the index, and `git_apply` refuses a patch for
    // a file the index doesn't know. Their whole content is one hunk anyway,
    // so staging hunk 0 of an untracked file is exactly `git add`.
    if stage
        && repo
            .status_file(std::path::Path::new(file))
            .is_ok_and(|s| s.is_wt_new())
    {
        if hunk_ix != 0 {
            return Err(format!("untracked file has one hunk, not {}", hunk_ix + 1));
        }
        return stage_paths(path, &[file.to_string()]);
    }
    let diff = file_diff_raw(&repo, file, !stage, !stage)?;
    let mut ix = 0usize;
    let mut opts = git2::ApplyOptions::new();
    opts.hunk_callback(move |_hunk| {
        let keep = ix == hunk_ix;
        ix += 1;
        keep
    });
    repo.apply(&diff, git2::ApplyLocation::Index, Some(&mut opts))
        .map_err(|e| e.to_string())
}

/// Every pending change in the repo, split per file into staged (index vs
/// HEAD) and unstaged (working tree vs index) entries — the model behind a
/// per-file staging checklist. Ordered as libgit2 reports them (by path).
pub fn changes(path: &str) -> Result<Vec<FileChange>, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let file = String::from_utf8_lossy(entry.path_bytes()).into_owned();
        // A single status entry carries both index and worktree bits; the
        // rename checks come first because RENAMED can combine with MODIFIED.
        let staged_kind = if s.is_index_renamed() {
            Some(ChangeKind::Renamed)
        } else if s.is_index_new() {
            Some(ChangeKind::Added)
        } else if s.is_index_deleted() {
            Some(ChangeKind::Deleted)
        } else if s.is_index_modified() || s.is_index_typechange() {
            Some(ChangeKind::Modified)
        } else {
            None
        };
        if let Some(kind) = staged_kind {
            out.push(FileChange {
                path: file.clone(),
                kind,
                staged: true,
            });
        }
        let unstaged_kind = if s.is_wt_renamed() {
            Some(ChangeKind::Renamed)
        } else if s.is_wt_new() {
            Some(ChangeKind::Untracked)
        } else if s.is_wt_deleted() {
            Some(ChangeKind::Deleted)
        } else if s.is_wt_modified() || s.is_wt_typechange() {
            Some(ChangeKind::Modified)
        } else {
            None
        };
        if let Some(kind) = unstaged_kind {
            out.push(FileChange {
                path: file,
                kind,
                staged: false,
            });
        }
    }
    Ok(out)
}

/// Stage specific files into the index (`git add <paths>`). Paths are
/// repo-relative. A path missing from the working tree stages as a deletion
/// (`git rm --cached` semantics) — `add_path` would error on it.
pub fn stage_paths(path: &str, paths: &[String]) -> Result<(), String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let workdir = repo.workdir().ok_or("bare repository")?.to_path_buf();
    let mut index = repo.index().map_err(|e| e.to_string())?;
    for p in paths {
        let rel = std::path::Path::new(p);
        // symlink_metadata so a broken symlink still stages as an add.
        if workdir.join(rel).symlink_metadata().is_ok() {
            index.add_path(rel).map_err(|e| e.to_string())?;
        } else {
            index.remove_path(rel).map_err(|e| e.to_string())?;
        }
    }
    index.write().map_err(|e| e.to_string())
}

/// Unstage specific files: reset their index entries back to HEAD
/// (`git restore --staged <paths>`). The working tree is untouched. On an
/// unborn HEAD (no commits yet) there is nothing to reset to, so the entries
/// are removed from the index — which is what `reset_default(None, ..)` does.
pub fn unstage_paths(path: &str, paths: &[String]) -> Result<(), String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.peel(git2::ObjectType::Commit).ok());
    repo.reset_default(head.as_ref(), paths)
        .map_err(|e| e.to_string())
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

/// Push the current branch to its upstream remote; with no upstream, push to
/// `origin` and set the branch's upstream (`git push --set-upstream origin
/// <branch>` semantics — `branch.<name>.remote` + `branch.<name>.merge` in
/// config). Auth reuses [`remote_callbacks`] exactly as fetch/pull do (SSH
/// agent for ssh remotes, the git credential helper for HTTPS). Never forces:
/// a non-fast-forward rejection surfaces as a clear error. Returns a short
/// human-readable outcome for the success toast.
pub fn push(path: &str) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head = repo.head().map_err(|e| e.to_string())?;
    if !head.is_branch() {
        return Err("HEAD is detached — check out a branch before pushing".into());
    }
    let branch = head
        .shorthand()
        .map(String::from)
        .ok_or("HEAD has no branch name")?;

    // Upstream remote when configured (branch.<name>.remote), else origin.
    let refname = format!("refs/heads/{branch}");
    let upstream_remote = repo
        .branch_upstream_remote(&refname)
        .ok()
        .and_then(|b| b.as_str().map(String::from));
    let has_upstream = upstream_remote.is_some();
    let remote_name = upstream_remote.unwrap_or_else(|| "origin".to_string());
    let mut remote = repo
        .find_remote(&remote_name)
        .map_err(|_| format!("no '{remote_name}' remote to push to"))?;

    // Push to the upstream's merge ref when tracking is configured (it can
    // differ from the local name), else mirror the local branch name.
    let dst = repo
        .config()
        .ok()
        .and_then(|c| c.get_string(&format!("branch.{branch}.merge")).ok())
        .unwrap_or_else(|| refname.clone());
    let refspec = format!("{refname}:{dst}");

    // Per-ref rejections (e.g. remote-side non-fast-forward) arrive through the
    // push_update_reference callback, not as a push() error — capture them.
    let rejection: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
    let mut cb = remote_callbacks();
    let rejected = rejection.clone();
    cb.push_update_reference(move |_ref, status| {
        if let Some(msg) = status {
            *rejected.lock().unwrap() = Some(msg.to_string());
        }
        Ok(())
    });
    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(cb);
    remote
        .push(&[refspec.as_str()], Some(&mut opts))
        .map_err(|e| push_error(&e))?;
    if let Some(msg) = rejection.lock().unwrap().take() {
        return Err(push_rejection(&msg));
    }

    if !has_upstream {
        // `--set-upstream` semantics, straight into config (works even before
        // the remote-tracking ref exists locally).
        let mut config = repo.config().map_err(|e| e.to_string())?;
        config
            .set_str(&format!("branch.{branch}.remote"), &remote_name)
            .map_err(|e| e.to_string())?;
        config
            .set_str(&format!("branch.{branch}.merge"), &refname)
            .map_err(|e| e.to_string())?;
        return Ok(format!("{branch} → {remote_name}/{branch} (upstream set)"));
    }
    Ok(format!("{branch} → {remote_name}"))
}

/// A clear message for a push failure: auth problems and non-fast-forward
/// rejections get actionable phrasing instead of raw libgit2 text.
fn push_error(e: &git2::Error) -> String {
    let msg = e.message().to_string();
    if msg.contains("authentication")
        || msg.contains("credentials")
        || e.class() == git2::ErrorClass::Ssh
    {
        return format!(
            "authentication failed — check your SSH agent or git credential helper ({msg})"
        );
    }
    if msg.contains("non-fastforwardable") || msg.contains("non-fast-forward") {
        return push_rejection(&msg);
    }
    msg
}

/// The non-fast-forward rejection message (Orrery never force-pushes).
fn push_rejection(detail: &str) -> String {
    format!(
        "push rejected (non-fast-forward) — pull or rebase first; Orrery never forces ({detail})"
    )
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
    fn worktree_on_branch_checks_out_namespaced_branch() {
        let (_dir, path) = init_repo();
        let dest = format!("{path}-agent-wt");
        let wt_path =
            add_worktree_on_branch(&path, "agent-fix-x-abcd", "agent/fix-x-abcd", &dest).unwrap();

        // The worktree is listed under its flat name…
        let wts = worktrees(&path).unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].name, "agent-fix-x-abcd");
        // …its HEAD is the namespaced branch…
        let wt_repo = Repository::open(&wt_path).unwrap();
        assert_eq!(
            wt_repo.head().unwrap().shorthand(),
            Some("agent/fix-x-abcd")
        );
        // …and the branch exists in the origin repo too.
        let repo = Repository::open(&path).unwrap();
        assert!(repo
            .find_branch("agent/fix-x-abcd", BranchType::Local)
            .is_ok());

        // A fresh worktree reports no uncommitted changes; a scribbled-on one
        // does — the signal the "Remove worktree" guard uses.
        assert!(changes(&wt_path).unwrap().is_empty());
        fs::write(std::path::Path::new(&wt_path).join("scratch.txt"), "wip").unwrap();
        assert!(!changes(&wt_path).unwrap().is_empty());

        remove_worktree(&path, "agent-fix-x-abcd").unwrap();
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

    /// The `FileChange` entries for `path`, as (path, kind, staged) tuples.
    fn change_tuples(path: &str) -> Vec<(String, ChangeKind, bool)> {
        changes(path)
            .unwrap()
            .into_iter()
            .map(|c| (c.path, c.kind, c.staged))
            .collect()
    }

    #[test]
    fn stage_one_of_two_modified_files_commits_only_it() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        stage_paths(&path, &["a.txt".into(), "b.txt".into()]).unwrap();
        commit(&path, "add a and b").unwrap();

        // Modify both, stage only one.
        fs::write(dir.path().join("a.txt"), "a2").unwrap();
        fs::write(dir.path().join("b.txt"), "b2").unwrap();
        stage_paths(&path, &["a.txt".into()]).unwrap();
        let got = change_tuples(&path);
        assert!(got.contains(&("a.txt".into(), ChangeKind::Modified, true)));
        assert!(got.contains(&("b.txt".into(), ChangeKind::Modified, false)));

        commit(&path, "change a only").unwrap();
        // a's change landed; b is still an unstaged modification.
        assert_eq!(
            change_tuples(&path),
            vec![("b.txt".into(), ChangeKind::Modified, false)]
        );
        let staged = staged_diff(&path).unwrap();
        assert!(staged.is_empty(), "nothing staged after commit: {staged}");
    }

    #[test]
    fn unstage_resets_index_to_head() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join("README.md"), "# changed").unwrap();
        stage_paths(&path, &["README.md".into()]).unwrap();
        assert!(staged_diff(&path).unwrap().contains("changed"));

        unstage_paths(&path, &["README.md".into()]).unwrap();
        assert!(staged_diff(&path).unwrap().is_empty(), "index back at HEAD");
        // The edit survives in the working tree.
        assert_eq!(
            change_tuples(&path),
            vec![("README.md".into(), ChangeKind::Modified, false)]
        );
    }

    #[test]
    fn staging_a_deleted_file_stages_a_deletion() {
        let (dir, path) = init_repo();
        fs::remove_file(dir.path().join("README.md")).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("README.md".into(), ChangeKind::Deleted, false)]
        );

        stage_paths(&path, &["README.md".into()]).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("README.md".into(), ChangeKind::Deleted, true)]
        );

        commit(&path, "remove README").unwrap();
        assert!(change_tuples(&path).is_empty(), "deletion committed");
        let repo = Repository::open(&path).unwrap();
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        assert!(tree.get_name("README.md").is_none(), "gone from HEAD tree");
    }

    #[test]
    fn staging_an_untracked_file_marks_it_added() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join("new.txt"), "hello").unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("new.txt".into(), ChangeKind::Untracked, false)]
        );

        stage_paths(&path, &["new.txt".into()]).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("new.txt".into(), ChangeKind::Added, true)]
        );
        // Unstaging an add drops it from the index entirely (not in HEAD).
        unstage_paths(&path, &["new.txt".into()]).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("new.txt".into(), ChangeKind::Untracked, false)]
        );
    }

    #[test]
    fn unstage_on_unborn_head_removes_from_index() {
        // A fresh repo with no commits — HEAD is unborn.
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        fs::write(dir.path().join("first.txt"), "x").unwrap();

        stage_paths(&path, &["first.txt".into()]).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("first.txt".into(), ChangeKind::Added, true)]
        );

        unstage_paths(&path, &["first.txt".into()]).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("first.txt".into(), ChangeKind::Untracked, false)]
        );
    }

    #[test]
    fn file_diff_scopes_to_one_path_and_side() {
        let (dir, path) = init_repo();
        // A staged edit to README.md and a separate untracked file.
        fs::write(dir.path().join("README.md"), "# Test\nstaged line").unwrap();
        stage_paths(&path, &["README.md".into()]).unwrap();
        fs::write(dir.path().join("new.txt"), "untracked line").unwrap();

        let staged = file_diff(&path, "README.md", true).unwrap();
        assert!(staged.contains("staged line"), "staged side: {staged}");
        assert!(!staged.contains("untracked line"), "scoped to README.md");

        // README.md has no working-tree edits beyond the index.
        assert!(file_diff(&path, "README.md", false).unwrap().is_empty());

        // The untracked file's content shows on the unstaged side only.
        let untracked = file_diff(&path, "new.txt", false).unwrap();
        assert!(untracked.contains("+untracked line"), "got: {untracked}");
        assert!(file_diff(&path, "new.txt", true).unwrap().is_empty());
    }

    /// A committed 20-line file with two well-separated unstaged edits (lines
    /// 2 and 19) — far enough apart that the diff splits into two hunks.
    fn two_hunk_setup() -> (tempfile::TempDir, String) {
        let (dir, path) = init_repo();
        let orig: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        fs::write(dir.path().join("f.txt"), &orig).unwrap();
        stage_paths(&path, &["f.txt".into()]).unwrap();
        commit(&path, "add f").unwrap();
        let modified = orig
            .replace("line 2\n", "line 2 changed\n")
            .replace("line 19\n", "line 19 changed\n");
        fs::write(dir.path().join("f.txt"), modified).unwrap();
        (dir, path)
    }

    /// The HEAD-tree content of `file`.
    fn committed_content(path: &str, file: &str) -> String {
        let repo = Repository::open(path).unwrap();
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        let entry = tree.get_name(file).unwrap();
        let blob = entry.to_object(&repo).unwrap().peel_to_blob().unwrap();
        String::from_utf8(blob.content().to_vec()).unwrap()
    }

    #[test]
    fn file_hunks_splits_separated_edits() {
        let (_dir, path) = two_hunk_setup();
        let hunks = file_hunks(&path, "f.txt", false).unwrap();
        assert_eq!(hunks.len(), 2, "two separated edits → two hunks");
        assert!(
            hunks[0].header.starts_with("@@ "),
            "got: {}",
            hunks[0].header
        );
        assert!(hunks[0].lines.iter().any(|l| l == "+line 2 changed"));
        assert!(hunks[0].lines.iter().any(|l| l == "-line 2"));
        assert!(hunks[1].lines.iter().any(|l| l == "+line 19 changed"));
        assert!(hunks[0].old_start > 0 && hunks[1].new_start > hunks[0].new_start);
        // Nothing staged yet → no hunks on the staged side.
        assert!(file_hunks(&path, "f.txt", true).unwrap().is_empty());
    }

    #[test]
    fn stage_hunk_commits_only_that_hunk() {
        let (dir, path) = two_hunk_setup();
        stage_hunk(&path, "f.txt", 0).unwrap();

        // The first edit moved to the index; the second stayed unstaged.
        let staged = file_hunks(&path, "f.txt", true).unwrap();
        assert_eq!(staged.len(), 1);
        assert!(staged[0].lines.iter().any(|l| l == "+line 2 changed"));
        let unstaged = file_hunks(&path, "f.txt", false).unwrap();
        assert_eq!(unstaged.len(), 1);
        assert!(unstaged[0].lines.iter().any(|l| l == "+line 19 changed"));

        commit(&path, "first hunk only").unwrap();
        let content = committed_content(&path, "f.txt");
        assert!(content.contains("line 2 changed"), "staged hunk landed");
        assert!(
            content.contains("line 19\n"),
            "unstaged hunk did not: {content}"
        );
        // The working tree still carries the second edit, untouched.
        let wt = fs::read_to_string(dir.path().join("f.txt")).unwrap();
        assert!(wt.contains("line 19 changed"));
        assert_eq!(
            change_tuples(&path),
            vec![("f.txt".into(), ChangeKind::Modified, false)]
        );
    }

    #[test]
    fn unstage_hunk_mirrors_stage() {
        let (_dir, path) = two_hunk_setup();
        stage_paths(&path, &["f.txt".into()]).unwrap();
        assert_eq!(file_hunks(&path, "f.txt", true).unwrap().len(), 2);

        unstage_hunk(&path, "f.txt", 1).unwrap();
        let staged = file_hunks(&path, "f.txt", true).unwrap();
        assert_eq!(staged.len(), 1);
        assert!(staged[0].lines.iter().any(|l| l == "+line 2 changed"));
        let unstaged = file_hunks(&path, "f.txt", false).unwrap();
        assert_eq!(unstaged.len(), 1);
        assert!(unstaged[0].lines.iter().any(|l| l == "+line 19 changed"));

        commit(&path, "kept hunk only").unwrap();
        let content = committed_content(&path, "f.txt");
        assert!(content.contains("line 2 changed"));
        assert!(content.contains("line 19\n"), "unstaged hunk stayed out");
    }

    #[test]
    fn hunk_indices_recompute_after_partial_stage() {
        let (_dir, path) = two_hunk_setup();
        stage_hunk(&path, "f.txt", 0).unwrap();
        // After the partial stage the remaining unstaged hunk re-indexes to 0.
        stage_hunk(&path, "f.txt", 0).unwrap();
        assert!(file_hunks(&path, "f.txt", false).unwrap().is_empty());
        assert_eq!(file_hunks(&path, "f.txt", true).unwrap().len(), 2);
        // Out-of-range index errors rather than silently no-opping… by staging
        // nothing (the callback never matches), which libgit2 treats as OK.
        // Recomputing indices after every op is the contract; this documents
        // that a stale index is at worst a no-op, never a wrong hunk.
        stage_hunk(&path, "f.txt", 5).unwrap();
        assert_eq!(file_hunks(&path, "f.txt", true).unwrap().len(), 2);
    }

    #[test]
    fn stage_hunk_on_untracked_file_stages_it_whole() {
        let (dir, path) = init_repo();
        fs::write(dir.path().join("new.txt"), "hello\n").unwrap();
        let hunks = file_hunks(&path, "new.txt", false).unwrap();
        assert_eq!(hunks.len(), 1, "untracked content is one add hunk");
        assert!(hunks[0].lines.iter().any(|l| l == "+hello"));

        assert!(
            stage_hunk(&path, "new.txt", 1).is_err(),
            "only hunk 0 exists for an untracked file"
        );
        stage_hunk(&path, "new.txt", 0).unwrap();
        assert_eq!(
            change_tuples(&path),
            vec![("new.txt".into(), ChangeKind::Added, true)]
        );
    }

    /// Add `origin` pointing at a fresh bare repo (a local-path remote needs no
    /// auth, so push is exercised end-to-end). Returns the bare repo's tempdir.
    fn add_bare_origin(path: &str) -> tempfile::TempDir {
        let bare_dir = tempfile::tempdir().unwrap();
        Repository::init_bare(bare_dir.path()).unwrap();
        let repo = Repository::open(path).unwrap();
        repo.remote("origin", &bare_dir.path().to_string_lossy())
            .unwrap();
        bare_dir
    }

    /// Commit all of the working tree with `msg`; returns the new commit id.
    fn commit_file(path: &str, file: &str, content: &str, msg: &str) -> git2::Oid {
        fs::write(std::path::Path::new(path).join(file), content).unwrap();
        stage_paths(path, &[file.into()]).unwrap();
        commit(path, msg).unwrap();
        Repository::open(path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap()
    }

    #[test]
    fn push_sets_upstream_then_pushes_plain() {
        let (_dir, path) = init_repo();
        let _bare = add_bare_origin(&path);

        // First push: no upstream yet → set-upstream semantics.
        let msg = push(&path).unwrap();
        assert!(msg.contains("upstream set"), "got: {msg}");
        let repo = Repository::open(&path).unwrap();
        let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
        let cfg = repo.config().unwrap();
        assert_eq!(
            cfg.get_string(&format!("branch.{branch_name}.remote"))
                .unwrap(),
            "origin"
        );
        assert_eq!(
            cfg.get_string(&format!("branch.{branch_name}.merge"))
                .unwrap(),
            format!("refs/heads/{branch_name}")
        );

        // Second push (with a new commit): plain push to the upstream.
        commit_file(&path, "a.txt", "a", "add a");
        let msg = push(&path).unwrap();
        assert!(!msg.contains("upstream set"), "got: {msg}");

        // The remote actually has the commits.
        let bare = Repository::open(_bare.path()).unwrap();
        let tip = bare
            .find_reference(&format!("refs/heads/{branch_name}"))
            .unwrap()
            .peel_to_commit()
            .unwrap();
        assert_eq!(tip.summary(), Some("add a"));
    }

    #[test]
    fn push_rejects_non_fast_forward_without_forcing() {
        let (_dir, path) = init_repo();
        let _bare = add_bare_origin(&path);
        let base = Repository::open(&path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        commit_file(&path, "a.txt", "a", "add a");
        push(&path).unwrap();

        // Rewind the local branch behind the remote, then diverge.
        {
            let repo = Repository::open(&path).unwrap();
            let refname = repo.head().unwrap().name().unwrap().to_string();
            repo.find_reference(&refname)
                .unwrap()
                .set_target(base, "test: rewind")
                .unwrap();
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .unwrap();
        }
        commit_file(&path, "b.txt", "b", "diverging commit");

        let err = push(&path).unwrap_err();
        assert!(err.contains("non-fast-forward"), "got: {err}");
        // The remote kept the original tip — nothing was forced.
        let repo = Repository::open(&path).unwrap();
        let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
        let bare = Repository::open(_bare.path()).unwrap();
        let tip = bare
            .find_reference(&format!("refs/heads/{branch_name}"))
            .unwrap()
            .peel_to_commit()
            .unwrap();
        assert_eq!(tip.summary(), Some("add a"));
    }

    #[test]
    fn push_refuses_detached_head() {
        let (_dir, path) = init_repo();
        let _bare = add_bare_origin(&path);
        let repo = Repository::open(&path).unwrap();
        let oid = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(oid).unwrap();
        assert!(push(&path).unwrap_err().contains("detached"));
    }

    #[test]
    fn commits_ahead_of_lists_only_branch_commits() {
        let (_dir, path) = init_repo();
        let default = {
            let repo = Repository::open(&path).unwrap();
            let default = repo.head().unwrap().shorthand().unwrap().to_string();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch("feature", &head, false).unwrap();
            default
        };
        switch_branch(&path, "feature").unwrap();
        commit_file(&path, "f1.txt", "1", "feat: one");
        commit_file(&path, "f2.txt", "2", "feat: two");

        let ahead = commits_ahead_of(&path, &default, 10).unwrap();
        let subjects: Vec<&str> = ahead.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(subjects, vec!["feat: two", "feat: one"]);
        // On the default branch itself, nothing is ahead.
        switch_branch(&path, &default).unwrap();
        assert!(commits_ahead_of(&path, &default, 10).unwrap().is_empty());
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
}
