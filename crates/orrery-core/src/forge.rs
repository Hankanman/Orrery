//! Git host providers (#17–#19). A small abstraction over GitHub and GitLab
//! (incl. self-hosted) that fetches the enrichment shown on cards: stars,
//! topics, open issues, latest release.
//!
//! GitHub uses `api.github.com`; GitLab uses `https://<domain>/api/v4`, so
//! self-hosted instances work by routing on the remote's domain. Requests are
//! unauthenticated by default (fine for public repos, rate-limited) and use a
//! bearer token when one is available.

use std::time::Duration;

use serde::Deserialize;

use crate::model::{Host, HostInfo};

const UA: &str = "Orrery/0.1 (+https://orrery.app)";

/// Fetch host enrichment for a repo. `domain` routes self-hosted GitLab;
/// `gitlab_hosts` is the user's trusted self-hosted GitLab allowlist (see
/// [`gitlab_host_trusted`]) — a GitLab token is attached only when `domain` is
/// trusted, so a hostile repo remote can't exfiltrate it.
pub async fn fetch(
    host: Host,
    domain: &str,
    slug: &str,
    token: Option<&str>,
    gitlab_hosts: &[String],
) -> Result<HostInfo, String> {
    match host {
        // GitHub always talks to the fixed api.github.com, so its token egress
        // needs no per-domain check.
        Host::Github => fetch_github(slug, token).await,
        Host::Gitlab => fetch_gitlab(domain, slug, token, gitlab_hosts).await,
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

fn client() -> reqwest::Client {
    // One shared client → connection/TLS reuse across the per-repo enrich calls.
    // reqwest::Client is Arc-backed, so cloning just shares the pool. Bounded
    // timeouts so a hung/blackholed host can't stall an enrich task forever.
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(UA)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
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

    let client = client();
    let mut req = client
        .get(format!("https://api.github.com/repos/{slug}"))
        .header("Accept", "application/vnd.github+json");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
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
    let latest_release = match rel.send().await {
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
    // Attach the token only to a trusted host; an arbitrary remote domain still
    // gets queried (public metadata) but never receives the token.
    let token = token.filter(|_| gitlab_host_trusted(domain, gitlab_hosts));
    let base = format!("https://{}/api/v4", domain);
    let encoded = urlencoding::encode(slug);
    let client = client();

    let mut req = client.get(format!("{base}/projects/{encoded}"));
    if let Some(t) = token {
        req = req.header("PRIVATE-TOKEN", t);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
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
        rel = rel.header("PRIVATE-TOKEN", t);
    }
    let latest_release = match rel.send().await {
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

#[cfg(test)]
mod tests {
    use super::{gitlab_host_trusted, valid_host};

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
}
