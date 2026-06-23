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
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(UA)
            // Small JSON responses → bound both connect and overall time so a
            // hung host can't stall an inbox refresh indefinitely.
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default()
    });
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

/// One entry in the activity feed. `kind` distinguishes a starred-repo release
/// from a followed-user action so the UI can render the right sentence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    /// "release" | "starred" | "created" | "forked" | "public"
    pub kind: String,
    /// The followed user who did it (activity events); None for starred releases.
    pub actor: Option<String>,
    pub repo: String,   // owner/name
    pub title: String,  // release title, or empty for plain actions
    pub tag: String,    // release tag, or empty
    pub detail: String, // release notes, truncated
    pub url: String,
    pub timestamp: i64, // unix seconds
    pub prerelease: bool,
    pub host: Host,
}

/// Unix seconds from an ISO-8601 UTC timestamp ("2024-01-15T10:30:00Z").
/// Dependency-free (no chrono); good enough for feed ordering/display.
fn parse_iso8601(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let se: i64 = s.get(17..19)?.parse().ok()?;
    // days since 1970-01-01 (Howard Hinnant's days_from_civil)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = (if y2 >= 0 { y2 } else { y2 - 399 }) / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + se)
}

fn truncate(mut s: String, max: usize) -> String {
    if s.chars().count() > max {
        s = s.chars().take(max).collect::<String>() + "…";
    }
    s
}

/// Latest releases across the repos you've starred, via GitHub GraphQL — one
/// request fetches 100 stars *and* their latest release (far cheaper than
/// per-repo REST). Up to ~200 most-recently-starred repos.
async fn release_items(token: &str) -> Result<Vec<FeedItem>, String> {
    const QUERY: &str = r#"query($cursor: String) {
      viewer {
        starredRepositories(first: 100, after: $cursor, orderBy: {field: STARRED_AT, direction: DESC}) {
          pageInfo { hasNextPage endCursor }
          nodes {
            nameWithOwner
            releases(first: 1, orderBy: {field: CREATED_AT, direction: DESC}) {
              nodes { name tagName url publishedAt description isPrerelease isDraft }
            }
          }
        }
      }
    }"#;

    #[derive(Deserialize)]
    struct Resp {
        data: Option<Data>,
    }
    #[derive(Deserialize)]
    struct Data {
        viewer: Viewer,
    }
    #[derive(Deserialize)]
    struct Viewer {
        #[serde(rename = "starredRepositories")]
        starred: Starred,
    }
    #[derive(Deserialize)]
    struct Starred {
        #[serde(rename = "pageInfo")]
        page_info: PageInfo,
        nodes: Vec<RepoNode>,
    }
    #[derive(Deserialize)]
    struct PageInfo {
        #[serde(rename = "hasNextPage")]
        has_next_page: bool,
        #[serde(rename = "endCursor")]
        end_cursor: Option<String>,
    }
    #[derive(Deserialize)]
    struct RepoNode {
        #[serde(rename = "nameWithOwner")]
        name_with_owner: String,
        releases: Releases,
    }
    #[derive(Deserialize)]
    struct Releases {
        nodes: Vec<Rel>,
    }
    #[derive(Deserialize)]
    struct Rel {
        #[serde(default)]
        name: Option<String>,
        #[serde(rename = "tagName")]
        tag_name: String,
        url: String,
        #[serde(rename = "publishedAt")]
        published_at: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(rename = "isPrerelease", default)]
        prerelease: bool,
        #[serde(rename = "isDraft", default)]
        draft: bool,
    }

    let mut items: Vec<FeedItem> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..2 {
        let body = serde_json::json!({ "query": QUERY, "variables": { "cursor": cursor } });
        let resp = client()
            .post(format!("{GH}/graphql"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("GitHub GraphQL {}", resp.status()));
        }
        let parsed: Resp = resp.json().await.map_err(|e| e.to_string())?;
        let Some(data) = parsed.data else { break };
        let starred = data.viewer.starred;
        for node in starred.nodes {
            let Some(rel) = node.releases.nodes.into_iter().next() else {
                continue;
            };
            if rel.draft {
                continue;
            }
            let title = match rel.name {
                Some(n) if !n.trim().is_empty() => n,
                _ => rel.tag_name.clone(),
            };
            items.push(FeedItem {
                kind: "release".into(),
                actor: None,
                repo: node.name_with_owner,
                title,
                tag: rel.tag_name,
                detail: truncate(rel.description.unwrap_or_default(), 320),
                url: rel.url,
                timestamp: rel
                    .published_at
                    .as_deref()
                    .and_then(parse_iso8601)
                    .unwrap_or(0),
                prerelease: rel.prerelease,
                host: Host::Github,
            });
        }
        if !starred.page_info.has_next_page {
            break;
        }
        cursor = starred.page_info.end_cursor;
    }
    Ok(items)
}

/// Activity from the people you follow — GitHub's "received events" (the home
/// dashboard feed). Surfaces the meaningful event types.
async fn following_items(token: &str) -> Result<Vec<FeedItem>, String> {
    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "type")]
        kind: Option<String>,
        actor: Option<Actor>,
        repo: Option<EvRepo>,
        payload: Option<Payload>,
        created_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct Actor {
        login: String,
    }
    #[derive(Deserialize)]
    struct EvRepo {
        name: String,
    }
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        ref_type: Option<String>,
        #[serde(default)]
        release: Option<EvRelease>,
    }
    #[derive(Deserialize)]
    struct EvRelease {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        tag_name: Option<String>,
        #[serde(default)]
        html_url: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        prerelease: bool,
    }

    let login = github_user(token).await?;
    let resp = client()
        .get(format!("{GH}/users/{login}/received_events?per_page=100"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub events {}", resp.status()));
    }
    let events: Vec<Event> = resp.json().await.map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for e in events {
        let ts = e.created_at.as_deref().and_then(parse_iso8601).unwrap_or(0);
        let actor = e.actor.map(|a| a.login);
        let Some(repo) = e.repo.map(|r| r.name) else {
            continue;
        };
        let repo_url = format!("https://github.com/{repo}");
        let item =
            |kind: &str, title: String, tag: String, detail: String, url: String, pre: bool| {
                FeedItem {
                    kind: kind.into(),
                    actor: actor.clone(),
                    repo: repo.clone(),
                    title,
                    tag,
                    detail,
                    url,
                    timestamp: ts,
                    prerelease: pre,
                    host: Host::Github,
                }
            };
        match e.kind.as_deref() {
            Some("ReleaseEvent") => {
                if let Some(rel) = e.payload.and_then(|p| p.release) {
                    let tag = rel.tag_name.unwrap_or_default();
                    let title = match rel.name {
                        Some(n) if !n.trim().is_empty() => n,
                        _ => tag.clone(),
                    };
                    out.push(item(
                        "release",
                        title,
                        tag,
                        truncate(rel.body.unwrap_or_default(), 320),
                        rel.html_url.unwrap_or(repo_url),
                        rel.prerelease,
                    ));
                }
            }
            Some("WatchEvent") => out.push(item(
                "starred",
                String::new(),
                String::new(),
                String::new(),
                repo_url,
                false,
            )),
            Some("CreateEvent")
                if e.payload.as_ref().and_then(|p| p.ref_type.as_deref()) == Some("repository") =>
            {
                out.push(item(
                    "created",
                    String::new(),
                    String::new(),
                    String::new(),
                    repo_url,
                    false,
                ));
            }
            Some("ForkEvent") => out.push(item(
                "forked",
                String::new(),
                String::new(),
                String::new(),
                repo_url,
                false,
            )),
            Some("PublicEvent") => out.push(item(
                "public",
                String::new(),
                String::new(),
                String::new(),
                repo_url,
                false,
            )),
            _ => {}
        }
    }
    Ok(out)
}

/// The unified activity feed: starred-repo releases + activity from people you
/// follow, merged newest-first and de-duplicated by URL. Each source is
/// best-effort — a failure in one still returns the other.
pub async fn github_feed() -> Result<Vec<FeedItem>, String> {
    let token = oauth::github_token().ok_or("connect GitHub to see the feed")?;

    let mut items: Vec<FeedItem> = Vec::new();
    let mut errors = Vec::new();
    // The two sources are independent network fetches, so run them concurrently:
    // cold-load latency becomes the slower of the two rather than their sum.
    let (releases, following) =
        futures_util::future::join(release_items(&token), following_items(&token)).await;
    for source in [releases, following] {
        match source {
            Ok(r) => items.extend(r),
            Err(e) => errors.push(e),
        }
    }
    if items.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    items.sort_by_key(|i| std::cmp::Reverse(i.timestamp));
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.url.clone()));
    items.truncate(80);
    Ok(items)
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
        return Err(format!(
            "GitHub auth failed ({}) — check the token scopes",
            resp.status()
        ));
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
    repository_url
        .rsplit("/repos/")
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{check_run_state, repo_from_url, rollup_state, split_slug, status_context_state};

    #[test]
    fn repo_from_url_extracts_owner_repo() {
        assert_eq!(
            repo_from_url("https://api.github.com/repos/Hankanman/Orrery"),
            "Hankanman/Orrery"
        );
        assert_eq!(repo_from_url("https://api.github.com/repos/a/b"), "a/b");
    }

    #[test]
    fn split_slug_splits_owner_and_name() {
        assert_eq!(split_slug("Hankanman/Orrery"), Ok(("Hankanman", "Orrery")));
        assert!(split_slug("noslash").is_err());
    }

    #[test]
    fn check_run_state_maps_status_and_conclusion() {
        assert_eq!(check_run_state(Some("IN_PROGRESS"), None), "pending");
        assert_eq!(
            check_run_state(Some("COMPLETED"), Some("SUCCESS")),
            "success"
        );
        assert_eq!(
            check_run_state(Some("COMPLETED"), Some("FAILURE")),
            "failure"
        );
        assert_eq!(
            check_run_state(Some("COMPLETED"), Some("CANCELLED")),
            "failure"
        );
        assert_eq!(
            check_run_state(Some("COMPLETED"), Some("SKIPPED")),
            "neutral"
        );
    }

    #[test]
    fn rollup_and_status_context_states() {
        assert_eq!(rollup_state("SUCCESS"), "success");
        assert_eq!(rollup_state("ERROR"), "failure");
        assert_eq!(rollup_state("EXPECTED"), "pending");
        assert_eq!(status_context_state(Some("SUCCESS")), "success");
        assert_eq!(status_context_state(Some("ERROR")), "failure");
        assert_eq!(status_context_state(None), "pending");
    }
}

async fn gh_search(token: &str, query: &str, kind: &str) -> Result<Vec<InboxItem>, String> {
    let url = format!(
        "{GH}/search/issues?per_page=50&q={}",
        urlencoding::encode(query)
    );
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
    items.extend(
        gh_search(
            &token,
            &format!("is:open is:pr review-requested:{login}"),
            "review",
        )
        .await?,
    );
    items.extend(
        gh_search(
            &token,
            &format!("is:open is:issue assignee:{login}"),
            "issue",
        )
        .await?,
    );
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

// ── PR action panel (#67): per-repo checks / review / mergeable + merge ──────

/// One open PR for a repo, with enough detail to decide whether to merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub base: String,
    pub head: String,
    pub author: Option<String>,
    /// "clean" | "conflicting" | "unknown"
    pub mergeable: String,
    /// "approved" | "changes_requested" | "review_required" | "none"
    pub review_decision: String,
    /// Aggregate check rollup: "success" | "failure" | "pending" | "none"
    pub checks_state: String,
    pub checks: Vec<CheckRun>,
    pub reviews: Vec<PrReview>,
}

/// One status check / CI context on a PR's head commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub name: String,
    /// "success" | "failure" | "pending" | "neutral"
    pub state: String,
    pub url: Option<String>,
}

/// A submitted review that affects mergeability (approval or change request).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrReview {
    pub author: String,
    /// "approved" | "changes_requested"
    pub state: String,
}

/// A repo's open PRs plus the merge methods the repo actually permits, so the
/// UI can offer only valid choices (squash / rebase / merge).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrPanel {
    /// Subset of ["squash", "rebase", "merge"] allowed by repo settings.
    pub merge_methods: Vec<String>,
    pub prs: Vec<PrDetail>,
}

fn split_slug(slug: &str) -> Result<(&str, &str), String> {
    slug.split_once('/')
        .ok_or_else(|| format!("not an owner/name slug: {slug}"))
}

/// Open PRs for a repo with checks, reviews, and mergeable state — one GraphQL
/// round-trip. Also reports which merge methods the repo allows.
pub async fn github_prs(slug: &str) -> Result<PrPanel, String> {
    const QUERY: &str = r#"query($owner: String!, $name: String!) {
      repository(owner: $owner, name: $name) {
        mergeCommitAllowed
        squashMergeAllowed
        rebaseMergeAllowed
        pullRequests(first: 20, states: OPEN, orderBy: {field: UPDATED_AT, direction: DESC}) {
          nodes {
            number title url isDraft baseRefName headRefName
            mergeable reviewDecision
            author { login }
            commits(last: 1) {
              nodes {
                commit {
                  statusCheckRollup {
                    state
                    contexts(first: 50) {
                      nodes {
                        __typename
                        ... on CheckRun { name conclusion status detailsUrl }
                        ... on StatusContext { context state targetUrl }
                      }
                    }
                  }
                }
              }
            }
            reviews(last: 30, states: [APPROVED, CHANGES_REQUESTED]) {
              nodes { author { login } state }
            }
          }
        }
      }
    }"#;

    #[derive(Deserialize)]
    struct Resp {
        data: Option<Data>,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<RepoNode>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RepoNode {
        merge_commit_allowed: bool,
        squash_merge_allowed: bool,
        rebase_merge_allowed: bool,
        pull_requests: PrNodes,
    }
    #[derive(Deserialize)]
    struct PrNodes {
        nodes: Vec<PrNode>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PrNode {
        number: u64,
        title: String,
        url: String,
        is_draft: bool,
        base_ref_name: String,
        head_ref_name: String,
        mergeable: Option<String>,
        review_decision: Option<String>,
        author: Option<Login>,
        commits: Commits,
        reviews: ReviewNodes,
    }
    #[derive(Deserialize)]
    struct Login {
        login: String,
    }
    #[derive(Deserialize)]
    struct Commits {
        nodes: Vec<CommitNode>,
    }
    #[derive(Deserialize)]
    struct CommitNode {
        commit: Commit,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Commit {
        status_check_rollup: Option<Rollup>,
    }
    #[derive(Deserialize)]
    struct Rollup {
        state: Option<String>,
        contexts: Contexts,
    }
    #[derive(Deserialize)]
    struct Contexts {
        nodes: Vec<ContextNode>,
    }
    // A union of CheckRun and StatusContext — fields are optional per variant.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ContextNode {
        #[serde(rename = "__typename")]
        typename: String,
        // CheckRun
        name: Option<String>,
        conclusion: Option<String>,
        status: Option<String>,
        details_url: Option<String>,
        // StatusContext
        context: Option<String>,
        state: Option<String>,
        target_url: Option<String>,
    }
    #[derive(Deserialize)]
    struct ReviewNodes {
        nodes: Vec<ReviewNode>,
    }
    #[derive(Deserialize)]
    struct ReviewNode {
        author: Option<Login>,
        state: String,
    }

    let token = oauth::github_token().ok_or("connect GitHub to view PRs")?;
    let (owner, name) = split_slug(slug)?;
    let body = serde_json::json!({
        "query": QUERY,
        "variables": { "owner": owner, "name": name },
    });
    let resp = client()
        .post(format!("{GH}/graphql"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub GraphQL {}", resp.status()));
    }
    let parsed: Resp = resp.json().await.map_err(|e| e.to_string())?;
    let repo = parsed
        .data
        .and_then(|d| d.repository)
        .ok_or("repository not found (or no access)")?;

    let mut merge_methods = Vec::new();
    if repo.squash_merge_allowed {
        merge_methods.push("squash".to_string());
    }
    if repo.rebase_merge_allowed {
        merge_methods.push("rebase".to_string());
    }
    if repo.merge_commit_allowed {
        merge_methods.push("merge".to_string());
    }

    let prs = repo
        .pull_requests
        .nodes
        .into_iter()
        .map(|p| {
            let rollup = p
                .commits
                .nodes
                .into_iter()
                .next()
                .and_then(|c| c.commit.status_check_rollup);
            let checks_state = rollup
                .as_ref()
                .and_then(|r| r.state.as_deref())
                .map(rollup_state)
                .unwrap_or("none")
                .to_string();
            let checks = rollup
                .map(|r| {
                    r.contexts
                        .nodes
                        .into_iter()
                        .map(|c| {
                            if c.typename == "CheckRun" {
                                CheckRun {
                                    name: c.name.unwrap_or_default(),
                                    state: check_run_state(
                                        c.status.as_deref(),
                                        c.conclusion.as_deref(),
                                    )
                                    .to_string(),
                                    url: c.details_url,
                                }
                            } else {
                                CheckRun {
                                    name: c.context.unwrap_or_default(),
                                    state: status_context_state(c.state.as_deref()).to_string(),
                                    url: c.target_url,
                                }
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let reviews = p
                .reviews
                .nodes
                .into_iter()
                .map(|r| PrReview {
                    author: r.author.map(|a| a.login).unwrap_or_default(),
                    state: if r.state == "APPROVED" {
                        "approved"
                    } else {
                        "changes_requested"
                    }
                    .to_string(),
                })
                .collect();
            PrDetail {
                number: p.number,
                title: p.title,
                url: p.url,
                draft: p.is_draft,
                base: p.base_ref_name,
                head: p.head_ref_name,
                author: p.author.map(|a| a.login),
                mergeable: match p.mergeable.as_deref() {
                    Some("MERGEABLE") => "clean",
                    Some("CONFLICTING") => "conflicting",
                    _ => "unknown",
                }
                .to_string(),
                review_decision: match p.review_decision.as_deref() {
                    Some("APPROVED") => "approved",
                    Some("CHANGES_REQUESTED") => "changes_requested",
                    Some("REVIEW_REQUIRED") => "review_required",
                    _ => "none",
                }
                .to_string(),
                checks_state,
                checks,
                reviews,
            }
        })
        .collect();

    Ok(PrPanel { merge_methods, prs })
}

/// Map GitHub's StatusState rollup enum to our four-state vocabulary.
fn rollup_state(s: &str) -> &'static str {
    match s {
        "SUCCESS" => "success",
        "FAILURE" | "ERROR" => "failure",
        "PENDING" | "EXPECTED" => "pending",
        _ => "none",
    }
}

/// Map a CheckRun's (status, conclusion) to our four-state vocabulary.
fn check_run_state(status: Option<&str>, conclusion: Option<&str>) -> &'static str {
    if status != Some("COMPLETED") {
        return "pending";
    }
    match conclusion {
        Some("SUCCESS") => "success",
        Some("FAILURE")
        | Some("TIMED_OUT")
        | Some("STARTUP_FAILURE")
        | Some("CANCELLED")
        | Some("ACTION_REQUIRED") => "failure",
        Some("NEUTRAL") | Some("SKIPPED") => "neutral",
        _ => "pending",
    }
}

/// Map a legacy commit-status StatusState to our four-state vocabulary.
fn status_context_state(s: Option<&str>) -> &'static str {
    match s {
        Some("SUCCESS") => "success",
        Some("FAILURE") | Some("ERROR") => "failure",
        _ => "pending",
    }
}

/// Squash/rebase/merge a PR via REST. GitHub enforces branch protection and
/// rejects with a descriptive error if the merge isn't permitted — we surface
/// that rather than pre-judging it client-side.
pub async fn github_merge_pr(slug: &str, number: u64, method: &str) -> Result<(), String> {
    let method = match method {
        "squash" | "rebase" | "merge" => method,
        other => return Err(format!("invalid merge method: {other}")),
    };
    let token = oauth::github_token().ok_or("connect GitHub to merge")?;
    let resp = client()
        .put(format!("{GH}/repos/{slug}/pulls/{number}/merge"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "merge_method": method }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        return Ok(());
    }
    // GitHub returns a JSON { message } explaining why (e.g. "Required status
    // checks must pass"); prefer it over the bare status code.
    let status = resp.status();
    let msg = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("GitHub merge {status}"));
    Err(msg)
}

/// Approve a PR (submit an APPROVE review with no body).
pub async fn github_approve_pr(slug: &str, number: u64) -> Result<(), String> {
    let token = oauth::github_token().ok_or("connect GitHub to approve")?;
    let resp = client()
        .post(format!("{GH}/repos/{slug}/pulls/{number}/reviews"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "event": "APPROVE" }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let msg = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("GitHub approve {status}"));
    Err(msg)
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
        return Ok(CiStatus {
            state: "none".into(),
        });
    };
    let resp = client()
        .get(format!("{GH}/repos/{slug}/actions/runs?per_page=1"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(CiStatus {
            state: "none".into(),
        });
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
    Ok(CiStatus {
        state: state.to_string(),
    })
}
