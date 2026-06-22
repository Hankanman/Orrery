//! Git host providers (#17–#19). A small abstraction over GitHub and GitLab
//! (incl. self-hosted) that fetches the enrichment shown on cards: stars,
//! topics, open issues, latest release.
//!
//! GitHub uses `api.github.com`; GitLab uses `https://<domain>/api/v4`, so
//! self-hosted instances work by routing on the remote's domain. Requests are
//! unauthenticated by default (fine for public repos, rate-limited) and use a
//! bearer token when one is available.

use serde::Deserialize;

use crate::model::{Host, HostInfo};

const UA: &str = "Orrery/0.1 (+https://orrery.app)";

/// Fetch host enrichment for a repo. `domain` routes self-hosted GitLab.
pub async fn fetch(
    host: Host,
    domain: &str,
    slug: &str,
    token: Option<&str>,
) -> Result<HostInfo, String> {
    match host {
        Host::Github => fetch_github(slug, token).await,
        Host::Gitlab => fetch_gitlab(domain, slug, token).await,
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

fn client() -> reqwest::Client {
    // One shared client → connection/TLS reuse across the per-repo enrich calls.
    // reqwest::Client is Arc-backed, so cloning just shares the pool.
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(UA)
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

async fn fetch_gitlab(domain: &str, slug: &str, token: Option<&str>) -> Result<HostInfo, String> {
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
    use super::valid_host;

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
}
