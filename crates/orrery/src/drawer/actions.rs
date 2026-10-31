//! The drawer's actions, as `impl OrreryApp` methods: opening it, and the
//! AI generators, push and Open PR flows behind its buttons.

use gpui::{AppContext, Context, SharedString, Window};

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
        crate::drawer::load_overview(self, cx);
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
