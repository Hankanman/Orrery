//! Discover git repos under the configured roots and extract their metadata
//! (git state via libgit2, README-derived name/description, heuristic language
//! and activity). Pure/synchronous — callers run it off the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use git2::{Branch, Repository, StatusOptions};
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::model::{Activity, GitStatus, Host, Repo};

const SEVEN_DAYS: i64 = 7 * 24 * 3600;
const THIRTY_DAYS: i64 = 30 * 24 * 3600;

/// Scan all roots and return the discovered repos, marking favorites.
pub fn scan(roots: &[String], depth: usize, ignore: &[String], favorites: &HashSet<String>, now: i64) -> Vec<Repo> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ignore_set = build_ignore(ignore);

    // Discover unique repo paths first (cheap directory walk), then build the
    // per-repo metadata (libgit2 status + README + language) in parallel — each
    // repo is independent and this is the bulk of scan time.
    let mut seen = HashSet::new();
    let mut targets: Vec<(PathBuf, &str, bool)> = Vec::new();
    for root in roots {
        for repo_path in find_repos(&expand(root), depth, &ignore_set) {
            let id = repo_path.to_string_lossy().into_owned();
            if seen.insert(id.clone()) {
                targets.push((repo_path, root.as_str(), favorites.contains(&id)));
            }
        }
    }

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(targets.len().max(1));
    let out = std::sync::Mutex::new(Vec::with_capacity(targets.len()));
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (out, next, targets) = (&out, &next, &targets);
        for _ in 0..threads {
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((path, root, fav)) = targets.get(i) else { break };
                if let Some(repo) = build_repo(path, root, *fav, now) {
                    out.lock().unwrap_or_else(|e| e.into_inner()).push(repo);
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Just the discovered repo paths (no metadata) — used by the watcher to decide
/// what to watch, far cheaper than a full scan.
pub(crate) fn repo_paths(roots: &[String], depth: usize, ignore: &[String]) -> Vec<PathBuf> {
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

fn build_repo(path: &Path, root: &str, favorite: bool, now: i64) -> Option<Repo> {
    let repo = Repository::open(path).ok()?;

    let git = git_status(&repo);
    let last_commit_unix = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.time().seconds())
        .unwrap_or(0);

    let (host, slug, remote_host) = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(String::from))
        .map(|url| parse_remote(&url))
        .unwrap_or((None, None, None));

    let dir_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    let (readme_title, description) = read_readme(path);
    let display_name = readme_title
        .or_else(|| slug.as_ref().and_then(|s| s.rsplit('/').next().map(String::from)))
        .unwrap_or_else(|| dir_name.clone());

    let activity = classify_activity(last_commit_unix, now);

    Some(Repo {
        id: path.to_string_lossy().into_owned(),
        display_name,
        slug,
        path: abbreviate(path),
        description,
        language: detect_language(path),
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

fn git_status(repo: &Repository) -> GitStatus {
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
        .unwrap_or_else(|| "HEAD".to_string());

    let (ahead, behind) = ahead_behind(repo).unwrap_or((0, 0));

    let mut opts = StatusOptions::new();
    // Count an untracked directory as one entry (like `git status -s`) rather
    // than recursing into it — otherwise a large untracked dir inflates "dirty".
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

fn ahead_behind(repo: &Repository) -> Option<(u32, u32)> {
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    let local = head.target()?;
    let upstream = Branch::wrap(head).upstream().ok()?;
    let upstream_oid = upstream.get().target()?;
    let (a, b) = repo.graph_ahead_behind(local, upstream_oid).ok()?;
    Some((a as u32, b as u32))
}

/// Parse an origin remote URL into (host, "owner/repo", domain).
fn parse_remote(url: &str) -> (Option<Host>, Option<String>, Option<String>) {
    // Strip protocol and any user@ prefix, then split the host from the path.
    let after_scheme = url.rsplit_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let tail = after_scheme
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(after_scheme);
    // tail is like "github.com/owner/repo.git" or "github.com:owner/repo.git"
    let (domain_raw, path_part) = tail.split_once(['/', ':']).unwrap_or((tail, ""));
    let path = path_part.trim_end_matches('/').trim_end_matches(".git");

    // Detect the provider from the host only (not the whole URL).
    let host = if domain_raw.contains("github.com") {
        Some(Host::Github)
    } else if domain_raw.contains("gitlab") {
        Some(Host::Gitlab)
    } else {
        None
    };

    let slug = {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 2 {
            Some(format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]))
        } else {
            None
        }
    };
    let domain = (!domain_raw.is_empty()).then(|| domain_raw.to_string());

    (host, slug, domain)
}

/// Returns (display title from first H1, first descriptive paragraph).
fn read_readme(path: &Path) -> (Option<String>, Option<String>) {
    let candidates = ["README.md", "Readme.md", "readme.md", "README.markdown", "README"];
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
    let stripped = s.replace(['*', '`', '_'], "");
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
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

fn extension_language(path: &Path) -> Option<String> {
    const EXT: &[(&str, &str)] = &[
        ("rs", "Rust"),
        ("ts", "TypeScript"),
        ("tsx", "TypeScript"),
        ("js", "JavaScript"),
        ("jsx", "JavaScript"),
        ("py", "Python"),
        ("go", "Go"),
        ("rb", "Ruby"),
        ("java", "Java"),
        ("kt", "Kotlin"),
        ("swift", "Swift"),
        ("c", "C"),
        ("h", "C"),
        ("cpp", "C++"),
        ("cc", "C++"),
        ("hpp", "C++"),
        ("cs", "C#"),
        ("php", "PHP"),
        ("sh", "Shell"),
        ("lua", "Lua"),
        ("zig", "Zig"),
    ];
    let map: HashMap<&str, &str> = EXT.iter().copied().collect();
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for entry in WalkDir::new(path)
        .max_depth(2)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != "node_modules")
        .flatten()
    {
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if let Some(lang) = map.get(ext) {
                *counts.entry(lang).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(lang, _)| lang.to_string())
}

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Expand a leading `~` to the home directory.
pub(crate) fn expand(path: &str) -> PathBuf {
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

    #[test]
    fn parse_remote_https_and_ssh_github() {
        assert_eq!(
            parse_remote("https://github.com/owner/repo.git"),
            (Some(Host::Github), Some("owner/repo".into()), Some("github.com".into()))
        );
        assert_eq!(
            parse_remote("git@github.com:owner/repo.git"),
            (Some(Host::Github), Some("owner/repo".into()), Some("github.com".into()))
        );
        // no trailing .git
        assert_eq!(parse_remote("https://github.com/owner/repo").1, Some("owner/repo".into()));
    }

    #[test]
    fn parse_remote_gitlab_including_self_hosted_and_nested() {
        assert_eq!(
            parse_remote("https://gitlab.com/group/proj.git"),
            (Some(Host::Gitlab), Some("group/proj".into()), Some("gitlab.com".into()))
        );
        // self-hosted GitLab → detected by host, domain captured for API base
        let (host, _, domain) = parse_remote("ssh://git@gitlab.example.com/team/app.git");
        assert_eq!(host, Some(Host::Gitlab));
        assert_eq!(domain, Some("gitlab.example.com".into()));
        // nested groups → last two path components
        assert_eq!(parse_remote("https://gitlab.com/group/sub/proj.git").1, Some("sub/proj".into()));
    }

    #[test]
    fn parse_remote_unknown_host_has_no_host_but_keeps_slug_and_domain() {
        assert_eq!(
            parse_remote("https://example.com/foo/bar.git"),
            (None, Some("foo/bar".into()), Some("example.com".into()))
        );
    }

    #[test]
    fn clean_markdown_strips_emphasis_and_links() {
        assert_eq!(clean_markdown("**Bold** `code` _x_"), "Bold code x");
        assert_eq!(clean_markdown("[Orrery](https://orrery.app) rocks"), "Orrery rocks");
        // multiple links on one line must all collapse to their text
        assert_eq!(clean_markdown("[A](u1) and [B](u2)"), "A and B");
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
        assert_eq!(classify_activity(now - 40 * 24 * 3600, now), Activity::Stale);
        // exact boundaries: comparison is strict `<`, so 7d → Idle, 30d → Stale
        assert_eq!(classify_activity(now - 7 * 24 * 3600, now), Activity::Idle);
        assert_eq!(classify_activity(now - 30 * 24 * 3600, now), Activity::Stale);
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
        assert!(!names.iter().any(|n| n == "sub"), "must not descend into a found repo");
        assert!(!names.iter().any(|n| n == "pkg"), "must skip ignored dirs");
    }
}
