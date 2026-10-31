//! View loading: lazy first-open loads per view, the Inbox and Feed network
//! loads, the Dev Tools fields, and the views' contextual sub-filter.

use gpui::{AppContext, Context, SharedString, Window};

use crate::shell::{OrreryApp, View, repaint_on_change};

impl OrreryApp {
    /// Load the inbox (PRs / reviews / issues / notifications) over the network.
    pub fn load_inbox(&mut self, cx: &mut Context<Self>) {
        use crate::views::inbox::{InboxData, InboxState, inbox_row, notice_row};
        self.inbox = InboxState::Loading;
        // A refresh supersedes any load still in flight (its result is dropped).
        self.passes.inbox_seq += 1;
        let seq = self.passes.inbox_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let items =
                crate::task::try_run(async { orrery_core::inbox::github_inbox().await }).await;
            let notes =
                crate::task::try_run(async { orrery_core::inbox::github_notifications().await })
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.inbox_seq != seq {
                    return;
                }
                this.inbox = match items {
                    Ok(i) => InboxState::Ready(InboxData {
                        items: i.iter().cloned().map(inbox_row).collect(),
                        notifications: notes
                            .unwrap_or_default()
                            .into_iter()
                            .map(notice_row)
                            .collect(),
                        raw: i,
                    }),
                    Err(e) => InboxState::Error(e.into()),
                };
                // Fresh inbox facts → refresh the attention surfaces.
                this.recompute_attention();
                cx.notify();
            });
        })
        .detach();
    }

    /// Lazy-load a view's data the first time it's opened (Idle → Loading).
    pub(crate) fn maybe_load_view(
        &mut self,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::views;
        match view {
            View::Inbox if matches!(self.inbox, views::inbox::InboxState::Idle) => {
                self.load_inbox(cx)
            }
            View::Feed if matches!(self.feed, views::feed::FeedState::Idle) => self.load_feed(cx),
            View::Explore if matches!(self.explore, views::explore::ExploreState::Idle) => {
                self.load_starred(cx)
            }
            View::Janitor if matches!(self.cleanup, views::cleanup::CleanupState::Idle) => {
                self.load_cleanup(cx)
            }
            View::Agents => {
                if matches!(self.agents, views::agents::AgentsState::Idle) {
                    self.load_agents(cx);
                } else {
                    // Already loaded once — just (re)start the live poll.
                    self.start_agents_poll(cx);
                }
            }
            View::Settings if self.settings.is_none() => self.open_settings(window, cx),
            View::Tools if self.devtools.is_none() => self.open_devtools(window, cx),
            _ => {}
        }
    }

    /// Create the Dev Tools input fields + per-input observations (so each tool's
    /// output recomputes live as you type).
    fn open_devtools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::views::devtools::{DevToolsState, new_uuid};
        use gpui_component::input::InputState;
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tools…"));
        let base64 = cx.new(|cx| InputState::new(window, cx).placeholder("text"));
        let hash = cx.new(|cx| InputState::new(window, cx).placeholder("text"));
        let json = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("{ }")
        });
        let base_conv = cx.new(|cx| InputState::new(window, cx).placeholder("decimal number"));
        let case_conv = cx.new(|cx| InputState::new(window, cx).placeholder("text"));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("text"));
        let jwt = cx.new(|cx| InputState::new(window, cx).placeholder("eyJ… JWT"));
        let timestamp =
            cx.new(|cx| InputState::new(window, cx).placeholder("unix epoch or RFC 3339"));
        let colour = cx.new(|cx| InputState::new(window, cx).placeholder("#1f6feb or r,g,b"));
        let regex_pat = cx.new(|cx| InputState::new(window, cx).placeholder("pattern"));
        let regex_text = cx.new(|cx| InputState::new(window, cx).placeholder("text to match"));
        let mut subs = Vec::new();
        for input in [
            &search,
            &base64,
            &hash,
            &json,
            &base_conv,
            &case_conv,
            &url,
            &jwt,
            &timestamp,
            &colour,
            &regex_pat,
            &regex_text,
        ] {
            subs.push(cx.subscribe(input, repaint_on_change));
        }
        self.devtools = Some(DevToolsState {
            search,
            uuid: new_uuid(),
            base64,
            hash,
            json,
            base_conv,
            case_conv,
            url,
            jwt,
            timestamp,
            colour,
            regex_pat,
            regex_text,
            _subs: subs,
        });
        cx.notify();
    }

    /// Load the activity/release feed over the network.
    pub fn load_feed(&mut self, cx: &mut Context<Self>) {
        use crate::views::feed::{FeedState, feed_row};
        self.feed = FeedState::Loading;
        self.passes.feed_seq += 1;
        let seq = self.passes.feed_seq;
        cx.notify();
        let now = crate::data::now_unix();
        cx.spawn(async move |this, cx| {
            // Items newer than the last time the Feed was viewed are "new". Read
            // the mark (off the UI thread) before the load, then advance it to
            // now so the next visit compares against this one.
            let since = cx
                .background_executor()
                .spawn(async {
                    orrery_core::cache::get_meta("feed_seen_at")
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0)
                })
                .await;
            let res = crate::task::try_run(async { orrery_core::inbox::github_feed().await }).await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.feed_seq != seq {
                    return; // a refresh superseded this load
                }
                this.feed = match res {
                    Ok(items) => {
                        crate::task::persist_meta("feed_seen_at", now.to_string());
                        FeedState::Ready(
                            items.into_iter().map(|f| feed_row(f, now, since)).collect(),
                        )
                    }
                    Err(e) => FeedState::Error(e.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Set/toggle the current view's contextual sub-filter (Feed/Inbox panels).
    /// Passing the already-active key clears it; `None` clears unconditionally.
    pub fn set_view_filter(&mut self, key: Option<SharedString>, cx: &mut Context<Self>) {
        self.view_filter = if key.is_some() && key == self.view_filter {
            None
        } else {
            key
        };
        cx.notify();
    }
}
