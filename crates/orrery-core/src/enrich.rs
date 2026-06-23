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

use futures_util::stream::{self, StreamExt};

use crate::model::{Host, Repo};
use crate::{cache, config, forge, oauth};

/// How long a cached host entry stays fresh. A rescan within this window does no
/// host-API calls (the slug is skipped), which keeps us well under GitHub's
/// unauthenticated 60/hour limit.
const TTL_SECS: i64 = 6 * 3_600;

/// Max concurrent host-API requests in flight during a refresh.
const CONCURRENCY: usize = 8;

/// Re-fetch host enrichment for every repo with a recognized remote whose cache
/// entry is missing or older than the TTL, and persist the results. Returns the
/// number of slugs whose enrichment was (re)written — `0` means nothing changed
/// (all fresh, or offline), so the caller can skip rebuilding the grid.
///
/// `force` ignores the TTL and re-fetches every repo (the manual "Fetch all").
pub async fn refresh(repos: &[Repo], now: i64, force: bool) -> usize {
    let cfg = config::load();
    let github = oauth::github_token();
    let gitlab = oauth::gitlab_token();
    let fresh = if force {
        std::collections::HashSet::new()
    } else {
        cache::fresh_host_slugs(TTL_SECS, now)
    };

    // Build the work list: repos with a host + slug whose cache is stale/missing.
    // Dedupe by slug so forks/mirrors of the same remote aren't fetched twice.
    let mut seen = std::collections::HashSet::new();
    let jobs: Vec<(Host, String, String)> = repos
        .iter()
        .filter_map(|r| {
            let host = r.host?;
            let slug = r.slug.clone()?;
            if fresh.contains(&slug) || !seen.insert(slug.clone()) {
                return None;
            }
            let domain = r.remote_host.clone().unwrap_or_default();
            Some((host, domain, slug))
        })
        .collect();

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
                    Ok(info) => Some((slug, info)),
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
    for (slug, info) in results.into_iter().flatten() {
        cache::store_host_info(&slug, &info, now);
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
