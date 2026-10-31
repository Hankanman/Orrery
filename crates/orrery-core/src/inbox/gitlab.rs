//! GitLab (gitlab.com and trusted self-hosted instances) for the inbox,
//! notifications and feed, over the REST API v4. Everything here needs the
//! user's token, so it only ever talks to a domain that passes
//! [`forge::gitlab_host_trusted`] — never to a domain taken from a repo
//! remote.
//!
//! - **Inbox**: MRs you authored ("pr"), MRs assigned to you ("pr"), MRs
//!   awaiting your review ("review"), issues assigned to you ("issue").
//! - **Notifications**: your pending to-do items (`/todos`).
//! - **Feed**: the latest release of each project you've starred.

use futures_util::stream::{self, StreamExt};
use serde::Deserialize;

use super::{paginate, parse_iso8601, truncate, FeedItem, InboxItem, Notification};
use crate::forge::{self, client};
use crate::model::Host;

const PER_PAGE: usize = 50;
const MAX_PAGES: usize = 3;

fn api(domain: &str) -> String {
    format!("https://{domain}/api/v4")
}

/// Refuse to send the token anywhere but a trusted GitLab.
fn check_trusted(domain: &str, gitlab_hosts: &[String]) -> Result<(), String> {
    if forge::gitlab_host_trusted(domain, gitlab_hosts) {
        Ok(())
    } else {
        Err(format!("not a trusted GitLab host: {domain}"))
    }
}

#[derive(Deserialize)]
struct Refs {
    full: String,
}

#[derive(Deserialize)]
struct Mr {
    iid: u64,
    title: String,
    web_url: String,
    #[serde(default)]
    draft: bool,
    /// The pre-15.0 name for `draft`.
    #[serde(default)]
    work_in_progress: bool,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    references: Option<Refs>,
}

#[derive(Deserialize)]
struct Issue {
    iid: u64,
    title: String,
    web_url: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    references: Option<Refs>,
}

#[derive(Deserialize)]
struct Todo {
    #[serde(default)]
    action_name: String,
    #[serde(default)]
    target_type: String,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    target: Option<TodoTarget>,
    #[serde(default)]
    project: Option<Project>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct TodoTarget {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
}

#[derive(Deserialize)]
struct Project {
    path_with_namespace: String,
}

/// A project path from GitLab's `references.full` ("group/sub/proj!12" /
/// "group/proj#3"), else from the web URL ("https://h/group/proj/-/merge_requests/12").
fn project_path(references: Option<&Refs>, web_url: &str) -> String {
    if let Some((path, _)) = references.and_then(|r| r.full.rsplit_once(['!', '#'])) {
        return path.to_string();
    }
    let after_host = web_url
        .split_once("://")
        .map_or(web_url, |(_, rest)| rest)
        .split_once('/')
        .map_or("", |(_, path)| path);
    after_host
        .split_once("/-/")
        .map_or(after_host, |(p, _)| p)
        .to_string()
}

fn ts(s: Option<&str>) -> i64 {
    s.and_then(parse_iso8601).unwrap_or(0)
}

fn mr_item(mr: Mr, kind: &str, domain: &str) -> InboxItem {
    let created_at = ts(mr.created_at.as_deref());
    InboxItem {
        kind: kind.into(),
        repo: project_path(mr.references.as_ref(), &mr.web_url),
        title: mr.title,
        url: mr.web_url,
        number: mr.iid,
        draft: mr.draft || mr.work_in_progress,
        host: Host::Gitlab,
        domain: domain.into(),
        created_at,
        waiting_since: created_at,
    }
}

fn issue_item(issue: Issue, domain: &str) -> InboxItem {
    let created_at = ts(issue.created_at.as_deref());
    InboxItem {
        kind: "issue".into(),
        repo: project_path(issue.references.as_ref(), &issue.web_url),
        title: issue.title,
        url: issue.web_url,
        number: issue.iid,
        draft: false,
        host: Host::Gitlab,
        domain: domain.into(),
        created_at,
        waiting_since: created_at,
    }
}

/// The URL a to-do points at (its target's page).
fn todo_url(todo: &Todo) -> Option<String> {
    todo.target
        .as_ref()
        .and_then(|t| t.web_url.clone())
        .or_else(|| todo.target_url.clone())
}

/// Put the inbox together: authored MRs, then review requests, then MRs
/// assigned to you that aren't already listed, then assigned issues — one
/// entry per URL. A review's `waiting_since` is the matching
/// `review_requested` to-do's creation time when there is one (the moment
/// the review was asked for), else the MR's opening time. Pure.
fn assemble(
    domain: &str,
    authored: Vec<Mr>,
    reviews: Vec<Mr>,
    assigned: Vec<Mr>,
    issues: Vec<Issue>,
    review_todos: &[Todo],
) -> Vec<InboxItem> {
    let requested_at: std::collections::HashMap<String, i64> = review_todos
        .iter()
        .filter(|t| t.action_name == "review_requested")
        .filter_map(|t| Some((todo_url(t)?, ts(t.created_at.as_deref()))))
        .filter(|(_, at)| *at > 0)
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mrs = authored
        .into_iter()
        .map(|m| (m, "pr"))
        .chain(reviews.into_iter().map(|m| (m, "review")))
        .chain(assigned.into_iter().map(|m| (m, "pr")));
    for (mr, kind) in mrs {
        if !seen.insert(mr.web_url.clone()) {
            continue;
        }
        let mut item = mr_item(mr, kind, domain);
        if kind == "review" {
            if let Some(at) = requested_at.get(&item.url) {
                item.waiting_since = *at;
            }
        }
        out.push(item);
    }
    out.extend(
        issues
            .into_iter()
            .filter(|i| seen.insert(i.web_url.clone()))
            .map(|i| issue_item(i, domain)),
    );
    out
}

fn todo_notification(todo: Todo, domain: &str) -> Notification {
    let url = todo_url(&todo);
    let title = todo
        .target
        .as_ref()
        .and_then(|t| t.title.clone())
        .filter(|t| !t.trim().is_empty())
        .or(todo.body)
        .unwrap_or_default();
    Notification {
        title,
        repo: todo
            .project
            .map(|p| p.path_with_namespace)
            .unwrap_or_default(),
        reason: todo.action_name,
        kind: todo.target_type,
        url,
        host: Host::Gitlab,
        domain: domain.into(),
        updated_at: ts(todo.created_at.as_deref()),
    }
}

/// A paginated GET of `{api}{path}` (which already carries its own query).
async fn list<T: serde::de::DeserializeOwned>(
    domain: &str,
    token: &str,
    path: &str,
) -> Result<Vec<T>, String> {
    let base = api(domain);
    paginate(
        token,
        domain,
        "GitLab API",
        PER_PAGE,
        MAX_PAGES,
        |page| format!("{base}{path}&per_page={PER_PAGE}&page={page}"),
        |v: Vec<T>| v,
    )
    .await
}

/// The signed-in user's username on `domain`.
async fn username(domain: &str, token: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct User {
        username: String,
    }
    let req = client()
        .get(format!("{}/user", api(domain)))
        .bearer_auth(token);
    let resp = forge::send(req, domain).await?;
    if !resp.status().is_success() {
        return Err(format!(
            "GitLab auth failed ({}) — check the token scopes",
            resp.status()
        ));
    }
    let user: User = resp.json().await.map_err(|e| e.to_string())?;
    if user.username.is_empty() {
        return Err("GitLab returned an empty username".into());
    }
    Ok(user.username)
}

/// Your GitLab inbox on `domain`: authored MRs, MRs assigned to you, MRs
/// awaiting your review, and issues assigned to you (open only). `domain`
/// must be `gitlab.com` or on `gitlab_hosts`; anything else is refused before
/// the token leaves the process. Any list failing fails the whole call (like
/// [`super::github_inbox`]); the review to-dos used for `waiting_since` are
/// best-effort.
pub async fn gitlab_inbox(
    domain: &str,
    token: &str,
    gitlab_hosts: &[String],
) -> Result<Vec<InboxItem>, String> {
    check_trusted(domain, gitlab_hosts)?;
    let user = urlencoding::encode(&username(domain, token).await?).into_owned();
    let review_path = format!("/merge_requests?state=opened&scope=all&reviewer_username={user}");
    let (lists, todos) = futures_util::future::join(
        futures_util::future::try_join4(
            list::<Mr>(
                domain,
                token,
                "/merge_requests?state=opened&scope=created_by_me",
            ),
            list::<Mr>(domain, token, &review_path),
            list::<Mr>(
                domain,
                token,
                "/merge_requests?state=opened&scope=assigned_to_me",
            ),
            list::<Issue>(domain, token, "/issues?state=opened&scope=assigned_to_me"),
        ),
        list::<Todo>(
            domain,
            token,
            "/todos?state=pending&action=review_requested&type=MergeRequest",
        ),
    )
    .await;
    let (authored, reviews, assigned, issues) = lists?;
    Ok(assemble(
        domain,
        authored,
        reviews,
        assigned,
        issues,
        &todos.unwrap_or_default(),
    ))
}

/// Your pending GitLab to-do items on `domain` (review requests, mentions,
/// assignments, failed pipelines…) as [`Notification`]s: `reason` is the
/// to-do's `action_name` (e.g. "build_failed"), `kind` its `target_type`.
/// Same trust rule as [`gitlab_inbox`].
pub async fn gitlab_todos(
    domain: &str,
    token: &str,
    gitlab_hosts: &[String],
) -> Result<Vec<Notification>, String> {
    check_trusted(domain, gitlab_hosts)?;
    let todos = list::<Todo>(domain, token, "/todos?state=pending").await?;
    Ok(todos
        .into_iter()
        .map(|t| todo_notification(t, domain))
        .collect())
}

#[derive(Deserialize)]
struct Starred {
    id: u64,
    path_with_namespace: String,
    web_url: String,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    name: Option<String>,
    tag_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    released_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    /// A release whose `released_at` is in the future.
    #[serde(default)]
    upcoming_release: bool,
    #[serde(default, rename = "_links")]
    links: Option<Links>,
}

#[derive(Deserialize)]
struct Links {
    #[serde(default, rename = "self")]
    self_url: Option<String>,
}

/// A feed entry for a starred project's latest release (`None` for an
/// upcoming one). Pure.
fn release_item(project: &Starred, rel: Release) -> Option<FeedItem> {
    if rel.upcoming_release {
        return None;
    }
    let url = rel
        .links
        .and_then(|l| l.self_url)
        .unwrap_or_else(|| format!("{}/-/releases/{}", project.web_url, rel.tag_name));
    let title = match rel.name {
        Some(n) if !n.trim().is_empty() => n,
        _ => rel.tag_name.clone(),
    };
    Some(FeedItem {
        kind: "release".into(),
        actor: None,
        repo: project.path_with_namespace.clone(),
        title,
        tag: rel.tag_name,
        detail: truncate(rel.description.unwrap_or_default(), 320),
        url,
        timestamp: ts(rel.released_at.as_deref().or(rel.created_at.as_deref())),
        // GitLab releases have no pre-release flag.
        prerelease: false,
        host: Host::Gitlab,
    })
}

/// How many starred projects the feed checks for releases (one request
/// each, so it is kept small), most recently active first.
const FEED_PROJECTS: usize = 30;

/// Releases from the projects you've starred on `domain`: the latest
/// release of each of your [`FEED_PROJECTS`] most recently active starred
/// projects. Same trust rule as [`gitlab_inbox`]. A project whose release
/// lookup fails is skipped; failing to list the stars fails the call.
pub async fn gitlab_feed(
    domain: &str,
    token: &str,
    gitlab_hosts: &[String],
) -> Result<Vec<FeedItem>, String> {
    check_trusted(domain, gitlab_hosts)?;
    let base = api(domain);
    let req = client()
        .get(format!(
            "{base}/projects?starred=true&simple=true&order_by=last_activity_at&per_page={FEED_PROJECTS}"
        ))
        .bearer_auth(token);
    let resp = forge::send(req, domain).await?;
    if !resp.status().is_success() {
        return Err(format!("GitLab API {}", resp.status()));
    }
    let starred: Vec<Starred> = resp.json().await.map_err(|e| e.to_string())?;

    let items = stream::iter(starred)
        .map(|project| {
            let base = base.clone();
            async move {
                let req = client()
                    .get(format!(
                        "{base}/projects/{}/releases?per_page=1",
                        project.id
                    ))
                    .bearer_auth(token);
                let resp = forge::send(req, domain).await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                let rel = resp.json::<Vec<Release>>().await.ok()?.into_iter().next()?;
                release_item(&project, rel)
            }
        })
        .buffer_unordered(4)
        .filter_map(|x| async move { x })
        .collect()
        .await;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mrs(v: serde_json::Value) -> Vec<Mr> {
        serde_json::from_value(v).unwrap()
    }

    fn mr_json(iid: u64, path: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 1000 + iid,
            "iid": iid,
            "project_id": 7,
            "title": format!("MR {iid}"),
            "state": "opened",
            "draft": false,
            "work_in_progress": false,
            "created_at": "2026-10-01T09:00:00.000Z",
            "updated_at": "2026-10-20T09:00:00.000Z",
            "web_url": format!("https://gitlab.acme.io/{path}/-/merge_requests/{iid}"),
            "references": {"short": format!("!{iid}"), "full": format!("{path}!{iid}")}
        })
    }

    #[test]
    fn project_path_prefers_references_then_web_url() {
        let refs = Refs {
            full: "group/sub/proj!12".into(),
        };
        assert_eq!(project_path(Some(&refs), ""), "group/sub/proj");
        let refs = Refs {
            full: "group/proj#3".into(),
        };
        assert_eq!(project_path(Some(&refs), ""), "group/proj");
        assert_eq!(
            project_path(None, "https://gitlab.com/a/b/c/-/merge_requests/4"),
            "a/b/c"
        );
    }

    #[test]
    fn assemble_dedupes_and_dates_reviews_from_todos() {
        let authored = mrs(serde_json::json!([mr_json(1, "team/app")]));
        let reviews = mrs(serde_json::json!([mr_json(2, "team/lib")]));
        // Assigned to you: one you also authored (dropped), one new.
        let assigned = mrs(serde_json::json!([
            mr_json(1, "team/app"),
            mr_json(3, "team/app")
        ]));
        let issues: Vec<Issue> = serde_json::from_value(serde_json::json!([{
            "iid": 9,
            "title": "Crash on start",
            "web_url": "https://gitlab.acme.io/team/app/-/issues/9",
            "created_at": "2026-09-01T00:00:00Z",
            "references": {"full": "team/app#9"}
        }]))
        .unwrap();
        let todos: Vec<Todo> = serde_json::from_value(serde_json::json!([{
            "id": 55,
            "action_name": "review_requested",
            "target_type": "MergeRequest",
            "target": {
                "iid": 2,
                "title": "MR 2",
                "web_url": "https://gitlab.acme.io/team/lib/-/merge_requests/2"
            },
            "target_url": "https://gitlab.acme.io/team/lib/-/merge_requests/2",
            "created_at": "2026-10-28T12:00:00.000Z"
        }]))
        .unwrap();

        let items = assemble(
            "gitlab.acme.io",
            authored,
            reviews,
            assigned,
            issues,
            &todos,
        );
        let summary: Vec<(&str, &str, u64)> = items
            .iter()
            .map(|i| (i.kind.as_str(), i.repo.as_str(), i.number))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("pr", "team/app", 1),
                ("review", "team/lib", 2),
                ("pr", "team/app", 3),
                ("issue", "team/app", 9),
            ]
        );
        assert!(items.iter().all(|i| i.host == Host::Gitlab));
        assert!(items.iter().all(|i| i.domain == "gitlab.acme.io"));
        let opened = parse_iso8601("2026-10-01T09:00:00Z").unwrap();
        assert_eq!(items[0].created_at, opened);
        assert_eq!(items[0].waiting_since, opened);
        // The review waits from when it was requested, not when it opened.
        assert_eq!(items[1].created_at, opened);
        assert_eq!(
            items[1].waiting_since,
            parse_iso8601("2026-10-28T12:00:00Z").unwrap()
        );
    }

    #[test]
    fn draft_flag_reads_both_names() {
        let mut old = mr_json(4, "a/b");
        old["work_in_progress"] = true.into();
        let mut new = mr_json(5, "a/b");
        new["draft"] = true.into();
        let items = assemble(
            "gitlab.com",
            mrs(serde_json::json!([old, new, mr_json(6, "a/b")])),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &[],
        );
        let drafts: Vec<bool> = items.iter().map(|i| i.draft).collect();
        assert_eq!(drafts, vec![true, true, false]);
    }

    #[test]
    fn todos_map_to_notifications() {
        let todo: Todo = serde_json::from_value(serde_json::json!({
            "id": 1,
            "project": {"id": 7, "path_with_namespace": "team/app"},
            "action_name": "build_failed",
            "target_type": "MergeRequest",
            "target": {"title": "Add caching", "web_url": "https://gitlab.com/team/app/-/merge_requests/8"},
            "body": "Add caching",
            "state": "pending",
            "created_at": "2026-10-30T08:00:00.000Z"
        }))
        .unwrap();
        let n = todo_notification(todo, "gitlab.com");
        assert_eq!(n.title, "Add caching");
        assert_eq!(n.repo, "team/app");
        assert_eq!(n.reason, "build_failed");
        assert_eq!(n.kind, "MergeRequest");
        assert_eq!(
            n.url.as_deref(),
            Some("https://gitlab.com/team/app/-/merge_requests/8")
        );
        assert_eq!(n.host, Host::Gitlab);
        assert_eq!(n.updated_at, parse_iso8601("2026-10-30T08:00:00Z").unwrap());

        // A to-do without a target title falls back to its body.
        let todo: Todo = serde_json::from_value(serde_json::json!({
            "action_name": "mentioned",
            "target_type": "Commit",
            "body": "see abc123"
        }))
        .unwrap();
        let n = todo_notification(todo, "gitlab.com");
        assert_eq!(n.title, "see abc123");
        assert_eq!(n.url, None);
    }

    #[test]
    fn releases_map_to_feed_items() {
        let project: Starred = serde_json::from_value(serde_json::json!({
            "id": 7,
            "path_with_namespace": "team/app",
            "web_url": "https://gitlab.com/team/app"
        }))
        .unwrap();
        let rel: Release = serde_json::from_value(serde_json::json!({
            "name": "",
            "tag_name": "v1.2.0",
            "description": "Fixes",
            "created_at": "2026-10-01T00:00:00Z",
            "released_at": "2026-10-02T00:00:00Z",
            "upcoming_release": false,
            "_links": {"self": "https://gitlab.com/team/app/-/releases/v1.2.0"}
        }))
        .unwrap();
        let item = release_item(&project, rel).unwrap();
        assert_eq!(item.title, "v1.2.0", "empty name falls back to the tag");
        assert_eq!(item.repo, "team/app");
        assert_eq!(item.url, "https://gitlab.com/team/app/-/releases/v1.2.0");
        assert_eq!(
            item.timestamp,
            parse_iso8601("2026-10-02T00:00:00Z").unwrap()
        );
        assert_eq!(item.host, Host::Gitlab);

        let upcoming: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v2", "upcoming_release": true
        }))
        .unwrap();
        assert!(release_item(&project, upcoming).is_none());
    }

    #[test]
    fn untrusted_hosts_are_refused_before_any_request() {
        assert!(check_trusted("gitlab.com", &[]).is_ok());
        assert!(check_trusted("gitlab.acme.io", &["gitlab.acme.io".into()]).is_ok());
        assert!(check_trusted("evil.com", &[]).is_err());
        assert!(check_trusted("gitlab.acme.io", &[]).is_err());
    }
}
