//! Cross-host "dev inbox" (Phase 7): the things a dev checks constantly — open
//! PRs, review requests, assigned issues, host notifications, CI status, and
//! starred repos. GitHub is implemented via its REST/search API using the
//! resolved token (stored OAuth → env → `gh`); GitLab support can layer on the
//! same shapes later.

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::forge::{self, client, GH_CORE};
use crate::model::Host;
use crate::oauth;

const GH: &str = "https://api.github.com";

/// Rate-limit buckets (see [`forge::send`]): GitHub meters search and GraphQL
/// separately from the core REST API.
const GH_SEARCH: &str = "GitHub search";
const GH_GRAPHQL: &str = "GitHub GraphQL";

/// The resolved token, or `msg` as the error (shown in the UI).
async fn token_or(msg: &str) -> Result<String, String> {
    oauth::github_token_async()
        .await
        .ok_or_else(|| msg.to_string())
}

// ── GraphQL ──────────────────────────────────────────────────────────────────

/// A GraphQL response envelope. GitHub answers HTTP 200 even when the query
/// failed (rate limited, missing scope, not found) and reports it in `errors`,
/// so this must be read — not just `data`.
#[derive(Deserialize)]
struct Gql<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GqlError>,
}
#[derive(Deserialize)]
struct GqlError {
    message: String,
}

/// Split an envelope into its data and its joined error messages. No data is
/// an `Err` carrying the real reason (or a generic one when GitHub gave none);
/// data *with* errors (partial result, e.g. `repository: null` + NOT_FOUND)
/// returns both so the caller can pick. Pure for testing.
fn gql_result<T>(resp: Gql<T>) -> Result<(T, Option<String>), String> {
    let errors = (!resp.errors.is_empty()).then(|| {
        resp.errors
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ")
    });
    match resp.data {
        Some(data) => Ok((data, errors)),
        None => Err(errors
            .map(|e| format!("GitHub GraphQL: {e}"))
            .unwrap_or_else(|| "GitHub GraphQL returned no data".into())),
    }
}

/// POST a GraphQL query and parse the envelope (see [`gql_result`]).
async fn graphql<T: DeserializeOwned>(
    token: &str,
    query: &str,
    variables: serde_json::Value,
) -> Result<(T, Option<String>), String> {
    let req = client()
        .post(format!("{GH}/graphql"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "query": query, "variables": variables }));
    let resp = forge::send(req, GH_GRAPHQL).await?;
    if !resp.status().is_success() {
        return Err(forge::github_error(resp, "GitHub GraphQL").await);
    }
    gql_result(resp.json::<Gql<T>>().await.map_err(|e| e.to_string())?)
}

// ── REST pagination ──────────────────────────────────────────────────────────

/// Fetch up to `max_pages` pages of `per_page` from a GitHub list endpoint —
/// `url(page)` builds each page's URL, `items` pulls the entries out of the
/// parsed page. Stops early when a page comes back short; any page failing
/// (401 expired token, 403/429 rate limit) propagates as an error rather than
/// a misleading partial/empty list. `what` labels the error.
async fn paginate<P: DeserializeOwned, T>(
    token: &str,
    bucket: &str,
    what: &str,
    per_page: usize,
    max_pages: usize,
    url: impl Fn(usize) -> String,
    items: impl Fn(P) -> Vec<T>,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for page in 1..=max_pages {
        let resp = forge::send(client().get(url(page)).bearer_auth(token), bucket).await?;
        if !resp.status().is_success() {
            return Err(format!("{what} {}", resp.status()));
        }
        let page_items = items(resp.json::<P>().await.map_err(|e| e.to_string())?);
        let page_len = page_items.len();
        out.extend(page_items);
        if !page_may_have_more(page_len, per_page) {
            break;
        }
    }
    Ok(out)
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
        // A failed query (rate limit, missing scope) surfaces its real reason
        // instead of an empty-but-Ok feed.
        let (data, _partial): (Data, _) =
            graphql(token, QUERY, serde_json::json!({ "cursor": cursor })).await?;
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
    let req = client()
        .get(format!("{GH}/users/{login}/received_events?per_page=100"))
        .bearer_auth(token);
    let resp = forge::send(req, GH_CORE).await?;
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
    let token = token_or("connect GitHub to see the feed").await?;

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

/// How long a resolved `/user` login is reused. The login for a token never
/// changes, so this only bounds how long a revoked token keeps "working" for
/// the login lookup (the real calls that follow still fail loudly).
const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);

/// Cached `(token fingerprint, login, fetched at)` — saves a `/user` round
/// trip on every inbox/feed refresh. Keyed by a hash of the token (not the
/// token itself), so a sign-out/switch of account misses the cache.
static LOGIN: std::sync::Mutex<Option<(u64, String, Instant)>> = std::sync::Mutex::new(None);

fn token_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// The cached login for `fp` if it's younger than [`LOGIN_TTL`]. Pure over
/// the cache slot for testing.
fn cached_login(slot: &Option<(u64, String, Instant)>, fp: u64, now: Instant) -> Option<String> {
    slot.as_ref()
        .filter(|(key, _, at)| *key == fp && now.duration_since(*at) < LOGIN_TTL)
        .map(|(_, login, _)| login.clone())
}

async fn github_user(token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }
    let fp = token_fingerprint(token);
    let cached = cached_login(
        &LOGIN.lock().unwrap_or_else(|e| e.into_inner()),
        fp,
        Instant::now(),
    );
    if let Some(login) = cached {
        return Ok(login);
    }
    let resp = forge::send(
        client().get(format!("{GH}/user")).bearer_auth(token),
        GH_CORE,
    )
    .await?;
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub auth failed ({}) — check the token scopes",
            resp.status()
        ));
    }
    let user: User = resp.json().await.map_err(|e| e.to_string())?;
    // The login is interpolated into URL paths below; GitHub logins are
    // alphanumeric + '-', so anything else is a malformed response.
    if user.login.is_empty()
        || !user
            .login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("GitHub returned an unexpected login".into());
    }
    *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((fp, user.login.clone(), Instant::now()));
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

/// Whether a just-fetched page suggests more results exist: a full page means
/// GitHub may have more, a short (or empty) page is the last one. Shared by
/// the bounded pagination loops below so they all stop early the same way.
fn page_may_have_more(page_len: usize, per_page: usize) -> bool {
    page_len >= per_page
}

#[cfg(test)]
mod tests {
    use super::{
        cached_login, check_run_state, gql_result, page_may_have_more, repo_from_url, rollup_state,
        split_slug, status_context_state, Gql, LOGIN_TTL,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn graphql_errors_surface_instead_of_empty_ok() {
        // GitHub's HTTP-200 rate-limit answer: no data, an errors array.
        let resp: Gql<serde_json::Value> = serde_json::from_value(serde_json::json!({
            "data": null,
            "errors": [{"type": "RATE_LIMITED", "message": "API rate limit exceeded"}],
        }))
        .unwrap();
        let err = gql_result(resp).unwrap_err();
        assert!(err.contains("API rate limit exceeded"), "{err}");

        // No data and no errors is still an error, not an empty success.
        let resp: Gql<serde_json::Value> = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(gql_result(resp).is_err());

        // Partial data: keep the data, hand back the reason too.
        let resp: Gql<serde_json::Value> = serde_json::from_value(serde_json::json!({
            "data": {"repository": null},
            "errors": [
                {"message": "Could not resolve to a Repository"},
                {"message": "second"}
            ],
        }))
        .unwrap();
        let (data, errs) = gql_result(resp).unwrap();
        assert!(data["repository"].is_null());
        assert_eq!(
            errs.as_deref(),
            Some("Could not resolve to a Repository; second")
        );

        // Clean success.
        let resp: Gql<serde_json::Value> =
            serde_json::from_value(serde_json::json!({"data": {"ok": 1}})).unwrap();
        assert_eq!(gql_result(resp).unwrap().1, None);
    }

    #[test]
    fn login_cache_is_keyed_by_token_and_expires() {
        let t0 = Instant::now();
        let slot = Some((7u64, "octocat".to_string(), t0));
        assert_eq!(cached_login(&slot, 7, t0).as_deref(), Some("octocat"));
        // Another token (signed out / switched account) misses.
        assert_eq!(cached_login(&slot, 8, t0), None);
        // Stale entries miss.
        assert_eq!(
            cached_login(&slot, 7, t0 + LOGIN_TTL + Duration::from_secs(1)),
            None
        );
        assert_eq!(cached_login(&None, 7, t0), None);
    }

    #[test]
    fn repo_from_url_extracts_owner_repo() {
        assert_eq!(
            repo_from_url("https://api.github.com/repos/Hankanman/Orrery"),
            "Hankanman/Orrery"
        );
        assert_eq!(repo_from_url("https://api.github.com/repos/a/b"), "a/b");
    }

    #[test]
    fn pagination_stops_on_short_page_and_continues_on_full_page() {
        // A full page means GitHub may have more → keep going.
        assert!(page_may_have_more(50, 50));
        // A short or empty page is the last one → stop.
        assert!(!page_may_have_more(49, 50));
        assert!(!page_may_have_more(0, 50));
        // A page longer than requested (shouldn't happen, but defensively):
        // still "may have more".
        assert!(page_may_have_more(61, 60));
    }

    #[test]
    fn split_slug_splits_owner_and_name() {
        assert_eq!(split_slug("Hankanman/Orrery"), Ok(("Hankanman", "Orrery")));
        assert!(split_slug("noslash").is_err());
        assert!(split_slug("o/../x").is_err());
        assert!(split_slug("group/sub/proj").is_err());
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

/// GitHub issue/PR search, paginated up to 3 pages × 50 items (150 per query)
/// — bounded because the search API has its own tight rate limit. Stops early
/// when a page comes back short. Any page failing (401 expired token, 403/429
/// rate limit) propagates as an error rather than a misleading empty inbox.
async fn gh_search(token: &str, query: &str, kind: &str) -> Result<Vec<InboxItem>, String> {
    const PER_PAGE: usize = 50;
    const MAX_PAGES: usize = 3;
    let q = urlencoding::encode(query);
    paginate(
        token,
        GH_SEARCH,
        "GitHub search",
        PER_PAGE,
        MAX_PAGES,
        |page| format!("{GH}/search/issues?per_page={PER_PAGE}&page={page}&q={q}"),
        |parsed: SearchResp| {
            parsed
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
                .collect()
        },
    )
    .await
}

/// Open PRs authored by the user, PRs awaiting their review, and issues
/// assigned to them. The three searches are independent, so they run
/// concurrently (latency = the slowest, not the sum); order in the result is
/// still pr → review → issue.
pub async fn github_inbox() -> Result<Vec<InboxItem>, String> {
    let token = token_or("connect GitHub to use the inbox").await?;
    let login = github_user(&token).await?;

    let authored = format!("is:open is:pr author:{login}");
    let reviews = format!("is:open is:pr review-requested:{login}");
    let assigned = format!("is:open is:issue assignee:{login}");
    let (mut items, reviews, issues) = futures_util::future::try_join3(
        gh_search(&token, &authored, "pr"),
        gh_search(&token, &reviews, "review"),
        gh_search(&token, &assigned, "issue"),
    )
    .await?;
    items.extend(reviews);
    items.extend(issues);
    Ok(items)
}

/// Unread notifications, paginated up to 3 pages × 50 (150 total) so a busy
/// account isn't silently cut off at the first page. Stops early when a page
/// comes back short; any page failing propagates as an error (same rule as
/// `gh_search`).
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
    const PER_PAGE: usize = 50;
    const MAX_PAGES: usize = 3;
    let token = token_or("connect GitHub to see notifications").await?;
    paginate(
        &token,
        GH_CORE,
        "GitHub API",
        PER_PAGE,
        MAX_PAGES,
        |page| format!("{GH}/notifications?per_page={PER_PAGE}&page={page}"),
        |raw: Vec<N>| {
            raw.into_iter()
                .map(|n| Notification {
                    title: n.subject.title,
                    repo: n.repository.full_name,
                    reason: n.reason,
                    kind: n.subject.kind,
                })
                .collect()
        },
    )
    .await
}

/// The user's starred repos, paginated up to 3 pages × 60 (180 total,
/// most-recently-starred first) — same order of cap as the feed's ~200-star
/// GraphQL fetch in `release_items`. Stops early when a page comes back
/// short; any page failing propagates as an error (same rule as `gh_search`).
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
    const PER_PAGE: usize = 60;
    const MAX_PAGES: usize = 3;
    let token = token_or("connect GitHub to browse stars").await?;
    paginate(
        &token,
        GH_CORE,
        "GitHub API",
        PER_PAGE,
        MAX_PAGES,
        |page| format!("{GH}/user/starred?per_page={PER_PAGE}&page={page}"),
        |raw: Vec<R>| {
            raw.into_iter()
                .map(|r| RemoteRepo {
                    slug: r.full_name,
                    description: r.description,
                    stars: r.stargazers_count,
                    language: r.language,
                    clone_url: r.clone_url,
                    host: Host::Github,
                })
                .collect()
        },
    )
    .await
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

/// Split a validated GitHub `owner/name` slug.
fn split_slug(slug: &str) -> Result<(&str, &str), String> {
    forge::check_github_slug(slug)?;
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

    let (owner, name) = split_slug(slug)?;
    let token = token_or("connect GitHub to view PRs").await?;
    let (data, errors): (Data, _) = graphql(
        &token,
        QUERY,
        serde_json::json!({ "owner": owner, "name": name }),
    )
    .await?;
    // A null repository comes with GitHub's reason in `errors` (not found,
    // missing scope, SAML enforcement…) — show that rather than guessing.
    let repo = data.repository.ok_or_else(|| {
        errors
            .map(|e| format!("GitHub GraphQL: {e}"))
            .unwrap_or_else(|| "repository not found (or no access)".into())
    })?;

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
    forge::check_github_slug(slug)?;
    let token = token_or("connect GitHub to merge").await?;
    let req = client()
        .put(format!("{GH}/repos/{slug}/pulls/{number}/merge"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "merge_method": method }));
    let resp = forge::send(req, GH_CORE).await?;
    if resp.status().is_success() {
        return Ok(());
    }
    // GitHub returns a JSON { message } explaining why (e.g. "Required status
    // checks must pass"); prefer it over the bare status code.
    Err(forge::github_error(resp, "GitHub merge").await)
}

/// Approve a PR (submit an APPROVE review with no body).
pub async fn github_approve_pr(slug: &str, number: u64) -> Result<(), String> {
    forge::check_github_slug(slug)?;
    let token = token_or("connect GitHub to approve").await?;
    let req = client()
        .post(format!("{GH}/repos/{slug}/pulls/{number}/reviews"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "event": "APPROVE" }));
    let resp = forge::send(req, GH_CORE).await?;
    if resp.status().is_success() {
        return Ok(());
    }
    Err(forge::github_error(resp, "GitHub approve").await)
}
