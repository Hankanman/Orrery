//! Repo search and repo actions shared by the desktop launchers: KDE KRunner
//! ([`crate::krunner`]) and the GNOME Shell search provider
//! ([`crate::gnome_search`]). Both match the same cached repo list the same
//! way and act on a result through the same launch paths, so a query finds
//! the same repos whichever launcher it was typed into.
//!
//! Matches read the cached repo list (populated by the app's scans), so
//! there's no filesystem walk per keystroke, and that list is itself held in
//! memory for a few seconds so a burst of keystrokes doesn't re-query SQLite
//! each time.

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orrery_core::model::Repo;
use orrery_core::{cache, config, launch};

/// How long a loaded repo list serves matches before re-reading the cache.
/// Launchers query per keystroke; a few seconds covers a typed query while
/// still picking up a fresh scan promptly.
const REPOS_TTL: Duration = Duration::from_secs(5);

/// The shortest query (all terms together) worth matching. One character
/// matches nearly every repo and just floods the launcher.
const MIN_QUERY_CHARS: usize = 2;

/// The repo list as last loaded, and when.
static REPOS: Mutex<Option<(Instant, Arc<Vec<Repo>>)>> = Mutex::new(None);

/// The cached repo list, reloaded from SQLite once it's older than [`REPOS_TTL`].
pub(crate) fn repos() -> Arc<Vec<Repo>> {
    let Ok(mut slot) = REPOS.lock() else {
        return Arc::new(cache::load_repos());
    };
    if let Some((at, repos)) = slot.as_ref() {
        if at.elapsed() < REPOS_TTL {
            return repos.clone();
        }
    }
    let repos = Arc::new(cache::load_repos());
    *slot = Some((Instant::now(), repos.clone()));
    repos
}

/// Split a free-text query (KRunner) into lowercased search terms. GNOME Shell
/// already passes terms; [`normalise_terms`] lowercases those.
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Lowercase and drop empty terms (GNOME Shell's term list).
pub(crate) fn normalise_terms(terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// How well one repo matches every term, in `0.0..=1.0`, or `None` if any term
/// misses. Per term, a name prefix beats a name substring, which beats the
/// `owner/name` slug, which beats the path; the score is the mean over terms.
/// `terms` must already be lowercase.
fn score(name: &str, slug: Option<&str>, path: &str, terms: &[String]) -> Option<f64> {
    if terms.is_empty() || terms.iter().map(String::len).sum::<usize>() < MIN_QUERY_CHARS {
        return None;
    }
    let name = name.to_lowercase();
    let slug = slug.unwrap_or("").to_lowercase();
    let path = path.to_lowercase();
    let mut total = 0.0;
    for t in terms {
        total += if name.starts_with(t.as_str()) {
            1.0
        } else if name.contains(t.as_str()) {
            0.8
        } else if slug.contains(t.as_str()) {
            0.6
        } else if path.contains(t.as_str()) {
            0.4
        } else {
            return None;
        };
    }
    Some(total / terms.len() as f64)
}

/// Whether `repo` matches every term (for refining a previous result set).
pub(crate) fn matches(repo: &Repo, terms: &[String]) -> bool {
    score(&repo.display_name, repo.slug.as_deref(), &repo.path, terms).is_some()
}

/// The best `limit` repos for `terms`, highest score first (ties by name).
pub(crate) fn search<'a>(
    repos: &'a [Repo],
    terms: &[String],
    limit: usize,
) -> Vec<(&'a Repo, f64)> {
    let mut hits: Vec<(&Repo, f64)> = repos
        .iter()
        .filter_map(|r| score(&r.display_name, r.slug.as_deref(), &r.path, terms).map(|s| (r, s)))
        .collect();
    hits.sort_by(|(a, sa), (b, sb)| {
        sb.total_cmp(sa)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    hits.truncate(limit);
    hits
}

/// A result's secondary line: its `owner/name` slug, else its path.
pub(crate) fn subtitle(repo: &Repo) -> String {
    repo.slug.clone().unwrap_or_else(|| repo.path.clone())
}

/// The repo with this id, read fresh from the cache. The session bus is
/// unauthenticated, so launchers only ever act on an id that is a known repo,
/// never an arbitrary path a caller made up.
pub(crate) fn known(id: &str) -> Option<Repo> {
    cache::load_repos().into_iter().find(|r| r.id == id)
}

/// What a launcher can do with a matched repo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepoAction {
    /// Open in the configured IDE (the default).
    Ide,
    /// Open the configured terminal coding agent in the repo.
    Agent,
    /// Open the repo's folder in the file manager.
    Folder,
    /// Copy the repo's path to the clipboard.
    CopyPath,
}

/// Run `action` on the repo with this id, if it's a known repo. Errors are
/// returned for the caller to log.
pub(crate) async fn run(action: RepoAction, id: &str) -> Result<(), String> {
    if known(id).is_none() {
        return Err(format!("not a known repo: {id}"));
    }
    match action {
        RepoAction::Ide => launch::launch(&config::load().ide_command, id),
        RepoAction::Agent => launch::launch(&config::load().agent_command, id),
        RepoAction::Folder => launch::open(id),
        RepoAction::CopyPath => copy_to_clipboard(id).await,
    }
}

/// Put `text` on the clipboard. A background service has no window to own a
/// selection, so this asks the desktop's clipboard manager over D-Bus (KDE's
/// Klipper, always present where KRunner is), falling back to `wl-copy` on
/// another Wayland desktop.
async fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let klipper = async {
        let conn = zbus::Connection::session().await?;
        conn.call_method(
            Some("org.kde.klipper"),
            "/klipper",
            Some("org.kde.klipper.klipper"),
            "setClipboardContents",
            &(text,),
        )
        .await
    };
    let klipper_err = match klipper.await {
        Ok(_) => return Ok(()),
        Err(e) => e,
    };
    Command::new("wl-copy")
        .arg("--")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("no clipboard: Klipper ({klipper_err}), wl-copy ({e})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &str) -> Vec<String> {
        query_terms(q)
    }

    #[test]
    fn short_queries_match_nothing() {
        assert_eq!(score("Orrery", None, "~/dev/orrery", &terms("o")), None);
        assert_eq!(score("Orrery", None, "~/dev/orrery", &terms("")), None);
        assert!(score("Orrery", None, "~/dev/orrery", &terms("or")).is_some());
    }

    #[test]
    fn every_term_must_match() {
        let s = Some("Hankanman/Orrery");
        assert!(score("Orrery", s, "~/dev/orrery", &terms("hank orr")).is_some());
        assert_eq!(score("Orrery", s, "~/dev/orrery", &terms("orr zed")), None);
    }

    #[test]
    fn name_prefix_outranks_slug_and_path() {
        let prefix = score("Orrery", None, "~/x", &terms("orr")).unwrap();
        let inner = score("My Orrery", None, "~/x", &terms("orr")).unwrap();
        let slug = score("Thing", Some("me/orrery"), "~/x", &terms("orr")).unwrap();
        let path = score("Thing", None, "~/orrery", &terms("orr")).unwrap();
        assert!(prefix > inner && inner > slug && slug > path);
    }

    #[test]
    fn matching_ignores_case() {
        assert!(score("Orrery", None, "~/x", &terms("ORR")).is_some());
        assert_eq!(
            normalise_terms(&["  Orr ".into(), "".into()]),
            vec!["orr".to_string()]
        );
    }
}
