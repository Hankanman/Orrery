//! Cross-host "dev inbox" (Phase 7): the things a dev checks constantly — open
//! PRs, review requests, assigned issues, host notifications, CI status, and
//! starred repos. GitHub is implemented via its REST/search API using the
//! resolved token (stored OAuth → env → `gh`); GitLab support can layer on the
//! same shapes later.

use serde::{Deserialize, Serialize};

use crate::model::Host;
use crate::oauth;

const UA: &str = "Orrery/0.1 (+https://orrery.app)";
const GH: &str = "https://api.github.com";

fn client() -> reqwest::Client {
    // Shared client (Arc-backed) so the inbox's several GitHub calls reuse one
    // connection pool instead of handshaking anew each time.
    static CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(|| reqwest::Client::builder().user_agent(UA).build().unwrap_or_default());
    CLIENT.clone()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// "pr" | "review" | "issue"
    pub kind: String,
    pub title: String,
    pub repo: String,
    pub url: String,
    pub number: u64,
    pub draft: bool,
    pub host: Host,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub title: String,
    pub repo: String,
    pub reason: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    pub slug: String,
    pub description: Option<String>,
    pub stars: u32,
    pub language: Option<String>,
    pub clone_url: String,
    pub host: Host,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiStatus {
    /// "success" | "failure" | "pending" | "none"
    pub state: String,
}

async fn github_user(token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }
    let resp = client()
        .get(format!("{GH}/user"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub auth failed ({}) — check the token scopes", resp.status()));
    }
    let user: User = resp.json().await.map_err(|e| e.to_string())?;
    Ok(user.login)
}

/// Shared GitHub issue-search shape (covers PRs and issues).
#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    items: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    title: String,
    html_url: String,
    number: u64,
    repository_url: String,
    #[serde(default)]
    draft: bool,
}

fn repo_from_url(repository_url: &str) -> String {
    repository_url.rsplit("/repos/").next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::repo_from_url;

    #[test]
    fn repo_from_url_extracts_owner_repo() {
        assert_eq!(repo_from_url("https://api.github.com/repos/Hankanman/Orrery"), "Hankanman/Orrery");
        assert_eq!(repo_from_url("https://api.github.com/repos/a/b"), "a/b");
    }
}

async fn gh_search(token: &str, query: &str, kind: &str) -> Result<Vec<InboxItem>, String> {
    let url = format!("{GH}/search/issues?per_page=50&q={}", urlencoding::encode(query));
    let resp = client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        // Surface auth/rate-limit failures instead of a misleading empty inbox.
        return Err(format!("GitHub search {}", resp.status()));
    }
    let parsed: SearchResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed
        .items
        .into_iter()
        .map(|i| InboxItem {
            kind: kind.to_string(),
            title: i.title,
            repo: repo_from_url(&i.repository_url),
            url: i.html_url,
            number: i.number,
            draft: i.draft,
            host: Host::Github,
        })
        .collect())
}

/// Open PRs authored by the user, PRs awaiting their review, and issues
/// assigned to them.
pub async fn github_inbox() -> Result<Vec<InboxItem>, String> {
    let token = oauth::github_token().ok_or("connect GitHub to use the inbox")?;
    let login = github_user(&token).await?;

    let mut items = Vec::new();
    items.extend(gh_search(&token, &format!("is:open is:pr author:{login}"), "pr").await?);
    items.extend(gh_search(&token, &format!("is:open is:pr review-requested:{login}"), "review").await?);
    items.extend(gh_search(&token, &format!("is:open is:issue assignee:{login}"), "issue").await?);
    Ok(items)
}

pub async fn github_notifications() -> Result<Vec<Notification>, String> {
    #[derive(Deserialize)]
    struct N {
        reason: String,
        subject: Subject,
        repository: Repo,
    }
    #[derive(Deserialize)]
    struct Subject {
        title: String,
        #[serde(rename = "type")]
        kind: String,
    }
    #[derive(Deserialize)]
    struct Repo {
        full_name: String,
    }
    let token = oauth::github_token().ok_or("connect GitHub to see notifications")?;
    let resp = client()
        .get(format!("{GH}/notifications?per_page=50"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API {}", resp.status()));
    }
    let raw: Vec<N> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(raw
        .into_iter()
        .map(|n| Notification {
            title: n.subject.title,
            repo: n.repository.full_name,
            reason: n.reason,
            kind: n.subject.kind,
        })
        .collect())
}

pub async fn github_starred() -> Result<Vec<RemoteRepo>, String> {
    #[derive(Deserialize)]
    struct R {
        full_name: String,
        description: Option<String>,
        #[serde(default)]
        stargazers_count: u32,
        language: Option<String>,
        clone_url: String,
    }
    let token = oauth::github_token().ok_or("connect GitHub to browse stars")?;
    let resp = client()
        .get(format!("{GH}/user/starred?per_page=60"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API {}", resp.status()));
    }
    let raw: Vec<R> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(raw
        .into_iter()
        .map(|r| RemoteRepo {
            slug: r.full_name,
            description: r.description,
            stars: r.stargazers_count,
            language: r.language,
            clone_url: r.clone_url,
            host: Host::Github,
        })
        .collect())
}

/// Latest GitHub Actions run conclusion for a repo's default branch.
pub async fn github_ci(slug: &str) -> Result<CiStatus, String> {
    #[derive(Deserialize)]
    struct Runs {
        #[serde(default)]
        workflow_runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        status: String,
        conclusion: Option<String>,
    }
    let Some(token) = oauth::github_token() else {
        return Ok(CiStatus { state: "none".into() });
    };
    let resp = client()
        .get(format!("{GH}/repos/{slug}/actions/runs?per_page=1"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(CiStatus { state: "none".into() });
    }
    let runs: Runs = resp.json().await.map_err(|e| e.to_string())?;
    let state = match runs.workflow_runs.first() {
        Some(r) if r.status != "completed" => "pending",
        Some(r) => match r.conclusion.as_deref() {
            Some("success") => "success",
            Some("failure") | Some("timed_out") | Some("startup_failure") => "failure",
            _ => "none",
        },
        None => "none",
    };
    Ok(CiStatus { state: state.to_string() })
}
