//! Central CI-status pass (#183): refresh the latest *default-branch* CI
//! state for every GitHub-remoted repo and persist it to the `ci_cache`
//! table, so `attention::CiFact`s — and the badges/tray/notifications behind
//! them — are fed centrally instead of per-view. The shape mirrors
//! [`crate::enrich`]: keyed (host domain, slug), TTL-paced, driven by the app
//! on the shared tokio runtime.
//!
//! **Errors never clobber cached state** (the #174 lesson): an `Err` from
//! [`github_ci`] — expired token, rate limit, 5xx — leaves the last
//! known state in place and is counted on the returned [`Outcome`] instead,
//! so the caller can surface an auth problem rather than watching CI facts
//! silently blank out. Only a definitive answer (including a definitive
//! "none") is written.
//!
//! **Hosts.** GitHub repos use the Actions API ([`github_ci`]); Gitea /
//! Forgejo / Codeberg repos use the combined commit-status API ([`gitea_ci`]),
//! which Gitea/Forgejo Actions, Woodpecker and other CI all report into.
//! **GitLab is out of scope** for now; its pipelines can layer on later
//! behind the same cache shape.

use std::collections::HashSet;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::attention::CiFact;
use crate::model::{Host, Repo};
use crate::{cache, enrich, forge, git_ops, oauth};

/// How long a cached CI entry stays fresh. CI is far more volatile than the
/// enrichment pass's stars/topics (6h TTL): a run can go red minutes after a
/// push, so entries are refetched after 5 minutes — still cheap (one small
/// request per repo, only for stale entries) against GitHub's 5000/hour
/// authenticated limit.
const TTL_SECS: i64 = 5 * 60;

/// Max concurrent CI-API requests in flight during a pass.
const CONCURRENCY: usize = 8;

/// A pending CI fetch: `(host, remote domain, slug, repo path)`. Domain +
/// slug is the `ci_cache` key (the #159 compound-key lesson); the path is
/// where the default branch is read from (local `origin/HEAD` — no extra API
/// call).
type Job = (Host, String, String, String);

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

/// Build the work list: GitHub- and Gitea-remoted repos with a slug whose
/// cache entry is stale/missing, deduped by (domain, slug) exactly like the
/// enrich pass (shared [`enrich::stale_remotes`]). Pure (no I/O);
/// unit-tested below.
fn build_jobs(repos: &[Repo], fresh: &HashSet<(String, String)>) -> Vec<Job> {
    // GitLab CI is out of scope (module docs).
    enrich::stale_remotes(repos, fresh, |r| {
        matches!(r.host, Some(Host::Github | Host::Gitea))
    })
    .into_iter()
    .filter_map(|(r, domain, slug)| Some((r.host?, domain, slug, r.id.clone())))
    .collect()
}

/// Drop the jobs the pass can't run with the tokens it has: GitHub's Actions
/// API needs a token, so without one GitHub jobs are skipped (nothing is
/// blanked). Gitea jobs always run — a public repo's statuses are readable
/// unauthenticated, and the token is attached only to a trusted host.
fn runnable(jobs: Vec<Job>, have_github_token: bool) -> Vec<Job> {
    jobs.into_iter()
        .filter(|(host, ..)| *host != Host::Github || have_github_token)
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

/// Refresh the default-branch CI state for every GitHub- or Gitea-remoted
/// repo whose cache entry is missing or older than the TTL, and persist the
/// results. Returns what happened (see [`Outcome`]); GitHub repos are skipped
/// without a GitHub token (an unconnected app legitimately has no CI source —
/// nothing is blanked).
///
/// `force` ignores the TTL and re-fetches every repo (the manual "Fetch all").
pub async fn refresh(repos: &[Repo], now: i64, force: bool) -> Outcome {
    let fresh = if force {
        HashSet::new()
    } else {
        cache::fresh_ci_keys(TTL_SECS, now)
    };
    let jobs = build_jobs(repos, &fresh);
    if jobs.is_empty() {
        return Outcome::default();
    }
    // Resolve the tokens once for the whole pass (they may spawn `gh`/`glab`),
    // not once per repo.
    let tokens = oauth::host_tokens_async().await;
    let jobs = runnable(jobs, tokens.github.is_some());
    if jobs.is_empty() {
        return Outcome::default();
    }
    let trusted = forge::TrustedHosts::from_config(&crate::config::load());

    let (tokens, trusted) = (&tokens, &trusted);
    let results: Vec<FetchResult> = stream::iter(jobs)
        .map(|(host, domain, slug, path)| async move {
            // Default branch from the local clone (origin/HEAD, else
            // main/master) — a fast ref read, no API call. `None` (bare
            // oddities) falls back to the latest run on any branch.
            let branch = git_ops::default_branch(&path);
            let status = match host {
                Host::Gitea => {
                    let token = tokens
                        .gitea
                        .as_deref()
                        .filter(|_| forge::gitea_host_trusted(&domain, &trusted.gitea));
                    gitea_ci(&domain, &slug, branch.as_deref(), token).await
                }
                _ => match tokens.github.as_deref() {
                    Some(token) => github_ci(token, &slug, branch.as_deref()).await,
                    None => Err("no GitHub token".into()),
                },
            };
            status.map(|status| (domain, slug, status))
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let fetched = results.len();
    cache::try_with_conn(|conn| Ok(apply_results_on(conn, results, now))).unwrap_or_else(|e| {
        Outcome {
            updated: 0,
            errors: fetched,
            last_error: Some(e),
        }
    })
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

// ── GitHub Actions fetch ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiStatus {
    /// "success" | "failure" | "pending" | "none"
    pub state: String,
    /// The run's web page (GitHub's `html_url`), when a run was found — lets
    /// surfaces link straight to the failing run.
    pub url: Option<String>,
}

impl CiStatus {
    fn none() -> Self {
        CiStatus {
            state: "none".into(),
            url: None,
        }
    }
}

/// Latest GitHub Actions run conclusion for a repo, filtered to `branch` when
/// given (the central CI pass passes the default branch so PR-branch runs
/// can't masquerade as trunk state). `token` is resolved once per pass by the
/// caller ([`refresh`]).
///
/// "No CI" (`state: "none"`) is only reported when GitHub answers
/// definitively: a 2xx with an empty run list, or a 404 (the repo isn't
/// visible to this token — deleted, renamed, or private without access — so
/// there is nothing to show). Auth failures (401 expired token), rate limits
/// (403/429), and server errors surface as `Err`, so a broken token reads as
/// an error, not as CI vanishing.
pub async fn github_ci(token: &str, slug: &str, branch: Option<&str>) -> Result<CiStatus, String> {
    #[derive(Deserialize)]
    struct Runs {
        #[serde(default)]
        workflow_runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        status: String,
        conclusion: Option<String>,
        html_url: Option<String>,
    }
    forge::check_github_slug(slug)?;
    let mut req = forge::client()
        .get(format!("https://api.github.com/repos/{slug}/actions/runs"))
        .query(&[("per_page", "1")]);
    if let Some(branch) = branch {
        req = req.query(&[("branch", branch)]);
    }
    let resp = forge::send(req.bearer_auth(token), forge::GH_CORE).await?;
    let status = resp.status();
    if !status.is_success() {
        if let Some(err) = ci_error_for_status(status) {
            return Err(err);
        }
        return Ok(CiStatus::none());
    }
    let runs: Runs = resp.json().await.map_err(|e| e.to_string())?;
    let Some(run) = runs.workflow_runs.into_iter().next() else {
        return Ok(CiStatus::none());
    };
    Ok(CiStatus {
        state: workflow_run_state(&run.status, run.conclusion.as_deref()).to_string(),
        url: run.html_url,
    })
}

/// How a non-2xx from the workflow-runs endpoint is treated: 404 means the
/// repo isn't visible with this token (deleted/renamed/no access) — genuinely
/// no CI to show, so `None`. Anything else (401 expired token, 403 rate
/// limit, 5xx) is a transport/auth failure the caller must see.
fn ci_error_for_status(status: reqwest::StatusCode) -> Option<String> {
    (status != reqwest::StatusCode::NOT_FOUND).then(|| format!("GitHub CI {status}"))
}

/// Map a workflow run's REST (status, conclusion) to our four-state vocabulary.
fn workflow_run_state(status: &str, conclusion: Option<&str>) -> &'static str {
    if status != "completed" {
        return "pending";
    }
    match conclusion {
        Some("success") => "success",
        Some("failure") | Some("timed_out") | Some("startup_failure") => "failure",
        _ => "none",
    }
}

// ── Gitea / Forgejo commit status ────────────────────────────────────────────

/// A Gitea `GET /repos/{owner}/{repo}/commits/{ref}/status` response (the
/// combined status of every context reported on that commit).
#[derive(Deserialize)]
struct GiteaCombined {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    statuses: Vec<GiteaStatus>,
}

#[derive(Deserialize)]
struct GiteaStatus {
    /// Older Gitea calls this `status`, newer adds `state`; read either.
    #[serde(default, alias = "state")]
    status: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
}

/// Map a Gitea combined status to our four-state vocabulary, linking the
/// first failing context (else the first context with a link). No contexts
/// at all is a definitive "none". `warning` is terminal but not a failure,
/// so it is ambient ("none") rather than raising attention. Pure.
fn gitea_combined_status(c: GiteaCombined) -> CiStatus {
    if c.statuses.is_empty() {
        return CiStatus::none();
    }
    let state = match c.state.as_deref() {
        Some("success") => "success",
        Some("failure") | Some("error") => "failure",
        Some("pending") => "pending",
        _ => "none",
    };
    let failing = |s: &&GiteaStatus| matches!(s.status.as_deref(), Some("failure" | "error"));
    let url = c
        .statuses
        .iter()
        .filter(|s| state != "failure" || failing(s))
        .find_map(|s| s.target_url.clone().filter(|u| !u.is_empty()));
    CiStatus {
        state: state.into(),
        url,
    }
}

/// Latest combined commit status of `branch` on a Gitea / Forgejo /
/// Codeberg repo. Same error rules as [`github_ci`]: a 404 (unknown repo or
/// ref) is a definitive "none"; 401/403/429/5xx are errors that leave the
/// cached state alone. `token` must already be filtered to a trusted host by
/// the caller. No default branch → "none" (Gitea can't resolve `HEAD` here).
pub async fn gitea_ci(
    domain: &str,
    slug: &str,
    branch: Option<&str>,
    token: Option<&str>,
) -> Result<CiStatus, String> {
    if !forge::valid_host(domain) {
        return Err(format!("refusing to query untrusted host: {domain}"));
    }
    if !forge::valid_gitea_slug(slug) {
        return Err(format!("not an owner/name slug: {slug}"));
    }
    let Some(branch) = branch else {
        return Ok(CiStatus::none());
    };
    let url = format!(
        "{}/repos/{slug}/commits/{}/status",
        forge::gitea_api(domain),
        urlencoding::encode(branch)
    );
    let mut req = forge::client().get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = forge::send(req, domain).await?;
    let status = resp.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(CiStatus::none());
        }
        return Err(format!("Gitea CI {status}"));
    }
    let combined: GiteaCombined = resp.json().await.map_err(|e| e.to_string())?;
    Ok(gitea_combined_status(combined))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::test_support::sample;
    use rusqlite::Connection;

    #[test]
    fn workflow_run_state_maps_status_and_conclusion() {
        assert_eq!(workflow_run_state("in_progress", None), "pending");
        assert_eq!(workflow_run_state("queued", Some("success")), "pending");
        assert_eq!(workflow_run_state("completed", Some("success")), "success");
        assert_eq!(workflow_run_state("completed", Some("failure")), "failure");
        assert_eq!(
            workflow_run_state("completed", Some("timed_out")),
            "failure"
        );
        assert_eq!(
            workflow_run_state("completed", Some("startup_failure")),
            "failure"
        );
        assert_eq!(workflow_run_state("completed", Some("cancelled")), "none");
        assert_eq!(workflow_run_state("completed", None), "none");
    }

    #[test]
    fn ci_errors_surface_auth_and_rate_limit_but_not_missing_repo() {
        use reqwest::StatusCode;
        // 404 = repo not visible to this token — genuinely nothing to show.
        assert_eq!(ci_error_for_status(StatusCode::NOT_FOUND), None);
        // Expired/revoked token, rate limit, and server errors must be errors,
        // not a silent "none" that blanks CI across the grid.
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            let err = ci_error_for_status(status).expect("must surface an error");
            assert!(err.contains(status.as_str()), "{err}");
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
    fn build_jobs_takes_github_and_gitea_repos_only() {
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
        let gt = sample("/gt", Host::Gitea, "codeberg.org", "o/gt");
        let jobs = build_jobs(&[gitlab, no_slug, no_host, gh, gt], &HashSet::new());
        assert_eq!(
            jobs,
            vec![
                (
                    Host::Github,
                    "github.com".into(),
                    "o/gh".into(),
                    "/gh".into()
                ),
                (
                    Host::Gitea,
                    "codeberg.org".into(),
                    "o/gt".into(),
                    "/gt".into()
                ),
            ]
        );
        // Without a GitHub token only the Gitea job survives.
        let kept = runnable(jobs.clone(), false);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, Host::Gitea);
        assert_eq!(runnable(jobs, true).len(), 2);
    }

    #[test]
    fn gitea_combined_status_maps_states_and_links_the_failure() {
        let parse = |v: serde_json::Value| {
            gitea_combined_status(serde_json::from_value::<GiteaCombined>(v).unwrap())
        };
        // Trimmed from a Codeberg combined-status response.
        let st = parse(serde_json::json!({
            "state": "failure",
            "sha": "abc",
            "total_count": 2,
            "statuses": [
                {"status": "success", "context": "lint", "target_url": "https://ci.example/1"},
                {"status": "failure", "context": "test", "target_url": "https://ci.example/2"}
            ]
        }));
        assert_eq!(st.state, "failure");
        assert_eq!(st.url.as_deref(), Some("https://ci.example/2"));

        let st = parse(serde_json::json!({
            "state": "success",
            "statuses": [{"state": "success", "target_url": "https://ci.example/9"}]
        }));
        assert_eq!(st.state, "success");
        assert_eq!(st.url.as_deref(), Some("https://ci.example/9"));

        assert_eq!(
            parse(serde_json::json!({"state": "error", "statuses": [{"status": "error"}]})).state,
            "failure"
        );
        assert_eq!(
            parse(serde_json::json!({"state": "pending", "statuses": [{"status": "pending"}]}))
                .state,
            "pending"
        );
        assert_eq!(
            parse(serde_json::json!({"state": "warning", "statuses": [{"status": "warning"}]}))
                .state,
            "none"
        );
        // No contexts reported: a definitive "none", whatever `state` says.
        let st = parse(serde_json::json!({"state": "pending", "statuses": []}));
        assert_eq!(st.state, "none");
        assert_eq!(st.url, None);
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
                (
                    Host::Github,
                    "github.com".into(),
                    "o/test".into(),
                    "/a".into()
                ),
                (
                    Host::Github,
                    "github.com".into(),
                    "o/stale".into(),
                    "/c".into()
                ),
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
