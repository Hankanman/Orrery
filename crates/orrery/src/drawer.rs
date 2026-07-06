//! RepoDrawer — the right-anchored detail panel. Opens
//! over the shell when a card is clicked; a scrim backdrop or the close button
//! dismisses it. Tabs: Overview / Changes / PR / Notes / Readme.
//!
//! This is the workhorse primitive — most journeys (catch-up, dive, commit, PR
//! triage) live here. All five tabs are in: Overview (Row facts + async
//! branches/commits/worktrees, with worktree add/remove), Readme (gpui-component
//! markdown), PR (lazy, GitHub-only, via the `task` bridge — rollups, per-check
//! breakdown + inline approve/merge), Changes (Unstaged/Staged file lists with
//! per-file stage/unstage, a per-file diff pane, and a commit composer acting
//! on the index, plus AI commit-message + changelog generation gated on
//! aiReady), and Notes (catch-up + AI "what changed" narrative + an editable
//! markdown note via gpui-component's multiline input).

use gpui::{
    AppContext, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity, div, px, rgb,
    rgba,
};
use orrery_core::{cache, git_ops, inbox, launch};

use crate::data::{self, Row};
use crate::icon::{brand, lucide};
use crate::shell::{DrawerTab, OrreryApp, Overlay};
use crate::theme::Theme;
use crate::toast::ToastKind;

const MONO: &str = "monospace";
const PANEL_W: f32 = 560.;
/// Recent commits shown in the Overview.
const LOG_LIMIT: usize = 8;

const TABS: [(DrawerTab, &str); 5] = [
    (DrawerTab::Overview, "Overview"),
    (DrawerTab::Changes, "Changes"),
    (DrawerTab::Pr, "PR"),
    (DrawerTab::Notes, "Notes"),
    (DrawerTab::Readme, "Readme"),
];

// ── async per-repo data ────────────────────────────────────────────────────
// The drawer's git data is loaded off the UI thread and marshalled back onto the
// foreground (the live-wiring pattern). `None` = still loading; `Some(vec)` =
// loaded (possibly empty). `repo` guards against a result landing after the
// drawer moved to a different repo or closed.

pub struct BranchRow {
    pub name: SharedString,
    pub current: bool,
    pub gone: bool,
    pub merged: bool,
}

pub struct CommitRow {
    pub summary: SharedString,
    pub author: SharedString,
    pub age: SharedString,
}

pub struct WorktreeRow {
    pub name: SharedString,
    pub path: SharedString,
}

/// Lazy-loaded state for a drawer tab whose data is fetched on first view.
#[derive(Default, PartialEq)]
pub enum ReadmeState {
    /// Not requested yet.
    #[default]
    Idle,
    Loading,
    /// Loaded — `None` means the repo has no README file.
    Ready(Option<SharedString>),
}

/// A render-ready open pull request (flattened from `inbox::PrDetail`).
pub struct PrRow {
    pub number: u64,
    pub title: SharedString,
    pub url: SharedString,
    pub draft: bool,
    pub author: SharedString,
    pub refs: SharedString,         // "head → base"
    pub mergeable: SharedString,    // clean | conflicting | unknown
    pub review: SharedString,       // approved | changes_requested | review_required | none
    pub checks: SharedString,       // success | failure | pending | none (rollup)
    pub check_runs: Vec<CheckItem>, // individual status checks / CI contexts
}

/// One status check on a PR, for the per-check breakdown under the rollup.
pub struct CheckItem {
    pub name: SharedString,
    pub state: SharedString, // success | failure | pending | neutral
    pub url: Option<SharedString>,
}

/// Lazy-loaded PR panel state (network).
#[derive(Default)]
pub enum PrState {
    #[default]
    Idle,
    Loading,
    Ready {
        methods: Vec<SharedString>,
        prs: Vec<PrRow>,
    },
    /// Not applicable (non-GitHub) or the fetch failed.
    Error(SharedString),
}

/// Lazy-loaded diff state (sync git, but loaded off the UI thread).
#[derive(Default)]
pub enum DiffState {
    #[default]
    Idle,
    Loading,
    /// The diff text ("" when the selected file has no diff on its side).
    Ready(SharedString),
}

/// One pending change in the Changes tab's Unstaged/Staged lists (render-ready
/// [`git_ops::FileChange`]). A path staged and then further edited appears in
/// both lists.
pub struct ChangeRow {
    pub path: SharedString,
    pub kind: git_ops::ChangeKind,
    pub staged: bool,
}

/// Notes tab data: the "resume where I left off" catch-up line (the note text
/// itself lives in `DrawerData::note_input`).
pub struct NotesData {
    pub catchup: SharedString,
    /// Commits since last seen — drives whether "Mark caught up" is offered.
    pub count: usize,
    pub first_visit: bool,
    /// AI "what changed" narrative, once generated (gated on `aiReady`).
    pub resume: Option<SharedString>,
}

/// Async-loaded data for the currently open repo's drawer. Overview loads on
/// open; Readme/PR/Changes/Notes load lazily when their tabs are first shown.
#[derive(Default)]
pub struct DrawerData {
    pub repo: SharedString,
    pub branches: Option<Vec<BranchRow>>,
    pub commits: Option<Vec<CommitRow>>,
    pub worktrees: Option<Vec<WorktreeRow>>,
    pub readme: ReadmeState,
    pub pr: PrState,
    /// The selected file's diff (per-file, capped render — see [`diff_block`]).
    pub diff: DiffState,
    /// The Changes tab's file lists; `None` until the tab first loads them.
    pub changes: Option<Vec<ChangeRow>>,
    /// The file whose diff the pane shows, as (path, staged-side).
    pub change_sel: Option<(SharedString, bool)>,
    /// AI-generated commit message for the staged diff, once requested.
    pub commit_suggestion: Option<SharedString>,
    /// AI-generated changelog from recent commits, once requested.
    pub changelog: Option<SharedString>,
    /// Notes tab (catch-up + saved note); `None` until first shown.
    pub notes: Option<NotesData>,
    /// The editable note field (gpui-component multiline input), seeded from the
    /// saved note when the Notes tab first opens.
    pub note_input: Option<Entity<gpui_component::input::InputState>>,
    /// The commit-message field for the Changes tab, created when it first opens.
    pub commit_input: Option<Entity<gpui_component::input::InputState>>,
    /// The new-worktree name field (Overview), created when the drawer opens.
    pub worktree_input: Option<Entity<gpui_component::input::InputState>>,
    /// The repo's default branch (origin/HEAD else main/master), loaded with
    /// the Overview — the base for "Open PR" and its visibility check.
    pub default_branch: Option<SharedString>,
    /// A commit landed in this drawer session — keeps Push offered even while
    /// the grid row's ahead count is stale (no upstream / pre-rescan).
    pub committed: bool,
    /// A push kicked off from this drawer is still running.
    pub push_busy: bool,
    /// An "Open PR" flow kicked off from this drawer is still running.
    pub pr_busy: bool,
    /// The PR created from this drawer, for the "View PR" affordance.
    pub pr_url: Option<SharedString>,
    /// The agent-dispatch task field (Overview), created when the drawer opens.
    pub dispatch_input: Option<Entity<gpui_component::input::InputState>>,
    /// Whether "Dispatch" runs the agent on a fresh worktree (#185).
    pub dispatch_fresh: bool,
}

impl DrawerData {
    /// Fresh, all-loading state for a newly opened repo.
    pub fn loading(repo: SharedString) -> Self {
        DrawerData {
            repo,
            ..Default::default()
        }
    }
}

type Loaded = (
    Vec<git_ops::BranchInfo>,
    Vec<git_ops::CommitInfo>,
    Vec<git_ops::WorktreeInfo>,
    Option<String>,
);

/// Read branches + recent log + worktrees + the default branch for `id` (all
/// git-heavy — runs on the background pool).
fn read_overview(id: &str) -> Loaded {
    (
        git_ops::branches(id).unwrap_or_default(),
        git_ops::recent_log(id, LOG_LIMIT).unwrap_or_default(),
        git_ops::worktrees(id).unwrap_or_default(),
        git_ops::default_branch(id),
    )
}

/// Apply a finished Overview load to the app, but only if the drawer still shows
/// the same repo (else the user moved on and this is stale).
fn store_overview(
    this: &WeakEntity<OrreryApp>,
    cx: &mut AsyncApp,
    repo: &SharedString,
    loaded: Loaded,
    now: i64,
) {
    let (branches, commits, worktrees, default_branch) = loaded;
    let _ = this.update(cx, |this, cx| {
        if &this.drawer.repo != repo {
            return;
        }
        this.drawer.branches = Some(branches.into_iter().map(branch_row).collect());
        this.drawer.commits = Some(commits.into_iter().map(|c| commit_row(c, now)).collect());
        this.drawer.worktrees = Some(worktrees.into_iter().map(worktree_row).collect());
        this.drawer.default_branch = default_branch.map(Into::into);
        cx.notify();
    });
}

/// Kick off the Overview load for `repo` (branches/commits/worktrees).
pub fn load_overview(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move { read_overview(&id) })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

/// Switch `repo` to `name`, then refresh the Overview. Spawn-only (the caller,
/// already holding `&mut OrreryApp`, sets the loading state). The `.git/HEAD`
/// change also trips the filesystem watcher, so the card row refreshes on its own.
pub fn switch_branch(repo: SharedString, name: SharedString, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let (id, branch) = (repo.to_string(), name.to_string());
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move {
                let _ = git_ops::switch_branch(&id, &branch);
                read_overview(&id)
            })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

/// Lazily load the repo's README (filesystem, sync) when the Readme tab opens.
pub fn load_readme(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let content = cx
            .background_executor()
            .spawn(async move { read_readme(&id) })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo == repo {
                this.drawer.readme = ReadmeState::Ready(content.map(SharedString::from));
                cx.notify();
            }
        });
    })
    .detach();
}

fn change_row(c: git_ops::FileChange) -> ChangeRow {
    ChangeRow {
        path: c.path.into(),
        kind: c.kind,
        staged: c.staged,
    }
}

/// Lazily load the Changes tab's file lists (sync git, off the UI thread).
/// The store step auto-selects a file and loads its diff.
pub fn load_changes(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let list = cx
            .background_executor()
            .spawn(async move { git_ops::changes(&id).unwrap_or_default() })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo == repo {
                store_changes(this, list, cx);
            }
        });
    })
    .detach();
}

/// Apply freshly read file lists: keep the selection if its entry survived
/// (same path on the other side counts — the file just moved lists), else fall
/// back to the first entry, then (re)load the selected file's diff. Runs after
/// every load / stage / unstage / commit, so the pane always tracks the index.
fn store_changes(
    this: &mut OrreryApp,
    list: Vec<git_ops::FileChange>,
    cx: &mut Context<OrreryApp>,
) {
    let list: Vec<ChangeRow> = list.into_iter().map(change_row).collect();
    let sel = this
        .drawer
        .change_sel
        .take()
        .and_then(|(p, s)| {
            if list.iter().any(|c| c.path == p && c.staged == s) {
                Some((p, s))
            } else {
                list.iter()
                    .find(|c| c.path == p)
                    .map(|c| (c.path.clone(), c.staged))
            }
        })
        .or_else(|| list.first().map(|c| (c.path.clone(), c.staged)));
    this.drawer.changes = Some(list);
    this.drawer.change_sel = sel.clone();
    match sel {
        Some((path, staged)) => {
            this.drawer.diff = DiffState::Loading;
            load_file_diff(this.drawer.repo.clone(), path, staged, cx);
        }
        None => this.drawer.diff = DiffState::Ready("".into()),
    }
    cx.notify();
}

/// Show `path`'s diff (from the staged or unstaged side) in the pane.
fn select_change(
    this: &mut OrreryApp,
    path: SharedString,
    staged: bool,
    cx: &mut Context<OrreryApp>,
) {
    if this
        .drawer
        .change_sel
        .as_ref()
        .is_some_and(|(p, s)| *p == path && *s == staged)
    {
        return;
    }
    this.drawer.change_sel = Some((path.clone(), staged));
    this.drawer.diff = DiffState::Loading;
    load_file_diff(this.drawer.repo.clone(), path, staged, cx);
    cx.notify();
}

/// Load one file's diff (sync git, off the UI thread). The result is dropped
/// if the drawer moved on or the selection changed while it was in flight.
fn load_file_diff(
    repo: SharedString,
    path: SharedString,
    staged: bool,
    cx: &mut Context<OrreryApp>,
) {
    let (id, file) = (repo.to_string(), path.to_string());
    cx.spawn(async move |this, cx| {
        let diff = cx
            .background_executor()
            .spawn(async move { git_ops::file_diff(&id, &file, staged).unwrap_or_default() })
            .await;
        let _ = this.update(cx, |this, cx| {
            let still_selected = this
                .drawer
                .change_sel
                .as_ref()
                .is_some_and(|(p, s)| *p == path && *s == staged);
            if this.drawer.repo == repo && still_selected {
                this.drawer.diff = DiffState::Ready(diff.into());
                cx.notify();
            }
        });
    })
    .detach();
}

/// Stage (or unstage) `paths`, then re-read the file lists + selected diff.
/// Failures surface as an error toast rather than vanishing silently.
fn set_staged(repo: SharedString, paths: Vec<String>, stage: bool, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let (result, list) = cx
            .background_executor()
            .spawn(async move {
                let result = if stage {
                    git_ops::stage_paths(&id, &paths)
                } else {
                    git_ops::unstage_paths(&id, &paths)
                };
                (result, git_ops::changes(&id).unwrap_or_default())
            })
            .await;
        let _ = this.update(cx, |this, cx| {
            if let Err(e) = result {
                let title = if stage {
                    "Stage failed"
                } else {
                    "Unstage failed"
                };
                this.push_toast(ToastKind::Error, title, Some(e.into()), cx);
            }
            if this.drawer.repo == repo {
                store_changes(this, list, cx);
            }
        });
    })
    .detach();
}

/// Commit the index as-is with `message`, toast the outcome, then refresh the
/// file lists + diff. The commit also trips the watcher, refreshing the card.
fn commit_staged(repo: SharedString, message: String, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    // The commit subject, for the success toast's detail line.
    let subject = message.lines().next().unwrap_or("").trim().to_string();
    cx.spawn(async move |this, cx| {
        let (result, list) = cx
            .background_executor()
            .spawn(async move {
                let result = git_ops::commit(&id, message.trim());
                (result, git_ops::changes(&id).unwrap_or_default())
            })
            .await;
        let _ = this.update(cx, |this, cx| {
            // The toast is global feedback — push it even if the drawer moved on.
            match &result {
                Ok(hash) => {
                    this.push_toast(
                        ToastKind::Success,
                        format!("Committed {hash}"),
                        Some(subject.clone().into()),
                        cx,
                    );
                }
                Err(e) => {
                    this.push_toast(
                        ToastKind::Error,
                        "Commit failed",
                        Some(e.clone().into()),
                        cx,
                    );
                }
            }
            if this.drawer.repo != repo {
                return;
            }
            if result.is_ok() {
                this.drawer.commit_suggestion = None;
                // Offer Push even while the grid row's ahead count is stale.
                this.drawer.committed = true;
            }
            store_changes(this, list, cx);
        });
    })
    .detach();
}

/// Read the "resume where I left off" catch-up for a repo (sync). The note text
/// itself is seeded straight into the editable field at tab-open.
fn read_notes(id: &str) -> NotesData {
    let (catchup, count, first_visit) = match cache::seen_sha(id) {
        None => (
            "First visit — nothing to catch up on yet.".to_string(),
            0,
            true,
        ),
        Some(since) => {
            let n = git_ops::log_since_sha(id, &since, 50)
                .map(|c| c.len())
                .unwrap_or(0);
            let msg = match n {
                0 => "All caught up since you last looked.".to_string(),
                1 => "1 commit since you last looked.".to_string(),
                n => format!("{n} commits since you last looked."),
            };
            (msg, n, false)
        }
    };
    NotesData {
        catchup: catchup.into(),
        count,
        first_visit,
        resume: None,
    }
}

/// Lazily load the Notes tab data when it first opens.
pub fn load_notes(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let notes = cx
            .background_executor()
            .spawn(async move { read_notes(&id) })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo == repo {
                this.drawer.notes = Some(notes);
                cx.notify();
            }
        });
    })
    .detach();
}

/// Persist the edited note (off the UI thread).
fn save_note(repo: SharedString, text: String, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |_this, cx| {
        cx.background_executor()
            .spawn(async move {
                let _ = cache::set_note(&id, &text);
            })
            .await;
    })
    .detach();
}

/// Record the current HEAD as "seen", then refresh the catch-up.
fn mark_seen(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let notes = cx
            .background_executor()
            .spawn(async move {
                if let Ok(sha) = git_ops::head_sha(&id) {
                    let _ = cache::set_seen(&id, &sha, now);
                }
                read_notes(&id)
            })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo == repo {
                this.drawer.notes = Some(notes);
                cx.notify();
            }
        });
    })
    .detach();
}

/// Create a worktree (+ branch) `name` in a sibling dir, then refresh Overview.
fn add_worktree(repo: SharedString, name: String, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let id = repo.to_string();
    let dest = format!("{id}-{name}");
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move {
                let _ = git_ops::add_worktree(&id, &name, &dest);
                read_overview(&id)
            })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

/// Unlink worktree `name` (leaves files on disk), then refresh Overview.
fn remove_worktree(repo: SharedString, name: String, cx: &mut Context<OrreryApp>) {
    let now = data::now_unix();
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move {
                let _ = git_ops::remove_worktree(&id, &name);
                read_overview(&id)
            })
            .await;
        store_overview(&this, cx, &repo, loaded, now);
    })
    .detach();
}

/// Read the first matching README from the repo root (mirrors the Tauri
/// `repo_readme` command).
fn read_readme(id: &str) -> Option<String> {
    const NAMES: [&str; 5] = [
        "README.md",
        "Readme.md",
        "readme.md",
        "README.markdown",
        "README",
    ];
    NAMES
        .iter()
        .find_map(|name| std::fs::read_to_string(std::path::Path::new(id).join(name)).ok())
}

fn branch_row(b: git_ops::BranchInfo) -> BranchRow {
    BranchRow {
        name: b.name.into(),
        current: b.is_head,
        gone: b.gone,
        merged: b.merged,
    }
}

fn commit_row(c: git_ops::CommitInfo, now: i64) -> CommitRow {
    CommitRow {
        summary: c.summary.into(),
        author: c.author.into(),
        age: data::rel_age(c.time_unix, now).into(),
    }
}

fn worktree_row(w: git_ops::WorktreeInfo) -> WorktreeRow {
    WorktreeRow {
        name: w.name.into(),
        path: w.path.into(),
    }
}

fn pr_row(p: inbox::PrDetail) -> PrRow {
    PrRow {
        number: p.number,
        title: data::oneline(p.title).into(),
        url: p.url.into(),
        draft: p.draft,
        author: p.author.unwrap_or_default().into(),
        refs: format!("{} → {}", p.head, p.base).into(),
        mergeable: p.mergeable.into(),
        review: p.review_decision.into(),
        checks: p.checks_state.into(),
        check_runs: p
            .checks
            .into_iter()
            .map(|c| CheckItem {
                name: c.name.into(),
                state: c.state.into(),
                url: c.url.map(Into::into),
            })
            .collect(),
    }
}

fn ready_pr(panel: inbox::PrPanel) -> PrState {
    PrState::Ready {
        methods: panel.merge_methods.into_iter().map(Into::into).collect(),
        prs: panel.prs.into_iter().map(pr_row).collect(),
    }
}

/// Lazily load the GitHub PR panel for `repo` (slug = owner/name). Network — runs
/// on the shared tokio runtime via [`crate::task`].
pub fn load_pr(repo: SharedString, slug: SharedString, cx: &mut Context<OrreryApp>) {
    let s = slug.to_string();
    cx.spawn(async move |this, cx| {
        let result = crate::task::run(async move { inbox::github_prs(&s).await }).await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo != repo {
                return;
            }
            this.drawer.pr = result
                .map(ready_pr)
                .unwrap_or_else(|e| PrState::Error(e.into()));
            cx.notify();
        });
    })
    .detach();
}

/// Merge PR `number` via `method`, then refresh the panel. Caller sets the
/// loading state.
fn merge_pr(
    repo: SharedString,
    slug: SharedString,
    number: u64,
    method: String,
    cx: &mut Context<OrreryApp>,
) {
    let (do_slug, re_slug) = (slug.to_string(), slug.to_string());
    cx.spawn(async move |this, cx| {
        let _ = crate::task::run(
            async move { inbox::github_merge_pr(&do_slug, number, &method).await },
        )
        .await;
        let panel = crate::task::run(async move { inbox::github_prs(&re_slug).await }).await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo != repo {
                return;
            }
            this.drawer.pr = panel
                .map(ready_pr)
                .unwrap_or_else(|e| PrState::Error(e.into()));
            cx.notify();
        });
    })
    .detach();
}

/// Approve PR `number`, then refresh the panel. Caller sets the loading state.
fn approve_pr(repo: SharedString, slug: SharedString, number: u64, cx: &mut Context<OrreryApp>) {
    let (do_slug, re_slug) = (slug.to_string(), slug.to_string());
    cx.spawn(async move |this, cx| {
        let _ =
            crate::task::run(async move { inbox::github_approve_pr(&do_slug, number).await }).await;
        let panel = crate::task::run(async move { inbox::github_prs(&re_slug).await }).await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo != repo {
                return;
            }
            this.drawer.pr = panel
                .map(ready_pr)
                .unwrap_or_else(|e| PrState::Error(e.into()));
            cx.notify();
        });
    })
    .detach();
}

#[allow(clippy::too_many_arguments)]
pub fn drawer(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    app: &Entity<OrreryApp>,
    data: &DrawerData,
    ide_cmd: &str,
    agent_cmd: &str,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    // Scrim: click anywhere outside the panel to dismiss.
    let backdrop = {
        let app = app.clone();
        div()
            .id("drawer-backdrop")
            .flex_1()
            .h_full()
            .bg(rgba(0x00000066))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let panel = div()
        .flex()
        .flex_col()
        .w(px(PANEL_W))
        .h_full()
        .bg(rgb(t.page))
        .border_l_1()
        .border_color(rgb(t.border))
        .child(header(row, t, app))
        .child(tab_bar(tab, t, app, data.repo.clone(), github_slug(row)))
        .child(body(row, tab, t, data, app, ai_ready, github_authed))
        .child(footer(row, t, ide_cmd, agent_cmd));

    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .flex()
        .flex_row()
        // Modal: block all mouse interaction with the grid behind, so clicks on
        // the panel don't also fall through to a card.
        .occlude()
        .child(backdrop)
        .child(panel)
}

fn header(row: &Row, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let close = {
        let app = app.clone();
        div()
            .id("drawer-close")
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(30.))
            .rounded(px(t.r_sm))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide("x", 17., t.fg1))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let mut title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .text_size(px(t.text_h3))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.fg0))
        .child(div().min_w(px(0.)).truncate().child(row.name.clone()));
    if !row.host.is_empty() {
        title = title.child(brand(&row.host, 15., t.fg2));
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .px(px(18.))
        .py(px(15.))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(4.))
                .child(title)
                .child(
                    div()
                        .truncate()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(SharedString::from(format!("{} · {}", row.slug, row.path))),
                ),
        )
        .child(close)
}

/// The owner/name slug if this repo is a usable GitHub remote, else `None`.
fn github_slug(row: &Row) -> Option<SharedString> {
    (row.host.as_ref() == "github" && row.slug.as_ref() != "no remote").then(|| row.slug.clone())
}

fn tab_bar(
    active: DrawerTab,
    t: &Theme,
    app: &Entity<OrreryApp>,
    repo: SharedString,
    pr_slug: Option<SharedString>,
) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .gap(px(2.))
        .px(px(12.))
        .border_b_1()
        .border_color(rgb(t.border));

    for (tab, label) in TABS {
        let is_active = tab == active;
        let fg = if is_active { t.fg0 } else { t.fg2 };
        let app = app.clone();
        let repo = repo.clone();
        let pr_slug = pr_slug.clone();
        // 1px underline on the active tab; page-coloured (invisible) otherwise so
        // the row height stays constant.
        let underline = if is_active { t.accent_bright } else { t.page };
        let item = div()
            .id(label)
            .px(px(11.))
            .py(px(10.))
            .text_size(px(t.text_small))
            .text_color(rgb(fg))
            .cursor_pointer()
            .border_b_1()
            .border_color(rgb(underline))
            .hover(|s| s.text_color(rgb(t.fg0)))
            .child(SharedString::from(label))
            .on_click(move |_ev, window, cx| {
                let (repo, pr_slug) = (repo.clone(), pr_slug.clone());
                app.update(cx, |this, cx| {
                    if let Some(Overlay::Drawer { tab: cur, .. }) = &mut this.overlay {
                        *cur = tab;
                    }
                    // Lazy-load the tab's data on first view.
                    if tab == DrawerTab::Readme && this.drawer.readme == ReadmeState::Idle {
                        this.drawer.readme = ReadmeState::Loading;
                        load_readme(repo, cx);
                    } else if tab == DrawerTab::Pr && matches!(this.drawer.pr, PrState::Idle) {
                        match pr_slug {
                            Some(slug) => {
                                this.drawer.pr = PrState::Loading;
                                load_pr(repo, slug, cx);
                            }
                            None => {
                                this.drawer.pr = PrState::Error("PR triage is GitHub-only.".into());
                            }
                        }
                    } else if tab == DrawerTab::Changes
                        && matches!(this.drawer.diff, DiffState::Idle)
                    {
                        this.drawer.diff = DiffState::Loading;
                        if this.drawer.commit_input.is_none() {
                            this.drawer.commit_input = Some(cx.new(|cx| {
                                gpui_component::input::InputState::new(window, cx)
                                    .placeholder("Commit message…")
                            }));
                        }
                        load_changes(repo, cx);
                    } else if tab == DrawerTab::Notes {
                        if this.drawer.notes.is_none() {
                            load_notes(repo.clone(), cx);
                        }
                        // Seed the editable note field from the saved note (sync).
                        if this.drawer.note_input.is_none() {
                            let initial = cache::note(&repo);
                            this.drawer.note_input = Some(cx.new(|cx| {
                                gpui_component::input::InputState::new(window, cx)
                                    .multi_line(true)
                                    .placeholder("Notes (markdown)…")
                                    .default_value(initial)
                            }));
                        }
                    }
                    cx.notify();
                });
            });
        bar = bar.child(item);
    }
    bar
}

#[allow(clippy::too_many_arguments)]
fn body(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    data: &DrawerData,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    let content = match tab {
        DrawerTab::Overview => overview(row, t, data, app).into_any_element(),
        DrawerTab::Readme => readme_view(data, t).into_any_element(),
        DrawerTab::Pr => pr_view(row, data, t, app).into_any_element(),
        DrawerTab::Changes => {
            changes_view(row, data, t, app, ai_ready, github_authed).into_any_element()
        }
        DrawerTab::Notes => notes_view(row, data, t, app, ai_ready).into_any_element(),
    };
    div()
        .id("drawer-body")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(px(18.))
        .gap(px(16.))
        .child(content)
}

/// Overview: the synchronous `Row` facts up top, then the async git data
/// (branches / recent commits / worktrees) loaded via [`load_overview`].
fn overview(row: &Row, t: &Theme, data: &DrawerData, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(16.));

    // Description.
    col = col.child(
        div()
            .text_size(px(t.text_small))
            .line_height(px(20.))
            .text_color(rgb(t.fg1))
            .child(row.description.clone()),
    );

    // AI summary, when present.
    if !row.ai_summary.is_empty() {
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(7.))
                .p(px(11.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(lucide("sparkles", 14., t.ai))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(t.text_data_sm))
                        .line_height(px(18.))
                        .text_color(rgb(t.ai))
                        .child(row.ai_summary.clone()),
                ),
        );
    }

    // Git status block.
    let mut status = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(16.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .child(seg("git-branch", row.branch.clone(), t.fg1));
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .text_color(rgb(color))
                .child(lucide("arrow-up", 13., color))
                .child(SharedString::from(row.ahead.to_string()))
                .child(lucide("arrow-down", 13., color))
                .child(SharedString::from(row.behind.to_string())),
        );
    }
    if row.dirty > 0 {
        status = status.child(seg(
            "circle-dot",
            SharedString::from(format!("{} dirty", row.dirty)),
            t.dirty,
        ));
    }
    col = col.child(status);

    // Host facts.
    let mut facts = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(16.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2));
    if row.private {
        facts = facts.child(seg("lock", SharedString::from("private"), t.fg3));
    }
    if !row.host.is_empty() {
        facts = facts.child(seg("star", row.stars.clone(), t.star));
    }
    if !row.release.is_empty() {
        facts = facts.child(seg("tag", row.release.clone(), t.fg2));
    }
    facts = facts.child(seg("clock", row.age.clone(), t.fg2));
    col = col.child(facts);

    // Async git data.
    col = col.child(branches_section(data, t, app));
    col = col.child(commits_section(data, t));
    col = col.child(worktrees_section(data, t, app));
    col.child(dispatch_section(data, t, app))
}

/// "Dispatch agent" (#185): a task-prompt field, a fresh-worktree toggle, and
/// the Dispatch button. Plain dispatch starts the configured agent in the repo
/// with the task appended (`agent_dispatch_args`); the toggle first creates an
/// `agent/…` branch + worktree and starts the agent there.
fn dispatch_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let repo = data.repo.clone();
    let mut s = section(t, "Dispatch agent", None);
    let Some(input) = &data.dispatch_input else {
        return s;
    };

    let fresh = data.dispatch_fresh;
    let toggle = {
        let app = app.clone();
        div()
            .id("dispatch-fresh")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(lucide(
                if fresh { "circle-check" } else { "circle-dot" },
                14.,
                if fresh { t.clean } else { t.fg3 },
            ))
            .child(
                div()
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg1))
                    .child("Fresh worktree"),
            )
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| this.toggle_dispatch_fresh(cx));
            })
    };

    let (app2, repo2, input2) = (app.clone(), repo, input.clone());
    s = s.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mt(px(2.))
            .child(gpui_component::input::Input::new(input))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(toggle)
                    .child(div().flex_1())
                    .child(pr_btn(
                        SharedString::from("dispatch-agent"),
                        "Dispatch",
                        t,
                        move |cx| {
                            let repo = repo2.clone();
                            let prompt = input2.read(cx).value().trim().to_string();
                            if prompt.is_empty() {
                                return;
                            }
                            app2.update(cx, |this, cx| {
                                this.dispatch_agent(repo, prompt, fresh, cx)
                            });
                        },
                    )),
            ),
    );
    s
}

/// Section wrapper: an uppercase label (+ optional count) over a list.
fn section(t: &Theme, title: &str, count: Option<usize>) -> Div {
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(title.to_uppercase()));
    if let Some(n) = count {
        head = head.child(SharedString::from(format!("· {n}")));
    }
    div().flex().flex_col().gap(px(3.)).child(head.mb(px(3.)))
}

/// A muted "Loading…" / empty / error placeholder line.
fn placeholder(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    div()
        .py(px(3.))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(text.into())
}

/// A small bordered pill tag (merged / gone).
fn tag(text: &str, color: u32, t: &Theme) -> impl IntoElement {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(color))
        .child(SharedString::from(text.to_string()))
}

fn branches_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut s = section(t, "Branches", data.branches.as_ref().map(|b| b.len()));
    match &data.branches {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No branches.", t)),
        Some(list) => {
            for b in list {
                s = s.child(branch_item(b, data.repo.clone(), t, app));
            }
        }
    }
    s
}

fn branch_item(
    b: &BranchRow,
    repo: SharedString,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let fg = if b.current { t.accent_bright } else { t.fg1 };
    let icon = if b.current { "check" } else { "git-branch" };
    let mut item = div()
        .id(SharedString::from(format!("br-{}", b.name)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(lucide(icon, 13., fg))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(b.name.clone()),
        );
    if b.merged {
        item = item.child(tag("merged", t.fg3, t));
    }
    if b.gone {
        item = item.child(tag("gone", t.behind, t));
    }
    // Only non-current branches are switchable.
    if !b.current {
        let name = b.name.clone();
        let app = app.clone();
        item = item
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .on_click(move |_ev, _win, cx| {
                let (repo, name) = (repo.clone(), name.clone());
                app.update(cx, |this, cx| {
                    this.drawer.branches = None; // optimistic loading state
                    switch_branch(repo, name, cx);
                    cx.notify();
                });
            });
    }
    item
}

fn commits_section(data: &DrawerData, t: &Theme) -> impl IntoElement {
    let mut s = section(t, "Recent commits", data.commits.as_ref().map(|c| c.len()));
    match &data.commits {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No commits.", t)),
        Some(list) => {
            for c in list {
                s = s.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .py(px(4.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(t.text_small))
                                .text_color(rgb(t.fg1))
                                .child(c.summary.clone()),
                        )
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(t.text_data_sm))
                                .text_color(rgb(t.fg3))
                                .child(SharedString::from(format!("{} · {}", c.author, c.age))),
                        ),
                );
            }
        }
    }
    s
}

fn worktrees_section(data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let repo = data.repo.clone();
    let mut s = section(t, "Worktrees", data.worktrees.as_ref().map(|w| w.len()));
    match &data.worktrees {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("None.", t)),
        Some(list) => {
            for w in list {
                let remove = {
                    let (app, repo, name) = (app.clone(), repo.clone(), w.name.to_string());
                    div()
                        .id(SharedString::from(format!("wt-rm-{}", w.name)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(22.))
                        .h(px(22.))
                        .rounded(px(t.r_xs))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.surface_hover)))
                        .child(lucide("x", 12., t.fg3))
                        .on_click(move |_ev, _win, cx| {
                            let (repo, name) = (repo.clone(), name.clone());
                            app.update(cx, |_this, cx| remove_worktree(repo, name, cx));
                        })
                };
                s = s.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(7.))
                        .py(px(4.))
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .child(lucide("folder-tree", 13., t.fg2))
                        .child(div().text_color(rgb(t.fg1)).child(w.name.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(rgb(t.fg3))
                                .child(w.path.clone()),
                        )
                        .child(remove),
                );
            }
        }
    }

    // Add a worktree: name field + Add button.
    if let Some(input) = &data.worktree_input {
        let (app, repo, input2) = (app.clone(), repo.clone(), input.clone());
        s = s.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .mt(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(gpui_component::input::Input::new(input)),
                )
                .child(pr_btn(SharedString::from("wt-add"), "Add", t, move |cx| {
                    let repo = repo.clone();
                    let name = input2.read(cx).value();
                    if name.trim().is_empty() {
                        return;
                    }
                    app.update(cx, |_this, cx| {
                        add_worktree(repo, name.trim().to_string(), cx)
                    });
                })),
        );
    }
    s
}

/// Changes tab: a commit composer acting on the index, a push / open-PR row
/// closing the commit → push → PR loop, the Unstaged/Staged file lists with
/// per-file stage/unstage, then the selected file's diff.
fn changes_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(12.));
    let has_staged = data
        .changes
        .as_ref()
        .is_some_and(|list| list.iter().any(|c| c.staged));

    // Commit composer (the field exists once the tab has been opened).
    if let Some(input) = &data.commit_input {
        let mut actions = div().flex().flex_row().items_center().gap(px(6.));
        // AI: suggest a commit message for the staged diff (gated on aiReady).
        if ai_ready {
            let app3 = app.clone();
            actions = actions.child(pr_btn(
                SharedString::from("gen-commit"),
                "Generate message",
                t,
                move |cx: &mut gpui::App| {
                    app3.update(cx, |this, cx| this.drawer_generate_commit(cx));
                },
            ));
        }
        actions = actions.child(div().flex_1());
        if has_staged {
            let repo = row.id.clone();
            let app2 = app.clone();
            let input2 = input.clone();
            actions = actions.child(pr_btn(
                SharedString::from("commit"),
                "Commit",
                t,
                move |cx: &mut gpui::App| {
                    let repo = repo.clone();
                    let msg = input2.read(cx).value();
                    if msg.trim().is_empty() {
                        return;
                    }
                    app2.update(cx, |_this, cx| commit_staged(repo, msg.to_string(), cx));
                },
            ));
        } else {
            // Nothing in the index — commit would be a no-op, so say why (once
            // the lists have actually loaded) instead of offering a live button.
            if data.changes.is_some() {
                actions = actions.child(
                    div()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg3))
                        .child(SharedString::from("Nothing staged")),
                );
            }
            actions = actions.child(disabled_btn("Commit", t));
        }
        col = col
            .child(gpui_component::input::Input::new(input))
            .child(actions);
    }

    // The AI commit-message suggestion, with a one-click commit using it.
    if let Some(msg) = &data.commit_suggestion {
        let repo = row.id.clone();
        let app4 = app.clone();
        let msg2 = msg.clone();
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(10.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(seg("sparkles", msg.clone(), t.fg1))
                .child(div().flex().flex_row().justify_end().child(pr_btn(
                    SharedString::from("commit-ai"),
                    "Commit this",
                    t,
                    move |cx: &mut gpui::App| {
                        let (repo, msg) = (repo.clone(), msg2.to_string());
                        if msg.trim().is_empty() {
                            return;
                        }
                        app4.update(cx, |_this, cx| commit_staged(repo, msg, cx));
                    },
                ))),
        );
    }

    // Push / Open PR — the last mile after committing. Push shows when there's
    // something to push (ahead, or a commit made in this drawer session while
    // the row's ahead count may be stale). Open PR shows only when it can work:
    // a GitHub remote, a GitHub token, a known default branch, and the current
    // branch isn't it — hidden otherwise, never broken (the aiReady philosophy;
    // only the AI *drafting* inside the flow is gated on aiReady).
    let show_push = row.ahead > 0 || data.committed;
    let show_pr = github_authed
        && github_slug(row).is_some()
        && data
            .default_branch
            .as_ref()
            .is_some_and(|d| *d != row.branch);
    if show_push || show_pr || data.pr_url.is_some() {
        let mut sync = div().flex().flex_row().items_center().gap(px(6.));
        if show_push {
            if data.push_busy {
                sync = sync.child(disabled_btn("Pushing…", t));
            } else {
                let app6 = app.clone();
                sync = sync.child(pr_btn(
                    SharedString::from("push"),
                    "Push",
                    t,
                    move |cx: &mut gpui::App| {
                        app6.update(cx, |this, cx| this.drawer_push(cx));
                    },
                ));
            }
        }
        if show_pr {
            if data.pr_busy {
                sync = sync.child(disabled_btn("Opening PR…", t));
            } else {
                let app7 = app.clone();
                sync = sync.child(pr_btn(
                    SharedString::from("open-pr"),
                    "Open PR",
                    t,
                    move |cx: &mut gpui::App| {
                        app7.update(cx, |this, cx| this.drawer_open_pr(cx));
                    },
                ));
            }
        }
        if let Some(url) = &data.pr_url {
            let url = url.clone();
            sync = sync.child(
                div()
                    .id("view-pr")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(t.r_sm))
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg1))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.surface_hover)))
                    .child(lucide("external-link", 12., t.fg2))
                    .child(SharedString::from("View PR"))
                    .on_click(move |_ev, _win, _cx| {
                        let _ = launch::open(&url);
                    }),
            );
        }
        col = col.child(sync);
    }

    // Unstaged / Staged file lists, then the selected file's diff.
    col = col.child(changes_section(row.id.clone(), data, false, t, app));
    col = col.child(changes_section(row.id.clone(), data, true, t, app));
    col = col.child(diff_pane(data, t));

    // AI changelog from recent commits (gated on aiReady).
    if ai_ready {
        let app5 = app.clone();
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(pr_btn(
                    SharedString::from("gen-changelog"),
                    "Generate changelog",
                    t,
                    move |cx: &mut gpui::App| {
                        app5.update(cx, |this, cx| this.drawer_generate_changelog(cx));
                    },
                )),
        );
    }
    if let Some(log) = &data.changelog {
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(seg("sparkles", "Changelog".into(), t.fg3))
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(
                            // The prompt asks for markdown bullets, so render as
                            // markdown — reflowed first, like the Readme tab, since
                            // the renderer panics on a run's embedded newlines.
                            gpui_component::text::markdown(crate::data::unwrap_soft_breaks(log)),
                        ),
                ),
        );
    }
    col
}

/// The kind marker for a change row: git's one-letter convention, coloured by
/// sentiment (M/A/D/R and "?" for untracked — each kind visibly distinct).
fn kind_badge(kind: git_ops::ChangeKind, t: &Theme) -> (&'static str, u32) {
    match kind {
        git_ops::ChangeKind::Modified => ("M", t.dirty),
        git_ops::ChangeKind::Added => ("A", t.clean),
        git_ops::ChangeKind::Deleted => ("D", t.behind),
        git_ops::ChangeKind::Renamed => ("R", t.accent_bright),
        git_ops::ChangeKind::Untracked => ("?", t.fg2),
    }
}

/// One of the Changes tab's file lists (`staged` picks which). Header: label +
/// count + a "Stage all"/"Unstage all" action; body: one row per file.
fn changes_section(
    repo: SharedString,
    data: &DrawerData,
    staged: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> Div {
    let title = if staged { "STAGED" } else { "UNSTAGED" };
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(title));

    let mut col = div().flex().flex_col().gap(px(2.));
    match &data.changes {
        None => col = col.child(placeholder("Loading…", t)),
        Some(list) => {
            let files: Vec<&ChangeRow> = list.iter().filter(|c| c.staged == staged).collect();
            head = head.child(SharedString::from(format!("· {}", files.len())));
            if files.is_empty() {
                let empty = if staged {
                    "Nothing staged."
                } else {
                    "No unstaged changes."
                };
                col = col.child(placeholder(empty, t));
            } else {
                // Stage all / Unstage all on the section header.
                let label = if staged { "Unstage all" } else { "Stage all" };
                let all: Vec<String> = files.iter().map(|c| c.path.to_string()).collect();
                let (app2, repo2) = (app.clone(), repo.clone());
                head = head.child(div().flex_1()).child(pr_btn(
                    SharedString::from(format!("stage-all-{staged}")),
                    label,
                    t,
                    move |cx: &mut gpui::App| {
                        let (repo, all) = (repo2.clone(), all.clone());
                        app2.update(cx, |_this, cx| set_staged(repo, all, !staged, cx));
                    },
                ));
                for c in files {
                    col = col.child(change_item(c, repo.clone(), data, t, app));
                }
            }
        }
    }
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(head.mb(px(3.)))
        .child(col)
}

/// One file row: kind letter + path + a stage/unstage icon button. Clicking
/// the row selects it for the diff pane; the trailing button moves it between
/// the index and the working tree ("+" stages, "x" unstages — the same
/// remove-from-list affordance as the worktree rows).
fn change_item(
    c: &ChangeRow,
    repo: SharedString,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let selected = data
        .change_sel
        .as_ref()
        .is_some_and(|(p, s)| *p == c.path && *s == c.staged);
    let (letter, color) = kind_badge(c.kind, t);

    let action = {
        let (app, repo, path) = (app.clone(), repo.clone(), c.path.to_string());
        let (staged, icon) = (c.staged, if c.staged { "x" } else { "plus" });
        div()
            .id(SharedString::from(format!(
                "chg-act-{}-{}",
                c.staged, c.path
            )))
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(t.r_xs))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide(icon, 12., t.fg3))
            .on_click(move |_ev, _win, cx| {
                // Don't also select the row underneath.
                cx.stop_propagation();
                let (repo, path) = (repo.clone(), path.clone());
                app.update(cx, |_this, cx| set_staged(repo, vec![path], !staged, cx));
            })
    };

    let row = div()
        .id(SharedString::from(format!("chg-{}-{}", c.staged, c.path)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(t.r_sm))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .cursor_pointer()
        .child(
            div()
                .w(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(color))
                .child(SharedString::from(letter)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(rgb(if selected { t.fg0 } else { t.fg1 }))
                .child(c.path.clone()),
        )
        .child(action);
    let row = if selected {
        row.bg(rgb(t.accent_wash))
    } else {
        row.hover(|s| s.bg(rgb(t.surface_hover)))
    };
    let (app, path, staged) = (app.clone(), c.path.clone(), c.staged);
    row.on_click(move |_ev, _win, cx| {
        let path = path.clone();
        app.update(cx, |this, cx| select_change(this, path, staged, cx));
    })
}

/// The selected file's diff, under a header naming the file and its side.
fn diff_pane(data: &DrawerData, t: &Theme) -> Div {
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from("DIFF"));
    if let Some((path, staged)) = &data.change_sel {
        let side = if *staged { "staged" } else { "unstaged" };
        head = head.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(SharedString::from(format!("· {path} ({side})"))),
        );
    }

    let body = match (&data.changes, &data.diff) {
        (None, _) => placeholder("Loading…", t).into_any_element(),
        (Some(list), _) if list.is_empty() => {
            placeholder("Working tree clean — nothing to commit.", t).into_any_element()
        }
        (_, DiffState::Ready(d)) if d.trim().is_empty() => {
            placeholder("No diff for this file.", t).into_any_element()
        }
        (_, DiffState::Ready(d)) => diff_block(d, t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(head.mb(px(3.)))
        .child(body)
}

/// Cap on rendered diff lines. The whole app re-renders on any `cx.notify()`
/// (agents poll, attention poll, appearance signals), so an unbounded diff
/// would rebuild thousands of elements on every background tick.
const DIFF_MAX_LINES: usize = 500;

/// Render a unified diff with per-line sentiment colouring, truncated to
/// [`DIFF_MAX_LINES`] with a muted "… n more lines" footer.
fn diff_block(diff: &str, t: &Theme) -> impl IntoElement {
    let mut block = div()
        .flex()
        .flex_col()
        .p(px(10.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    let mut lines = diff.lines();
    for line in lines.by_ref().take(DIFF_MAX_LINES) {
        let color = match line.as_bytes().first() {
            Some(b'+') => t.clean,
            Some(b'-') => t.behind,
            Some(b'@') => t.accent_bright,
            _ => t.fg2,
        };
        block = block.child(
            div()
                .text_color(rgb(color))
                .child(SharedString::from(line.to_string())),
        );
    }
    let hidden = lines.count();
    if hidden > 0 {
        block = block.child(
            div()
                .pt(px(6.))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(format!("… {hidden} more lines"))),
        );
    }
    block
}

/// Pick an icon for an individual check state.
fn check_icon(state: &str) -> &'static str {
    match state {
        "success" => "circle-check",
        "failure" => "circle-alert",
        "pending" => "clock",
        _ => "circle-dot",
    }
}

/// Colour a PR state string (checks / mergeable / review) by sentiment.
fn state_color(s: &str, t: &Theme) -> u32 {
    match s {
        "success" | "clean" | "approved" => t.clean,
        "failure" | "conflicting" | "changes_requested" => t.behind,
        "pending" => t.fg2,
        _ => t.fg3,
    }
}

/// PR tab: the open pull requests with checks/review/mergeable rollups and inline
/// approve / merge actions.
fn pr_view(row: &Row, data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    match &data.pr {
        PrState::Ready { prs, .. } if prs.is_empty() => {
            placeholder("No open pull requests.", t).into_any_element()
        }
        PrState::Ready { methods, prs } => {
            let repo = row.id.clone();
            let slug = row.slug.clone();
            let mut col = div().flex().flex_col().gap(px(10.));
            for pr in prs {
                col = col.child(pr_card(pr, methods, repo.clone(), slug.clone(), t, app));
            }
            col.into_any_element()
        }
        PrState::Error(e) => placeholder(e.clone(), t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    }
}

#[allow(clippy::too_many_arguments)]
fn pr_card(
    pr: &PrRow,
    methods: &[SharedString],
    repo: SharedString,
    slug: SharedString,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    // Title row (click opens the PR on the host).
    let title = {
        let url = pr.url.clone();
        let mut row = div()
            .id(SharedString::from(format!("pr-{}", pr.number)))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child(SharedString::from(format!("#{}", pr.number))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(t.text_small))
                    .text_color(rgb(t.fg0))
                    .child(pr.title.clone()),
            )
            .on_click(move |_ev, _win, _cx| {
                let _ = launch::open(&url);
            });
        if pr.draft {
            row = row.child(tag("draft", t.fg3, t));
        }
        row
    };

    // State rollups.
    let states = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(8.))
        .child(seg("git-pull-request", pr.refs.clone(), t.fg2))
        .child(seg_str(
            "circle-check",
            &pr.checks,
            state_color(&pr.checks, t),
        ))
        .child(seg_str(
            "git-merge",
            &pr.mergeable,
            state_color(&pr.mergeable, t),
        ))
        .child(seg_str("eye", &pr.review, state_color(&pr.review, t)));

    // Per-check breakdown: one clickable chip per status check / CI context,
    // so you can see which check failed and jump to its run (not just the rollup).
    let check_runs = (!pr.check_runs.is_empty()).then(|| {
        let mut wrap = div().flex().flex_row().flex_wrap().gap(px(6.));
        for c in &pr.check_runs {
            let color = state_color(&c.state, t);
            let chip = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .px(px(7.))
                .py(px(3.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.button_bg))
                .text_color(rgb(color))
                .child(lucide(check_icon(&c.state), 12., color))
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(c.name.clone()),
                );
            // Clickable (jumps to the CI run) only when the check carries a URL.
            let chip = match &c.url {
                Some(url) => {
                    let (url, hov) = (url.clone(), t.surface_hover);
                    chip.id(SharedString::from(format!(
                        "check-{}-{}",
                        pr.number, c.name
                    )))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(hov)))
                    .child(lucide("external-link", 11., t.fg3))
                    .on_click(move |_ev, _win, _cx| {
                        let _ = launch::open(&url);
                    })
                    .into_any_element()
                }
                None => chip.into_any_element(),
            };
            wrap = wrap.child(chip);
        }
        wrap
    });

    // Actions: approve + each allowed merge method.
    let mut actions = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(6.))
        .child(pr_btn(
            SharedString::from(format!("approve-{}", pr.number)),
            "Approve",
            t,
            {
                let (repo, slug, number) = (repo.clone(), slug.clone(), pr.number);
                let app = app.clone();
                move |cx: &mut gpui::App| {
                    let (repo, slug) = (repo.clone(), slug.clone());
                    app.update(cx, |this, cx| {
                        this.drawer.pr = PrState::Loading;
                        approve_pr(repo, slug, number, cx);
                        cx.notify();
                    });
                }
            },
        ));
    for method in methods {
        let label = capitalize(method);
        actions = actions.child(pr_btn(
            SharedString::from(format!("merge-{}-{method}", pr.number)),
            &label,
            t,
            {
                let (repo, slug, number, method) =
                    (repo.clone(), slug.clone(), pr.number, method.to_string());
                let app = app.clone();
                move |cx: &mut gpui::App| {
                    let (repo, slug, method) = (repo.clone(), slug.clone(), method.clone());
                    app.update(cx, |this, cx| {
                        this.drawer.pr = PrState::Loading;
                        merge_pr(repo, slug, number, method, cx);
                        cx.notify();
                    });
                }
            },
        ));
    }

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(11.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(title)
        .child(states);
    if let Some(check_runs) = check_runs {
        card = card.child(check_runs);
    }
    if !pr.author.is_empty() {
        card = card.child(
            div()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(format!("by {}", pr.author))),
        );
    }
    card.child(actions)
}

/// A small PR action button.
fn pr_btn(
    id: SharedString,
    label: &str,
    t: &Theme,
    on: impl Fn(&mut gpui::App) + 'static,
) -> impl IntoElement {
    let (hov_border, hov_fg) = (t.border_strong, t.fg0);
    div()
        .id(id)
        .px(px(10.))
        .py(px(5.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov_border)).text_color(rgb(hov_fg)))
        .child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| on(cx))
}

/// A [`pr_btn`]-shaped button in its disabled state: muted, inert, no hover.
fn disabled_btn(label: &str, t: &Theme) -> impl IntoElement {
    div()
        .px(px(10.))
        .py(px(5.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(label.to_string()))
}

/// Title-case a lowercase merge-method name ("squash" → "Squash").
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Like [`seg`] but takes a `&str` label.
fn seg_str(icon: &str, label: &str, color: u32) -> impl IntoElement {
    seg(icon, SharedString::from(label.to_string()), color)
}

/// Readme tab: the rendered README, or a placeholder while loading / when absent.
fn readme_view(data: &DrawerData, t: &Theme) -> impl IntoElement {
    match &data.readme {
        ReadmeState::Ready(Some(src)) => {
            // Reflow soft-wrapped lines first — gpui-component's renderer panics
            // on a paragraph's embedded newlines (see data::unwrap_soft_breaks).
            gpui_component::text::markdown(crate::data::unwrap_soft_breaks(src)).into_any_element()
        }
        ReadmeState::Ready(None) => placeholder("No README in this repo.", t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    }
}

/// Notes tab: a "resume where I left off" catch-up + an editable markdown note
/// (gpui-component multiline input) with Save.
fn notes_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
) -> Div {
    let mut col = div().flex().flex_col().gap(px(14.));
    let Some(n) = &data.notes else {
        return col.child(placeholder("Loading…", t));
    };

    // Catch-up row.
    let mut catch = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.))
        .p(px(11.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(lucide("history", 15., t.accent_bright))
        .child(
            div()
                .flex_1()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(n.catchup.clone()),
        );
    // AI "what changed" catch-up (gated on aiReady; only when there are commits).
    if ai_ready && n.count > 0 {
        let app2 = app.clone();
        catch = catch.child(pr_btn(
            SharedString::from("gen-resume"),
            "Catch me up",
            t,
            move |cx| {
                app2.update(cx, |this, cx| this.drawer_generate_resume(cx));
            },
        ));
    }
    if n.count > 0 || n.first_visit {
        let (app, repo) = (app.clone(), row.id.clone());
        catch = catch.child(pr_btn(
            SharedString::from("mark-seen"),
            "Mark caught up",
            t,
            move |cx| {
                let repo = repo.clone();
                app.update(cx, |_this, cx| mark_seen(repo, cx));
            },
        ));
    }
    col = col.child(catch);

    // The AI narrative, once generated.
    if let Some(resume) = &n.resume {
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(11.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(seg("sparkles", "What changed".into(), t.fg3))
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(
                            // Render as markdown (reflowed, like the Readme tab): the
                            // prompt asks for plain sentences but models still emit
                            // multi-line text, which plain text elements panic on.
                            gpui_component::text::markdown(crate::data::unwrap_soft_breaks(resume)),
                        ),
                ),
        );
    }

    // Editable note field + Save.
    let mut note = section(t, "Note", None);
    if let Some(input) = &data.note_input {
        let (app, repo, input2) = (app.clone(), row.id.clone(), input.clone());
        note = note
            .child(
                div()
                    .min_h(px(160.))
                    .child(gpui_component::input::Input::new(input).h_full()),
            )
            .child(div().flex().flex_row().justify_end().child(pr_btn(
                SharedString::from("save-note"),
                "Save note",
                t,
                move |cx| {
                    let (repo, text) = (repo.clone(), input2.read(cx).value());
                    app.update(cx, |_this, cx| save_note(repo, text.to_string(), cx));
                },
            )));
    }
    col.child(note)
}

fn footer(row: &Row, t: &Theme, ide_cmd: &str, agent_cmd: &str) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .px(px(18.))
        .py(px(14.))
        .border_t_1()
        .border_color(rgb(t.border))
        .child(launch_btn(
            "drawer-ide",
            SharedString::from("Open in IDE"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
                move || {
                    let _ = launch::launch(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-agent",
            SharedString::from("Agent"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
                move || {
                    let _ = launch::spawn(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-folder",
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            {
                let path = row.id.clone();
                move || {
                    let _ = launch::open(&path);
                }
            },
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        bar = bar.child(launch_btn(
            "drawer-host",
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move || {
                let _ = launch::open(&url);
            },
        ));
    }
    bar
}

/// A drawer launcher button. `on` runs a side-effecting launch (no app state).
fn launch_btn(
    id: &'static str,
    content: impl IntoElement,
    wide: bool,
    t: &Theme,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let (hov_border, hov_fg) = (t.border_strong, t.fg0);
    let b = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(9.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .font_family(MONO)
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov_border)).text_color(rgb(hov_fg)))
        .on_click(move |_ev, _win, _cx| on())
        .child(content);
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(40.))
    }
}

/// Inline icon+label segment (shared shape with the card's status segs).
fn seg(icon: &str, label: SharedString, color: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide(icon, 13., color))
        .child(label)
}
