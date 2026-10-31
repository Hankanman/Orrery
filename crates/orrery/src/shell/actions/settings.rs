//! Settings: open an editing session over the config, edit its roots, and
//! save (then rescan).

use gpui::{Context, Window};

use crate::shell::OrreryApp;

impl OrreryApp {
    /// Start a settings editing session, seeding the field inputs from config,
    /// and kick off the live network checks (GitHub auth + AI reachability).
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = Some(crate::views::settings::SettingsState::new(
            &self.config,
            window,
            cx,
        ));
        self.refresh_github_authed(cx);
        self.ai_refresh(cx);
        self.load_index_stats(cx);
        cx.notify();
    }

    /// Append the typed root to the draft.
    pub fn settings_add_root(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.settings else { return };
        let val = s.add_root.read(cx).value().trim().to_string();
        if val.is_empty() {
            return;
        }
        if let Some(s) = &mut self.settings {
            s.draft.roots.push(val);
            s.saved = false;
        }
        cx.notify();
    }

    /// Read the field inputs into the draft, persist it, and rescan.
    pub fn settings_save(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.settings else { return };
        let mut draft = s.draft.clone();
        draft.ide_command = s.ide.read(cx).value().to_string();
        draft.agent_command = s.agent.read(cx).value().to_string();
        draft.agent_dispatch_args = s.agent_dispatch.read(cx).value().to_string();
        draft.ollama_host = s.ollama_host.read(cx).value().to_string();
        draft.ai_model = s.ai_model.read(cx).value().to_string();
        draft.embed_model = s.embed_model.read(cx).value().to_string();
        draft.llama_server_path = s.llama_server.read(cx).value().to_string();
        draft.github_client_id = s.client_id.read(cx).value().to_string();
        draft.ignore = s
            .ignore
            .read(cx)
            .value()
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        draft.scan_depth = s
            .scan_depth
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
            .unwrap_or(draft.scan_depth)
            .clamp(1, 8);

        self.config = draft.clone();
        let to_save = draft.clone();
        if let Some(s) = &mut self.settings {
            s.draft = draft;
            s.saved = true;
        }
        cx.notify();
        // Write the config off the UI thread, and rescan only once it has
        // landed — the rescan re-reads the roots from it.
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move { orrery_core::config::save(&to_save) })
                .await;
            let _ = this.update(cx, |this, cx| this.rescan(cx));
        })
        .detach();
    }
}
