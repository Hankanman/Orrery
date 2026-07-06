//! Refresh host enrichment (stars / topics / open issues / latest release /
//! visibility) for scanned repos from GitHub & GitLab, then persist it to the
//! host cache so the grid can overlay it (see [`cache::apply_host_info`]).
//!
//! This is the producer side of the enrichment pipeline; `cache::apply_host_info`
//! / `data` is the consumer. It is network-bearing and async, so the app drives
//! it on the shared tokio runtime (`task::run`) rather than the gpui background
//! executor (which has no reactor).
//!
//! **Token egress.** GitHub tokens only ever go to `api.github.com`. GitLab
//! tokens are attached by [`forge::fetch`] only when the remote's domain is
//! trusted (`gitlab.com` or a host on the user's `gitlab_hosts` allowlist), so a
//! hostile repo remote can't exfiltrate one. Public metadata is still fetched
//! unauthenticated for untrusted hosts.

use std::collections::HashSet;

use futures_util::stream::{self, StreamExt};

use crate::model::{Host, Repo};
use crate::{cache, config, forge, oauth};

/// How long a cached host entry stays fresh. A rescan within this window does no
/// host-API calls (the entry is skipped), which keeps us well under GitHub's
/// unauthenticated 60/hour limit.
const TTL_SECS: i64 = 6 * 3_600;

/// Max concurrent host-API requests in flight during a refresh.
const CONCURRENCY: usize = 8;

/// A pending host-API fetch: `(host kind, remote domain, slug)`. The domain +
/// slug pair is also the host-cache key (see [`cache::store_host_info`]).
type Job = (Host, String, String);

/// Build the work list: repos with a host + slug whose cache entry is
/// stale/missing. Keyed and deduped by (remote domain, slug) — the same key the
/// host cache uses — so forks/mirrors of one remote aren't fetched twice, while
/// two repos that share a slug on *different* hosts (e.g. github.com + a
/// self-hosted GitLab) each keep their own job and freshness (#159).
///
/// Pure (no I/O): `fresh` comes from [`cache::fresh_host_keys`], empty when
/// forcing. Unit-tested below.
fn build_jobs(repos: &[Repo], fresh: &HashSet<(String, String)>) -> Vec<Job> {
    let mut seen = HashSet::new();
    repos
        .iter()
        .filter_map(|r| {
            let host = r.host?;
            let slug = r.slug.clone()?;
            let domain = r.remote_host.clone().unwrap_or_default();
            let key = (domain.clone(), slug.clone());
            if fresh.contains(&key) || !seen.insert(key) {
                return None;
            }
            Some((host, domain, slug))
        })
        .collect()
}

/// Re-fetch host enrichment for every repo with a recognized remote whose cache
/// entry is missing or older than the TTL, and persist the results. Returns the
/// number of entries whose enrichment was (re)written — `0` means nothing
/// changed (all fresh, or offline), so the caller can skip rebuilding the grid.
///
/// `force` ignores the TTL and re-fetches every repo (the manual "Fetch all").
pub async fn refresh(repos: &[Repo], now: i64, force: bool) -> usize {
    let cfg = config::load();
    let github = oauth::github_token();
    let gitlab = oauth::gitlab_token();
    let fresh = if force {
        HashSet::new()
    } else {
        cache::fresh_host_keys(TTL_SECS, now)
    };

    let jobs = build_jobs(repos, &fresh);
    if jobs.is_empty() {
        return 0;
    }

    let results = stream::iter(jobs)
        .map(|(host, domain, slug)| {
            let github = github.clone();
            let gitlab = gitlab.clone();
            let gitlab_hosts = cfg.gitlab_hosts.clone();
            async move {
                let token = match host {
                    Host::Github => github.as_deref(),
                    Host::Gitlab => gitlab.as_deref(),
                };
                match forge::fetch(host, &domain, &slug, token, &gitlab_hosts).await {
                    Ok(info) => Some((domain, slug, info)),
                    // A failed fetch (offline, rate-limited, 404, untrusted) just
                    // leaves the prior cached value in place — graceful by design.
                    Err(_) => None,
                }
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    let mut updated = 0;
    for (domain, slug, info) in results.into_iter().flatten() {
        cache::store_host_info(&domain, &slug, &info, now);
        updated += 1;
    }
    updated
}

/// Refresh enrichment for the current cached repo snapshot, honoring the TTL.
/// Convenience for the app, which holds render rows rather than `Repo`s.
pub async fn refresh_cached(now: i64) -> usize {
    let repos = cache::load_repos();
    refresh(&repos, now, false).await
}

/// Force-refresh enrichment for every cached repo, ignoring the TTL (the manual
/// "Fetch all" action).
pub async fn refresh_cached_all(now: i64) -> usize {
    let repos = cache::load_repos();
    refresh(&repos, now, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};

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

    fn key(domain: &str, slug: &str) -> (String, String) {
        (domain.into(), slug.into())
    }

    #[test]
    fn same_slug_on_two_hosts_builds_two_jobs() {
        // Regression for #159: a github.com repo and a self-hosted GitLab repo
        // sharing "o/test" must both be fetched, not deduped into one job.
        let repos = [
            sample("/a", Host::Github, "github.com", "o/test"),
            sample("/b", Host::Gitlab, "gitlab.acme.io", "o/test"),
        ];
        let jobs = build_jobs(&repos, &HashSet::new());
        assert_eq!(
            jobs,
            vec![
                (Host::Github, "github.com".into(), "o/test".into()),
                (Host::Gitlab, "gitlab.acme.io".into(), "o/test".into()),
            ]
        );
    }

    #[test]
    fn dedupes_forks_of_the_same_remote() {
        // Two checkouts of the same remote → one fetch.
        let repos = [
            sample("/a", Host::Github, "github.com", "o/test"),
            sample("/a-fork", Host::Github, "github.com", "o/test"),
            sample("/b", Host::Github, "github.com", "o/other"),
        ];
        let jobs = build_jobs(&repos, &HashSet::new());
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].2, "o/test");
        assert_eq!(jobs[1].2, "o/other");
    }

    #[test]
    fn freshness_is_per_host_not_per_slug() {
        // Regression for #159: a fresh github.com entry must not suppress the
        // fetch for the same slug on another host.
        let repos = [
            sample("/a", Host::Github, "github.com", "o/test"),
            sample("/b", Host::Gitlab, "gitlab.acme.io", "o/test"),
        ];
        let fresh = HashSet::from([key("github.com", "o/test")]);
        let jobs = build_jobs(&repos, &fresh);
        assert_eq!(
            jobs,
            vec![(Host::Gitlab, "gitlab.acme.io".into(), "o/test".into())]
        );
    }

    #[test]
    fn fresh_entries_are_skipped() {
        let repos = [
            sample("/a", Host::Github, "github.com", "o/fresh"),
            sample("/b", Host::Github, "github.com", "o/stale"),
        ];
        let fresh = HashSet::from([key("github.com", "o/fresh")]);
        let jobs = build_jobs(&repos, &fresh);
        assert_eq!(
            jobs,
            vec![(Host::Github, "github.com".into(), "o/stale".into())]
        );
    }

    #[test]
    fn repos_without_host_or_slug_are_skipped() {
        let no_host = Repo {
            host: None,
            ..sample("/a", Host::Github, "github.com", "o/test")
        };
        let no_slug = Repo {
            slug: None,
            ..sample("/b", Host::Github, "github.com", "o/test")
        };
        assert!(build_jobs(&[no_host, no_slug], &HashSet::new()).is_empty());
    }

    #[test]
    fn missing_remote_host_falls_back_to_empty_domain() {
        // A repo with a recognized host but no parsed domain still enriches,
        // keyed under the empty domain (matches how results are stored).
        let repo = Repo {
            remote_host: None,
            ..sample("/a", Host::Github, "github.com", "o/test")
        };
        let jobs = build_jobs(std::slice::from_ref(&repo), &HashSet::new());
        assert_eq!(jobs, vec![(Host::Github, String::new(), "o/test".into())]);
        // ...and its freshness is tracked under that same empty-domain key.
        let fresh = HashSet::from([key("", "o/test")]);
        assert!(build_jobs(std::slice::from_ref(&repo), &fresh).is_empty());
    }
}
