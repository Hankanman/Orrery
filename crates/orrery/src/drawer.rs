//! RepoDrawer — the right-anchored detail panel (port of `RepoDrawer.tsx`). Opens
//! over the shell when a card is clicked; a scrim backdrop or the close button
//! dismisses it. Tabs: Overview / Changes / PR / Notes / Readme.
//!
//! This is the workhorse primitive — most journeys (catch-up, dive, commit, PR
//! triage) live here. All five tabs are in: Overview (Row facts + async
//! branches/commits/worktrees, with worktree add/remove), Readme (gpui-component
//! markdown), PR (lazy, GitHub-only, via the `task` bridge — rollups + inline
//! approve/merge), Changes (staged diff + commit-message field + Commit), and
//! Notes (catch-up + an editable markdown note via gpui-component's multiline
//! input). Follow-up: the AI "generate commit message" (gated on aiReady, #102).

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
    pub refs: SharedString,      // "head → base"
    pub mergeable: SharedString, // clean | conflicting | unknown
    pub review: SharedString,    // approved | changes_requested | review_required | none
    pub checks: SharedString,    // success | failure | pending | none
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

/// Lazy-loaded staged-diff state (sync git, but loaded off the UI thread).
#[derive(Default)]
pub enum DiffState {
    #[default]
    Idle,
    Loading,
    /// The staged diff text ("" when nothing is staged).
    Ready(SharedString),
}

/// Notes tab data: the "resume where I left off" catch-up line (the note text
/// itself lives in `DrawerData::note_input`).
pub struct NotesData {
    pub catchup: SharedString,
    /// Commits since last seen — drives whether "Mark caught up" is offered.
    pub count: usize,
    pub first_visit: bool,
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
    pub diff: DiffState,
    /// Notes tab (catch-up + saved note); `None` until first shown.
    pub notes: Option<NotesData>,
    /// The editable note field (gpui-component multiline input), seeded from the
    /// saved note when the Notes tab first opens.
    pub note_input: Option<Entity<gpui_component::input::InputState>>,
    /// The commit-message field for the Changes tab, created when it first opens.
    pub commit_input: Option<Entity<gpui_component::input::InputState>>,
    /// The new-worktree name field (Overview), created when the drawer opens.
    pub worktree_input: Option<Entity<gpui_component::input::InputState>>,
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
);

/// Read branches + recent log + worktrees for `id` (all git-heavy — runs on the
/// background pool).
fn read_overview(id: &str) -> Loaded {
    (
        git_ops::branches(id).unwrap_or_default(),
        git_ops::recent_log(id, LOG_LIMIT).unwrap_or_default(),
        git_ops::worktrees(id).unwrap_or_default(),
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
    let (branches, commits, worktrees) = loaded;
    let _ = this.update(cx, |this, cx| {
        if &this.drawer.repo != repo {
            return;
        }
        this.drawer.branches = Some(branches.into_iter().map(branch_row).collect());
        this.drawer.commits = Some(commits.into_iter().map(|c| commit_row(c, now)).collect());
        this.drawer.worktrees = Some(worktrees.into_iter().map(worktree_row).collect());
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

/// Lazily load the staged diff (sync git, off the UI thread) for the Changes tab.
pub fn load_diff(repo: SharedString, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let diff = cx
            .background_executor()
            .spawn(async move { git_ops::staged_diff(&id).unwrap_or_default() })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo == repo {
                this.drawer.diff = DiffState::Ready(diff.into());
                cx.notify();
            }
        });
    })
    .detach();
}

/// Commit the staged changes with `message`, then refresh the (now-empty) diff
/// and clear the field. The commit trips the watcher, refreshing the card.
fn commit_staged(repo: SharedString, message: String, cx: &mut Context<OrreryApp>) {
    let id = repo.to_string();
    cx.spawn(async move |this, cx| {
        let _ = cx
            .background_executor()
            .spawn(async move { git_ops::commit(&id, message.trim()) })
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.drawer.repo != repo {
                return;
            }
            this.drawer.diff = DiffState::Loading;
            load_diff(repo, cx);
            cx.notify();
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
        .child(body(row, tab, t, data, app))
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
                        load_diff(repo, cx);
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

fn body(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    data: &DrawerData,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let content = match tab {
        DrawerTab::Overview => overview(row, t, data, app).into_any_element(),
        DrawerTab::Readme => readme_view(data, t).into_any_element(),
        DrawerTab::Pr => pr_view(row, data, t, app).into_any_element(),
        DrawerTab::Changes => changes_view(row, data, t, app).into_any_element(),
        DrawerTab::Notes => notes_view(row, data, t, app).into_any_element(),
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
    col.child(worktrees_section(data, t, app))
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

/// Changes tab: a commit-message field + Commit, then the staged diff.
fn changes_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(12.));

    // Commit composer (the field exists once the tab has been opened).
    if let Some(input) = &data.commit_input {
        let repo = row.id.clone();
        let app2 = app.clone();
        let input2 = input.clone();
        col = col.child(gpui_component::input::Input::new(input)).child(
            div().flex().flex_row().justify_end().child(pr_btn(
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
            )),
        );
    }

    // Staged diff.
    let diff = match &data.diff {
        DiffState::Ready(d) if d.trim().is_empty() => {
            placeholder("Nothing staged — `git add` your changes first.", t).into_any_element()
        }
        DiffState::Ready(d) => diff_block(d, t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    };
    col.child(diff)
}

/// Render a unified diff with per-line sentiment colouring.
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
    for line in diff.lines() {
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
    block
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
fn notes_view(row: &Row, data: &DrawerData, t: &Theme, app: &Entity<OrreryApp>) -> Div {
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
