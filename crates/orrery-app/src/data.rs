//! Bridge from `orrery-core` (`cache` / `model`) to a flat, render-ready `Row`.
//! The card reads `Row` and never touches the core's serde types directly.

use gpui::SharedString;
use orrery_core::{cache, model};

/// Everything the grid card renders, flattened from `model::Repo`.
pub struct Row {
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

/// Load real repos from the shipping SQLite cache. Returns the rows plus the
/// number of distinct scanned roots (for the header's "N roots · M repos").
pub fn load(now: i64) -> (Vec<Row>, usize) {
    let repos = cache::load_repos();
    let roots: std::collections::HashSet<&str> = repos.iter().map(|r| r.root.as_str()).collect();
    let n_roots = roots.len();
    (to_rows(repos, now), n_roots)
}
