//! Overlays layered over the shell — the repo drawer, the command palette
//! and the new-project dialog: the [`Overlay`] state, its routing to the
//! right renderer, and the new-project dialog's actions.

use gpui::{AppContext, Context, IntoElement, SharedString, Window};

use super::{OrreryApp, repaint_on_change};
use crate::theme::Theme;

/// A modal layered over the shell (drawer / palette / dialog). Rendered last in
/// `render`, above the active view; `Esc`/backdrop dismisses it.
pub enum Overlay {
    /// The repo detail drawer, keyed by repo id (stable across rescans), with
    /// the active tab.
    Drawer { repo: SharedString, tab: DrawerTab },
    /// The command palette (Ctrl+K).
    Palette(crate::palette::PaletteData),
    /// The new-project dialog (header "+").
    NewProject(crate::views::newproject::NewProjectData),
}

/// The RepoDrawer's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DrawerTab {
    Overview,
    Changes,
    Pr,
    Notes,
    Readme,
}

impl OrreryApp {
    /// Dismiss whatever overlay is open.
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    /// Open the new-project dialog (clone / init into a workspace root).
    pub fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::views::newproject::{NewMode, NewProjectData};
        use gpui_component::input::InputState;
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/owner/repo"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("repo"));
        let remote =
            cx.new(|cx| InputState::new(window, cx).placeholder("git@github.com:owner/repo.git"));
        let template = cx.new(|cx| InputState::new(window, cx).placeholder("~/templates/rust"));
        let subs = [&url, &name, &remote, &template]
            .into_iter()
            .map(|input| cx.subscribe(input, repaint_on_change))
            .collect();
        self.overlay = Some(Overlay::NewProject(NewProjectData {
            mode: NewMode::Clone,
            url,
            name,
            remote,
            template,
            first_commit: true,
            root: 0,
            status: "".into(),
            busy: false,
            _subs: subs,
        }));
        cx.notify();
    }

    /// Toggle the new-project "make initial commit" option.
    pub fn new_project_toggle_first_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(Overlay::NewProject(d)) = &mut self.overlay {
            d.first_commit = !d.first_commit;
        }
        cx.notify();
    }

    /// Switch the new-project dialog's mode (clone vs create).
    pub fn new_project_set_mode(
        &mut self,
        mode: crate::views::newproject::NewMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(Overlay::NewProject(d)) = &mut self.overlay {
            d.mode = mode;
            d.status = "".into();
        }
        cx.notify();
    }

    /// Cycle the new-project destination root.
    pub fn new_project_cycle_root(&mut self, cx: &mut Context<Self>) {
        let n = self.config.roots.len();
        if let Some(Overlay::NewProject(d)) = &mut self.overlay
            && n > 0
        {
            d.root = (d.root + 1) % n;
        }
        cx.notify();
    }

    /// Validate + run the new-project dialog (clone/init off the UI thread), then
    /// rescan and close on success.
    pub fn submit_new_project(&mut self, cx: &mut Context<Self>) {
        use crate::views::newproject::NewMode;
        let Some(Overlay::NewProject(d)) = &self.overlay else {
            return;
        };
        if d.busy {
            return;
        }
        let mode = d.mode;
        let name = d.name.read(cx).value().trim().to_string();
        let url = d.url.read(cx).value().trim().to_string();
        let remote = d.remote.read(cx).value().trim().to_string();
        let template = d.template.read(cx).value().trim().to_string();
        let first_commit = d.first_commit;
        let Some(root) = self.config.roots.get(d.root).cloned() else {
            self.set_new_project_status("Add a workspace root in Settings first.", cx);
            return;
        };
        if name.is_empty() {
            self.set_new_project_status("Enter a folder name.", cx);
            return;
        }
        if mode == NewMode::Clone && url.is_empty() {
            self.set_new_project_status("Enter a repository URL.", cx);
            return;
        }
        let dest = format!("{}/{}", root.trim_end_matches('/'), name);
        if let Some(Overlay::NewProject(d)) = &mut self.overlay {
            d.busy = true;
            d.status = if mode == NewMode::Clone {
                "Cloning…".into()
            } else {
                "Creating…".into()
            };
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match mode {
                        NewMode::Clone => orrery_core::git_ops::clone(&url, &dest),
                        NewMode::Create => orrery_core::git_ops::init(
                            &dest,
                            &name,
                            (!template.is_empty()).then_some(template.as_str()),
                            (!remote.is_empty()).then_some(remote.as_str()),
                            first_commit.then_some("Initial commit"),
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.close_overlay();
                    this.rescan(cx);
                }
                Err(e) => {
                    if let Some(Overlay::NewProject(d)) = &mut this.overlay {
                        d.busy = false;
                        d.status = format!("Failed: {e}").into();
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_new_project_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        if let Some(Overlay::NewProject(d)) = &mut self.overlay {
            d.status = msg.to_string().into();
        }
        cx.notify();
    }

    /// Build the active overlay's element, if one is open. Returns `None` when
    /// the drawer's repo has vanished (e.g. a rescan dropped it) — which also
    /// leaves the stale overlay to be cleared on the next interaction.
    pub(super) fn overlay_element(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        match &self.overlay {
            Some(Overlay::Drawer { repo, tab }) => {
                let row = self.rows.iter().find(|r| &r.id == repo)?;
                let cmds = (
                    self.config.ide_command.clone(),
                    self.config.agent_command.clone(),
                );
                Some(
                    crate::drawer::drawer(
                        row,
                        *tab,
                        t,
                        &cx.entity(),
                        &self.drawer,
                        &cmds.0,
                        &cmds.1,
                        self.services.ai_ready,
                        self.services.github_authed,
                    )
                    .into_any_element(),
                )
            }
            Some(Overlay::Palette(data)) => {
                let items = self.palette_items(cx);
                Some(
                    crate::palette::render(data, &items, &self.rows, t, &cx.entity())
                        .into_any_element(),
                )
            }
            Some(Overlay::NewProject(data)) => Some(
                crate::views::newproject::render(data, &self.config.roots, t, &cx.entity())
                    .into_any_element(),
            ),
            None => None,
        }
    }
}
