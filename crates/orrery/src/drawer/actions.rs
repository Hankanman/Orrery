//! The drawer's actions, as `impl OrreryApp` methods: opening it, the async
//! per-tab loads (Overview, Readme, Changes + diff, Notes, PR) and the
//! mutations behind its buttons (branch switch, worktrees, staging,
//! commit, notes, PR approve/merge, push, Open PR, AI generators).
//!
//! Every load takes a [`Ticket`] for its [`Slot`] and stores its result only
//! while that ticket is current, so a stale or superseded load never lands.

use gpui::{AppContext, AsyncApp, Context, SharedString, WeakEntity, Window};
use orrery_core::{cache, git_ops, inbox};

use super::{
    BranchRow, ChangeRow, CheckItem, CommitRow, DiffState, HunkRow, LOG_LIMIT, NotesData, PrRow,
    PrState, ReadmeState, Slot, Ticket, WorktreeRow,
};
use crate::data;
use crate::shell::{DrawerTab, OrreryApp, Overlay, draft_and_create_pr};
use crate::toast::ToastKind;

/// Recent commit subject lines for a repo (newest first) — the input for the
/// AI changelog / resume prompts. Empty on any git error.
fn recent_summaries(id: &str, limit: usize) -> Vec<String> {
    orrery_core::git_ops::recent_log(id, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

impl OrreryApp {
    /// Open the repo detail drawer for `repo` (id) on Overview, and kick off its
    /// async git load.
    pub fn open_drawer(&mut self, repo: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Drawer {
            repo: repo.clone(),
            tab: DrawerTab::Overview,
        });
        self.drawer = crate::drawer::DrawerData::loading(repo.clone());
        // The Overview's CI line: the repo's cached default-branch CI state
        // (kept fresh by the central pass — see `refresh_ci`). Sync map
        // lookup, no I/O.
        self.drawer.ci = self
            .repos
            .iter()
            .find(|r| r.id.as_str() == repo.as_ref())
            .and_then(|r| {
                let slug = r.slug.as_deref()?;
                let host = r.remote_host.as_deref().unwrap_or_default();
                self.ci_states
                    .get(&(host.to_string(), slug.to_string()))
                    .cloned()
            });
        // The new-worktree field lives in Overview, shown immediately on open.
        self.drawer.worktree_input = Some(cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("new-worktree-name")
        }));
        // The dispatch task-prompt field (Overview's "Dispatch agent" section) —
        // created here because InputState needs the Window.
        self.drawer.dispatch_input = Some(cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("Task for the agent…")
        }));
        self.drawer_load_overview(cx);
        cx.notify();
    }

    /// Drawer Changes tab: generate a commit message for the staged diff (gated
    /// on `aiReady`). The (subject, body) suggestion lands in
    /// `drawer.commit_suggestion` and, on success, pre-fills the composer's
    /// subject + body fields so it's editable before committing.
    pub fn drawer_generate_commit(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        let repo = self.drawer.repo.clone();
        self.drawer.commit_suggestion =
            Some(crate::drawer::CommitSuggestion::new("Generating…", ""));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let diff = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::staged_diff(&id).unwrap_or_default() })
                .await;
            let result = if diff.trim().is_empty() {
                Err("Nothing staged — `git add` your changes first.".to_string())
            } else {
                crate::task::try_run(async move { orrery_core::ai::commit_message(&diff).await })
                    .await
                    .map(|m| orrery_core::ai::split_commit_message(&m))
            };
            // `update_in` (not `update`): populating the input fields needs the
            // window InputState::set_value requires.
            let _ = this.update_in(cx, |this, window, cx| {
                if this.drawer.repo != repo {
                    return;
                }
                match result {
                    Ok((subject, body)) => {
                        // The subject renders in a single-line seg — GPUI
                        // panics on embedded newlines, so flatten defensively.
                        let subject = crate::data::oneline(subject);
                        if let Some(input) = &this.drawer.commit_input {
                            input.update(cx, |st, cx| st.set_value(subject.clone(), window, cx));
                        }
                        if let Some(input) = &this.drawer.commit_body_input {
                            input.update(cx, |st, cx| st.set_value(body.clone(), window, cx));
                        }
                        this.drawer.commit_suggestion =
                            Some(crate::drawer::CommitSuggestion::new(subject, body));
                    }
                    Err(e) => {
                        this.drawer.commit_suggestion = Some(crate::drawer::CommitSuggestion::new(
                            crate::data::oneline(e),
                            "",
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drawer: generate a markdown changelog from recent commits (gated on
    /// `aiReady`). Lands in `drawer.changelog`.
    pub fn drawer_generate_changelog(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        let repo = self.drawer.repo.clone();
        self.drawer.changelog = Some("Generating…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let commits = cx
                .background_executor()
                .spawn(async move { recent_summaries(&id, 30) })
                .await;
            let text =
                crate::task::try_run(async move { orrery_core::ai::changelog(&commits).await })
                    .await
                    .unwrap_or_else(|e| format!("Changelog failed: {e}"));
            // Reflow once here, not per frame (the markdown renderer panics on
            // a run's embedded newlines).
            let text = crate::data::unwrap_soft_breaks(&text);
            let _ = this.update(cx, |this, cx| {
                if this.drawer.repo == repo {
                    this.drawer.changelog = Some(text.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Drawer Notes tab: generate an AI "what changed" catch-up from recent
    /// commits (gated on `aiReady`). Lands in `drawer.notes.resume`.
    pub fn drawer_generate_resume(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        let repo = self.drawer.repo.clone();
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        if let Some(n) = &mut self.drawer.notes {
            n.resume = Some("Generating…".into());
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let commits = cx
                .background_executor()
                .spawn(async move { recent_summaries(&id, 30) })
                .await;
            let text =
                crate::task::try_run(async move { orrery_core::ai::resume(&name, &commits).await })
                    .await
                    .map(|m| m.trim().to_string())
                    .unwrap_or_else(|e| format!("Catch-up failed: {e}"));
            let text = crate::data::unwrap_soft_breaks(&text);
            let _ = this.update(cx, |this, cx| {
                if this.drawer.repo == repo
                    && let Some(n) = &mut this.drawer.notes
                {
                    n.resume = Some(text.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Drawer Changes tab: push the current branch (setting the upstream when
    /// there is none — see `git_ops::push`). Sync-but-network git work, so it
    /// runs on the background executor; progress and the outcome flow through
    /// one keyed toast ("Pushing…" → Pushed / Push failed).
    pub fn drawer_push(&mut self, cx: &mut Context<Self>) {
        if self.drawer.push_busy {
            return;
        }
        let repo = self.drawer.repo.clone();
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        self.drawer.push_busy = true;
        let key = format!("push:{repo}");
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            format!("Pushing {name}…"),
            None,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let id = repo.to_string();
            let result = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::push(&id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.drawer.repo == repo {
                    this.drawer.push_busy = false;
                }
                match result {
                    Ok(msg) => {
                        if this.drawer.repo == repo {
                            this.drawer.committed = false;
                        }
                        this.upsert_toast(key, ToastKind::Success, "Pushed", Some(msg.into()), cx);
                    }
                    Err(e) => {
                        this.upsert_toast(key, ToastKind::Error, "Push failed", Some(e.into()), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drawer Changes tab: open a GitHub PR for the current branch — push
    /// first (ensuring the branch exists upstream), draft a title/body from
    /// the commit range (AI when `aiReady`, else the latest subject + commit
    /// list — the button works either way), then create the PR. Progress and
    /// the outcome flow through one keyed toast; the success toast links to
    /// the PR, as does a "View PR" affordance in the drawer.
    pub fn drawer_open_pr(&mut self, cx: &mut Context<Self>) {
        if self.drawer.pr_busy {
            return;
        }
        let repo = self.drawer.repo.clone();
        let Some(row) = self.rows.iter().find(|r| r.id == repo) else {
            return;
        };
        let slug = row.slug.to_string();
        let head = row.branch.to_string();
        let Some(base) = self.drawer.default_branch.as_ref().map(|b| b.to_string()) else {
            return;
        };
        if head == base {
            return;
        }
        let ai_ready = self.services.ai_ready;
        self.drawer.pr_busy = true;
        let key = format!("pr:{repo}");
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            "Opening PR…",
            Some(format!("{head} → {base}").into()),
            cx,
        );
        cx.spawn(async move |this, cx| {
            // 1. Ensure the branch is pushed (also sets the upstream if new).
            let id = repo.to_string();
            let pushed = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::push(&id) })
                .await;
            if let Err(e) = pushed {
                let _ = this.update(cx, |this, cx| {
                    if this.drawer.repo == repo {
                        this.drawer.pr_busy = false;
                    }
                    this.upsert_toast(key, ToastKind::Error, "Push failed", Some(e.into()), cx);
                    cx.notify();
                });
                return;
            }
            // 2–4. Draft the title/body from the commit range and create it.
            let (title, result) =
                draft_and_create_pr(cx, ai_ready, repo.to_string(), slug, head, base).await;
            let _ = this.update(cx, |this, cx| {
                if this.drawer.repo == repo {
                    this.drawer.pr_busy = false;
                    if let Ok(url) = &result {
                        this.drawer.pr_url = Some(url.clone().into());
                    }
                }
                this.finish_pr_toast(key, title, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Toggle the drawer's "fresh worktree" dispatch option.
    pub fn toggle_dispatch_fresh(&mut self, cx: &mut Context<Self>) {
        self.drawer.dispatch_fresh = !self.drawer.dispatch_fresh;
        cx.notify();
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

/// Apply a finished Overview load to the app, but only if `ticket` is still
/// current (else the user moved on, or a newer reload superseded this one).
fn store_overview(
    this: &WeakEntity<OrreryApp>,
    cx: &mut AsyncApp,
    ticket: Ticket,
    loaded: Loaded,
    now: i64,
) {
    let (branches, commits, worktrees, default_branch) = loaded;
    let _ = this.update(cx, |this, cx| {
        if !this.drawer.is_current(ticket) {
            return;
        }
        this.drawer.branches = Some(branches.into_iter().map(branch_row).collect());
        this.drawer.commits = Some(commits.into_iter().map(|c| commit_row(c, now)).collect());
        this.drawer.worktrees = Some(worktrees.into_iter().map(worktree_row).collect());
        this.drawer.default_branch = default_branch.map(Into::into);
        cx.notify();
    });
}

fn change_row(c: git_ops::FileChange) -> ChangeRow {
    ChangeRow {
        path: c.path.into(),
        kind: c.kind,
        staged: c.staged,
    }
}

/// Flatten a [`git_ops::Hunk`] for rendering (all single lines — GPUI text
/// elements panic on embedded newlines, and `lines()` guarantees none).
fn hunk_row(h: git_ops::Hunk) -> HunkRow {
    HunkRow {
        header: h.header.into(),
        lines: h.lines.into_iter().map(SharedString::from).collect(),
    }
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

/// Read the first matching README from the repo root.
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

/// The async per-tab loads and the git/cache/network mutations behind the
/// drawer's buttons. (These were free functions taking `&mut OrreryApp`;
/// they're methods now, like the rest of the drawer's actions.)
impl OrreryApp {
    /// Run `op` (sync git, may be a no-op) and then re-read the Overview on the
    /// background pool, storing it if still current. Backs the Overview load and
    /// every Overview mutation (branch switch, worktree add/remove).
    fn drawer_reload_overview(
        &mut self,
        op: impl FnOnce(&str) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let now = data::now_unix();
        let ticket = self.drawer.ticket(Slot::Overview);
        let id = self.drawer.repo.to_string();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    op(&id);
                    read_overview(&id)
                })
                .await;
            store_overview(&this, cx, ticket, loaded, now);
        })
        .detach();
    }

    /// Kick off the Overview load for the open drawer (branches/commits/worktrees).
    pub(crate) fn drawer_load_overview(&mut self, cx: &mut Context<Self>) {
        self.drawer_reload_overview(|_| {}, cx);
    }

    /// Switch the drawer's repo to `name`, then refresh the Overview. The caller
    /// sets the loading state. The `.git/HEAD` change also trips the filesystem
    /// watcher, so the card row refreshes on its own.
    pub(super) fn drawer_switch_branch(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.drawer_reload_overview(
            move |id| {
                let _ = git_ops::switch_branch(id, &name);
            },
            cx,
        );
    }

    /// Lazily load the repo's README (filesystem, sync) when the Readme tab opens.
    /// It's reflowed once here, off the UI thread, rather than on every frame.
    pub(super) fn drawer_load_readme(&mut self, cx: &mut Context<Self>) {
        let ticket = self.drawer.ticket(Slot::Readme);
        let id = self.drawer.repo.to_string();
        cx.spawn(async move |this, cx| {
            let content = cx
                .background_executor()
                .spawn(async move {
                    // Reflow soft-wrapped lines first — gpui-component's renderer
                    // panics on a paragraph's embedded newlines.
                    read_readme(&id).map(|src| SharedString::from(data::unwrap_soft_breaks(&src)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.drawer.is_current(ticket) {
                    this.drawer.readme = ReadmeState::Ready(content);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Run `op` (sync git) on the drawer's repo, then re-read the Changes tab's
    /// file lists, both on the background pool. `on_done` gets `op`'s result on
    /// the foreground (for toasts — pushed even if the drawer moved on); the
    /// lists are stored only if this is still the newest Changes load.
    fn drawer_reload_changes<R: Send + 'static>(
        &mut self,
        op: impl FnOnce(&str) -> R + Send + 'static,
        on_done: impl FnOnce(&mut OrreryApp, &R, &mut Context<OrreryApp>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let ticket = self.drawer.ticket(Slot::Changes);
        let id = self.drawer.repo.to_string();
        cx.spawn(async move |this, cx| {
            let (result, list) = cx
                .background_executor()
                .spawn(async move {
                    let result = op(&id);
                    (result, git_ops::changes(&id).unwrap_or_default())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                on_done(this, &result, cx);
                if this.drawer.is_current(ticket) {
                    this.drawer_store_changes(list, cx);
                }
            });
        })
        .detach();
    }

    /// Lazily load the Changes tab's file lists (sync git, off the UI thread).
    /// The store step auto-selects a file and loads its diff.
    pub(super) fn drawer_load_changes(&mut self, cx: &mut Context<Self>) {
        self.drawer_reload_changes(|_| (), |_, _, _| {}, cx);
    }

    /// Apply freshly read file lists: keep the selection if its entry survived
    /// (same path on the other side counts — the file just moved lists), else fall
    /// back to the first entry, then (re)load the selected file's diff. Runs after
    /// every load / stage / unstage / commit, so the pane always tracks the index.
    fn drawer_store_changes(&mut self, list: Vec<git_ops::FileChange>, cx: &mut Context<Self>) {
        let list: Vec<ChangeRow> = list.into_iter().map(change_row).collect();
        let sel = self
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
        self.drawer.changes = Some(list);
        self.drawer.change_sel = sel.clone();
        match sel {
            Some((path, staged)) => {
                self.drawer.diff = DiffState::Loading;
                self.drawer_load_file_diff(path, staged, cx);
            }
            None => self.drawer.diff = DiffState::Ready(Vec::new()),
        }
        cx.notify();
    }

    /// Show `path`'s diff (from the staged or unstaged side) in the pane.
    pub(super) fn drawer_select_change(
        &mut self,
        path: SharedString,
        staged: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .drawer
            .change_sel
            .as_ref()
            .is_some_and(|(p, s)| *p == path && *s == staged)
        {
            return;
        }
        self.drawer.change_sel = Some((path.clone(), staged));
        self.drawer.diff = DiffState::Loading;
        self.drawer_load_file_diff(path, staged, cx);
        cx.notify();
    }

    /// Load one file's diff as hunks (sync git, off the UI thread). The result is
    /// dropped if the drawer moved on or a newer diff load (another selection,
    /// or a refresh of this one) superseded it while in flight.
    fn drawer_load_file_diff(&mut self, path: SharedString, staged: bool, cx: &mut Context<Self>) {
        let ticket = self.drawer.ticket(Slot::Diff);
        let (id, file) = (self.drawer.repo.to_string(), path.to_string());
        cx.spawn(async move |this, cx| {
            let hunks = cx
                .background_executor()
                .spawn(async move { git_ops::file_hunks(&id, &file, staged).unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.drawer.is_current(ticket) {
                    this.drawer.diff = DiffState::Ready(hunks.into_iter().map(hunk_row).collect());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Stage (or unstage) one hunk of the selected file, then re-read the file
    /// lists + diff exactly like the file-level ops do. `staged` is the side the
    /// hunk was shown on (staged → unstage it). Failures surface as a toast.
    pub(super) fn drawer_hunk_op(
        &mut self,
        path: SharedString,
        staged: bool,
        hunk_ix: usize,
        cx: &mut Context<Self>,
    ) {
        let file = path.to_string();
        self.drawer_reload_changes(
            move |id| {
                if staged {
                    git_ops::unstage_hunk(id, &file, hunk_ix)
                } else {
                    git_ops::stage_hunk(id, &file, hunk_ix)
                }
            },
            move |this, result, cx| {
                if let Err(e) = result {
                    let title = if staged {
                        "Unstage hunk failed"
                    } else {
                        "Stage hunk failed"
                    };
                    this.push_toast(ToastKind::Error, title, Some(e.clone().into()), cx);
                }
            },
            cx,
        );
    }

    /// Stage (or unstage) `paths`, then re-read the file lists + selected diff.
    /// Failures surface as an error toast rather than vanishing silently.
    pub(super) fn drawer_set_staged(
        &mut self,
        paths: Vec<String>,
        stage: bool,
        cx: &mut Context<Self>,
    ) {
        self.drawer_reload_changes(
            move |id| {
                if stage {
                    git_ops::stage_paths(id, &paths)
                } else {
                    git_ops::unstage_paths(id, &paths)
                }
            },
            move |this, result, cx| {
                if let Err(e) = result {
                    let title = if stage {
                        "Stage failed"
                    } else {
                        "Unstage failed"
                    };
                    this.push_toast(ToastKind::Error, title, Some(e.clone().into()), cx);
                }
            },
            cx,
        );
    }

    /// Commit the index as-is with `message`, toast the outcome, then refresh the
    /// file lists + diff. The commit also trips the watcher, refreshing the card.
    pub(super) fn drawer_commit_staged(&mut self, message: String, cx: &mut Context<Self>) {
        let repo = self.drawer.repo.clone();
        // The commit subject, for the success toast's detail line.
        let subject = message.lines().next().unwrap_or("").trim().to_string();
        self.drawer_reload_changes(
            move |id| git_ops::commit(id, message.trim()),
            move |this, result, cx| {
                // The toast is global feedback — push it even if the drawer moved on.
                match result {
                    Ok(hash) => {
                        this.push_toast(
                            ToastKind::Success,
                            format!("Committed {hash}"),
                            Some(subject.into()),
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
                if result.is_ok() && this.drawer.repo == repo {
                    this.drawer.commit_suggestion = None;
                    // Offer Push even while the grid row's ahead count is stale.
                    this.drawer.committed = true;
                }
            },
            cx,
        );
    }

    /// Run `op` (cache write, may be a no-op) and then read the Notes tab data on
    /// the background pool, storing it if still current. Keeps any AI narrative
    /// already generated this session.
    fn drawer_reload_notes(
        &mut self,
        op: impl FnOnce(&str) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let ticket = self.drawer.ticket(Slot::Notes);
        let id = self.drawer.repo.to_string();
        cx.spawn(async move |this, cx| {
            let notes = cx
                .background_executor()
                .spawn(async move {
                    op(&id);
                    read_notes(&id)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.drawer.is_current(ticket) {
                    this.drawer.notes = Some(notes);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Lazily load the Notes tab data when it first opens.
    pub(super) fn drawer_load_notes(&mut self, cx: &mut Context<Self>) {
        self.drawer_reload_notes(|_| {}, cx);
    }

    /// Persist the edited note for `repo` (off the UI thread).
    pub(super) fn drawer_save_note(
        &mut self,
        repo: SharedString,
        text: String,
        cx: &mut Context<Self>,
    ) {
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

    /// Seed the (just created, empty) note field from the saved note, read off the
    /// UI thread. Skipped if the drawer moved on or the user already started
    /// typing — never clobber their text with the stored copy.
    pub(super) fn drawer_seed_note_input(&mut self, cx: &mut Context<Self>) {
        // Guarded by the session alone (no slot ticket — that would supersede
        // the catch-up load the Notes tab starts alongside this).
        let epoch = self.drawer.epoch;
        let id = self.drawer.repo.to_string();
        cx.spawn(async move |this, cx| {
            let initial = cx
                .background_executor()
                .spawn(async move { cache::note(&id) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.drawer.epoch != epoch || initial.is_empty() {
                    return;
                }
                if let Some(input) = &this.drawer.note_input
                    && input.read(cx).value().is_empty()
                {
                    input.update(cx, |st, cx| st.set_value(initial, window, cx));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Record the current HEAD as "seen", then refresh the catch-up.
    pub(super) fn drawer_mark_seen(&mut self, cx: &mut Context<Self>) {
        let now = data::now_unix();
        self.drawer_reload_notes(
            move |id| {
                if let Ok(sha) = git_ops::head_sha(id) {
                    let _ = cache::set_seen(id, &sha, now);
                }
            },
            cx,
        );
    }

    /// Create a worktree (+ branch) `name` in a sibling dir, then refresh Overview.
    pub(super) fn drawer_add_worktree(&mut self, name: String, cx: &mut Context<Self>) {
        self.drawer_reload_overview(
            move |id| {
                let dest = format!("{id}-{name}");
                let _ = git_ops::add_worktree(id, &name, &dest);
            },
            cx,
        );
    }

    /// Unlink worktree `name` (leaves files on disk), then refresh Overview.
    pub(super) fn drawer_remove_worktree(&mut self, name: String, cx: &mut Context<Self>) {
        self.drawer_reload_overview(
            move |id| {
                let _ = git_ops::remove_worktree(id, &name);
            },
            cx,
        );
    }

    /// Run `op` (a network PR action, may be a no-op) and then re-fetch the GitHub
    /// PR panel for `slug` (owner/name) on the shared tokio runtime via
    /// [`crate::task`], storing it if still current. The caller sets the loading
    /// state.
    fn drawer_reload_pr<F>(&mut self, slug: SharedString, op: F, cx: &mut Context<Self>)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let ticket = self.drawer.ticket(Slot::Pr);
        let s = slug.to_string();
        cx.spawn(async move |this, cx| {
            let panel = crate::task::try_run(async move {
                op.await;
                inbox::github_prs(&s).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if !this.drawer.is_current(ticket) {
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

    /// Lazily load the GitHub PR panel for the open drawer (slug = owner/name).
    pub(super) fn drawer_load_pr(&mut self, slug: SharedString, cx: &mut Context<Self>) {
        self.drawer_reload_pr(slug, async {}, cx);
    }

    /// Merge PR `number` via `method`, then refresh the panel. Caller sets the
    /// loading state.
    pub(super) fn drawer_merge_pr(
        &mut self,
        slug: SharedString,
        number: u64,
        method: String,
        cx: &mut Context<Self>,
    ) {
        let do_slug = slug.to_string();
        self.drawer_reload_pr(
            slug,
            async move {
                let _ = inbox::github_merge_pr(&do_slug, number, &method).await;
            },
            cx,
        );
    }

    /// Approve PR `number`, then refresh the panel. Caller sets the loading state.
    pub(super) fn drawer_approve_pr(
        &mut self,
        slug: SharedString,
        number: u64,
        cx: &mut Context<Self>,
    ) {
        let do_slug = slug.to_string();
        self.drawer_reload_pr(
            slug,
            async move {
                let _ = inbox::github_approve_pr(&do_slug, number).await;
            },
            cx,
        );
    }
}
