//! Bridge from `orrery-core` (`cache` / `model`) to a flat, render-ready `Row`.
//! The card reads `Row` and never touches the core's serde types directly.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::SharedString;
use orrery_core::{cache, config, model, scan};

/// Everything the grid card renders, flattened from `model::Repo`.
pub struct Row {
    pub id: SharedString,  // absolute path — launch cwd + favorite/cache key
    pub url: SharedString, // host web URL, or "" (open-on-host button)
    pub name: SharedString,
    pub slug: SharedString, // "owner/repo" or "no remote"
    pub path: SharedString,
    pub description: SharedString,
    pub language: SharedString, // "" when unknown
    pub branch: SharedString,
    pub age: SharedString, // e.g. "3d ago"
    pub release: SharedString,
    pub ai_summary: SharedString,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub stars: SharedString, // pre-formatted (e.g. "1.2k")
    pub host: SharedString,  // "github" / "gitlab" / "" (brand-icon name)
    pub private: bool,
    pub favorite: bool,
}

fn rel_age(last_commit_unix: i64, now: i64) -> String {
    if last_commit_unix <= 0 {
        return "—".into();
    }
    let secs = (now - last_commit_unix).max(0);
    let days = secs / 86_400;
    if days >= 365 {
        format!("{}y ago", days / 365)
    } else if days >= 1 {
        format!("{days}d ago")
    } else {
        let hours = secs / 3_600;
        if hours >= 1 {
            format!("{hours}h ago")
        } else {
            let mins = (secs / 60).max(1);
            format!("{mins}m ago")
        }
    }
}

/// Mirror of `formatStars` in the frontend: 1234 → "1.2k".
fn fmt_stars(stars: u32) -> String {
    if stars >= 1000 {
        format!("{:.1}k", stars as f64 / 1000.0)
    } else {
        stars.to_string()
    }
}

pub fn to_rows(repos: Vec<model::Repo>, now: i64) -> Vec<Row> {
    repos
        .into_iter()
        .map(|r| Row {
            id: r.id.into(),
            url: match (r.remote_host.as_deref(), r.slug.as_deref()) {
                (Some(host), Some(slug)) => format!("https://{host}/{slug}"),
                _ => String::new(),
            }
            .into(),
            name: r.display_name.into(),
            slug: r.slug.unwrap_or_else(|| "no remote".into()).into(),
            path: r.path.into(),
            description: r
                .description
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| "No README description.".into())
                .into(),
            language: r.language.unwrap_or_default().into(),
            branch: r.git.branch.into(),
            age: rel_age(r.last_commit_unix, now).into(),
            release: r.latest_release.unwrap_or_default().into(),
            ai_summary: r.ai_summary.unwrap_or_default().into(),
            ahead: r.git.ahead,
            behind: r.git.behind,
            dirty: r.git.dirty,
            stars: fmt_stars(r.stars).into(),
            host: match r.host {
                Some(model::Host::Github) => "github",
                Some(model::Host::Gitlab) => "gitlab",
                None => "",
            }
            .into(),
            private: r.private,
            favorite: r.favorite,
        })
        .collect()
}

/// Current Unix time in seconds (for relative ages); 0 if the clock is before
/// the epoch.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn count_roots(repos: &[model::Repo]) -> usize {
    repos
        .iter()
        .map(|r| r.root.as_str())
        .collect::<HashSet<&str>>()
        .len()
}

/// Load real repos from the shipping SQLite cache. Returns the rows plus the
/// number of distinct scanned roots (for the header's "N roots · M repos").
pub fn load(now: i64) -> (Vec<Row>, usize) {
    let repos = cache::load_repos();
    let n_roots = count_roots(&repos);
    (to_rows(repos, now), n_roots)
}

/// Re-scan the configured roots from disk (git-heavy — call off the UI thread),
/// refresh the cache, and return render-ready rows + root count. Mirrors the
/// Tauri `scan_repos` command: the filesystem watcher triggers this so the grid
/// reflects on-disk changes live.
pub fn rescan() -> (Vec<Row>, usize) {
    let now = now_unix();
    let cfg = config::load();
    let favorites = cache::favorites();
    let mut repos = scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &favorites, now);
    // Carry over persisted host enrichment (stars/visibility) until a fresh
    // enrich pass re-confirms it, then snapshot for instant next-launch paint.
    cache::apply_host_info(&mut repos);
    let _ = cache::store_repos(&repos);
    let n_roots = count_roots(&repos);
    (to_rows(repos, now), n_roots)
}
