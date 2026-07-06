//! Central CI-status pass (#183): refresh the latest *default-branch* CI
//! state for every GitHub-remoted repo and persist it to the `ci_cache`
//! table, so `attention::CiFact`s — and the badges/tray/notifications behind
//! them — are fed centrally instead of per-view. The shape mirrors
//! [`crate::enrich`]: keyed (host domain, slug), TTL-paced, driven by the app
//! on the shared tokio runtime.
//!
//! **Errors never clobber cached state** (the #174 lesson): an `Err` from
//! [`inbox::github_ci`] — expired token, rate limit, 5xx — leaves the last
//! known state in place and is counted on the returned [`Outcome`] instead,
//! so the caller can surface an auth problem rather than watching CI facts
//! silently blank out. Only a definitive answer (including a definitive
//! "none") is written.
//!
//! **GitLab is out of scope** for now: `inbox::github_ci` only speaks the
//! GitHub Actions API, so jobs are built solely for `Host::Github` repos.
//! GitLab pipelines can layer on later behind the same cache shape.

use std::collections::HashSet;

use futures_util::stream::{self, StreamExt};

use crate::attention::CiFact;
use crate::inbox::{self, CiStatus};
use crate::model::{Host, Repo};
use crate::{cache, git_ops, oauth};

/// How long a cached CI entry stays fresh. CI is far more volatile than the
/// enrichment pass's stars/topics (6h TTL): a run can go red minutes after a
/// push, so entries are refetched after 5 minutes — still cheap (one small
/// request per repo, only for stale entries) against GitHub's 5000/hour
/// authenticated limit.
const TTL_SECS: i64 = 5 * 60;

/// Max concurrent CI-API requests in flight during a pass.
const CONCURRENCY: usize = 8;

/// A pending CI fetch: `(remote domain, slug, repo path)`. Domain + slug is
/// the `ci_cache` key (the #159 compound-key lesson); the path is where the
/// default branch is read from (local `origin/HEAD` — no extra API call).
type Job = (String, String, String);

/// What one pass did. `updated` counts entries (re)written; `errors` counts
/// fetches that failed *without* touching the cache (last known state kept),
/// with `last_error` describing the most recent one — so the caller can tell
/// "all fresh" (updated 0, errors 0) from "the pass is broken" (errors == the
/// whole job list) and surface the latter.
#[derive(Debug, Default)]
pub struct Outcome {
    pub updated: usize,
    pub errors: usize,
    pub last_error: Option<String>,
}

impl Outcome {
    /// True when the pass attempted work and none of it landed — the signal
    /// that something systemic (expired token, rate limit, outage) is wrong,
    /// as opposed to a repo-local hiccup.
    pub fn all_failed(&self) -> bool {
        self.errors > 0 && self.updated == 0
    }
}

/// Build the work list: GitHub-remoted repos with a slug whose cache entry is
/// stale/missing, deduped by (domain, slug) like the enrich pass. Pure (no
/// I/O); unit-tested below.
fn build_jobs(repos: &[Repo], fresh: &HashSet<(String, String)>) -> Vec<Job> {
    let mut seen = HashSet::new();
    repos
        .iter()
        .filter_map(|r| {
            if r.host != Some(Host::Github) {
                return None; // GitLab CI is out of scope (module docs).
            }
            let slug = r.slug.clone()?;
            let domain = r.remote_host.clone().unwrap_or_default();
            let key = (domain.clone(), slug.clone());
            if fresh.contains(&key) || !seen.insert(key) {
                return None;
            }
            Some((domain, slug, r.id.clone()))
        })
        .collect()
}

/// One fetch result, keyed for the cache write.
type FetchResult = Result<(String, String, CiStatus), String>;

/// Fold fetch results into cache writes: `Ok` entries are persisted, `Err`s
/// are only counted — never written — so a failing pass keeps the last known
/// state (#174). Split out (over a plain connection) for the regression test.
fn apply_results_on(conn: &rusqlite::Connection, results: Vec<FetchResult>, now: i64) -> Outcome {
    let mut outcome = Outcome::default();
    for result in results {
        match result {
            Ok((domain, slug, status)) => {
                cache::store_ci_state_on(
                    conn,
                    &domain,
                    &slug,
                    &status.state,
                    status.url.as_deref(),
                    now,
                );
                outcome.updated += 1;
            }
            Err(e) => {
                outcome.errors += 1;
                outcome.last_error = Some(e);
            }
        }
    }
    outcome
}

/// Refresh the default-branch CI state for every GitHub-remoted repo whose
/// cache entry is missing or older than the TTL, and persist the results.
/// Returns what happened (see [`Outcome`]); with no token the pass is a no-op
/// (an unconnected app legitimately has no CI source — nothing is blanked).
///
/// `force` ignores the TTL and re-fetches every repo (the manual "Fetch all").
pub async fn refresh(repos: &[Repo], now: i64, force: bool) -> Outcome {
    if oauth::github_token().is_none() {
        return Outcome::default();
    }
    let fresh = if force {
        HashSet::new()
    } else {
        cache::fresh_ci_keys(TTL_SECS, now)
    };
    let jobs = build_jobs(repos, &fresh);
    if jobs.is_empty() {
        return Outcome::default();
    }

    let results: Vec<FetchResult> = stream::iter(jobs)
        .map(|(domain, slug, path)| async move {
            // Default branch from the local clone (origin/HEAD, else
            // main/master) — a fast ref read, no API call. `None` (bare
            // oddities) falls back to the latest run on any branch.
            let branch = git_ops::default_branch(&path);
            match inbox::github_ci(&slug, branch.as_deref()).await {
                Ok(status) => Ok((domain, slug, status)),
                Err(e) => Err(e),
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    match cache::open() {
        Ok(conn) => apply_results_on(&conn, results, now),
        Err(e) => Outcome {
            updated: 0,
            errors: results.len(),
            last_error: Some(e),
        },
    }
}

/// Refresh CI state for the current cached repo snapshot, honoring the TTL.
/// Convenience for the app, which holds render rows rather than `Repo`s.
pub async fn refresh_cached(now: i64) -> Outcome {
    let repos = cache::load_repos();
    refresh(&repos, now, false).await
}

/// Build the attention model's [`CiFact`]s from the cached CI states,
/// restricted to remotes present in the current fleet — a cached row for a
/// repo that was deleted locally must not keep raising attention. Pure;
/// unit-tested below.
pub fn facts(
    states: &std::collections::HashMap<(String, String), cache::CiEntry>,
    repos: &[Repo],
) -> Vec<CiFact> {
    let present: HashSet<(&str, &str)> = repos
        .iter()
        .filter_map(|r| {
            let slug = r.slug.as_deref()?;
            Some((r.remote_host.as_deref().unwrap_or_default(), slug))
        })
        .collect();
    let mut facts: Vec<CiFact> = states
        .iter()
        .filter(|((host, slug), _)| present.contains(&(host.as_str(), slug.as_str())))
        .map(|((host, slug), entry)| CiFact {
            remote_host: host.clone(),
            slug: slug.clone(),
            state: entry.state.clone(),
            url: entry.url.clone(),
        })
        .collect();
    // HashMap iteration order is arbitrary; sort so attention::compute gets
    // deterministic input (its own sort is stable across equal items).
    facts.sort_by(|a, b| (&a.remote_host, &a.slug).cmp(&(&b.remote_host, &b.slug)));
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};
    use rusqlite::Connection;

    fn sample(id: &str, host: Host, domain: &str, slug: &str) -> Repo {
        Repo {
            id: id.to_string(),
            display_name: "Test".into(),
            slug: Some(slug.into()),
            path: "~/dev/test".into(),
            description: None,
            language: Some("Rust".into()),
            git: GitStatus::default(),
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: Some(host),
            remote_host: Some(domain.into()),
            stars: 0,
            topics: Vec::new(),
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
        }
    }

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        cache::init(&conn).unwrap();
        conn
    }

    fn key(domain: &str, slug: &str) -> (String, String) {
        (domain.into(), slug.into())
    }

    #[test]
    fn build_jobs_takes_github_repos_only() {
        // GitLab CI is out of scope; repos without a slug can't be queried.
        let gitlab = sample("/gl", Host::Gitlab, "gitlab.com", "o/gl");
        let no_slug = Repo {
            slug: None,
            ..sample("/ns", Host::Github, "github.com", "o/ns")
        };
        let no_host = Repo {
            host: None,
            ..sample("/nh", Host::Github, "github.com", "o/nh")
        };
        let gh = sample("/gh", Host::Github, "github.com", "o/gh");
        let jobs = build_jobs(&[gitlab, no_slug, no_host, gh], &HashSet::new());
        assert_eq!(
            jobs,
            vec![("github.com".into(), "o/gh".into(), "/gh".into())]
        );
    }

    #[test]
    fn build_jobs_dedupes_and_skips_fresh() {
        let repos = [
            sample("/a", Host::Github, "github.com", "o/test"),
            sample("/a-fork", Host::Github, "github.com", "o/test"),
            sample("/b", Host::Github, "github.com", "o/fresh"),
            sample("/c", Host::Github, "github.com", "o/stale"),
        ];
        let fresh = HashSet::from([key("github.com", "o/fresh")]);
        let jobs = build_jobs(&repos, &fresh);
        assert_eq!(
            jobs,
            vec![
                ("github.com".into(), "o/test".into(), "/a".into()),
                ("github.com".into(), "o/stale".into(), "/c".into()),
            ]
        );
    }

    #[test]
    fn errors_do_not_clobber_cached_state() {
        // Regression for the #174 lesson: an expired token (Err) must keep the
        // last known state — CI must read as an error, not as vanishing.
        let conn = mem();
        cache::store_ci_state_on(
            &conn,
            "github.com",
            "o/test",
            "failure",
            Some("https://github.com/o/test/actions/runs/1"),
            1_000,
        );

        let outcome =
            apply_results_on(&conn, vec![Err("GitHub CI 401 Unauthorized".into())], 2_000);
        assert_eq!(outcome.updated, 0);
        assert_eq!(outcome.errors, 1);
        assert_eq!(
            outcome.last_error.as_deref(),
            Some("GitHub CI 401 Unauthorized")
        );
        assert!(outcome.all_failed());

        let states = cache::all_ci_states_on(&conn);
        let entry = &states[&key("github.com", "o/test")];
        assert_eq!(entry.state, "failure", "Err must not overwrite state");
        assert_eq!(entry.updated_at, 1_000, "Err must not refresh the TTL");
    }

    #[test]
    fn ok_results_are_written_alongside_errors() {
        let conn = mem();
        let outcome = apply_results_on(
            &conn,
            vec![
                Ok((
                    "github.com".into(),
                    "o/red".into(),
                    CiStatus {
                        state: "failure".into(),
                        url: Some("https://github.com/o/red/actions/runs/7".into()),
                    },
                )),
                Err("GitHub CI 500".into()),
            ],
            1_000,
        );
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.errors, 1);
        assert!(!outcome.all_failed(), "partial success is not a dead pass");

        let states = cache::all_ci_states_on(&conn);
        let entry = &states[&key("github.com", "o/red")];
        assert_eq!(entry.state, "failure");
        assert_eq!(
            entry.url.as_deref(),
            Some("https://github.com/o/red/actions/runs/7")
        );
    }

    #[test]
    fn facts_cover_present_repos_only() {
        let mut states = std::collections::HashMap::new();
        states.insert(
            key("github.com", "o/test"),
            cache::CiEntry {
                state: "failure".into(),
                url: Some("https://github.com/o/test/actions/runs/2".into()),
                updated_at: 1_000,
            },
        );
        // A remote no longer in the fleet must not produce a fact...
        states.insert(
            key("github.com", "o/deleted"),
            cache::CiEntry {
                state: "failure".into(),
                url: None,
                updated_at: 1_000,
            },
        );
        // ...and neither may the same slug on a different host (#159).
        states.insert(
            key("gitlab.acme.io", "o/test"),
            cache::CiEntry {
                state: "failure".into(),
                url: None,
                updated_at: 1_000,
            },
        );
        let repos = [sample("/a", Host::Github, "github.com", "o/test")];
        let facts = facts(&states, &repos);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].remote_host, "github.com");
        assert_eq!(facts[0].slug, "o/test");
        assert_eq!(facts[0].state, "failure");
        assert_eq!(
            facts[0].url.as_deref(),
            Some("https://github.com/o/test/actions/runs/2")
        );
    }
}
