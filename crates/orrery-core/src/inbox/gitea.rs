//! Gitea / Forgejo / Codeberg for the inbox and notifications, over the
//! REST API v1. Everything here needs the user's token, so it only ever talks
//! to `codeberg.org` or a domain on `gitea_hosts` (see
//! [`forge::gitea_host_trusted`]).
//!
//! - **Inbox**: PRs you opened ("pr"), PRs awaiting your review ("review"),
//!   issues assigned to you ("issue"), via `/repos/issues/search`.
//! - **Notifications**: your unread notification threads.

use serde::Deserialize;

use super::{paginate, parse_iso8601, InboxItem, Notification};
use crate::forge;
use crate::model::Host;

const PER_PAGE: usize = 50;
const MAX_PAGES: usize = 3;

/// Refuse to send the token anywhere but a trusted Gitea.
fn check_trusted(domain: &str, gitea_hosts: &[String]) -> Result<(), String> {
    if forge::gitea_host_trusted(domain, gitea_hosts) {
        Ok(())
    } else {
        Err(format!("not a trusted Gitea host: {domain}"))
    }
}

fn ts(s: Option<&str>) -> i64 {
    s.and_then(parse_iso8601).unwrap_or(0)
}

#[derive(Deserialize)]
struct RepoMeta {
    full_name: String,
}

#[derive(Deserialize)]
struct PrMeta {
    /// Newer Gitea/Forgejo only; older instances mark drafts by title prefix.
    #[serde(default)]
    draft: bool,
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    html_url: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    repository: Option<RepoMeta>,
    #[serde(default)]
    pull_request: Option<PrMeta>,
}

/// Gitea's draft convention for instances without the `draft` field: a
/// "WIP:" / "Draft:" style title prefix (its default `WORK_IN_PROGRESS_PREFIXES`).
fn draft_title(title: &str) -> bool {
    let t = title.trim_start().to_ascii_lowercase();
    ["wip:", "[wip]", "draft:", "[draft]"]
        .iter()
        .any(|p| t.starts_with(p))
}

fn issue_item(i: Issue, kind: &str, domain: &str) -> InboxItem {
    let created_at = ts(i.created_at.as_deref());
    let draft = i.pull_request.as_ref().is_some_and(|p| p.draft)
        || (i.pull_request.is_some() && draft_title(&i.title));
    InboxItem {
        kind: kind.into(),
        repo: i.repository.map(|r| r.full_name).unwrap_or_default(),
        title: i.title,
        url: i.html_url,
        number: i.number,
        draft,
        host: Host::Gitea,
        domain: domain.into(),
        created_at,
        // Gitea doesn't say when a review was requested; the PR's opening
        // time is an upper bound.
        waiting_since: created_at,
    }
}

/// Authored PRs, then review requests, then assigned issues, one entry per
/// URL. Pure.
fn assemble(
    domain: &str,
    authored: Vec<Issue>,
    reviews: Vec<Issue>,
    issues: Vec<Issue>,
) -> Vec<InboxItem> {
    let mut seen = std::collections::HashSet::new();
    authored
        .into_iter()
        .map(|i| (i, "pr"))
        .chain(reviews.into_iter().map(|i| (i, "review")))
        .chain(issues.into_iter().map(|i| (i, "issue")))
        .filter(|(i, _)| seen.insert(i.html_url.clone()))
        .map(|(i, kind)| issue_item(i, kind, domain))
        .collect()
}

/// One filtered page set of `/repos/issues/search` (open items only).
async fn search(domain: &str, token: &str, filter: &str) -> Result<Vec<Issue>, String> {
    let base = forge::gitea_api(domain);
    paginate(
        token,
        domain,
        "Gitea API",
        PER_PAGE,
        MAX_PAGES,
        |page| {
            format!("{base}/repos/issues/search?state=open&{filter}&limit={PER_PAGE}&page={page}")
        },
        |v: Vec<Issue>| v,
    )
    .await
}

/// Your Gitea / Forgejo inbox on `domain`: open PRs you created, open PRs
/// awaiting your review, and open issues assigned to you. `domain` must be
/// `codeberg.org` or on `gitea_hosts`; anything else is refused before the
/// token leaves the process. Any list failing fails the whole call.
pub async fn gitea_inbox(
    domain: &str,
    token: &str,
    gitea_hosts: &[String],
) -> Result<Vec<InboxItem>, String> {
    check_trusted(domain, gitea_hosts)?;
    let (authored, reviews, issues) = futures_util::future::try_join3(
        search(domain, token, "type=pulls&created=true"),
        search(domain, token, "type=pulls&review_requested=true"),
        search(domain, token, "type=issues&assigned=true"),
    )
    .await?;
    Ok(assemble(domain, authored, reviews, issues))
}

#[derive(Deserialize)]
struct Thread {
    subject: Subject,
    #[serde(default)]
    repository: Option<RepoMeta>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct Subject {
    #[serde(default)]
    title: String,
    /// "Issue" | "Pull" | "Commit" | "Repository"
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    html_url: Option<String>,
    /// "open" | "closed" | "merged"
    #[serde(default)]
    state: Option<String>,
}

fn thread_notification(t: Thread, domain: &str) -> Notification {
    Notification {
        title: t.subject.title,
        repo: t.repository.map(|r| r.full_name).unwrap_or_default(),
        reason: t.subject.state.unwrap_or_default(),
        kind: t.subject.kind,
        url: t.subject.html_url.filter(|u| !u.is_empty()),
        host: Host::Gitea,
        domain: domain.into(),
        updated_at: ts(t.updated_at.as_deref()),
    }
}

/// Your unread Gitea / Forgejo notifications on `domain`. Gitea gives no
/// "reason", so `reason` carries the subject's state ("open", "merged"…).
/// Same trust rule as [`gitea_inbox`].
pub async fn gitea_notifications(
    domain: &str,
    token: &str,
    gitea_hosts: &[String],
) -> Result<Vec<Notification>, String> {
    check_trusted(domain, gitea_hosts)?;
    let base = forge::gitea_api(domain);
    let threads = paginate(
        token,
        domain,
        "Gitea API",
        PER_PAGE,
        MAX_PAGES,
        |page| format!("{base}/notifications?status-types=unread&limit={PER_PAGE}&page={page}"),
        |v: Vec<Thread>| v,
    )
    .await?;
    Ok(threads
        .into_iter()
        .map(|t| thread_notification(t, domain))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issues(v: serde_json::Value) -> Vec<Issue> {
        serde_json::from_value(v).unwrap()
    }

    fn pr_json(n: u64, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 900 + n,
            "number": n,
            "title": title,
            "html_url": format!("https://codeberg.org/o/r/pulls/{n}"),
            "state": "open",
            "created_at": "2026-10-01T10:00:00+02:00",
            "updated_at": "2026-10-02T10:00:00+02:00",
            "repository": {"id": 1, "name": "r", "owner": "o", "full_name": "o/r"},
            "pull_request": {"merged": false, "merged_at": null, "draft": false}
        })
    }

    #[test]
    fn assemble_maps_kinds_dedupes_and_reads_offsets() {
        let authored = issues(serde_json::json!([pr_json(1, "Add x")]));
        // Requested to review your own PR (dropped) and someone else's.
        let reviews = issues(serde_json::json!([
            pr_json(1, "Add x"),
            pr_json(2, "WIP: Fix y")
        ]));
        let assigned = issues(serde_json::json!([{
            "number": 7,
            "title": "Bug",
            "html_url": "https://codeberg.org/o/r/issues/7",
            "created_at": "2026-09-01T00:00:00Z",
            "repository": {"full_name": "o/r"},
            "pull_request": null
        }]));
        let items = assemble("codeberg.org", authored, reviews, assigned);
        let summary: Vec<(&str, u64, bool)> = items
            .iter()
            .map(|i| (i.kind.as_str(), i.number, i.draft))
            .collect();
        assert_eq!(
            summary,
            vec![("pr", 1, false), ("review", 2, true), ("issue", 7, false)]
        );
        assert!(items
            .iter()
            .all(|i| i.host == Host::Gitea && i.repo == "o/r"));
        // "+02:00" is honoured: 10:00 local is 08:00 UTC.
        assert_eq!(
            items[0].created_at,
            parse_iso8601("2026-10-01T08:00:00Z").unwrap()
        );
        assert_eq!(items[1].waiting_since, items[1].created_at);
    }

    #[test]
    fn draft_detection_reads_flag_or_title() {
        assert!(draft_title("WIP: thing"));
        assert!(draft_title("  [draft] thing"));
        assert!(!draft_title("Fix wipe: thing"));
        let mut flagged = pr_json(3, "Ready");
        flagged["pull_request"]["draft"] = true.into();
        let items = assemble(
            "codeberg.org",
            issues(serde_json::json!([flagged])),
            vec![],
            vec![],
        );
        assert!(items[0].draft);
        // An issue titled "WIP:" isn't a draft PR.
        let issue = issues(serde_json::json!([{
            "number": 4, "title": "WIP: plan", "html_url": "https://codeberg.org/o/r/issues/4"
        }]));
        let items = assemble("codeberg.org", vec![], vec![], issue);
        assert!(!items[0].draft);
    }

    #[test]
    fn notification_threads_map_to_notifications() {
        let t: Thread = serde_json::from_value(serde_json::json!({
            "id": 12,
            "repository": {"full_name": "o/r"},
            "subject": {
                "title": "Add x",
                "url": "https://codeberg.org/api/v1/repos/o/r/pulls/1",
                "html_url": "https://codeberg.org/o/r/pulls/1",
                "type": "Pull",
                "state": "open"
            },
            "unread": true,
            "pinned": false,
            "updated_at": "2026-10-30T09:00:00Z",
            "url": "https://codeberg.org/api/v1/notifications/threads/12"
        }))
        .unwrap();
        let n = thread_notification(t, "codeberg.org");
        assert_eq!(n.title, "Add x");
        assert_eq!(n.repo, "o/r");
        assert_eq!(n.kind, "Pull");
        assert_eq!(n.reason, "open");
        assert_eq!(n.url.as_deref(), Some("https://codeberg.org/o/r/pulls/1"));
        assert_eq!(n.host, Host::Gitea);
        assert_eq!(n.domain, "codeberg.org");
    }

    #[test]
    fn untrusted_hosts_are_refused_before_any_request() {
        assert!(check_trusted("codeberg.org", &[]).is_ok());
        assert!(check_trusted("git.acme.io", &["git.acme.io".into()]).is_ok());
        assert!(check_trusted("git.acme.io", &[]).is_err());
        assert!(check_trusted("gitlab.com", &[]).is_err());
    }
}
