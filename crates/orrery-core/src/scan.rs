//! Discover git repos under the configured roots and extract their metadata
//! (git state via libgit2, README-derived name/description, heuristic language
//! and activity). Pure/synchronous — callers run it off the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use git2::Repository;
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::forge::TrustedHosts;
use crate::model::{Activity, Host, Repo};

const SEVEN_DAYS: i64 = 7 * 24 * 3600;
const THIRTY_DAYS: i64 = 30 * 24 * 3600;

/// Scan all roots and return the discovered repos, marking favorites.
/// `gitlab_hosts` is the user's self-hosted GitLab allowlist, so a GitLab
/// whose hostname doesn't start with `gitlab.` is still recognised (see
/// [`parse_remote`]). Linked worktrees are not included; see
/// [`scan_with_worktrees`].
///
/// The Gitea / Forgejo allowlist isn't a parameter here: it is read from the
/// (cached) loaded config. Prefer [`scan_with_hosts`], which takes both
/// allowlists explicitly.
pub fn scan(
    roots: &[String],
    depth: usize,
    ignore: &[String],
    favorites: &HashSet<String>,
    gitlab_hosts: &[String],
    now: i64,
) -> Vec<Repo> {
    let hosts = legacy_hosts(gitlab_hosts);
    scan_with_hosts(roots, depth, ignore, favorites, &hosts, now)
}

/// The allowlists for the GitLab-only [`scan`] / [`scan_repo`] signatures:
/// the given GitLab list plus the configured Gitea list.
fn legacy_hosts(gitlab_hosts: &[String]) -> TrustedHosts {
    TrustedHosts {
        gitlab: gitlab_hosts.to_vec(),
        gitea: crate::config::load().gitea_hosts,
    }
}

/// [`scan`] with both self-hosted allowlists (GitLab and Gitea / Forgejo),
/// usually `TrustedHosts::from_config(&cfg)`.
pub fn scan_with_hosts(
    roots: &[String],
    depth: usize,
    ignore: &[String],
    favorites: &HashSet<String>,
    hosts: &TrustedHosts,
    now: i64,
) -> Vec<Repo> {
    let ignore_set = build_ignore(ignore);
    // Discover unique repo paths first (cheap directory walk), then build the
    // per-repo metadata in parallel.
    let targets = main_targets(roots, depth, &ignore_set, favorites);
    build_all(&targets, hosts, now)
}

/// [`scan_with_hosts`], plus every linked worktree of the fleet as its own [`Repo`],
/// marked with `worktree_of` (the main checkout's id) and `worktree_branch`,
/// so the UI can group it under the parent's card rather than show it as an
/// unrelated repo.
///
/// Worktrees come from two places: each main checkout's own worktree list
/// (libgit2), which finds them wherever they live, and directories under a
/// root whose `.git` is a *file* pointing at a linked worktree (so a worktree
/// whose main checkout sits outside every root is still found). Submodules
/// also have a `.git` file but aren't worktrees, so they stay skipped.
///
/// Tool-managed worktrees are skipped: any whose path (below the main
/// checkout, or below the root it sits under) has a component matching the
/// `ignore` globs (the default config ignores `.claude`, which covers
/// `.claude/worktrees/`), and Orrery's own dispatch worktrees under the app
/// data dir, which the agent views already track.
pub fn scan_with_worktrees(
    roots: &[String],
    depth: usize,
    ignore: &[String],
    favorites: &HashSet<String>,
    hosts: &TrustedHosts,
    now: i64,
) -> Vec<Repo> {
    let ignore_set = build_ignore(ignore);
    let mut targets = main_targets(roots, depth, &ignore_set, favorites);
    let worktrees = worktree_targets(roots, depth, &ignore_set, favorites, &targets);
    targets.extend(worktrees);
    build_all(&targets, hosts, now)
}

/// One checkout to build a [`Repo`] for.
struct Target {
    path: PathBuf,
    root: String,
    favorite: bool,
    /// The main checkout's id, for a linked worktree.
    worktree_of: Option<String>,
}

/// The main checkouts under every root, deduped across overlapping roots.
fn main_targets(
    roots: &[String],
    depth: usize,
    ignore_set: &GlobSet,
    favorites: &HashSet<String>,
) -> Vec<Target> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for root in roots {
        for repo_path in find_repos(&expand(root), depth, ignore_set) {
            let id = repo_path.to_string_lossy().into_owned();
            if seen.insert(id.clone()) {
                targets.push(Target {
                    path: repo_path,
                    root: root.clone(),
                    favorite: favorites.contains(&id),
                    worktree_of: None,
                });
            }
        }
    }
    targets
}

/// Linked worktrees of the main checkouts in `mains`, plus linked worktrees
/// found directly under a root (see [`scan_with_worktrees`]).
fn worktree_targets(
    roots: &[String],
    depth: usize,
    ignore_set: &GlobSet,
    favorites: &HashSet<String>,
    mains: &[Target],
) -> Vec<Target> {
    let expanded: Vec<(PathBuf, &String)> = roots.iter().map(|r| (expand(r), r)).collect();
    let dispatch_dir = dirs::data_dir().map(|d| d.join("orrery").join("worktrees"));
    let mut seen: HashSet<PathBuf> = mains.iter().map(|t| canonical(&t.path)).collect();
    let mut out = Vec::new();
    let mut push = |path: PathBuf, parent: &Path, root: &str| {
        let skip = dispatch_dir.as_ref().is_some_and(|d| path.starts_with(d))
            || worktree_ignored(&path, parent, &expanded, ignore_set)
            || !seen.insert(canonical(&path));
        if skip {
            return;
        }
        let id = path.to_string_lossy().into_owned();
        out.push(Target {
            favorite: favorites.contains(&id),
            path,
            root: root.to_string(),
            worktree_of: Some(parent.to_string_lossy().into_owned()),
        });
    };

    for main in mains {
        for wt in linked_worktree_paths(&main.path) {
            push(wt, &main.path, &main.root);
        }
    }
    for (root_path, root) in &expanded {
        for path in find_linked_worktrees(root_path, depth, ignore_set) {
            if let Some(parent) = worktree_parent(&path) {
                push(path, &parent, root.as_str());
            }
        }
    }
    out
}

/// Live linked worktrees of the repo at `main` (missing/pruned ones skipped).
fn linked_worktree_paths(main: &Path) -> Vec<PathBuf> {
    let Ok(repo) = Repository::open(main) else {
        return Vec::new();
    };
    let Ok(names) = repo.worktrees() else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|n| n.ok().flatten())
        .filter_map(|name| repo.find_worktree(name).ok())
        .filter(|wt| wt.validate().is_ok())
        .map(|wt| wt.path().to_path_buf())
        .collect()
}

/// The main checkout a linked worktree belongs to: the working directory of
/// its common git dir (or the common dir itself for a bare main repo). `None`
/// when `path` isn't a linked worktree (e.g. a submodule).
fn worktree_parent(path: &Path) -> Option<PathBuf> {
    let repo = Repository::open(path).ok()?;
    if !repo.is_worktree() {
        return None;
    }
    let common = canonical(repo.commondir());
    Some(match common.file_name() {
        Some(n) if n == ".git" => common.parent()?.to_path_buf(),
        _ => common,
    })
}

/// Whether a worktree is tool-managed per the ignore globs: any component of
/// its path below the main checkout (when it lives inside it), else below the
/// root it sits under, matches. Components *above* that base are the user's
/// own directory layout and aren't matched.
fn worktree_ignored(
    path: &Path,
    parent: &Path,
    roots: &[(PathBuf, &String)],
    ignore_set: &GlobSet,
) -> bool {
    let base = if path.starts_with(parent) {
        Some(parent)
    } else {
        roots
            .iter()
            .map(|(p, _)| p.as_path())
            .filter(|p| path.starts_with(p))
            .max_by_key(|p| p.as_os_str().len())
    };
    let rel = match base.and_then(|b| path.strip_prefix(b).ok()) {
        Some(rel) => rel,
        // Outside every root: judge only its own directory name.
        None => match path.file_name() {
            Some(name) => Path::new(name),
            None => return false,
        },
    };
    rel.components()
        .any(|c| ignore_set.is_match(c.as_os_str().to_string_lossy().as_ref()))
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Build every target's [`Repo`] in parallel: each repo is independent, and
/// the per-repo metadata (libgit2 status + README + language) is the bulk of
/// scan time.
fn build_all(targets: &[Target], hosts: &TrustedHosts, now: i64) -> Vec<Repo> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(targets.len().max(1));
    let out = std::sync::Mutex::new(Vec::with_capacity(targets.len()));
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (out, next) = (&out, &next);
        for _ in 0..threads {
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(t) = targets.get(i) else {
                    break;
                };
                let Some(mut repo) = build_repo(&t.path, &t.root, t.favorite, hosts, now) else {
                    continue;
                };
                if let Some(parent) = &t.worktree_of {
                    repo.worktree_of = Some(parent.clone());
                    repo.worktree_branch =
                        (repo.git.branch != "HEAD").then(|| repo.git.branch.clone());
                }
                out.lock().unwrap_or_else(|e| e.into_inner()).push(repo);
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Rebuild one already-discovered repo's metadata (a scoped rescan after a
/// filesystem event), with the same rules as [`scan`]. `None` when `path` is
/// no longer a repo (deleted or moved), so the caller drops it. Reads the
/// Gitea allowlist from config like [`scan`]; prefer [`scan_repo_with_hosts`].
pub fn scan_repo(
    path: &Path,
    root: &str,
    favorite: bool,
    gitlab_hosts: &[String],
    now: i64,
) -> Option<Repo> {
    build_repo(path, root, favorite, &legacy_hosts(gitlab_hosts), now)
}

/// [`scan_repo`] with both self-hosted allowlists (see [`scan_with_hosts`]).
pub fn scan_repo_with_hosts(
    path: &Path,
    root: &str,
    favorite: bool,
    hosts: &TrustedHosts,
    now: i64,
) -> Option<Repo> {
    build_repo(path, root, favorite, hosts, now)
}

/// Just the discovered repo paths (no metadata) — used by the watcher to decide
/// what to watch, far cheaper than a full scan.
pub fn repo_paths(roots: &[String], depth: usize, ignore: &[String]) -> Vec<PathBuf> {
    let ignore_set = build_ignore(ignore);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        for path in find_repos(&expand(root), depth, &ignore_set) {
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    out
}

fn build_ignore(ignore: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in ignore {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Walk `root` up to `depth` levels, collecting directories that contain a
/// `.git` entry. Skips ignored directory names and does not descend into a
/// repo once found (so submodules/nested repos aren't double-counted).
fn find_repos(root: &Path, depth: usize, ignore: &GlobSet) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    let mut it = WalkDir::new(root).max_depth(depth).into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name != "." && ignore.is_match(name.as_ref()) {
            it.skip_current_dir();
            continue;
        }
        let dotgit = entry.path().join(".git");
        if dotgit.exists() {
            // A `.git` *directory* is a real working checkout. A `.git` *file* is
            // a linked-worktree or submodule pointer — skip those so the same
            // repository isn't listed twice. Either way, don't descend further.
            if dotgit.is_dir() {
                repos.push(entry.path().to_path_buf());
            }
            it.skip_current_dir();
        }
    }
    repos
}

/// Directories under `root` (to `depth`) whose `.git` is a *file*: linked
/// worktree candidates, with the same ignore/no-descend rules as
/// [`find_repos`]. Submodule pointers (also `.git` files) are filtered out
/// later by [`worktree_parent`].
fn find_linked_worktrees(root: &Path, depth: usize, ignore: &GlobSet) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut it = WalkDir::new(root).max_depth(depth).into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name != "." && ignore.is_match(name.as_ref()) {
            it.skip_current_dir();
            continue;
        }
        let dotgit = entry.path().join(".git");
        if dotgit.exists() {
            if dotgit.is_file() {
                found.push(entry.path().to_path_buf());
            }
            it.skip_current_dir();
        }
    }
    found
}

fn build_repo(
    path: &Path,
    root: &str,
    favorite: bool,
    hosts: &TrustedHosts,
    now: i64,
) -> Option<Repo> {
    let repo = Repository::open(path).ok()?;

    let git = crate::git_ops::status_of(&repo);
    let head_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let last_commit_unix = head_commit
        .as_ref()
        .map(|c| c.time().seconds())
        .unwrap_or(0);

    let (host, slug, remote_host) = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().map(String::from))
        .map(|url| parse_remote(&url, hosts))
        .unwrap_or((None, None, None));

    let dir_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    let (readme_title, description) = read_readme(path);
    let display_name = readme_title
        .or_else(|| {
            slug.as_ref()
                .and_then(|s| s.rsplit('/').next().map(String::from))
        })
        .unwrap_or_else(|| dir_name.clone());

    let activity = classify_activity(last_commit_unix, now);

    Some(Repo {
        id: path.to_string_lossy().into_owned(),
        display_name,
        slug,
        path: abbreviate(path),
        description,
        language: cached_language(path, head_commit.map(|c| c.id())),
        git,
        last_commit_unix,
        activity,
        root: root.to_string(),
        host,
        remote_host,
        stars: 0,
        topics: Vec::new(),
        open_issues: 0,
        latest_release: None,
        private: false,
        favorite,
        ai_summary: None,
        worktree_of: None,
        worktree_branch: None,
    })
}

/// Map last-commit recency to an activity bucket (no commit → stale).
fn classify_activity(last_commit_unix: i64, now: i64) -> Activity {
    if last_commit_unix == 0 {
        return Activity::Stale;
    }
    match now - last_commit_unix {
        d if d < SEVEN_DAYS => Activity::Active,
        d if d < THIRTY_DAYS => Activity::Idle,
        _ => Activity::Stale,
    }
}

/// Parse an origin remote URL into (host, slug, domain).
///
/// The provider is decided by an *exact* host match — `github.com` /
/// `www.github.com` for GitHub; `gitlab.com`, a valid DNS host whose first
/// label is `gitlab` (self-hosted `gitlab.acme.io`), or a host on the user's
/// `gitlab_hosts` allowlist (`code.acme.io`), for GitLab — so lookalikes
/// like `notgithub.com` or `github.company.com` aren't sent to the GitHub API
/// and handed another repo's stars/visibility. (A self-hosted GitLab only ever
/// gets a token when it's on the user's `gitlab_hosts` allowlist; see
/// `forge::gitlab_host_trusted`.) Gitea / Forgejo is `codeberg.org` or a host
/// on the `gitea_hosts` allowlist; there is no name heuristic for it.
///
/// The slug is the full project path: `owner/repo` on GitHub and Gitea
/// (exactly two segments, else none), every group level on GitLab
/// (`group/sub/proj`). It is interpolated into API URL paths, so a slug that
/// fails `forge::valid_slug` (bad chars, `..` segments) is dropped.
fn parse_remote(url: &str, hosts: &TrustedHosts) -> (Option<Host>, Option<String>, Option<String>) {
    // Split the authority ([user@]host[:port]) from the path. scp-like remotes
    // ("git@host:owner/repo.git") use ':' as the separator and have no port.
    let (authority, path_part) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => match url.split_once(':') {
            // A ':' after a '/' means a local path, not scp syntax.
            Some((auth, path)) if !auth.contains('/') => (auth, path),
            _ => ("", url),
        },
    };
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let domain_raw = match host_port.find(']') {
        // Bracketed IPv6 literal; keep it whole (it never matches a provider).
        Some(end) if host_port.starts_with('[') => &host_port[..=end],
        _ => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    let path = path_part.trim_end_matches('/').trim_end_matches(".git");

    // Detect the provider from the host only (not the whole URL), exactly.
    let lower = domain_raw.to_ascii_lowercase();
    let host = match lower.as_str() {
        "github.com" | "www.github.com" => Some(Host::Github),
        "gitlab.com" | "www.gitlab.com" => Some(Host::Gitlab),
        d if d.starts_with("gitlab.") && crate::forge::valid_host(d) => Some(Host::Gitlab),
        // Allowlisted self-hosted GitLab under any name. Detection only: the
        // token-egress check in `forge` is separate and unchanged.
        d if crate::forge::gitlab_host_trusted(d, &hosts.gitlab) => Some(Host::Gitlab),
        // Codeberg, or an allowlisted Gitea / Forgejo.
        d if crate::forge::gitea_host_trusted(d, &hosts.gitea) => Some(Host::Gitea),
        _ => None,
    };

    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let slug = match host {
        Some(Host::Github | Host::Gitea) if parts.len() != 2 => None,
        _ if parts.len() < 2 => None,
        _ => Some(parts.join("/")),
    }
    .filter(|s| crate::forge::valid_slug(s));
    let domain = (!domain_raw.is_empty()).then(|| domain_raw.to_string());

    (host, slug, domain)
}

/// Returns (display title from first H1, first descriptive paragraph).
fn read_readme(path: &Path) -> (Option<String>, Option<String>) {
    let candidates = [
        "README.md",
        "Readme.md",
        "readme.md",
        "README.markdown",
        "README",
    ];
    let content = candidates
        .iter()
        .find_map(|name| std::fs::read_to_string(path.join(name)).ok());
    let Some(content) = content else {
        return (None, None);
    };

    let mut title = None;
    let mut description = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if title.is_none() {
            if let Some(h1) = line.strip_prefix("# ") {
                let cleaned = clean_markdown(h1);
                if !cleaned.is_empty() {
                    title = Some(cleaned);
                }
                continue;
            }
        }
        // First non-heading, non-badge, non-decoration line → description.
        if description.is_none()
            && !line.starts_with('#')
            && !line.starts_with('[')
            && !line.starts_with('!')
            && !line.starts_with('<')
            && !line.starts_with('=')
            && !line.starts_with('-')
            && !line.starts_with('|')
            && !line.starts_with('>')
        {
            let cleaned = clean_markdown(line);
            if cleaned.len() > 3 {
                description = Some(truncate(&cleaned, 200));
            }
        }
        if title.is_some() && description.is_some() {
            break;
        }
    }
    (title, description)
}

/// Strip the most common inline markdown so titles/descriptions read cleanly.
fn clean_markdown(s: &str) -> String {
    let stripped = strip_emphasis(s);
    // Collapse every [text](url) → text, left to right (handles multiple links).
    let mut out = String::with_capacity(stripped.len());
    let mut rest = stripped.as_str();
    while let Some(bracket) = rest.find("](") {
        let Some(open) = rest[..bracket].rfind('[') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..bracket]);
        let after = &rest[bracket + 2..];
        match after.find(')') {
            Some(close) => rest = &after[close + 1..],
            None => {
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Drop `*` and backtick markers, and `_` where it can be emphasis — at a word
/// edge. A `_` inside a word (`my_crate`) is part of the name and is kept.
fn strip_emphasis(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let keep = match c {
            '*' | '`' => false,
            '_' => {
                let prev = i.checked_sub(1).map(|j| chars[j]);
                let next = chars.get(i + 1);
                prev.is_some_and(char::is_alphanumeric) && next.is_some_and(|n| n.is_alphanumeric())
            }
            _ => true,
        };
        if keep {
            out.push(c);
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

/// [`detect_language`], cached per repo for the life of the process and keyed
/// on the HEAD commit: the extension fallback walks the tree, and most repos
/// are unchanged between rescans. A new commit re-detects; a working-tree-only
/// change is picked up when the watcher reports it ([`invalidate_language`]).
/// Repos with no commits aren't cached.
fn cached_language(path: &Path, head: Option<git2::Oid>) -> Option<String> {
    let Some(head) = head else {
        return detect_language(path);
    };
    let cache = language_cache();
    if let Some((oid, lang)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if *oid == head {
            return lang.clone();
        }
    }
    let lang = detect_language(path);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), (head, lang.clone()));
    lang
}

type LanguageCache = Mutex<HashMap<PathBuf, (git2::Oid, Option<String>)>>;

fn language_cache() -> &'static LanguageCache {
    static CACHE: OnceLock<LanguageCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Forget a repo's cached language so its next scan re-detects it — for a
/// rescan triggered by a working-tree change (a new manifest, a pile of new
/// source files) that leaves HEAD where it was.
pub fn invalidate_language(path: &Path) {
    language_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(path);
}

/// Heuristic primary language: manifest files first, then a shallow extension
/// frequency scan as a fallback.
fn detect_language(path: &Path) -> Option<String> {
    const MANIFESTS: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("go.mod", "Go"),
        ("pyproject.toml", "Python"),
        ("requirements.txt", "Python"),
        ("setup.py", "Python"),
        ("Gemfile", "Ruby"),
        ("composer.json", "PHP"),
        ("pom.xml", "Java"),
        ("build.gradle", "Java"),
        ("pubspec.yaml", "Dart"),
        ("mix.exs", "Elixir"),
        ("CMakeLists.txt", "C++"),
    ];
    for (file, lang) in MANIFESTS {
        if path.join(file).exists() {
            // package.json is special: TS if a tsconfig is present.
            return Some(lang.to_string());
        }
    }
    if path.join("package.json").exists() {
        return Some(if path.join("tsconfig.json").exists() {
            "TypeScript".to_string()
        } else {
            "JavaScript".to_string()
        });
    }
    extension_language(path)
}

/// Source extension → language. Code languages only (config/markup like JSON or
/// CSS rarely indicate a repo's *primary* language and would drown out code).
/// Names mirror `devicon_stem` in the UI so the language mark resolves.
const EXT_LANG: &[(&str, &str)] = &[
    ("rs", "Rust"),
    ("ts", "TypeScript"),
    ("tsx", "TypeScript"),
    ("mts", "TypeScript"),
    ("cts", "TypeScript"),
    ("js", "JavaScript"),
    ("jsx", "JavaScript"),
    ("mjs", "JavaScript"),
    ("cjs", "JavaScript"),
    ("py", "Python"),
    ("pyi", "Python"),
    ("go", "Go"),
    ("rb", "Ruby"),
    ("java", "Java"),
    ("kt", "Kotlin"),
    ("kts", "Kotlin"),
    ("swift", "Swift"),
    ("c", "C"),
    ("h", "C"),
    ("cpp", "C++"),
    ("cc", "C++"),
    ("cxx", "C++"),
    ("hpp", "C++"),
    ("hh", "C++"),
    ("cs", "C#"),
    ("php", "PHP"),
    ("sh", "Shell"),
    ("bash", "Shell"),
    ("zsh", "Shell"),
    ("lua", "Lua"),
    ("zig", "Zig"),
    ("vue", "Vue"),
    ("svelte", "Svelte"),
    ("dart", "Dart"),
    ("ex", "Elixir"),
    ("exs", "Elixir"),
    ("hs", "Haskell"),
    ("scala", "Scala"),
    ("sc", "Scala"),
    ("nix", "Nix"),
    ("nim", "Nim"),
    ("clj", "Clojure"),
    ("cljs", "Clojure"),
    ("erl", "Erlang"),
    ("ml", "OCaml"),
];

/// Directory names pruned from the language scan — VCS, dependency/build output,
/// and virtualenvs — so vendored code doesn't dominate the count.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "venv",
    ".venv",
    "__pycache__",
    ".next",
    ".svelte-kit",
    "Pods",
];

fn extension_language(path: &Path) -> Option<String> {
    let map: HashMap<&str, &str> = EXT_LANG.iter().copied().collect();
    let mut counts: HashMap<&str, u32> = HashMap::new();
    // Walk a few levels deep (covers src/main/java/… layouts) but prune vendored
    // and hidden directories so the count reflects the project's own code.
    for entry in WalkDir::new(path)
        .max_depth(4)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            let pruned = SKIP_DIRS.contains(&name)
                // hidden dirs (.github, .cargo, …) below the root
                || (e.depth() > 0 && e.file_type().is_dir() && name.starts_with('.'));
            !pruned
        })
        .flatten()
    {
        if let Some(lang) = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| map.get(ext.to_ascii_lowercase().as_str()).copied())
        {
            *counts.entry(lang).or_default() += 1;
        }
    }
    // Most files wins; break ties by name for a deterministic result.
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(lang, _)| lang.to_string())
}

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Expand a leading `~` to the home directory.
pub fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    if path == "~" {
        if let Some(h) = home() {
            return h;
        }
    }
    PathBuf::from(path)
}

/// Abbreviate the home prefix to `~` for display.
fn abbreviate(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(h) = home() {
        let h = h.to_string_lossy();
        if let Some(rest) = s.strip_prefix(h.as_ref()) {
            return format!("~{rest}");
        }
    }
    s.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The GitLab-allowlist-only form most tests below use (shadows the glob
    /// import); Gitea detection has its own test against `super::parse_remote`.
    fn parse_remote(
        url: &str,
        gitlab_hosts: &[String],
    ) -> (Option<Host>, Option<String>, Option<String>) {
        let hosts = TrustedHosts {
            gitlab: gitlab_hosts.to_vec(),
            gitea: Vec::new(),
        };
        super::parse_remote(url, &hosts)
    }

    #[test]
    fn parse_remote_detects_codeberg_and_allowlisted_gitea() {
        let none = TrustedHosts::default();
        assert_eq!(
            super::parse_remote("https://codeberg.org/forgejo/forgejo.git", &none),
            (
                Some(Host::Gitea),
                Some("forgejo/forgejo".into()),
                Some("codeberg.org".into())
            )
        );
        assert_eq!(
            super::parse_remote("git@codeberg.org:o/r.git", &none).0,
            Some(Host::Gitea)
        );
        // Gitea has no nested groups: three segments is not a slug.
        assert_eq!(
            super::parse_remote("https://codeberg.org/a/b/c", &none).1,
            None
        );
        // A self-hosted Forgejo is only Gitea when allowlisted.
        let hosts = TrustedHosts {
            gitlab: Vec::new(),
            gitea: vec!["Git.Acme.io".into()],
        };
        assert_eq!(
            super::parse_remote("ssh://git@git.acme.io:2222/team/app.git", &hosts),
            (
                Some(Host::Gitea),
                Some("team/app".into()),
                Some("git.acme.io".into())
            )
        );
        assert_eq!(
            super::parse_remote("https://git.acme.io/team/app", &none).0,
            None
        );
        // Lookalikes stay unknown.
        assert_eq!(
            super::parse_remote("https://codeberg.org.evil.io/o/r", &none).0,
            None
        );
    }

    #[test]
    fn parse_remote_https_and_ssh_github() {
        assert_eq!(
            parse_remote("https://github.com/owner/repo.git", &[]),
            (
                Some(Host::Github),
                Some("owner/repo".into()),
                Some("github.com".into())
            )
        );
        assert_eq!(
            parse_remote("git@github.com:owner/repo.git", &[]),
            (
                Some(Host::Github),
                Some("owner/repo".into()),
                Some("github.com".into())
            )
        );
        // no trailing .git
        assert_eq!(
            parse_remote("https://github.com/owner/repo", &[]).1,
            Some("owner/repo".into())
        );
    }

    #[test]
    fn parse_remote_gitlab_including_self_hosted_and_nested() {
        assert_eq!(
            parse_remote("https://gitlab.com/group/proj.git", &[]),
            (
                Some(Host::Gitlab),
                Some("group/proj".into()),
                Some("gitlab.com".into())
            )
        );
        // self-hosted GitLab → detected by host, domain captured for API base
        let (host, _, domain) = parse_remote("ssh://git@gitlab.example.com/team/app.git", &[]);
        assert_eq!(host, Some(Host::Gitlab));
        assert_eq!(domain, Some("gitlab.example.com".into()));
        // nested groups → the full path (the API needs every group level)
        assert_eq!(
            parse_remote("https://gitlab.com/group/sub/proj.git", &[]).1,
            Some("group/sub/proj".into())
        );
        assert_eq!(
            parse_remote("git@gitlab.acme.io:a/b/c/proj.git", &[]).1,
            Some("a/b/c/proj".into())
        );
        // ssh:// with a port: the port is neither domain nor slug.
        assert_eq!(
            parse_remote("ssh://git@gitlab.acme.io:2222/group/sub/proj.git", &[]),
            (
                Some(Host::Gitlab),
                Some("group/sub/proj".into()),
                Some("gitlab.acme.io".into())
            )
        );
    }

    #[test]
    fn parse_remote_host_detection_is_exact_not_substring() {
        // Lookalikes must not be treated as GitHub (they'd be enriched with
        // another repo's stars/private flag from api.github.com).
        for url in [
            "https://github.company.com/o/r.git",
            "https://notgithub.com/o/r.git",
            "git@github.com.evil.io:o/r.git",
            "https://evilgithub.com/o/r",
        ] {
            assert_eq!(parse_remote(url, &[]).0, None, "{url}");
        }
        assert_eq!(
            parse_remote("https://www.github.com/o/r.git", &[]).0,
            Some(Host::Github)
        );
        assert_eq!(
            parse_remote("https://GitHub.com/o/r", &[]).0,
            Some(Host::Github)
        );
        // "gitlab" merely appearing in the host isn't GitLab.
        for url in [
            "https://mygitlab.corp/o/r.git",
            "https://notgitlab.com/o/r.git",
            "https://169.254.169.254/o/r.git",
        ] {
            assert_eq!(parse_remote(url, &[]).0, None, "{url}");
        }
        assert_eq!(
            parse_remote("https://gitlab.com/o/r", &[]).0,
            Some(Host::Gitlab)
        );
    }

    #[test]
    fn parse_remote_rejects_unsafe_or_malformed_slugs() {
        // Slugs are interpolated into API paths — traversal must not survive.
        assert_eq!(parse_remote("https://github.com/o/../../user", &[]).1, None);
        assert_eq!(
            parse_remote("https://gitlab.com/g/../proj.git", &[]).1,
            None
        );
        assert_eq!(parse_remote("https://github.com/o/r%2F..", &[]).1, None);
        assert_eq!(parse_remote("https://github.com/o/r?x=1", &[]).1, None);
        // GitHub slugs are exactly owner/name.
        assert_eq!(parse_remote("https://github.com/o/r/extra", &[]).1, None);
        assert_eq!(parse_remote("https://github.com/o", &[]).1, None);
        // Credentials in the URL never leak into the domain.
        assert_eq!(
            parse_remote("https://user:tok@github.com/o/r.git", &[]),
            (
                Some(Host::Github),
                Some("o/r".into()),
                Some("github.com".into())
            )
        );
    }

    #[test]
    fn parse_remote_unknown_host_has_no_host_but_keeps_slug_and_domain() {
        assert_eq!(
            parse_remote("https://example.com/foo/bar.git", &[]),
            (None, Some("foo/bar".into()), Some("example.com".into()))
        );
    }

    #[test]
    fn parse_remote_detects_allowlisted_self_hosted_gitlab() {
        let hosts = vec!["Code.Acme.io".to_string()];
        // Not `gitlab.*`, but on the allowlist (case-insensitive) → GitLab.
        assert_eq!(
            parse_remote("git@code.acme.io:team/app.git", &hosts),
            (
                Some(Host::Gitlab),
                Some("team/app".into()),
                Some("code.acme.io".into())
            )
        );
        // Off the allowlist it stays unknown.
        assert_eq!(parse_remote("git@code.acme.io:team/app.git", &[]).0, None);
        assert_eq!(parse_remote("https://other.io/team/app", &hosts).0, None);
        // An allowlisted IP literal still fails `valid_host`, so isn't GitLab.
        let ip = vec!["169.254.169.254".to_string()];
        assert_eq!(parse_remote("https://169.254.169.254/o/r", &ip).0, None);
    }

    #[test]
    fn clean_markdown_strips_emphasis_and_links() {
        assert_eq!(clean_markdown("**Bold** `code` _x_"), "Bold code x");
        assert_eq!(
            clean_markdown("[Orrery](https://orrery.app) rocks"),
            "Orrery rocks"
        );
        // multiple links on one line must all collapse to their text
        assert_eq!(clean_markdown("[A](u1) and [B](u2)"), "A and B");
    }

    #[test]
    fn clean_markdown_keeps_underscores_inside_words() {
        assert_eq!(clean_markdown("my_crate"), "my_crate");
        assert_eq!(clean_markdown("`my_crate` is _great_"), "my_crate is great");
        assert_eq!(
            clean_markdown("__bold__ snake_case_name"),
            "bold snake_case_name"
        );
    }

    #[test]
    fn read_readme_takes_h1_title_and_first_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("README.md"),
            "# **my_tool**\n\n[![ci](badge.svg)](ci)\n\n> a quote\n\nA *fast* tool for [things](https://x).\n\nMore text.\n",
        )
        .unwrap();
        let (title, desc) = read_readme(dir.path());
        assert_eq!(title.as_deref(), Some("my_tool"));
        assert_eq!(desc.as_deref(), Some("A fast tool for things."));
    }

    #[test]
    fn read_readme_handles_missing_and_title_only() {
        let none = tempfile::tempdir().unwrap();
        assert_eq!(read_readme(none.path()), (None, None));

        // Lowercase name, no H1: the first prose line is the description.
        let plain = tempfile::tempdir().unwrap();
        fs::write(plain.path().join("readme.md"), "Just a description here\n").unwrap();
        assert_eq!(
            read_readme(plain.path()),
            (None, Some("Just a description here".into()))
        );

        // A long description is truncated with an ellipsis.
        let long = tempfile::tempdir().unwrap();
        fs::write(
            long.path().join("README.md"),
            format!("# T\n{}\n", "x".repeat(300)),
        )
        .unwrap();
        let (title, desc) = read_readme(long.path());
        assert_eq!(title.as_deref(), Some("T"));
        let desc = desc.unwrap();
        assert_eq!(desc.chars().count(), 201);
        assert!(desc.ends_with('…'));
    }

    #[test]
    fn scan_builds_repo_metadata_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proj");
        fs::create_dir_all(&path).unwrap();
        let repo = Repository::init(&path).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "t").unwrap();
            cfg.set_str("user.email", "t@t").unwrap();
        }
        repo.remote("origin", "git@github.com:owner/proj.git")
            .unwrap();
        fs::write(
            path.join("README.md"),
            "# Proj Title\n\nDoes useful things.\n",
        )
        .unwrap();
        fs::write(path.join("Cargo.toml"), "[package]").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.add_path(Path::new("Cargo.toml")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // One uncommitted file → dirty count of 1.
        fs::write(path.join("wip.txt"), "x").unwrap();
        // A non-repo sibling dir is ignored.
        fs::create_dir_all(root.path().join("not-a-repo")).unwrap();

        let id = path.to_string_lossy().into_owned();
        let favorites: HashSet<String> = [id.clone()].into_iter().collect();
        let now = sig.when().seconds() + 60;
        let roots = vec![root.path().to_string_lossy().into_owned()];
        // `scan_with_hosts`, not `scan`: the latter reads the user's config.
        let none = TrustedHosts::default();
        let repos = scan_with_hosts(&roots, 3, &[], &favorites, &none, now);

        assert_eq!(repos.len(), 1);
        let r = &repos[0];
        assert_eq!(r.id, id);
        assert_eq!(r.display_name, "Proj Title");
        assert_eq!(r.description.as_deref(), Some("Does useful things."));
        assert_eq!(r.language.as_deref(), Some("Rust"));
        assert_eq!(r.slug.as_deref(), Some("owner/proj"));
        assert_eq!(r.host, Some(Host::Github));
        assert_eq!(r.git.dirty, 1);
        assert_eq!(r.activity, Activity::Active);
        assert!(r.favorite);
        assert_eq!(r.root, roots[0]);

        // A rescan of the unchanged repo hits the language cache and agrees.
        assert_eq!(
            scan_with_hosts(&roots, 3, &[], &favorites, &none, now)[0]
                .language
                .as_deref(),
            Some("Rust")
        );
    }

    #[test]
    fn scan_with_worktrees_marks_linked_worktrees_and_skips_tool_managed() {
        let (dir, main_path) = crate::git_ops::test_support::init_repo();
        let root = tempfile::tempdir().unwrap();
        // Move the main checkout under a root so the walk finds it.
        let main = root.path().join("proj");
        fs::rename(&main_path, &main).unwrap();
        drop(dir);
        let repo = Repository::open(&main).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();

        // A sibling worktree on its own branch: a fleet member.
        let branch = repo.branch("feature", &head, false).unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch.get()));
        let sibling = root.path().join("proj-feature");
        repo.worktree("feature", &sibling, Some(&opts)).unwrap();

        // A tool-managed worktree inside the repo under `.claude/`: skipped.
        fs::create_dir_all(main.join(".claude/worktrees")).unwrap();
        let tool = main.join(".claude/worktrees/agent-1");
        repo.worktree("agent-1", &tool, None).unwrap();

        // A `.git` file that isn't a worktree (broken submodule pointer).
        fs::create_dir_all(root.path().join("stray")).unwrap();
        fs::write(root.path().join("stray/.git"), "gitdir: /nowhere\n").unwrap();

        let roots = vec![root.path().to_string_lossy().into_owned()];
        let ignore = vec![".claude".to_string()];
        let none = HashSet::new();

        // Plain `scan` is unchanged: just the main checkout.
        let plain = scan(&roots, 3, &ignore, &none, &[], 0);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].worktree_of, None);

        let hosts = TrustedHosts::default();
        let mut repos = scan_with_worktrees(&roots, 3, &ignore, &none, &hosts, 0);
        repos.sort_by(|a, b| a.id.cmp(&b.id));
        let ids: Vec<&str> = repos.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(repos.len(), 2, "{ids:?}");
        let main_id = main.to_string_lossy().into_owned();
        assert_eq!(repos[0].id, main_id);
        assert_eq!(repos[0].worktree_of, None);
        let wt = &repos[1];
        assert!(wt.id.ends_with("proj-feature"), "{}", wt.id);
        assert_eq!(
            canonical(Path::new(wt.worktree_of.as_deref().unwrap())),
            canonical(&main)
        );
        assert_eq!(wt.worktree_branch.as_deref(), Some("feature"));
        assert_eq!(wt.root, roots[0]);

        // Without the ignore glob the `.claude` worktree is a member too.
        let all = scan_with_worktrees(&roots, 3, &[], &none, &hosts, 0);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn cached_language_redetects_on_new_head() {
        let dir = tempfile::tempdir().unwrap();
        let a = git2::Oid::from_str("1111111111111111111111111111111111111111").unwrap();
        let b = git2::Oid::from_str("2222222222222222222222222222222222222222").unwrap();
        fs::write(dir.path().join("x.py"), "").unwrap();
        assert_eq!(cached_language(dir.path(), Some(a)), Some("Python".into()));
        // Same HEAD → cached, even though the tree changed.
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(cached_language(dir.path(), Some(a)), Some("Python".into()));
        // New HEAD → re-detected.
        assert_eq!(cached_language(dir.path(), Some(b)), Some("Rust".into()));
        // No HEAD → never cached.
        assert_eq!(cached_language(dir.path(), None), Some("Rust".into()));
        // A working-tree change on the same HEAD shows once invalidated.
        fs::write(dir.path().join("go.mod"), "").unwrap();
        fs::remove_file(dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(cached_language(dir.path(), Some(b)), Some("Rust".into()));
        invalidate_language(dir.path());
        assert_eq!(cached_language(dir.path(), Some(b)), Some("Go".into()));
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_needed() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn classify_activity_buckets() {
        let now = 1_000_000_000;
        assert_eq!(classify_activity(0, now), Activity::Stale);
        assert_eq!(classify_activity(now - 3600, now), Activity::Active);
        assert_eq!(classify_activity(now - 10 * 24 * 3600, now), Activity::Idle);
        assert_eq!(
            classify_activity(now - 40 * 24 * 3600, now),
            Activity::Stale
        );
        // exact boundaries: comparison is strict `<`, so 7d → Idle, 30d → Stale
        assert_eq!(classify_activity(now - 7 * 24 * 3600, now), Activity::Idle);
        assert_eq!(
            classify_activity(now - 30 * 24 * 3600, now),
            Activity::Stale
        );
    }

    #[test]
    fn expand_and_abbreviate_round_trip_home() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(expand("~/dev/x"), home.join("dev/x"));
        assert_eq!(expand("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(abbreviate(&home.join("dev/x")), "~/dev/x");
    }

    #[test]
    fn detect_language_prefers_manifests() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_language(dir.path()), Some("Rust".into()));

        let js = tempfile::tempdir().unwrap();
        fs::write(js.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_language(js.path()), Some("JavaScript".into()));

        let ts = tempfile::tempdir().unwrap();
        fs::write(ts.path().join("package.json"), "{}").unwrap();
        fs::write(ts.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect_language(ts.path()), Some("TypeScript".into()));
    }

    #[test]
    fn detect_language_falls_back_to_extensions() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.py"), "").unwrap();
        fs::write(dir.path().join("b.py"), "").unwrap();
        fs::write(dir.path().join("c.js"), "").unwrap();
        assert_eq!(detect_language(dir.path()), Some("Python".into()));
    }

    #[test]
    fn detect_language_prunes_vendored_and_reads_nested() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The project's own code lives a few levels deep…
        fs::create_dir_all(root.join("src/app/core")).unwrap();
        fs::write(root.join("src/app/core/main.rs"), "").unwrap();
        fs::write(root.join("src/app/core/util.rs"), "").unwrap();
        // …while a vendored dir is stuffed with another language that must NOT
        // win (it's pruned).
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        for i in 0..20 {
            fs::write(root.join(format!("node_modules/pkg/{i}.js")), "").unwrap();
        }
        assert_eq!(detect_language(root), Some("Rust".into()));
    }

    #[test]
    fn detect_language_covers_added_extensions() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("App.vue"), "").unwrap();
        assert_eq!(detect_language(dir.path()), Some("Vue".into()));
    }

    #[test]
    fn repo_paths_finds_repos_skips_ignored_and_nested() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/.git")).unwrap();
        fs::create_dir_all(root.join("b/.git")).unwrap();
        // nested repo inside a found repo — must not descend into it
        fs::create_dir_all(root.join("a/sub/.git")).unwrap();
        // ignored directory containing a repo — must be skipped
        fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();

        let roots = vec![root.to_string_lossy().into_owned()];
        let found = repo_paths(&roots, 4, &["node_modules".to_string()]);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(found.len(), 2, "found: {names:?}");
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
        assert!(
            !names.iter().any(|n| n == "sub"),
            "must not descend into a found repo"
        );
        assert!(!names.iter().any(|n| n == "pkg"), "must skip ignored dirs");
    }
}
