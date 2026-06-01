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
    let ignore_set = build_ignore(ignore);
    let mut seen = HashSet::new();
    let mut repos = Vec::new();

    for root in roots {
        let root_path = expand(root);
        for repo_path in find_repos(&root_path, depth, &ignore_set) {
            let id = repo_path.to_string_lossy().into_owned();
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(repo) = build_repo(&repo_path, root, favorites.contains(&id), now) {
                repos.push(repo);
            }
        }
    }
    repos
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
        if entry.path().join(".git").exists() {
            repos.push(entry.path().to_path_buf());
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

    let (host, slug) = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(String::from))
        .map(|url| parse_remote(&url))
        .unwrap_or((None, None));

    let dir_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    let (readme_title, description) = read_readme(path);
    let display_name = readme_title
        .or_else(|| slug.as_ref().and_then(|s| s.rsplit('/').next().map(String::from)))
        .unwrap_or_else(|| dir_name.clone());

    let activity = if last_commit_unix == 0 {
        Activity::Stale
    } else {
        match now - last_commit_unix {
            d if d < SEVEN_DAYS => Activity::Active,
            d if d < THIRTY_DAYS => Activity::Idle,
            _ => Activity::Stale,
        }
    };

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
        stars: 0,
        favorite,
        ai_summary: None,
    })
}

fn git_status(repo: &Repository) -> GitStatus {
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from))
        .unwrap_or_else(|| "HEAD".to_string());

    let (ahead, behind) = ahead_behind(repo).unwrap_or((0, 0));

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
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

/// Parse an origin remote URL into (host, "owner/repo").
fn parse_remote(url: &str) -> (Option<Host>, Option<String>) {
    let host = if url.contains("github.com") {
        Some(Host::Github)
    } else if url.contains("gitlab") {
        Some(Host::Gitlab)
    } else {
        None
    };

    // Strip protocol/host prefix, then a trailing ".git".
    let tail = url
        .rsplit_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    // tail is like "github.com/owner/repo.git" or "github.com:owner/repo.git"
    let path = tail
        .split_once(|c| c == '/' || c == ':')
        .map(|(_, rest)| rest)
        .unwrap_or(tail)
        .trim_end_matches('/')
        .trim_end_matches(".git");

    let slug = {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 2 {
            Some(format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]))
        } else {
            None
        }
    };

    (host, slug)
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
    let mut out = s.replace(['*', '`', '_'], "");
    // Collapse [text](url) → text
    while let (Some(open), Some(close)) = (out.find("]("), out.rfind(')')) {
        if let Some(start) = out[..open].rfind('[') {
            if close > open {
                let text = out[start + 1..open].to_string();
                out.replace_range(start..=close, &text);
                continue;
            }
        }
        break;
    }
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
fn expand(path: &str) -> PathBuf {
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
