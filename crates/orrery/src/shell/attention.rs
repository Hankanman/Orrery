//! The attention pipeline's app side: recompute the ranked "needs you" list
//! from what the app holds, then route it to the tray summary and to desktop
//! notifications for newly appeared urgent items (deduped by a persisted
//! seen-set).

use gpui::{Context, SharedString};
use orrery_core::attention::{AttentionItem, AttentionKind, Severity};
use orrery_core::model::AppConfig;

use super::OrreryApp;

/// Cache-meta key persisting the urgent-item keys already notified, so a
/// restart doesn't re-notify everything still pending. Distinct from the
/// platform notifier's `attention_seen` (whose keys are its own format).
const ATTENTION_SEEN_KEY: &str = "attention_urgent_seen";

/// Stable identity of an attention item for notification dedupe: kind + the
/// most specific repo key + summary. The same fact rendered from either inbox
/// source (background poll / Inbox view) produces the same key.
fn attention_key(item: &AttentionItem) -> String {
    let repo = item.repo.id.as_ref().cloned().unwrap_or_else(|| {
        format!(
            "{}/{}",
            item.repo.remote_host.as_deref().unwrap_or(""),
            item.repo.slug.as_deref().unwrap_or(&item.repo.name),
        )
    });
    format!("{:?}|{repo}|{}", item.kind, item.summary)
}

/// Does config allow desktop-notifying this urgent kind? Layered under the
/// `notify_enabled` + `notify_attention` master switches; the pre-existing
/// per-kind toggles keep their meaning now that these kinds notify from the
/// model instead of the platform poller.
fn urgent_kind_enabled(cfg: &AppConfig, kind: AttentionKind) -> bool {
    match kind {
        AttentionKind::ReviewRequested => cfg.notify_review_requested,
        AttentionKind::CiFailing => cfg.notify_ci_failure,
        _ => true,
    }
}

/// Max attention items surfaced in the tray menu.
const TRAY_TOP: usize = 3;

/// Fold the ranked attention list into the tray's compact summary: actionable
/// counts (Urgent + Attention — Info is ambient and stays off the tray) and
/// the top few lines. `items` is already severity-sorted, so the top lines
/// are the most urgent.
fn tray_summary(items: &[AttentionItem]) -> orrery_platform::tray::TrayAttention {
    let mut summary = orrery_platform::tray::TrayAttention::default();
    for item in items.iter().filter(|i| i.severity != Severity::Info) {
        summary.total += 1;
        if item.severity == Severity::Urgent {
            summary.urgent += 1;
        }
        if summary.top.len() < TRAY_TOP {
            summary
                .top
                .push(format!("{} · {}", item.repo.name, item.summary));
        }
    }
    summary
}

impl OrreryApp {
    /// Recompute the ranked attention list (and the per-repo severity lookup)
    /// from what the app already holds, then route it downstream: push the
    /// summary to the tray (if it changed) and desktop-notify urgent items
    /// that newly appeared. Cheap and foreground-safe — runs after each
    /// source update; no polling happens here (the notification send is a
    /// detached fire-and-forget task, and the tray push is a short
    /// cross-thread round-trip skipped when unchanged).
    ///
    /// Freshness follows source freshness: local git facts refresh with every
    /// (re)scan, inbox facts with each attention poll (and each Inbox load,
    /// which is fresher while it lasts), prunable counts with each Cleanup
    /// scan, agent sessions with the agents poll — and facts from a lazy view
    /// are simply absent until it has loaded. CI facts come from `ci_states`
    /// (cache-seeded at launch, refreshed by the central pass in
    /// [`Self::refresh_ci`]), restricted to remotes still in the fleet.
    pub fn recompute_attention(&mut self) {
        use orrery_core::attention::{self, AgentFact, PrunableFact};
        // Prefer the Inbox view's facts (they're the freshest the moment it
        // loads); fall back to the background poll's so the model — and the
        // tray/notifications behind it — works with the window never touched.
        let inbox: &[orrery_core::inbox::InboxItem] = match &self.inbox {
            crate::views::inbox::InboxState::Ready(d) => &d.raw,
            _ => self.polled_inbox.as_deref().unwrap_or(&[]),
        };
        let prunable: Vec<PrunableFact> = match &self.cleanup {
            crate::views::cleanup::CleanupState::Ready(repos) => repos
                .iter()
                .map(|r| PrunableFact {
                    repo_id: r.id.to_string(),
                    count: r.branches.len() as u32,
                })
                .collect(),
            _ => Vec::new(),
        };
        // Live sessions raise running facts; dispatched worktrees whose
        // session finished with commits to review (detected + persisted by
        // the agents scan, #185) raise finished facts against their origin
        // repo — see `views::agents::agent_facts` for the full mapping.
        let agents: Vec<AgentFact> = match &self.agents {
            crate::views::agents::AgentsState::Ready(data) => {
                crate::views::agents::agent_facts(data)
            }
            _ => Vec::new(),
        };
        let ci = orrery_core::ci::facts(&self.ci_states, &self.repos);
        self.attention_items = attention::compute(&self.repos, inbox, &ci, &prunable, &agents);
        self.derived.attention_gen += 1; // the Attention filter reads it
        // Items are severity-sorted (Urgent first), so a repo's first
        // occurrence is its highest severity.
        self.attention_by_repo.clear();
        for item in &self.attention_items {
            if let Some(id) = &item.repo.id {
                self.attention_by_repo
                    .entry(SharedString::from(id.clone()))
                    .or_insert(item.severity);
            }
        }
        self.push_tray_attention();
        self.notify_fresh_urgent();
    }

    /// Mirror the attention model onto the tray: actionable counts + the top
    /// items. Skips the cross-thread update when the summary hasn't changed.
    fn push_tray_attention(&mut self) {
        let summary = tray_summary(&self.attention_items);
        if summary == self.tray_attention {
            return;
        }
        if let Some(tray) = &self.tray {
            tray.set_attention(summary.clone());
        }
        self.tray_attention = summary;
    }

    /// Desktop-notify urgent attention items that newly appeared in this
    /// recompute, once per appearance: the key-set of already-surfaced items
    /// is kept on the app (and persisted, so a restart doesn't re-notify
    /// what's still pending — the same trick as the platform notifier's poll
    /// dedupe).
    fn notify_fresh_urgent(&mut self) {
        use std::collections::HashSet;
        // Urgent kinds derive from host facts: review requests from the
        // inbox, CI failures from `ci_states`. Until an inbox source has
        // produced — the background poll or the Inbox view — an empty urgent
        // set means "not loaded yet", not "all clear": diffing against it
        // would first persist an empty snapshot, then re-notify every
        // still-pending item on the next poll of every launch. CI facts
        // don't need their own gate: they're cache-seeded at launch (so any
        // still-failing repo is already in the persisted seen-set) and only
        // ever refreshed by the pass, never blanked (an `Err` keeps the last
        // known state, see `orrery_core::ci`).
        let inbox_loaded = self.polled_inbox.is_some()
            || matches!(self.inbox, crate::views::inbox::InboxState::Ready(_));
        if !inbox_loaded {
            return;
        }
        let current: HashSet<String> = self
            .attention_items
            .iter()
            .filter(|i| i.severity == Severity::Urgent)
            .map(attention_key)
            .collect();
        // First recompute with facts this session → seed from the persisted
        // snapshot, so only items that appeared since the last run notify.
        // No snapshot at all (first-ever run) → baseline silently, like the
        // platform notifier's first poll.
        // The persisted snapshot is read off the UI thread at launch
        // ([`Self::load_attention_seen`]); hold off until it has landed, so
        // a slow read can't be mistaken for a first-ever run.
        if !self.passes.attention_seeded {
            return;
        }
        let prev = self.attention_seen.take();
        if let Some(prev) = &prev
            && self.config.notify_enabled
            && self.config.notify_attention
        {
            for item in self.attention_items.iter().filter(|i| {
                i.severity == Severity::Urgent
                    && urgent_kind_enabled(&self.config, i.kind)
                    && !prev.contains(&attention_key(i))
            }) {
                let title = item.repo.name.clone();
                let body = item.summary.clone();
                crate::task::spawn_detached(async move {
                    let _ = orrery_platform::notify::send(&title, &body).await;
                });
            }
        }
        if prev.as_ref() != Some(&current)
            && let Ok(blob) = serde_json::to_string(&current)
        {
            crate::task::persist_meta(ATTENTION_SEEN_KEY, blob);
        }
        self.attention_seen = Some(current);
    }

    /// Launch: read the persisted already-notified set off the UI thread, then
    /// let the urgent-notification diff run (recomputing once, in case host
    /// facts arrived while it loaded).
    pub(super) fn load_attention_seen(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let prev: Option<std::collections::HashSet<String>> = cx
                .background_executor()
                .spawn(async {
                    orrery_core::cache::get_meta(ATTENTION_SEEN_KEY)
                        .and_then(|s| serde_json::from_str(&s).ok())
                })
                .await;
            let _ = this.update(cx, |this, _cx| {
                if this.attention_seen.is_none() {
                    this.attention_seen = prev;
                }
                this.passes.attention_seeded = true;
                this.recompute_attention();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::attention::RepoRef;

    fn item(kind: AttentionKind, name: &str, id: Option<&str>) -> AttentionItem {
        AttentionItem {
            repo: RepoRef {
                id: id.map(str::to_string),
                remote_host: Some("github.com".into()),
                slug: Some(format!("o/{name}")),
                name: name.into(),
            },
            kind,
            severity: kind.severity(),
            summary: format!("{kind:?} in {name}"),
            detail: None,
        }
    }

    #[test]
    fn tray_summary_counts_actionable_and_caps_top_lines() {
        // 2 urgent + 2 attention + 1 info → total 4, urgent 2, top capped at 3
        // (severity-ordered, like the model's output).
        let items = vec![
            item(AttentionKind::CiFailing, "a", Some("/a")),
            item(AttentionKind::ReviewRequested, "b", None),
            item(AttentionKind::DirtyWorktree, "c", Some("/c")),
            item(AttentionKind::Ahead, "d", Some("/d")),
            item(AttentionKind::PrunableBranches, "e", Some("/e")),
        ];
        let s = tray_summary(&items);
        assert_eq!((s.total, s.urgent), (4, 2));
        assert_eq!(
            s.top,
            vec![
                "a · CiFailing in a",
                "b · ReviewRequested in b",
                "c · DirtyWorktree in c",
            ]
        );

        let quiet = tray_summary(&[item(AttentionKind::AgentRunning, "x", Some("/x"))]);
        assert_eq!(quiet, orrery_platform::tray::TrayAttention::default());
    }

    #[test]
    fn attention_key_is_stable_and_repo_specific() {
        let a = item(AttentionKind::ReviewRequested, "a", Some("/a"));
        assert_eq!(attention_key(&a), attention_key(&a.clone()));
        // Same fact, different repo → different key. Local id wins; host+slug
        // is the fallback so unlinked host facts stay distinct across hosts.
        assert_ne!(
            attention_key(&a),
            attention_key(&item(AttentionKind::ReviewRequested, "a", Some("/b")))
        );
        let unlinked = item(AttentionKind::ReviewRequested, "a", None);
        assert!(attention_key(&unlinked).contains("github.com/o/a"));
    }

    #[test]
    fn urgent_kind_toggles_keep_their_meaning() {
        let mut cfg = AppConfig::default();
        assert!(urgent_kind_enabled(&cfg, AttentionKind::ReviewRequested));
        cfg.notify_review_requested = false;
        assert!(!urgent_kind_enabled(&cfg, AttentionKind::ReviewRequested));
        cfg.notify_ci_failure = false;
        assert!(!urgent_kind_enabled(&cfg, AttentionKind::CiFailing));
    }
}
