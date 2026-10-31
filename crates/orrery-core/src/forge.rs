//! Git host providers (#17–#19). A small abstraction over GitHub, GitLab and
//! Gitea / Forgejo / Codeberg (incl. self-hosted) that fetches the enrichment
//! shown on cards: stars, topics, open issues, latest release.
//!
//! GitHub uses `api.github.com`; GitLab uses `https://<domain>/api/v4` and
//! Gitea `https://<domain>/api/v1`, so self-hosted instances work by routing
//! on the remote's domain. Requests are
//! unauthenticated by default (fine for public repos, rate-limited) and use a
//! bearer token when one is available.
//!
//! This module also owns the shared HTTP plumbing every forge/OAuth call goes
//! through: one [`client`] (same-origin-only redirects), slug validation, the
//! GitHub error-body parser, and rate-limit backoff ([`send`]).

use std::time::Duration;

use serde::Deserialize;

use crate::model::{Host, HostInfo};

/// The User-Agent every Orrery forge/OAuth request carries (GitHub rejects
/// requests without one).
pub(crate) const UA: &str = "Orrery/0.1 (+https://orrery.app)";

/// Most redirects one request may follow before giving up.
const MAX_REDIRECTS: usize = 5;

/// Rate-limit bucket for GitHub's core REST API (see [`send`]).
pub(crate) const GH_CORE: &str = "GitHub";

/// The user's trusted self-hosted forge allowlists (`gitlab_hosts` /
/// `gitea_hosts` in config). They drive both remote detection (`scan`) and
/// token egress: a token is only attached to `gitlab.com` / `codeberg.org` or
/// a domain on the matching list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedHosts {
    pub gitlab: Vec<String>,
    pub gitea: Vec<String>,
}

impl TrustedHosts {
    pub fn from_config(cfg: &crate::model::AppConfig) -> Self {
        TrustedHosts {
            gitlab: cfg.gitlab_hosts.clone(),
            gitea: cfg.gitea_hosts.clone(),
        }
    }
}

/// Fetch host enrichment for a repo. `domain` routes self-hosted GitLab and
/// Gitea; `trusted` is the user's allowlists (see [`gitlab_host_trusted`] /
/// [`gitea_host_trusted`]) — a GitLab or Gitea token is attached only when
/// `domain` is trusted, so a hostile repo remote can't exfiltrate it.
pub async fn fetch(
    host: Host,
    domain: &str,
    slug: &str,
    token: Option<&str>,
    trusted: &TrustedHosts,
) -> Result<HostInfo, String> {
    match host {
        // GitHub always talks to the fixed api.github.com, so its token egress
        // needs no per-domain check.
        Host::Github => fetch_github(slug, token).await,
        Host::Gitlab => fetch_gitlab(domain, slug, token, &trusted.gitlab).await,
        Host::Gitea => fetch_gitea(domain, slug, token, &trusted.gitea).await,
    }
}

/// Guard the GitLab API host. `domain` comes from a repo's `.git/config`, so a
/// malicious/misconfigured remote could otherwise point requests (with a bearer
/// token attached) at an internal address or metadata endpoint (SSRF). Accept
/// only plain DNS hostnames — no IPs, ports, schemes, paths, or credentials.
pub(crate) fn valid_host(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && !domain.contains('/')
        && !domain.contains(':')
        && !domain.contains('@')
        && !domain.contains(' ')
        && domain.parse::<std::net::IpAddr>().is_err()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Whether a GitLab token may be sent to `domain`. Enforces the documented
/// token-egress invariant: only `gitlab.com` or a host the user explicitly added
/// to `gitlab_hosts` is trusted. `valid_host` (SSRF) is a necessary precondition
/// — an untrusted-but-valid host can still be queried *unauthenticated* for
/// public project metadata, but never receives the token. Matching is
/// case-insensitive on the host.
pub(crate) fn gitlab_host_trusted(domain: &str, gitlab_hosts: &[String]) -> bool {
    if !valid_host(domain) {
        return false;
    }
    domain.eq_ignore_ascii_case("gitlab.com")
        || gitlab_hosts
            .iter()
            .any(|h| h.trim().eq_ignore_ascii_case(domain))
}

/// Whether a Gitea / Forgejo token may be sent to `domain`: the same egress
/// rule as [`gitlab_host_trusted`], with `codeberg.org` as the always-trusted
/// public instance and `gitea_hosts` as the user's allowlist.
pub(crate) fn gitea_host_trusted(domain: &str, gitea_hosts: &[String]) -> bool {
    if !valid_host(domain) {
        return false;
    }
    domain.eq_ignore_ascii_case("codeberg.org")
        || gitea_hosts
            .iter()
            .any(|h| h.trim().eq_ignore_ascii_case(domain))
}

/// The trusted domains for one forge kind, normalised (trimmed, lowercased,
/// deduped, SSRF-checked), public instance first. These are the hosts the
/// inbox/notification passes query with a token. GitHub is always just
/// `github.com` (its API host is fixed).
pub(crate) fn trusted_domains(host: Host, trusted: &TrustedHosts) -> Vec<String> {
    let (public, list): (&str, &[String]) = match host {
        Host::Github => return vec!["github.com".to_string()],
        Host::Gitlab => ("gitlab.com", &trusted.gitlab),
        Host::Gitea => ("codeberg.org", &trusted.gitea),
    };
    let mut out = vec![public.to_string()];
    for h in list {
        let h = h.trim().to_ascii_lowercase();
        if valid_host(&h) && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// A valid Gitea `owner/name` slug — exactly two segments (Gitea and Forgejo
/// have no nested namespaces).
pub(crate) fn valid_gitea_slug(slug: &str) -> bool {
    valid_slug(slug) && slug.split('/').count() == 2
}

/// Whether a redirect from `prev` to `next` may be followed. Only a same-origin
/// HTTPS hop is allowed: no scheme downgrade (https→http), no change of host
/// or port, no embedded credentials, and never an IP literal or dot-less
/// internal name. This is what keeps a token on its host — reqwest strips
/// `Authorization` on a cross-host hop, but refusing the hop outright covers
/// every header, and also stops an untrusted host from bouncing even an
/// unauthenticated request to e.g. `169.254.169.254` (SSRF past
/// [`valid_host`]).
pub(crate) fn redirect_allowed(prev: &reqwest::Url, next: &reqwest::Url) -> bool {
    if prev.scheme() != "https" || next.scheme() != "https" {
        return false;
    }
    let (Some(from), Some(to)) = (prev.host_str(), next.host_str()) else {
        return false;
    };
    valid_host(to)
        && from.eq_ignore_ascii_case(to)
        && prev.port_or_known_default() == next.port_or_known_default()
        && next.username().is_empty()
        && next.password().is_none()
}

/// The one HTTP client for every forge (GitHub/GitLab) and OAuth call: a
/// shared connection/TLS pool (reqwest::Client is Arc-backed, so cloning just
/// shares it), one User-Agent, the same-origin-only redirect policy
/// ([`redirect_allowed`]), and bounded connect/read/overall timeouts so a
/// hung or blackholed host can't stall a refresh forever.
pub(crate) fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let allowed = attempt
                .previous()
                .last()
                .is_some_and(|prev| redirect_allowed(prev, attempt.url()));
            if allowed {
                attempt.follow()
            } else {
                // Hand the 3xx back to the caller (it surfaces as a plain
                // error status) rather than following it off-origin.
                attempt.stop()
            }
        });
        crate::http::client_builder()
            .user_agent(UA)
            .redirect(policy)
            .connect_timeout(Duration::from_secs(8))
            .read_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}

/// Whether `slug` is safe to interpolate into an API URL path: two or more
/// `/`-separated segments of `[A-Za-z0-9._-]`, none empty, `.` or `..`. Slugs
/// come from a repo's remote URL (attacker-controllable), so without this a
/// remote like `o/../../user` could steer a request to another endpoint.
pub(crate) fn valid_slug(slug: &str) -> bool {
    slug.len() <= 1024
        && slug.split('/').count() >= 2
        && slug.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        })
}

/// A valid GitHub `owner/name` slug — exactly two segments (GitHub has no
/// nested namespaces).
pub(crate) fn valid_github_slug(slug: &str) -> bool {
    valid_slug(slug) && slug.split('/').count() == 2
}

/// Reject a slug that isn't a valid GitHub `owner/name` before it reaches a URL.
pub(crate) fn check_github_slug(slug: &str) -> Result<(), String> {
    if valid_github_slug(slug) {
        Ok(())
    } else {
        Err(format!("not an owner/name slug: {slug}"))
    }
}

/// GitHub's explanation of a rejected request — the JSON `message`, plus any
/// `errors[].message` details (validation errors, "A pull request already
/// exists…", "Required status checks must pass"). Pure for testing; `None`
/// when the body has no `message`.
pub(crate) fn github_error_message(body: &serde_json::Value) -> Option<String> {
    let msg = body.get("message")?.as_str()?.to_string();
    let errs = body
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|s| !s.is_empty());
    Some(match errs {
        Some(errs) => format!("{msg}: {errs}"),
        None => msg,
    })
}

/// Turn a non-2xx GitHub response into an error string: GitHub's own
/// explanation when the body carries one, else `"{what} {status}"`.
pub(crate) async fn github_error(resp: reqwest::Response, what: &str) -> String {
    let status = resp.status();
    resp.json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| github_error_message(&v))
        .unwrap_or_else(|| format!("{what} {status}"))
}

// ── Rate limits ──────────────────────────────────────────────────────────────

/// Longest we'll ever back off for, whatever a header claims.
const MAX_BACKOFF_SECS: u64 = 3_600;

/// How long to back off after a response, from its rate-limit headers:
/// `Retry-After` on a 403/429 (secondary limits), or `X-RateLimit-Remaining:
/// 0` (until `X-RateLimit-Reset`). A bare 429 with no hints waits a minute.
/// `None` = no limit in play. Pure for testing.
pub(crate) fn rate_limit_wait(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    now_unix: i64,
) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let limited = matches!(status.as_u16(), 403 | 429);
    let secs = if let Some(after) = header("retry-after").filter(|_| limited) {
        after.trim().parse::<u64>().unwrap_or(60)
    } else if header("x-ratelimit-remaining").map(str::trim) == Some("0") {
        header("x-ratelimit-reset")
            .and_then(|r| r.trim().parse::<i64>().ok())
            .map(|reset| (reset - now_unix).max(1) as u64)
            .unwrap_or(60)
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        60
    } else {
        return None;
    };
    Some(Duration::from_secs(secs.clamp(1, MAX_BACKOFF_SECS)))
}

type BackoffMap = std::collections::HashMap<String, std::time::Instant>;

/// Per-bucket "don't call before" deadlines, keyed by a label such as
/// [`GH_CORE`], `"GitHub search"` or a GitLab domain.
static BACKOFF: std::sync::LazyLock<std::sync::Mutex<BackoffMap>> =
    std::sync::LazyLock::new(Default::default);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn rate_limited(bucket: &str, secs: u64) -> String {
    format!("{bucket} rate limit reached — retry in {secs}s")
}

/// Send a forge request, respecting rate limits: while `bucket` is backing off
/// this fails fast without touching the network (so an 8-wide CI pass doesn't
/// keep hammering a limited API), and a response that signals a limit arms
/// the backoff. A rate-limited 403/429 comes back as `Err` naming the wait;
/// any other response (including other errors) is returned to the caller.
pub(crate) async fn send(
    req: reqwest::RequestBuilder,
    bucket: &str,
) -> Result<reqwest::Response, String> {
    let now = std::time::Instant::now();
    let until = BACKOFF
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(bucket)
        .copied();
    if let Some(until) = until.filter(|u| *u > now) {
        return Err(rate_limited(bucket, (until - now).as_secs().max(1)));
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if let Some(wait) = rate_limit_wait(status, resp.headers(), now_unix()) {
        BACKOFF
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(bucket.to_string(), std::time::Instant::now() + wait);
        if matches!(status.as_u16(), 403 | 429) {
            return Err(rate_limited(bucket, wait.as_secs()));
        }
    }
    Ok(resp)
}

async fn fetch_github(slug: &str, token: Option<&str>) -> Result<HostInfo, String> {
    #[derive(Deserialize)]
    struct Repo {
        #[serde(default)]
        stargazers_count: u32,
        #[serde(default)]
        open_issues_count: u32,
        #[serde(default)]
        topics: Vec<String>,
        #[serde(default)]
        private: bool,
    }

    check_github_slug(slug)?;
    let client = client();
    let mut req = client
        .get(format!("https://api.github.com/repos/{slug}"))
        .header("Accept", "application/vnd.github+json");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = send(req, GH_CORE).await?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API {}", resp.status()));
    }
    let repo: Repo = resp.json().await.map_err(|e| e.to_string())?;

    // Latest release is optional (404 when a repo has none).
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let mut rel = client
        .get(format!(
            "https://api.github.com/repos/{slug}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json");
    if let Some(t) = token {
        rel = rel.bearer_auth(t);
    }
    let latest_release = match send(rel, GH_CORE).await {
        Ok(r) if r.status().is_success() => r.json::<Release>().await.ok().map(|r| r.tag_name),
        _ => None,
    };

    Ok(HostInfo {
        stars: repo.stargazers_count,
        topics: repo.topics,
        open_issues: repo.open_issues_count,
        latest_release,
        private: repo.private,
    })
}

/// The JSON body for a GitHub create-PR request. Pure for testing.
pub(crate) fn pr_request_body(
    head: &str,
    base: &str,
    title: &str,
    body: &str,
) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "head": head,
        "base": base,
        "body": body,
    })
}

/// Open a pull request on GitHub (`POST /repos/{slug}/pulls`) and return its
/// web URL. GitHub-only for now (GitLab merge requests come later). Token
/// egress: like all GitHub calls here, this only ever talks to the fixed
/// `api.github.com` — the host is never derived from a repo remote, so the
/// GitLab trust gating above is untouched.
pub async fn create_pr(
    slug: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    token: &str,
) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Created {
        html_url: String,
    }
    check_github_slug(slug)?;
    let req = client()
        .post(format!("https://api.github.com/repos/{slug}/pulls"))
        .header("Accept", "application/vnd.github+json")
        .bearer_auth(token)
        .json(&pr_request_body(head, base, title, body));
    let resp = send(req, GH_CORE).await?;
    if resp.status().is_success() {
        let created: Created = resp.json().await.map_err(|e| e.to_string())?;
        return Ok(created.html_url);
    }
    // GitHub explains rejections in JSON ("A pull request already exists…",
    // validation errors); surface that over the bare status code.
    Err(github_error(resp, "GitHub PR create").await)
}

async fn fetch_gitlab(
    domain: &str,
    slug: &str,
    token: Option<&str>,
    gitlab_hosts: &[String],
) -> Result<HostInfo, String> {
    #[derive(Deserialize)]
    struct Project {
        #[serde(default)]
        star_count: u32,
        #[serde(default)]
        open_issues_count: u32,
        #[serde(default)]
        topics: Vec<String>,
        /// "public" | "internal" | "private"; absent on older instances.
        #[serde(default)]
        visibility: Option<String>,
    }

    if !valid_host(domain) {
        return Err(format!("refusing to query untrusted host: {domain}"));
    }
    if !valid_slug(slug) {
        return Err(format!("not a group/project slug: {slug}"));
    }
    // Attach the token only to a trusted host; an arbitrary remote domain still
    // gets queried (public metadata) but never receives the token. It goes in
    // `Authorization: Bearer` (GitLab accepts PATs/OAuth tokens there) rather
    // than a custom `PRIVATE-TOKEN` header, which reqwest would *not* strip on
    // a cross-host redirect; the client's redirect policy refuses those anyway.
    let token = token.filter(|_| gitlab_host_trusted(domain, gitlab_hosts));
    let base = format!("https://{}/api/v4", domain);
    let encoded = urlencoding::encode(slug);
    let client = client();

    let mut req = client.get(format!("{base}/projects/{encoded}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = send(req, domain).await?;
    if !resp.status().is_success() {
        return Err(format!("GitLab API {}", resp.status()));
    }
    let project: Project = resp.json().await.map_err(|e| e.to_string())?;

    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let mut rel = client.get(format!("{base}/projects/{encoded}/releases?per_page=1"));
    if let Some(t) = token {
        rel = rel.bearer_auth(t);
    }
    let latest_release = match send(rel, domain).await {
        Ok(r) if r.status().is_success() => r
            .json::<Vec<Release>>()
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|r| r.tag_name),
        _ => None,
    };

    Ok(HostInfo {
        stars: project.star_count,
        topics: project.topics,
        open_issues: project.open_issues_count,
        latest_release,
        private: project.visibility.as_deref().is_some_and(|v| v != "public"),
    })
}

/// The Gitea API base for a domain (`https://<domain>/api/v1`).
pub(crate) fn gitea_api(domain: &str) -> String {
    format!("https://{domain}/api/v1")
}

/// The fields of a Gitea / Forgejo `GET /repos/{owner}/{repo}` response that
/// enrichment uses. Split out so canned JSON can be tested.
#[derive(Deserialize)]
pub(crate) struct GiteaRepo {
    #[serde(default)]
    stars_count: u32,
    #[serde(default)]
    open_issues_count: u32,
    /// Present on current Gitea/Forgejo; absent on old instances.
    #[serde(default)]
    topics: Option<Vec<String>>,
    #[serde(default)]
    private: bool,
    /// Visible to signed-in users of the instance only.
    #[serde(default)]
    internal: bool,
}

impl GiteaRepo {
    pub(crate) fn into_host_info(self, latest_release: Option<String>) -> HostInfo {
        HostInfo {
            stars: self.stars_count,
            topics: self.topics.unwrap_or_default(),
            open_issues: self.open_issues_count,
            latest_release,
            private: self.private || self.internal,
        }
    }
}

/// One entry of a Gitea release list (only the tag matters for cards).
#[derive(Deserialize)]
pub(crate) struct GiteaRelease {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) draft: bool,
}

/// The newest non-draft release tag from a Gitea release list. Pure.
pub(crate) fn gitea_latest_release(releases: Vec<GiteaRelease>) -> Option<String> {
    releases.into_iter().find(|r| !r.draft).map(|r| r.tag_name)
}

/// Enrichment from a Gitea / Forgejo / Codeberg instance: stars, topics,
/// visibility and open issues from `/api/v1/repos/{owner}/{repo}`, plus the
/// latest release. Same egress rule as GitLab: the token is attached only to
/// `codeberg.org` or an allowlisted host, in `Authorization: Bearer` (which
/// Gitea accepts for access tokens and reqwest strips on a cross-host hop).
async fn fetch_gitea(
    domain: &str,
    slug: &str,
    token: Option<&str>,
    gitea_hosts: &[String],
) -> Result<HostInfo, String> {
    if !valid_host(domain) {
        return Err(format!("refusing to query untrusted host: {domain}"));
    }
    if !valid_gitea_slug(slug) {
        return Err(format!("not an owner/name slug: {slug}"));
    }
    let token = token.filter(|_| gitea_host_trusted(domain, gitea_hosts));
    let base = gitea_api(domain);
    let client = client();

    let mut req = client.get(format!("{base}/repos/{slug}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = send(req, domain).await?;
    if !resp.status().is_success() {
        return Err(format!("Gitea API {}", resp.status()));
    }
    let repo: GiteaRepo = resp.json().await.map_err(|e| e.to_string())?;

    // Latest release is optional; drafts only show up for writers, so they
    // are filtered client-side as well as by the query.
    let mut rel = client.get(format!("{base}/repos/{slug}/releases?draft=false&limit=5"));
    if let Some(t) = token {
        rel = rel.bearer_auth(t);
    }
    let latest_release = match send(rel, domain).await {
        Ok(r) if r.status().is_success() => r
            .json::<Vec<GiteaRelease>>()
            .await
            .ok()
            .and_then(gitea_latest_release),
        _ => None,
    };

    Ok(repo.into_host_info(latest_release))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use reqwest::{StatusCode, Url};

    #[test]
    fn valid_host_accepts_dns_names_and_rejects_unsafe() {
        assert!(valid_host("gitlab.com"));
        assert!(valid_host("gitlab.acme.io"));
        // SSRF vectors / malformed hosts
        assert!(!valid_host("169.254.169.254"), "metadata IP");
        assert!(!valid_host("127.0.0.1"));
        assert!(!valid_host("localhost"), "no dot");
        assert!(!valid_host("gitlab.com:8080"), "port");
        assert!(!valid_host("evil.com/path"), "path");
        assert!(!valid_host("user@evil.com"), "credentials");
        assert!(!valid_host(""));
    }

    #[test]
    fn gitlab_token_only_trusted_for_allowlisted_or_dot_com() {
        let allow = vec![
            "gitlab.acme.io".to_string(),
            "  Git.Internal.Corp ".to_string(),
        ];
        // gitlab.com is always trusted; the allowlist is honored case-insensitively
        // and tolerant of surrounding whitespace in config entries.
        assert!(gitlab_host_trusted("gitlab.com", &[]));
        assert!(gitlab_host_trusted("GitLab.com", &[]));
        assert!(gitlab_host_trusted("gitlab.acme.io", &allow));
        assert!(gitlab_host_trusted("git.internal.corp", &allow));
        // An arbitrary remote domain must NOT receive the token, even though it
        // is a structurally valid host — this is the egress invariant.
        assert!(!gitlab_host_trusted("evil.com", &allow));
        assert!(!gitlab_host_trusted("gitlab.com.evil.com", &allow));
        // SSRF-shaped hosts are never trusted regardless of the allowlist.
        assert!(!gitlab_host_trusted("169.254.169.254", &allow));
        assert!(!gitlab_host_trusted("gitlab.acme.io:22", &allow));
    }

    #[test]
    fn gitea_token_only_trusted_for_codeberg_or_allowlisted() {
        let allow = vec![" Git.Acme.IO ".to_string()];
        assert!(gitea_host_trusted("codeberg.org", &[]));
        assert!(gitea_host_trusted("Codeberg.org", &[]));
        assert!(gitea_host_trusted("git.acme.io", &allow));
        // Egress invariant: an arbitrary (even well-formed) host never gets it,
        // and the GitLab allowlist doesn't leak into Gitea's or vice versa.
        assert!(!gitea_host_trusted("evil.com", &allow));
        assert!(!gitea_host_trusted("codeberg.org.evil.com", &allow));
        assert!(!gitea_host_trusted("gitlab.com", &allow));
        assert!(!gitlab_host_trusted("codeberg.org", &[]));
        assert!(!gitea_host_trusted("169.254.169.254", &allow));
        assert!(!gitea_host_trusted("git.acme.io:3000", &allow));
    }

    #[test]
    fn trusted_domains_normalise_and_lead_with_the_public_instance() {
        let trusted = TrustedHosts {
            gitlab: vec![
                " GitLab.Acme.io ".into(),
                "gitlab.com".into(),
                "10.0.0.1".into(),
            ],
            gitea: vec!["git.acme.io".into(), "git.acme.io".into()],
        };
        assert_eq!(
            trusted_domains(Host::Gitlab, &trusted),
            vec!["gitlab.com", "gitlab.acme.io"]
        );
        assert_eq!(
            trusted_domains(Host::Gitea, &trusted),
            vec!["codeberg.org", "git.acme.io"]
        );
        assert_eq!(trusted_domains(Host::Github, &trusted), vec!["github.com"]);
    }

    #[test]
    fn gitea_repo_json_maps_to_host_info() {
        // Trimmed from a real Codeberg `GET /api/v1/repos/{owner}/{repo}`.
        let repo: GiteaRepo = serde_json::from_value(serde_json::json!({
            "id": 1,
            "full_name": "forgejo/forgejo",
            "private": false,
            "internal": false,
            "stars_count": 2950,
            "open_issues_count": 1234,
            "open_pr_counter": 80,
            "topics": ["forge", "git"],
            "default_branch": "forgejo"
        }))
        .unwrap();
        let info = repo.into_host_info(Some("v9.0.0".into()));
        assert_eq!(info.stars, 2950);
        assert_eq!(info.open_issues, 1234);
        assert_eq!(info.topics, vec!["forge", "git"]);
        assert_eq!(info.latest_release.as_deref(), Some("v9.0.0"));
        assert!(!info.private);

        // Older instances omit topics; internal repos are not public.
        let repo: GiteaRepo =
            serde_json::from_value(serde_json::json!({ "internal": true })).unwrap();
        let info = repo.into_host_info(None);
        assert!(info.topics.is_empty());
        assert!(info.private);
    }

    #[test]
    fn gitea_latest_release_skips_drafts() {
        let rels: Vec<GiteaRelease> = serde_json::from_value(serde_json::json!([
            {"tag_name": "v2.0.0-draft", "draft": true},
            {"tag_name": "v1.9.0", "draft": false, "prerelease": false},
        ]))
        .unwrap();
        assert_eq!(gitea_latest_release(rels).as_deref(), Some("v1.9.0"));
        assert_eq!(gitea_latest_release(Vec::new()), None);
    }

    #[test]
    fn gitea_slugs_are_exactly_owner_name() {
        assert!(valid_gitea_slug("forgejo/forgejo"));
        assert!(!valid_gitea_slug("group/sub/proj"));
        assert!(!valid_gitea_slug("o/../x"));
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn redirect_policy_allows_same_origin_https_only() {
        let from = url("https://gitlab.acme.io/api/v4/projects/a%2Fb");
        // Same host, still HTTPS (e.g. a renamed project) → follow.
        assert!(redirect_allowed(
            &from,
            &url("https://gitlab.acme.io/api/v4/projects/42")
        ));
        assert!(redirect_allowed(
            &from,
            &url("https://GitLab.Acme.io/api/v4/projects/42")
        ));
        // Cross-host: the token (or any header) must not follow it off-host.
        assert!(!redirect_allowed(&from, &url("https://evil.com/steal")));
        assert!(!redirect_allowed(
            &from,
            &url("https://gitlab.acme.io.evil.com/x")
        ));
        // Scheme downgrade.
        assert!(!redirect_allowed(
            &from,
            &url("http://gitlab.acme.io/api/v4/projects/42")
        ));
        // Port change on the same host.
        assert!(!redirect_allowed(
            &from,
            &url("https://gitlab.acme.io:8443/api/v4/projects/42")
        ));
        // Embedded credentials.
        assert!(!redirect_allowed(
            &from,
            &url("https://user:pw@gitlab.acme.io/x")
        ));
    }

    #[test]
    fn redirect_policy_blocks_ip_literals_and_internal_names() {
        // An untrusted host bouncing a request at the cloud metadata endpoint
        // or loopback must be refused even though no token is attached.
        let from = url("https://evil.example/api/v4/projects/a%2Fb");
        for target in [
            "https://169.254.169.254/latest/meta-data/",
            "http://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://localhost/",
            "https://metadata/",
        ] {
            assert!(!redirect_allowed(&from, &url(target)), "{target}");
        }
        // Even "same host" can't make an IP literal acceptable.
        let ip = url("https://10.0.0.1/a");
        assert!(!redirect_allowed(&ip, &url("https://10.0.0.1/b")));
    }

    #[test]
    fn slug_validation_rejects_traversal_and_odd_chars() {
        assert!(valid_slug("owner/repo"));
        assert!(valid_slug("Hank-man/Orr_ery.rs"));
        assert!(valid_slug("group/sub/proj"), "GitLab nested groups");
        assert!(!valid_slug("noslash"));
        assert!(!valid_slug("o/../../user"));
        assert!(!valid_slug("../repo"));
        assert!(!valid_slug("o/."));
        assert!(!valid_slug("o//r"));
        assert!(!valid_slug("/o/r"));
        assert!(!valid_slug("o/r?x=1"));
        assert!(!valid_slug("o/r#frag"));
        assert!(!valid_slug("o/r%2F.."));
        assert!(!valid_slug("o/r b"));
        assert!(valid_github_slug("owner/repo"));
        assert!(
            !valid_github_slug("group/sub/proj"),
            "GitHub has no nesting"
        );
        assert!(check_github_slug("o/../x").is_err());
    }

    #[test]
    fn github_error_message_joins_details() {
        let v = serde_json::json!({
            "message": "Validation Failed",
            "errors": [{"message": "A pull request already exists for o:x."}],
        });
        assert_eq!(
            github_error_message(&v).as_deref(),
            Some("Validation Failed: A pull request already exists for o:x.")
        );
        let v = serde_json::json!({ "message": "Required status checks must pass" });
        assert_eq!(
            github_error_message(&v).as_deref(),
            Some("Required status checks must pass")
        );
        let v = serde_json::json!({ "message": "Bad", "errors": [] });
        assert_eq!(github_error_message(&v).as_deref(), Some("Bad"));
        assert_eq!(github_error_message(&serde_json::json!({})), None);
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn rate_limit_wait_reads_headers() {
        let now = 1_000;
        // Secondary limit: Retry-After on a 403.
        assert_eq!(
            rate_limit_wait(
                StatusCode::FORBIDDEN,
                &headers(&[("retry-after", "30")]),
                now
            ),
            Some(Duration::from_secs(30))
        );
        // Primary limit exhausted: wait until the reset.
        assert_eq!(
            rate_limit_wait(
                StatusCode::FORBIDDEN,
                &headers(&[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1120")
                ]),
                now
            ),
            Some(Duration::from_secs(120))
        );
        // The last allowed call succeeded but the budget is now spent → arm.
        assert_eq!(
            rate_limit_wait(
                StatusCode::OK,
                &headers(&[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1005")
                ]),
                now
            ),
            Some(Duration::from_secs(5))
        );
        // Bare 429.
        assert_eq!(
            rate_limit_wait(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), now),
            Some(Duration::from_secs(60))
        );
        // A huge reset is capped.
        assert_eq!(
            rate_limit_wait(
                StatusCode::FORBIDDEN,
                &headers(&[("retry-after", "999999")]),
                now
            ),
            Some(Duration::from_secs(MAX_BACKOFF_SECS))
        );
        // Not limited: plain 403 (permissions), budget remaining, Retry-After on a 200.
        assert_eq!(
            rate_limit_wait(StatusCode::FORBIDDEN, &HeaderMap::new(), now),
            None
        );
        assert_eq!(
            rate_limit_wait(
                StatusCode::OK,
                &headers(&[("x-ratelimit-remaining", "4999")]),
                now
            ),
            None
        );
        assert_eq!(
            rate_limit_wait(StatusCode::OK, &headers(&[("retry-after", "30")]), now),
            None
        );
    }

    #[test]
    fn pr_request_body_carries_all_fields() {
        let body = pr_request_body("feat/x", "main", "feat: add x", "- one\n- two");
        assert_eq!(body["head"], "feat/x");
        assert_eq!(body["base"], "main");
        assert_eq!(body["title"], "feat: add x");
        assert_eq!(body["body"], "- one\n- two");
        // Exactly the documented fields — nothing extra rides along.
        assert_eq!(body.as_object().unwrap().len(), 4);
    }
}
