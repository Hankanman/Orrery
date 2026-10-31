//! GitHub: the account state (device-flow sign-in, sign-out, token check)
//! and the shared tail of both "Open PR" flows (drawer branch, agent
//! worktree).

use gpui::Context;

use crate::shell::OrreryApp;
use crate::toast::ToastKind;

/// The shared tail of both "Open PR" flows (drawer branch, agent worktree),
/// run once `head` is pushed: read the commit range `dir` has over `base`,
/// draft the title/body from it — AI-drafted when `ai_ready`, else the
/// fallback; only the drafting is aiReady-gated, never the button — then
/// create the PR on `slug`. Returns the title and the PR URL (or the error).
pub(crate) async fn draft_and_create_pr(
    cx: &mut gpui::AsyncApp,
    ai_ready: bool,
    dir: String,
    slug: String,
    head: String,
    base: String,
) -> (String, Result<String, String>) {
    // The branch's commit range, for drafting the title/body.
    let base2 = base.clone();
    let commits: Vec<String> = cx
        .background_executor()
        .spawn(async move {
            orrery_core::git_ops::commits_ahead_of(&dir, &base2, 30)
                .unwrap_or_default()
                .into_iter()
                .map(|c| c.summary)
                .collect()
        })
        .await;
    let drafted = if ai_ready && !commits.is_empty() {
        let (h, c) = (head.clone(), commits.clone());
        crate::task::try_run(async move { orrery_core::ai::pr_description(&h, &c).await })
            .await
            .ok()
            .and_then(|d| orrery_core::ai::split_pr_draft(&d))
    } else {
        None
    };
    let (title, body) =
        drafted.unwrap_or_else(|| orrery_core::ai::fallback_pr_draft(&head, &commits));
    // Create the PR (network → the shared tokio runtime).
    let t2 = title.clone();
    let result = crate::task::try_run(async move {
        let token = orrery_core::oauth::github_token_async()
            .await
            .ok_or("connect GitHub to open a PR")?;
        orrery_core::forge::create_pr(&slug, &head, &base, &t2, &body, &token).await
    })
    .await;
    (title, result)
}

impl OrreryApp {
    /// Re-resolve whether a GitHub token is available (may shell out to `gh`, so
    /// off the UI thread).
    pub(super) fn refresh_github_authed(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let authed = cx
                .background_executor()
                .spawn(async { orrery_core::oauth::github_authed() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.services.github_authed = authed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Begin the GitHub device-flow login: request a code, show it, then poll
    /// until the user authorizes (or it fails / expires).
    pub fn github_connect(&mut self, cx: &mut Context<Self>) {
        use crate::views::settings::GithubDevice;
        if self.services.github_device.is_some() {
            return;
        }
        self.services.github_device = Some(GithubDevice {
            user_code: "…".into(),
            verification_uri: "https://github.com/login/device".into(),
            status: "Requesting a device code…".into(),
        });
        cx.notify();

        let client_id = orrery_core::oauth::github_client_id();
        cx.spawn(async move |this, cx| {
            let id = client_id.clone();
            let started =
                crate::task::try_run(async move { orrery_core::oauth::device_start(&id).await })
                    .await;
            let start = match started {
                Ok(d) => d,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(d) = &mut this.services.github_device {
                            d.status = format!("Failed: {e}").into();
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let device_code = start.device_code.clone();
            let interval = start.interval.max(1);
            if this
                .update(cx, |this, cx| {
                    this.services.github_device = Some(GithubDevice {
                        user_code: start.user_code.into(),
                        verification_uri: start.verification_uri.into(),
                        status: "Waiting for authorization…".into(),
                    });
                    cx.notify();
                })
                .is_err()
            {
                return;
            }

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(interval))
                    .await;
                // Stop if the user dismissed the flow (e.g. navigated / signed out).
                if this
                    .update(cx, |this, _| this.services.github_device.is_none())
                    .unwrap_or(true)
                {
                    return;
                }
                let id = client_id.clone();
                let code = device_code.clone();
                let poll = crate::task::try_run(async move {
                    orrery_core::oauth::device_poll(&id, &code).await
                })
                .await;
                let status = match poll {
                    Ok(p) => p.status,
                    Err(e) => e,
                };
                match status.as_str() {
                    "authorized" => {
                        let _ = this.update(cx, |this, cx| {
                            this.services.github_device = None;
                            this.services.github_authed = true;
                            cx.notify();
                        });
                        return;
                    }
                    "authorization_pending" | "slow_down" => continue,
                    other => {
                        let msg = match other {
                            "expired_token" => "The code expired — try again.".to_string(),
                            "access_denied" => "Authorization was denied.".to_string(),
                            e => format!("Login failed: {e}"),
                        };
                        let _ = this.update(cx, |this, cx| {
                            if let Some(d) = &mut this.services.github_device {
                                d.status = msg.into();
                            }
                            cx.notify();
                        });
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// Forget the stored GitHub token.
    pub fn github_sign_out(&mut self, cx: &mut Context<Self>) {
        orrery_core::oauth::sign_out();
        self.services.github_device = None;
        self.services.github_authed = orrery_core::oauth::github_authed();
        cx.notify();
    }

    /// Resolve an "Open PR" flow's keyed progress toast to its outcome: a
    /// success toast linking to the new PR, or the error.
    pub(crate) fn finish_pr_toast(
        &mut self,
        key: String,
        title: String,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        // (The toast ids these return aren't needed here.)
        let _ = match result {
            Ok(url) => self.upsert_toast_link(
                key,
                ToastKind::Success,
                "PR opened",
                Some(title.into()),
                url.into(),
                cx,
            ),
            Err(e) => {
                self.upsert_toast(key, ToastKind::Error, "Open PR failed", Some(e.into()), cx)
            }
        };
    }
}
