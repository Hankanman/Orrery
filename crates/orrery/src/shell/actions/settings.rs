//! Settings: open an editing session over the config, edit its roots, and
//! save (then rescan).

use gpui::{Context, Window};
use orrery_core::model::ThemePreference;

use crate::shell::OrreryApp;

impl OrreryApp {
    /// Start a settings editing session, seeding the field inputs from config,
    /// and kick off the live network checks (GitHub auth + AI reachability).
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = crate::views::settings::SettingsState::new(&self.config, window, cx);
        settings.system_motion = self.system_reduced_motion;
        self.settings = Some(settings);
        self.refresh_github_authed(cx);
        self.ai_refresh(cx);
        self.load_index_stats(cx);
        cx.notify();
    }

    /// Settings → Appearance → Theme. Unlike the other settings it applies
    /// and saves at once (you judge a theme by seeing it), and saves only this
    /// key: other unsaved edits in the draft stay drafts.
    pub fn set_theme_preference(&mut self, pref: ThemePreference, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.settings {
            s.draft.theme = pref;
        }
        if self.config.theme == pref {
            return;
        }
        self.config.theme = pref;
        self.apply_theme(cx);
        cx.notify();
        let to_save = self.config.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = orrery_core::config::save(&to_save) {
                    eprintln!("[settings] could not save the theme: {e}");
                }
            })
            .detach();
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
        draft.gitlab_hosts = parse_hosts(&s.gitlab_hosts.read(cx).value());
        draft.gitea_hosts = parse_hosts(&s.gitea_hosts.read(cx).value());
        let days = |input: &gpui::Entity<gpui_component::input::InputState>, old: u32| {
            input
                .read(cx)
                .value()
                .trim()
                .parse::<u32>()
                .unwrap_or(old)
                .min(365)
        };
        draft.stale_pr_days = days(&s.stale_pr_days, draft.stale_pr_days);
        draft.stale_review_days = days(&s.stale_review_days, draft.stale_review_days);

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

/// A comma-separated list of host domains, normalised: a pasted scheme,
/// path or trailing slash is dropped and the domain lower-cased, so
/// `https://Git.Acme.io/` is stored as `git.acme.io`. Duplicates and
/// blanks are skipped.
fn parse_hosts(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', ' ', '\n']) {
        let part = part.trim();
        let part = part
            .strip_prefix("https://")
            .or_else(|| part.strip_prefix("http://"))
            .unwrap_or(part);
        let domain = part.split('/').next().unwrap_or("").to_ascii_lowercase();
        if !domain.is_empty() && !out.contains(&domain) {
            out.push(domain);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_hosts;

    #[test]
    fn hosts_are_normalised_domains() {
        assert_eq!(
            parse_hosts(" https://Git.Acme.io/ , gitlab.corp.net,git.acme.io,, http://x.io/a/b"),
            vec!["git.acme.io", "gitlab.corp.net", "x.io"]
        );
        assert!(parse_hosts("  ").is_empty());
    }
}
