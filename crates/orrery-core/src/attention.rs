//! The attention engine's pure core (#183): fold everything Orrery already
//! knows — local git state, the host inbox, CI, branch hygiene, agent
//! sessions — into one prioritized "needs you now" list.
//!
//! No I/O and no UI live here. Callers gather the facts (the scan snapshot,
//! an inbox fetch, CI polls, `git_ops::prunable`, the platform crate's agent
//! detection) and hand them to [`compute`]; every surface — sidebar badges,
//! the grid's Attention filter, the tray icon, toasts, notifications —
//! consumes the same ranked output, so urgency is decided in exactly one
//! place.

use serde::{Deserialize, Serialize};

use crate::inbox::InboxItem;
use crate::model::{Host, Repo};

/// What kind of thing needs attention. Extensible: append new variants (the
/// declaration order is the within-severity sort order, so append where the
/// new kind should rank among its severity peers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionKind {
    /// The latest default-branch CI run failed.
    CiFailing,
    /// A PR is waiting on your review.
    ReviewRequested,
    /// A coding-agent session finished and its output awaits you.
    AgentFinished,
    /// Uncommitted changes in the working tree.
    DirtyWorktree,
    /// Local commits not pushed to the upstream.
    Ahead,
    /// Upstream commits not pulled yet.
    Behind,
    /// A PR you authored is open (waiting on reviewers/CI, not on you).
    PrAssigned,
    /// Merged or upstream-gone branches that can be pruned.
    PrunableBranches,
    /// A coding-agent session is currently running.
    AgentRunning,
}

/// How urgently a kind needs the user. Variant order is the sort order
/// (ascending sort puts `Urgent` first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Someone or something *external* is blocked on you right now.
    Urgent,
    /// Your own work is parked or at risk; deal with it today.
    Attention,
    /// Ambient state / hygiene; act at leisure.
    Info,
}

impl AttentionKind {
    /// The severity tier for this kind. The rationale is a three-way split on
    /// *who is waiting*:
    ///
    /// - **Urgent — others are blocked on you.** `CiFailing`: a red default
    ///   branch blocks everyone building on it, and the longer it sits the
    ///   harder the bisect. `ReviewRequested`: a human asked for you by name
    ///   and cannot merge until you act.
    /// - **Attention — your own work is parked or at risk.** `AgentFinished`:
    ///   results are ready and idle; the session was the point, so look soon.
    ///   `DirtyWorktree`: uncommitted changes exist only in that working tree.
    ///   `Ahead`: unpushed commits exist only on this disk — invisible to the
    ///   team and unbacked-up.
    /// - **Info — ambient state, no deadline.** `Behind`: upstream moved but
    ///   nothing of yours is at risk; you'll fast-forward on the next pull.
    ///   `PrAssigned`: your open PR is waiting on reviewers/CI, not on you.
    ///   `PrunableBranches`: hygiene. `AgentRunning`: working as intended —
    ///   a passive readout, not a call to action.
    pub fn severity(self) -> Severity {
        match self {
            AttentionKind::CiFailing | AttentionKind::ReviewRequested => Severity::Urgent,
            AttentionKind::AgentFinished | AttentionKind::DirtyWorktree | AttentionKind::Ahead => {
                Severity::Attention
            }
            AttentionKind::Behind
            | AttentionKind::PrAssigned
            | AttentionKind::PrunableBranches
            | AttentionKind::AgentRunning => Severity::Info,
        }
    }
}

/// How an attention item points back at a repo. Local facts carry the stable
/// repo id (the absolute path, `Repo::id`); host facts carry the slug and —
/// where the source knows it — the remote host domain, the same compound key
/// as the enrichment cache (#159), and gain the local id when a scanned repo
/// matches. `name` is always set (display name, slug, or path basename) so
/// every surface has something to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// Local repo id (absolute path) when the repo is in the scanned fleet.
    pub id: Option<String>,
    /// Remote host domain (e.g. "github.com", "gitlab.acme.io"), when known.
    pub remote_host: Option<String>,
    /// owner/name slug, when the repo has a recognized remote.
    pub slug: Option<String>,
    /// Human display name — never empty.
    pub name: String,
}

/// One prioritized "needs you" item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionItem {
    pub repo: RepoRef,
    pub kind: AttentionKind,
    /// Denormalized from `kind.severity()` so surfaces never re-derive it (and
    /// so per-kind user tuning can override it later without changing shape).
    pub severity: Severity,
    /// One glanceable line.
    pub summary: String,
    /// Optional second line / routing hint (branch, PR URL, …).
    pub detail: Option<String>,
}

/// A repo's latest default-branch CI state, as polled by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiFact {
    /// Remote host domain (e.g. "github.com") — with `slug` this is the same
    /// compound key as the enrichment cache (#159), so the same "owner/repo"
    /// slug on two hosts can't cross-link.
    pub remote_host: String,
    pub slug: String,
    /// The shared four-state CI vocabulary from `inbox`:
    /// "success" | "failure" | "pending" | "none". Only "failure" raises
    /// attention.
    pub state: String,
}

/// Prunable-branch count for a local repo (from `git_ops::prunable`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrunableFact {
    /// Matches `Repo::id` (absolute path).
    pub repo_id: String,
    pub count: u32,
}

/// A coding-agent session fact. The platform crate detects sessions via
/// /proc, but core must not depend on it, so the input shape lives here and
/// the caller maps into it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFact {
    /// Matches `Repo::id` (absolute path) — agent sessions are a local fact.
    pub repo_id: String,
    /// Display label for the agent program (e.g. "claude").
    pub program: String,
    /// True while the session is alive; false for a session the caller
    /// observed finishing since its last poll.
    pub running: bool,
}

/// Score everything into one prioritized list. Pure: same inputs, same
/// output. Sorted by severity (`Urgent` first), then kind (declaration
/// order), then repo name (case-insensitive), then summary — a total, stable
/// order so surfaces render identically for identical fleets.
pub fn compute(
    repos: &[Repo],
    inbox: &[InboxItem],
    ci: &[CiFact],
    prunable: &[PrunableFact],
    agents: &[AgentFact],
) -> Vec<AttentionItem> {
    let mut items: Vec<AttentionItem> = Vec::new();

    // Local git state, straight off the scan snapshot.
    for repo in repos {
        let on_branch = || Some(format!("on {}", repo.git.branch));
        if repo.git.dirty > 0 {
            items.push(item(
                local_ref(repo),
                AttentionKind::DirtyWorktree,
                count(repo.git.dirty, "uncommitted change", "uncommitted changes"),
                on_branch(),
            ));
        }
        if repo.git.ahead > 0 {
            items.push(item(
                local_ref(repo),
                AttentionKind::Ahead,
                format!("{} not pushed", count(repo.git.ahead, "commit", "commits")),
                on_branch(),
            ));
        }
        if repo.git.behind > 0 {
            items.push(item(
                local_ref(repo),
                AttentionKind::Behind,
                format!(
                    "{} behind upstream",
                    count(repo.git.behind, "commit", "commits")
                ),
                on_branch(),
            ));
        }
    }

    // Host inbox: review requests + your open PRs. Assigned issues aren't an
    // attention type (same rule as the platform notifier); drafts can't be
    // acted on (no merge, review not yet requested "for real"), so a draft
    // never raises a review item.
    for it in inbox {
        match it.kind.as_str() {
            "review" if !it.draft => items.push(item(
                host_ref(repos, it.host, &it.repo),
                AttentionKind::ReviewRequested,
                format!("Review requested: {} (#{})", it.title, it.number),
                Some(it.url.clone()),
            )),
            "pr" => items.push(item(
                host_ref(repos, it.host, &it.repo),
                AttentionKind::PrAssigned,
                format!("Open PR #{}: {}", it.number, it.title),
                Some(it.url.clone()),
            )),
            _ => {}
        }
    }

    // CI: only a definitive failure raises attention — "pending" and "none"
    // are ambient, and "success" is the goal state.
    for c in ci {
        if c.state == "failure" {
            items.push(item(
                remote_ref(repos, &c.remote_host, &c.slug),
                AttentionKind::CiFailing,
                "CI failing on the default branch".to_string(),
                None,
            ));
        }
    }

    for p in prunable {
        if p.count > 0 {
            items.push(item(
                id_ref(repos, &p.repo_id),
                AttentionKind::PrunableBranches,
                count(p.count, "prunable branch", "prunable branches"),
                None,
            ));
        }
    }

    for a in agents {
        let (kind, verb) = if a.running {
            (AttentionKind::AgentRunning, "running")
        } else {
            (AttentionKind::AgentFinished, "finished")
        };
        items.push(item(
            id_ref(repos, &a.repo_id),
            kind,
            format!("Agent {verb}: {}", a.program),
            None,
        ));
    }

    items.sort_by_cached_key(|i| {
        (
            i.severity,
            i.kind,
            i.repo.name.to_lowercase(),
            i.summary.clone(),
        )
    });
    items
}

fn item(
    repo: RepoRef,
    kind: AttentionKind,
    summary: String,
    detail: Option<String>,
) -> AttentionItem {
    AttentionItem {
        repo,
        kind,
        severity: kind.severity(),
        summary,
        detail,
    }
}

/// "1 uncommitted change" / "3 uncommitted changes" — the plural form is
/// explicit so irregular nouns ("branches") read right.
fn count(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn local_ref(repo: &Repo) -> RepoRef {
    RepoRef {
        id: Some(repo.id.clone()),
        remote_host: repo.remote_host.clone(),
        slug: repo.slug.clone(),
        name: repo.display_name.clone(),
    }
}

/// Link a (host, slug) inbox fact to the local fleet. The inbox only knows
/// the host *kind* (GitHub/GitLab), not the domain, so this is the finest
/// key it can offer — still host-qualified, so "o/r" on GitHub never links
/// to a local "o/r" cloned from GitLab.
fn host_ref(repos: &[Repo], host: Host, slug: &str) -> RepoRef {
    repos
        .iter()
        .find(|r| r.host == Some(host) && r.slug.as_deref() == Some(slug))
        .map(local_ref)
        .unwrap_or_else(|| RepoRef {
            id: None,
            remote_host: None,
            slug: Some(slug.to_string()),
            name: slug.to_string(),
        })
}

/// Link a (remote_host domain, slug) fact — the full enrichment-cache key —
/// to the local fleet.
fn remote_ref(repos: &[Repo], remote_host: &str, slug: &str) -> RepoRef {
    repos
        .iter()
        .find(|r| r.remote_host.as_deref() == Some(remote_host) && r.slug.as_deref() == Some(slug))
        .map(local_ref)
        .unwrap_or_else(|| RepoRef {
            id: None,
            remote_host: Some(remote_host.to_string()),
            slug: Some(slug.to_string()),
            name: slug.to_string(),
        })
}

/// Link a local-repo-id fact to the fleet; falls back to the path basename
/// as the display name if the id isn't in the snapshot (e.g. mid-rescan).
fn id_ref(repos: &[Repo], repo_id: &str) -> RepoRef {
    repos
        .iter()
        .find(|r| r.id == repo_id)
        .map(local_ref)
        .unwrap_or_else(|| RepoRef {
            id: Some(repo_id.to_string()),
            remote_host: None,
            slug: None,
            name: repo_id
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(repo_id)
                .to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};

    fn repo(id: &str) -> Repo {
        Repo {
            id: id.to_string(),
            display_name: "Test".into(),
            slug: Some("o/test".into()),
            path: "~/dev/test".into(),
            description: None,
            language: Some("Rust".into()),
            git: GitStatus {
                branch: "main".into(),
                ahead: 0,
                behind: 0,
                dirty: 0,
            },
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: Some(Host::Github),
            remote_host: Some("github.com".into()),
            stars: 0,
            topics: Vec::new(),
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
        }
    }

    fn inbox_item(kind: &str, slug: &str, draft: bool) -> InboxItem {
        InboxItem {
            kind: kind.to_string(),
            title: "Fix the thing".into(),
            repo: slug.to_string(),
            url: format!("https://github.com/{slug}/pull/7"),
            number: 7,
            draft,
            host: Host::Github,
        }
    }

    fn compute_repos(repos: &[Repo]) -> Vec<AttentionItem> {
        compute(repos, &[], &[], &[], &[])
    }

    #[test]
    fn quiet_fleet_produces_empty() {
        // Clean repos, empty inbox, green CI, no prunables, no agents.
        let repos = vec![repo("/a"), repo("/b")];
        let ci = vec![CiFact {
            remote_host: "github.com".into(),
            slug: "o/test".into(),
            state: "success".into(),
        }];
        assert!(compute(&repos, &[], &ci, &[], &[]).is_empty());
    }

    #[test]
    fn dirty_worktree_triggers_attention() {
        let mut r = repo("/a");
        r.git.dirty = 3;
        let items = compute_repos(&[r]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::DirtyWorktree);
        assert_eq!(items[0].severity, Severity::Attention);
        assert_eq!(items[0].summary, "3 uncommitted changes");
        assert_eq!(items[0].detail.as_deref(), Some("on main"));
        assert_eq!(items[0].repo.id.as_deref(), Some("/a"));
    }

    #[test]
    fn ahead_triggers_attention() {
        let mut r = repo("/a");
        r.git.ahead = 1;
        let items = compute_repos(&[r]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Ahead);
        assert_eq!(items[0].severity, Severity::Attention);
        assert_eq!(items[0].summary, "1 commit not pushed");
    }

    #[test]
    fn behind_triggers_info() {
        let mut r = repo("/a");
        r.git.behind = 2;
        let items = compute_repos(&[r]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Behind);
        assert_eq!(items[0].severity, Severity::Info);
        assert_eq!(items[0].summary, "2 commits behind upstream");
    }

    #[test]
    fn ci_failure_triggers_urgent_but_other_states_do_not() {
        let repos = vec![repo("/a")];
        let fact = |state: &str| CiFact {
            remote_host: "github.com".into(),
            slug: "o/test".into(),
            state: state.into(),
        };
        for quiet in ["success", "pending", "none"] {
            assert!(
                compute(&repos, &[], &[fact(quiet)], &[], &[]).is_empty(),
                "{quiet} must not raise attention"
            );
        }
        let items = compute(&repos, &[], &[fact("failure")], &[], &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::CiFailing);
        assert_eq!(items[0].severity, Severity::Urgent);
        // Linked to the local repo via the (host, slug) compound key.
        assert_eq!(items[0].repo.id.as_deref(), Some("/a"));
    }

    #[test]
    fn ci_failure_keys_by_host_and_slug() {
        // Same slug, different host domain → must not link to the local repo
        // (the #159 enrichment-cache lesson).
        let repos = vec![repo("/a")]; // github.com / o/test
        let items = compute(
            &repos,
            &[],
            &[CiFact {
                remote_host: "gitlab.acme.io".into(),
                slug: "o/test".into(),
                state: "failure".into(),
            }],
            &[],
            &[],
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].repo.id, None, "must not cross-link across hosts");
        assert_eq!(items[0].repo.remote_host.as_deref(), Some("gitlab.acme.io"));
    }

    #[test]
    fn review_request_triggers_urgent_and_drafts_are_skipped() {
        let items = compute(&[], &[inbox_item("review", "o/test", false)], &[], &[], &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::ReviewRequested);
        assert_eq!(items[0].severity, Severity::Urgent);
        assert_eq!(items[0].summary, "Review requested: Fix the thing (#7)");
        assert_eq!(
            items[0].detail.as_deref(),
            Some("https://github.com/o/test/pull/7")
        );

        // A draft PR can't be merged; review isn't actionable yet.
        let drafts = compute(&[], &[inbox_item("review", "o/test", true)], &[], &[], &[]);
        assert!(drafts.is_empty());
    }

    #[test]
    fn own_pr_triggers_info_and_issues_are_ignored() {
        let items = compute(
            &[],
            &[
                inbox_item("pr", "o/test", false),
                inbox_item("issue", "o/test", false),
            ],
            &[],
            &[],
            &[],
        );
        assert_eq!(items.len(), 1, "assigned issues aren't an attention type");
        assert_eq!(items[0].kind, AttentionKind::PrAssigned);
        assert_eq!(items[0].severity, Severity::Info);
        assert_eq!(items[0].summary, "Open PR #7: Fix the thing");
    }

    #[test]
    fn inbox_links_to_local_repo_by_host_and_slug() {
        // A GitHub inbox item matches the GitHub clone, not a GitLab repo
        // that happens to share the slug.
        let mut gitlab_twin = repo("/b");
        gitlab_twin.host = Some(Host::Gitlab);
        gitlab_twin.remote_host = Some("gitlab.com".into());
        let repos = vec![gitlab_twin, repo("/a")];
        let items = compute(
            &repos,
            &[inbox_item("review", "o/test", false)],
            &[],
            &[],
            &[],
        );
        assert_eq!(items[0].repo.id.as_deref(), Some("/a"));
        assert_eq!(items[0].repo.name, "Test");

        // No local match → slug-only ref, still renderable.
        let items = compute(&[], &[inbox_item("review", "o/x", false)], &[], &[], &[]);
        assert_eq!(items[0].repo.id, None);
        assert_eq!(items[0].repo.name, "o/x");
    }

    #[test]
    fn prunable_branches_trigger_info_only_when_nonzero() {
        let repos = vec![repo("/a")];
        let none = PrunableFact {
            repo_id: "/a".into(),
            count: 0,
        };
        assert!(compute(&repos, &[], &[], &[none], &[]).is_empty());

        let some = PrunableFact {
            repo_id: "/a".into(),
            count: 2,
        };
        let items = compute(&repos, &[], &[], &[some], &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::PrunableBranches);
        assert_eq!(items[0].severity, Severity::Info);
        assert_eq!(items[0].summary, "2 prunable branches");
    }

    #[test]
    fn agent_running_is_info_and_finished_is_attention() {
        let repos = vec![repo("/a")];
        let fact = |running: bool| AgentFact {
            repo_id: "/a".into(),
            program: "claude".into(),
            running,
        };
        let items = compute(&repos, &[], &[], &[], &[fact(true), fact(false)]);
        assert_eq!(items.len(), 2);
        // Finished sorts first (Attention < Info).
        assert_eq!(items[0].kind, AttentionKind::AgentFinished);
        assert_eq!(items[0].severity, Severity::Attention);
        assert_eq!(items[0].summary, "Agent finished: claude");
        assert_eq!(items[1].kind, AttentionKind::AgentRunning);
        assert_eq!(items[1].severity, Severity::Info);
        assert_eq!(items[1].summary, "Agent running: claude");
    }

    #[test]
    fn unknown_agent_repo_falls_back_to_path_basename() {
        let items = compute(
            &[],
            &[],
            &[],
            &[],
            &[AgentFact {
                repo_id: "/home/dev/mystery".into(),
                program: "claude".into(),
                running: true,
            }],
        );
        assert_eq!(items[0].repo.name, "mystery");
        assert_eq!(items[0].repo.id.as_deref(), Some("/home/dev/mystery"));
    }

    #[test]
    fn every_kind_maps_to_its_documented_severity() {
        use AttentionKind::*;
        for (kind, severity) in [
            (CiFailing, Severity::Urgent),
            (ReviewRequested, Severity::Urgent),
            (AgentFinished, Severity::Attention),
            (DirtyWorktree, Severity::Attention),
            (Ahead, Severity::Attention),
            (Behind, Severity::Info),
            (PrAssigned, Severity::Info),
            (PrunableBranches, Severity::Info),
            (AgentRunning, Severity::Info),
        ] {
            assert_eq!(kind.severity(), severity, "{kind:?}");
        }
    }

    #[test]
    fn output_sorts_by_severity_then_kind_then_repo_name() {
        // Repo "b" is only dirty (Attention); repo "a" is behind (Info) and has
        // failing CI (Urgent). Expect: Urgent(a) → Attention(b) → Info(a).
        let mut a = repo("/a");
        a.display_name = "alpha".into();
        a.slug = Some("o/alpha".into());
        a.git.behind = 1;
        let mut b = repo("/b");
        b.display_name = "beta".into();
        b.slug = Some("o/beta".into());
        b.git.dirty = 1;
        let ci = vec![CiFact {
            remote_host: "github.com".into(),
            slug: "o/alpha".into(),
            state: "failure".into(),
        }];
        let items = compute(&[a, b], &[], &ci, &[], &[]);
        let got: Vec<(AttentionKind, &str)> = items
            .iter()
            .map(|i| (i.kind, i.repo.name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (AttentionKind::CiFailing, "alpha"),
                (AttentionKind::DirtyWorktree, "beta"),
                (AttentionKind::Behind, "alpha"),
            ]
        );

        // Same severity + kind → repo name breaks the tie, case-insensitively.
        let mut x = repo("/x");
        x.display_name = "Zed".into();
        x.git.dirty = 1;
        let mut y = repo("/y");
        y.display_name = "apricot".into();
        y.git.dirty = 1;
        let items = compute_repos(&[x, y]);
        let names: Vec<&str> = items.iter().map(|i| i.repo.name.as_str()).collect();
        assert_eq!(names, vec!["apricot", "Zed"]);
    }
}
