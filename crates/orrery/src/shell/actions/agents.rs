//! Agents: detect running coding-agent sessions (and keep polling while the
//! view is open or a dispatched session runs), dispatch an agent onto a repo
//! (optionally on a fresh worktree), and act on a finished dispatch — review
//! its diff, open a PR, or remove/discard the worktree behind the two-stage
//! confirm.

use gpui::{Context, SharedString};

use crate::shell::{OrreryApp, View, draft_and_create_pr};
use crate::toast::ToastKind;

impl OrreryApp {
    /// Scan the machine for running agent sessions (off the UI thread).
    /// Initial load when the Agents view first opens: scan with the spinner, then
    /// start the live poll.
    pub fn load_agents(&mut self, cx: &mut Context<Self>) {
        self.scan_agents(true, cx);
        self.start_agents_poll(cx);
    }

    /// Scan running agents off the UI thread, then update both the Agents list and
    /// the `active_agents` set (which drives the card indicator). `loading` shows
    /// the spinner; the poll passes `false` to refresh in place. Only repaints when
    /// the active set changed or the Agents view is showing. Single-flight: two
    /// overlapping scans could both see a session finish and toast it twice,
    /// so a request mid-scan reruns once afterwards instead.
    pub(crate) fn scan_agents(&mut self, loading: bool, cx: &mut Context<Self>) {
        use crate::views::agents::AgentsState;
        if loading {
            self.agents = AgentsState::Loading;
            cx.notify();
        }
        if !self.passes.agents.begin() {
            return;
        }
        let rows = self.rows.clone();
        let agent_command = self.config.agent_command.clone();
        cx.spawn(async move |this, cx| {
            let mut result = cx
                .background_executor()
                .spawn(async move { crate::views::agents::scan(&rows, &agent_command) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Card indicator: repos with a live session, including the
                // origin repo of any live dispatched-worktree session.
                let active: std::collections::HashSet<SharedString> = result
                    .sessions
                    .iter()
                    .map(|a| a.repo.clone())
                    .chain(
                        result
                            .dispatched
                            .iter()
                            .filter(|d| d.pid.is_some())
                            .map(|d| d.origin.clone()),
                    )
                    .collect();
                let changed = active != this.active_agents;
                let supervising = result.dispatched.iter().any(|d| d.pid.is_some());
                let finished = std::mem::take(&mut result.finished_now);
                // Collapse the review panel if its worktree vanished (e.g.
                // discarded from another surface).
                if let Some(open) = &this.agents_ui.review
                    && !result.dispatched.iter().any(|d| d.worktree_path == *open)
                {
                    this.agents_ui.review = None;
                    this.agents_ui.review_diff = None;
                }
                this.active_agents = active;
                this.agents = AgentsState::Ready(result);
                // Fresh agent facts → refresh the attention surfaces (cheap;
                // the notify below only fires when something visible changed).
                this.recompute_attention();
                // Surface the one-shot finish events (#185): always a toast;
                // a desktop notification when config allows. AgentFinished is
                // Attention-tier (not Urgent, so the urgent-diff notifier
                // skips it), but a finished dispatch is the whole point of
                // the session — it notifies from the transition itself,
                // gated by `notify_agent_finished`. The persisted
                // `finished_at` means each finish fires exactly once, across
                // polls and restarts alike.
                for f in finished {
                    let commits = if f.commits == 1 {
                        "1 commit".to_string()
                    } else {
                        format!("{} commits", f.commits)
                    };
                    let title = format!("Agent finished on {}", f.origin_name);
                    let body = format!("{commits} on {} — review in Agents", f.branch);
                    this.push_toast(
                        crate::toast::ToastKind::Info,
                        title.clone(),
                        Some(body.clone().into()),
                        cx,
                    );
                    if this.config.notify_enabled
                        && this.config.notify_attention
                        && this.config.notify_agent_finished
                    {
                        crate::task::spawn_detached(async move {
                            let _ = orrery_platform::notify::send(&title, &body).await;
                        });
                    }
                }
                // Keep supervising live dispatched sessions even when the
                // Agents view isn't open, so the finish transition is
                // observed promptly (no-op if the poll is already running).
                if supervising {
                    this.start_agents_poll(cx);
                }
                if changed || this.view == View::Agents {
                    cx.notify();
                }
                if this.passes.agents.finish() {
                    this.scan_agents(false, cx);
                }
            });
        })
        .detach();
    }

    /// Re-scan agents every 5s while the Agents view is open — so the list
    /// stays live (terminated sessions drop off, new ones appear) — or while
    /// a dispatched-worktree session is running, so its finish transition is
    /// observed even with the view closed (#185: the attention item and
    /// notification must not depend on the user watching the Agents view).
    /// Exits when neither holds; restarted by `maybe_load_view` on re-entry
    /// and by any scan that sees a live dispatched session.
    pub(super) fn start_agents_poll(&mut self, cx: &mut Context<Self>) {
        if self.agents_ui.polling {
            return;
        }
        self.agents_ui.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                let keep = this
                    .update(cx, |this, _| {
                        this.view == View::Agents || this.supervising_dispatch()
                    })
                    .unwrap_or(false);
                if !keep {
                    let _ = this.update(cx, |this, _| this.agents_ui.polling = false);
                    break;
                }
                if this
                    .update(cx, |this, cx| this.scan_agents(false, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Is a dispatched-worktree agent session currently alive? While true the
    /// agents poll keeps running off-view to catch the finish transition.
    fn supervising_dispatch(&self) -> bool {
        match &self.agents {
            crate::views::agents::AgentsState::Ready(data) => {
                data.dispatched.iter().any(|d| d.pid.is_some())
            }
            _ => false,
        }
    }

    /// Terminate an agent process by pid, then re-scan the list.
    pub fn terminate_agent(&mut self, pid: u32, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { orrery_platform::agents::terminate(pid) })
                .await;
            let _ = this.update(cx, |this, cx| this.scan_agents(false, cx));
        })
        .detach();
    }

    /// Dispatch a coding agent onto `repo` with a task `prompt` (#185). Plain
    /// dispatch launches `agent_command` + `agent_dispatch_args` in the repo
    /// directory, exactly like the drawer's Agent button plus the prompt as an
    /// argument. With `fresh`, a branch `agent/<slug>-<rand>` and a worktree
    /// under the app data dir are created first (sync git — background
    /// executor), the pairing is recorded in the cache, and the agent starts in
    /// the worktree. Success/failure lands as a toast either way.
    pub fn dispatch_agent(
        &mut self,
        repo: SharedString,
        prompt: String,
        fresh: bool,
        cx: &mut Context<Self>,
    ) {
        use crate::toast::ToastKind;
        let cmd = self.config.agent_command.clone();
        let args = self.config.agent_dispatch_args.clone();

        if !fresh {
            match orrery_core::launch::spawn_with_prompt(&cmd, &args, &repo, &prompt) {
                Ok(_) => {
                    self.push_toast(
                        ToastKind::Success,
                        "Agent dispatched",
                        Some(repo.clone()),
                        cx,
                    );
                }
                Err(e) => {
                    self.push_toast(ToastKind::Error, "Dispatch failed", Some(e.into()), cx);
                }
            }
            return;
        }

        let names = orrery_core::dispatch::names(&prompt);
        let Some(dest) = orrery_core::dispatch::worktree_dest(&repo, &names.worktree) else {
            self.push_toast(
                ToastKind::Error,
                "Dispatch failed",
                Some("no data directory for worktrees".into()),
                cx,
            );
            return;
        };
        let toast_key = SharedString::from(format!("dispatch:{repo}"));
        self.upsert_toast(
            toast_key.clone(),
            ToastKind::Progress,
            format!("Creating worktree on {}…", names.branch),
            Some(repo.clone()),
            cx,
        );

        let branch = names.branch.clone();
        let id = repo.to_string();
        let now = crate::data::now_unix();
        cx.spawn(async move |this, cx| {
            // Worktree add is sync libgit2 work — keep it off the UI thread.
            let (id2, name, branch2, prompt2, dest2) = (
                id.clone(),
                names.worktree.clone(),
                names.branch.clone(),
                prompt.clone(),
                dest.to_string_lossy().into_owned(),
            );
            let created: Result<String, String> = cx
                .background_executor()
                .spawn(async move {
                    if let Some(parent) = std::path::Path::new(&dest2).parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let wt_path = orrery_core::git_ops::add_worktree_on_branch(
                        &id2, &name, &branch2, &dest2,
                    )?;
                    orrery_core::cache::record_agent_worktree(
                        &orrery_core::cache::AgentWorktree {
                            worktree_path: wt_path.clone(),
                            repo_id: id2,
                            branch: branch2,
                            worktree_name: name,
                            prompt: prompt2,
                            created_at: now,
                            last_seen_alive: 0,
                            finished_at: 0,
                            commits_ahead: 0,
                            pr_url: String::new(),
                        },
                    )?;
                    Ok(wt_path)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                use crate::toast::ToastKind;
                match created {
                    Ok(wt_path) => {
                        let launched = orrery_core::launch::spawn_with_prompt(
                            &this.config.agent_command,
                            &this.config.agent_dispatch_args,
                            &wt_path,
                            &prompt,
                        );
                        match launched {
                            Ok(_) => this.upsert_toast(
                                toast_key,
                                ToastKind::Success,
                                "Agent dispatched on fresh worktree",
                                Some(branch.into()),
                                cx,
                            ),
                            Err(e) => this.upsert_toast(
                                toast_key,
                                ToastKind::Error,
                                "Worktree created, but the agent failed to launch",
                                Some(e.into()),
                                cx,
                            ),
                        };
                        // Show the new worktree in the open drawer's Overview
                        // and the Agents view without waiting for the poll.
                        if this.drawer.repo == repo {
                            this.drawer_load_overview(cx);
                        }
                        this.scan_agents(false, cx);
                    }
                    Err(e) => {
                        this.upsert_toast(
                            toast_key,
                            ToastKind::Error,
                            "Dispatch failed",
                            Some(e.into()),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// First click on a dispatched worktree's "Remove worktree" button: arm the
    /// two-stage confirm (see [`crate::shell::ArmedConfirm`]).
    pub fn arm_worktree_remove(&mut self, path: SharedString, cx: &mut Context<Self>) {
        self.arm_confirm(|app| &mut app.agents_ui.confirm, path, cx);
    }

    /// Remove a dispatched agent worktree: refused (with a toast) while it has
    /// uncommitted changes, otherwise unlink it from the origin repo, delete
    /// its directory (it lives under our data dir), and forget the pairing.
    /// The `agent/…` branch is kept — it holds the agent's commits. Only
    /// reached via the two-stage confirm ([`Self::arm_worktree_remove`]).
    pub fn remove_dispatch_worktree(
        &mut self,
        path: SharedString,
        origin: SharedString,
        name: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.agents_ui.confirm.disarm();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (p, o, n) = (path.to_string(), origin.to_string(), name.to_string());
            let result: Result<(), String> = cx
                .background_executor()
                .spawn(async move {
                    let dirty = !orrery_core::git_ops::changes(&p)
                        .map_err(|e| format!("can't inspect worktree: {e}"))?
                        .is_empty();
                    if dirty {
                        return Err("the worktree has uncommitted changes".into());
                    }
                    orrery_core::git_ops::remove_worktree(&o, &n)?;
                    // Unlinking leaves the files; this worktree is ours (under
                    // the app data dir), so clean up the directory too.
                    std::fs::remove_dir_all(&p).map_err(|e| e.to_string())?;
                    orrery_core::cache::remove_agent_worktree(&p)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                use crate::toast::ToastKind;
                match result {
                    Ok(()) => this.push_toast(
                        ToastKind::Success,
                        "Worktree removed",
                        Some(path.clone()),
                        cx,
                    ),
                    Err(e) => this.push_toast(
                        ToastKind::Error,
                        "Worktree not removed",
                        Some(e.into()),
                        cx,
                    ),
                };
                this.scan_agents(false, cx);
            });
        })
        .detach();
    }

    /// Toggle the inline review diff for a finished dispatched worktree
    /// (#185): the `agent/…` branch's changes vs the origin's default branch
    /// (merge-base scoped — see `git_ops::branch_diff`), loaded off the UI
    /// thread and rendered with the drawer's capped diff renderer. One panel
    /// at a time; toggling another card's Review replaces it.
    pub fn toggle_agent_review(
        &mut self,
        path: SharedString,
        origin: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.agents_ui.review.as_ref() == Some(&path) {
            self.agents_ui.review = None;
            self.agents_ui.review_diff = None;
            cx.notify();
            return;
        }
        self.agents_ui.review = Some(path.clone());
        self.agents_ui.review_diff = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (p, o) = (path.to_string(), origin.to_string());
            let diff = cx
                .background_executor()
                .spawn(async move {
                    let base = orrery_core::git_ops::default_branch(&o)
                        .ok_or("origin has no default branch")?;
                    orrery_core::git_ops::branch_diff(&p, &base)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Dropped if the panel moved to another card meanwhile.
                if this.agents_ui.review.as_ref() == Some(&path) {
                    this.agents_ui.review_diff =
                        Some(diff.unwrap_or_else(|e| format!("Diff failed: {e}")).into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Land a finished dispatched worktree as a PR (#185): push the `agent/…`
    /// branch from the worktree (its HEAD — sets the upstream on first push),
    /// draft a title/body from the commit range (AI when `aiReady`, fallback
    /// otherwise), create the PR against the origin's default branch, and
    /// persist the PR URL on the pairing row — which clears the worktree's
    /// AgentFinished attention item (the work is handed off). Mirrors
    /// [`Self::drawer_open_pr`], re-targeted at the worktree.
    pub fn open_worktree_pr(
        &mut self,
        path: SharedString,
        origin: SharedString,
        branch: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.agents_ui.pr_busy.contains(&path) {
            return;
        }
        let Some(row) = self.rows.iter().find(|r| r.id == origin) else {
            self.push_toast(
                ToastKind::Error,
                "Open PR failed",
                Some("origin repo is not in the fleet".into()),
                cx,
            );
            return;
        };
        let Some(slug) = crate::drawer::github_slug(row) else {
            self.push_toast(
                ToastKind::Error,
                "Open PR failed",
                Some("origin has no GitHub remote".into()),
                cx,
            );
            return;
        };
        let slug = slug.to_string();
        let ai_ready = self.services.ai_ready;
        self.agents_ui.pr_busy.insert(path.clone());
        let key = format!("wt-pr:{path}");
        self.upsert_toast(
            key.clone(),
            ToastKind::Progress,
            "Opening PR…",
            Some(branch.clone()),
            cx,
        );
        cx.spawn(async move |this, cx| {
            // 1. Resolve the base and push the branch from the worktree.
            let (p, o) = (path.to_string(), origin.to_string());
            let pushed: Result<String, String> = cx
                .background_executor()
                .spawn(async move {
                    let base = orrery_core::git_ops::default_branch(&o)
                        .ok_or("origin has no default branch")?;
                    orrery_core::git_ops::push(&p)?;
                    Ok(base)
                })
                .await;
            let base = match pushed {
                Ok(base) => base,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.agents_ui.pr_busy.remove(&path);
                        this.upsert_toast(key, ToastKind::Error, "Push failed", Some(e.into()), cx);
                        cx.notify();
                    });
                    return;
                }
            };
            // 2–4. Draft the title/body from the commit range and create it.
            let (title, result) = draft_and_create_pr(
                cx,
                ai_ready,
                path.to_string(),
                slug,
                branch.to_string(),
                base,
            )
            .await;
            // 5. Persist the hand-off so the attention item clears (and stays
            //    cleared across restarts), then refresh the cards.
            if let Ok(url) = &result {
                let (p3, u) = (path.to_string(), url.clone());
                cx.background_executor()
                    .spawn(async move {
                        let _ = orrery_core::cache::set_agent_worktree_pr(&p3, &u);
                    })
                    .await;
            }
            let _ = this.update(cx, |this, cx| {
                this.agents_ui.pr_busy.remove(&path);
                let opened = result.is_ok();
                this.finish_pr_toast(key, title, result, cx);
                if opened {
                    this.scan_agents(false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Discard a finished dispatched worktree's outcome (#185): remove the
    /// worktree AND delete its `agent/…` branch — the commits go with it,
    /// which is the point (contrast [`Self::remove_dispatch_worktree`], which
    /// keeps the branch). Refused while the worktree has uncommitted changes,
    /// like the plain removal. Only reached via the two-stage confirm
    /// (`arm_worktree_remove` with the `discard:`-prefixed key). Dropping the
    /// cache row clears the AgentFinished attention item.
    pub fn discard_dispatch_worktree(
        &mut self,
        path: SharedString,
        origin: SharedString,
        name: SharedString,
        branch: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.agents_ui.confirm.disarm();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (p, o, n, b) = (
                path.to_string(),
                origin.to_string(),
                name.to_string(),
                branch.to_string(),
            );
            let result: Result<(), String> = cx
                .background_executor()
                .spawn(async move {
                    let dirty = !orrery_core::git_ops::changes(&p)
                        .map_err(|e| format!("can't inspect worktree: {e}"))?
                        .is_empty();
                    if dirty {
                        return Err("the worktree has uncommitted changes".into());
                    }
                    // Unlink first so the branch is no longer checked out
                    // anywhere (delete_branch refuses otherwise), then drop
                    // the branch, the directory (ours, under the app data
                    // dir), and the pairing row.
                    orrery_core::git_ops::remove_worktree(&o, &n)?;
                    orrery_core::git_ops::delete_branch(&o, &b)?;
                    std::fs::remove_dir_all(&p).map_err(|e| e.to_string())?;
                    orrery_core::cache::remove_agent_worktree(&p)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                use crate::toast::ToastKind;
                match result {
                    Ok(()) => this.push_toast(
                        ToastKind::Success,
                        "Worktree discarded",
                        Some(branch.clone()),
                        cx,
                    ),
                    Err(e) => {
                        this.push_toast(ToastKind::Error, "Discard failed", Some(e.into()), cx)
                    }
                };
                if this.agents_ui.review.as_ref() == Some(&path) {
                    this.agents_ui.review = None;
                    this.agents_ui.review_diff = None;
                }
                this.scan_agents(false, cx);
            });
        })
        .detach();
    }
}
