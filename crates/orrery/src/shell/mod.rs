//! App shell — the chrome that wraps every view: the 52px header (brand,
//! roots·repos, search, new/rescan), the 236px left rail with the 8 primary nav
//! items, and the main column.
//!
//! The nav is live: clicking an item switches the active `View`; each view loads
//! its data lazily on first selection.

use std::rc::Rc;

use gpui::{
    AppContext, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px, rgb,
};
use gpui_component::input::{InputEvent, InputState};

use orrery_core::attention::{AttentionItem, AttentionKind, Severity};
use orrery_core::model::AppConfig;

use crate::data::Row;
use crate::icon::lucide;
use crate::theme::Theme;
use crate::toast::ToastKind;

mod attention;
mod grid;
mod passes;

use grid::{Facets, GridState, VisibleKey, columns};
use passes::{Passes, SnapKind};

/// Text-input subscription handler: repaint on edits only. Observing an
/// `InputState` instead would also fire on every cursor blink.
fn repaint_on_change(
    _this: &mut OrreryApp,
    _input: Entity<InputState>,
    ev: &InputEvent,
    cx: &mut Context<OrreryApp>,
) {
    if matches!(ev, InputEvent::Change) {
        cx.notify();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Grid,
    Inbox,
    Feed,
    Explore,
    Agents,
    Tools,
    Janitor,
    Settings,
}

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

/// (view, lucide-icon, label) — labels match the real sidebar (route ≠ label).
const NAV: [(View, &str, &str); 8] = [
    (View::Grid, "layout-grid", "Mission Control"),
    (View::Inbox, "inbox", "Inbox"),
    (View::Feed, "rss", "Feed"),
    (View::Explore, "compass", "Explore"),
    (View::Agents, "square-terminal", "Agents"),
    (View::Tools, "wrench", "Dev Tools"),
    (View::Janitor, "scissors", "Cleanup"),
    (View::Settings, "settings", "Settings"),
];

pub struct OrreryApp {
    pub view: View,
    pub rows: Vec<Row>,
    pub roots: usize,
    /// The raw core snapshot behind `rows` — the attention model's local-git
    /// input (host/slug/git facts the flat `Row` doesn't carry). Updated in
    /// lockstep with `rows` via [`Self::apply_snapshot`].
    pub repos: Vec<orrery_core::model::Repo>,
    pub theme: Rc<Theme>,
    pub config: AppConfig,
    /// Current attention glance lines (PRs/reviews/CI) from the background
    /// poller — the Inbox nav badge's fallback until the inbox itself loads.
    /// Empty until the first poll lands.
    pub attention: Vec<String>,
    /// The raw inbox facts from the background attention poll — the attention
    /// model's host input until the Inbox view loads its own (fresher) copy.
    /// `None` until the first poll lands, which is how
    /// [`Self::recompute_attention`] tells "no host facts yet" from "inbox
    /// genuinely empty".
    pub polled_inbox: Option<Vec<orrery_core::inbox::InboxItem>>,
    /// Cached CI states keyed (remote host domain, slug) — the attention
    /// model's CI input and the drawer Overview's CI line. Seeded from
    /// `ci_cache` at launch (instant, offline-tolerant) and reloaded after
    /// each central CI pass ([`Self::refresh_ci`]).
    pub ci_states: std::collections::HashMap<(String, String), orrery_core::cache::CiEntry>,
    /// The last whole-pass CI failure surfaced as a toast, so an ongoing
    /// outage (expired token polled every few minutes) toasts once per
    /// distinct error, not per pass.
    pub ci_last_error: Option<String>,
    /// The ranked "needs you" list from `orrery_core::attention::compute`
    /// (Urgent first). Recomputed on each source update — rescan, inbox load,
    /// cleanup scan, agents poll — never per frame.
    pub attention_items: Vec<AttentionItem>,
    /// Highest severity per local repo id, derived from `attention_items` in
    /// [`Self::recompute_attention`] — the grid Attention filter + card-dot
    /// lookup.
    pub attention_by_repo: std::collections::HashMap<SharedString, Severity>,
    /// Keys of the urgent attention items already surfaced as desktop
    /// notifications, so an item notifies once per appearance, not per
    /// recompute. `None` until the first recompute with host facts, which
    /// seeds it from the persisted snapshot (so a restart doesn't re-notify
    /// everything still pending).
    pub attention_seen: Option<std::collections::HashSet<String>>,
    /// The attention summary last pushed to the tray — recomputes skip the
    /// (cross-thread) tray round-trip when nothing changed.
    pub tray_attention: orrery_platform::tray::TrayAttention,
    /// The modal layered over the active view, if any (drawer/palette/dialog).
    pub overlay: Option<Overlay>,
    /// Async-loaded git data for the open drawer (branches/commits/worktrees).
    pub drawer: crate::drawer::DrawerData,
    /// Inbox view state (lazy, loaded when the nav item is first selected).
    pub inbox: crate::views::inbox::InboxState,
    /// Feed / Explore / Cleanup view state (lazy, loaded on first select).
    pub feed: crate::views::feed::FeedState,
    pub explore: crate::views::explore::ExploreState,
    pub cleanup: crate::views::cleanup::CleanupState,
    /// Repo id whose Cleanup "Prune" button is armed, awaiting a confirming
    /// second click before the (irreversible) bulk branch delete runs.
    pub cleanup_confirm: Option<SharedString>,
    /// Bumped each time a confirm is armed, so a stale revert timer from an
    /// earlier arm can't clear a newer one.
    pub cleanup_confirm_gen: u64,
    /// Agents view state (lazy; detected agent sessions on the machine).
    pub agents: crate::views::agents::AgentsState,
    /// Repo ids (paths) with a live agent session — drives the card indicator.
    /// Refreshed on rescan and by the Agents-view poll.
    pub active_agents: std::collections::HashSet<SharedString>,
    /// Whether the Agents-view poll loop is running (guards against duplicates).
    pub agents_polling: bool,
    /// Worktree path whose Agents "Remove worktree" button is armed, awaiting a
    /// confirming second click (the cleanup-confirm pattern). Discard arms the
    /// same slot with a `discard:`-prefixed key so the two destructive buttons
    /// can't confirm each other.
    pub agents_confirm: Option<SharedString>,
    /// Bumped each time a worktree-remove confirm is armed, so a stale revert
    /// timer from an earlier arm can't clear a newer one.
    pub agents_confirm_gen: u64,
    /// Worktree path whose finished-session review diff is expanded on the
    /// Agents view, if any (one at a time — the diff is the heavy part).
    pub agents_review: Option<SharedString>,
    /// The loaded review diff for [`Self::agents_review`]; `None` while the
    /// background branch-diff load is in flight.
    pub agents_review_diff: Option<SharedString>,
    /// Worktree paths with an Open-PR flow in flight (re-click guard).
    pub agents_pr_busy: std::collections::HashSet<SharedString>,
    /// Slugs currently being cloned from the Explore view.
    pub explore_cloning: std::collections::HashSet<SharedString>,
    /// Explore clone failures keyed by slug, shown on the card; cleared on retry.
    pub explore_errors: std::collections::HashMap<SharedString, SharedString>,
    /// Settings editing session (draft config + field inputs); created on first
    /// open, kept so edits survive navigating away.
    pub settings: Option<crate::views::settings::SettingsState>,
    /// Dev Tools fields (created on first open).
    pub devtools: Option<crate::views::devtools::DevToolsState>,
    /// External-service status: GitHub auth + AI backend reachability.
    pub services: Services,
    /// Whether the system tray came up — gates close-to-tray.
    pub tray_active: bool,
    /// Handle to the live tray (when it came up) — the attention summary is
    /// pushed through it after each [`Self::recompute_attention`]. Updates are
    /// a short synchronous round-trip to the tray thread.
    pub tray: Option<orrery_platform::tray::TrayHandle>,
    /// Handle to the fs-watcher thread — re-armed when repos/roots are added at
    /// runtime (Settings save, New Project, Explore clone) so new paths get
    /// live change events without a restart.
    pub watcher: orrery_platform::watcher::WatcherHandle,
    /// Fleet multi-selection: repo ids picked via card checkbox / Ctrl+click.
    /// Keyed by id so the selection survives filter changes; pruned to
    /// existing repos on rescan. Drives the fleet bar (see `fleet.rs`).
    pub selected: std::collections::HashSet<SharedString>,
    /// The in-flight fleet bulk run, if any — one at a time; carries the
    /// engine's cancel flag + the live done/total counter.
    pub fleet_run: Option<crate::fleet::FleetRun>,
    /// Monotonic fleet-run id source — guards stale progress events.
    pub fleet_seq: u64,
    /// The pending bulk-prune confirm (the fleet bar's confirm strip), if any.
    /// Dropped on selection changes / Esc / Cancel / a run starting, so a
    /// stale plan can never execute (see `fleet.rs`).
    pub fleet_prune: Option<crate::fleet::PrunePlan>,
    /// Bumped each time a prune scan starts, so a stale scan result from a
    /// cancelled/superseded confirm can't resurrect the strip.
    pub fleet_prune_seq: u64,
    /// Active toasts, oldest first (rendered bottom-right by
    /// `toast::toast_layer`; see `toast.rs` for the lifecycle).
    pub toasts: Vec<crate::toast::Toast>,
    /// Monotonic toast-id source — unique ids double as the stale-timer guard.
    pub toast_seq: u64,
    /// Mission Control's UI state (filters, sort, layout, saved views, graph).
    pub grid: GridState,
    /// Single-flight guards for the background passes (see [`Passes`]).
    pub passes: Passes,
    /// Render-path caches derived from `rows` (see [`Derived`]).
    pub derived: Derived,
    /// When the roots were last scanned from disk (Unix secs), for the
    /// sidebar footer; `None` if never recorded.
    pub scanned_at: Option<i64>,
    /// The active contextual sub-filter for the current non-Grid view (e.g. the
    /// Feed/Inbox category panels). Ephemeral: reset when the view changes so
    /// filters don't bleed across views.
    pub view_filter: Option<SharedString>,
    /// App-root focus handle, so global key bindings (Esc) dispatch here.
    pub focus: FocusHandle,
}

/// Status of the external integrations Orrery talks to — GitHub (auth) and the
/// local AI backend — grouped out of [`OrreryApp`]. Surfaced in Settings;
/// `ai_ready`/`github_authed` also gate affordances app-wide.
#[derive(Default)]
pub struct Services {
    /// Whether a GitHub token is currently resolvable (Settings account panel).
    pub github_authed: bool,
    /// An in-progress GitHub device-flow login, if any.
    pub github_device: Option<crate::views::settings::GithubDevice>,
    /// Live AI-backend reachability + model list (Settings AI panel).
    pub ai_status: crate::views::settings::AiStatus,
    /// AI is enabled and reachable — gates semantic search + AI affordances.
    pub ai_ready: bool,
}

/// Render-path caches: values derived from `rows` (and the grid filters) that
/// used to be recomputed on every frame. Each entry is keyed by the
/// generation counters below, so it recomputes lazily — on the first render
/// after its inputs change — and never goes stale. Interior mutability lets
/// the `&self` render path fill them; GPUI renders on one thread.
#[derive(Default)]
pub struct Derived {
    /// Bumped whenever `rows` changes (snapshot applied, favorite toggled).
    rows_gen: u64,
    /// Bumped whenever the attention model is recomputed.
    attention_gen: u64,
    visible: std::cell::RefCell<Option<(VisibleKey, Rc<Vec<usize>>)>>,
    facets: std::cell::RefCell<Option<(u64, Rc<Facets>)>>,
    slugs: std::cell::RefCell<Option<(u64, Rc<std::collections::HashSet<SharedString>>)>>,
}

impl Derived {
    pub fn rows_changed(&mut self) {
        self.rows_gen += 1;
    }

    pub fn rows_gen(&self) -> u64 {
        self.rows_gen
    }
}

/// Recent commit subject lines for a repo (newest first) — the input for the
/// AI changelog / resume prompts. Empty on any git error.
fn recent_summaries(id: &str, limit: usize) -> Vec<String> {
    orrery_core::git_ops::recent_log(id, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

/// The shared tail of both "Open PR" flows (drawer branch, agent worktree),
/// run once `head` is pushed: read the commit range `dir` has over `base`,
/// draft the title/body from it — AI-drafted when `ai_ready`, else the
/// fallback; only the drafting is aiReady-gated, never the button — then
/// create the PR on `slug`. Returns the title and the PR URL (or the error).
async fn draft_and_create_pr(
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

/// The error toast (title, detail) for a branch-prune outcome, if anything
/// went wrong: the whole prune failing, or individual branches that couldn't
/// be deleted. `None` when every prunable branch went.
fn prune_failure(
    report: &Result<orrery_core::git_ops::PruneReport, String>,
) -> Option<(String, String)> {
    match report {
        Err(e) => Some(("Prune failed".into(), crate::data::oneline(e.clone()))),
        Ok(r) if r.failed.is_empty() => None,
        Ok(r) => {
            let n = r.failed.len();
            let title = format!(
                "Couldn't delete {n} branch{}",
                if n == 1 { "" } else { "es" }
            );
            let detail = r
                .failed
                .iter()
                .map(|(branch, err)| format!("{branch}: {err}"))
                .collect::<Vec<_>>()
                .join("; ");
            Some((title, crate::data::oneline(detail)))
        }
    }
}

impl OrreryApp {
    /// Toggle a repo's favorite (card star): update the row now, persist the
    /// flag off the UI thread.
    pub fn set_favorite(&mut self, idx: usize, next: bool, cx: &mut Context<Self>) {
        let Some(r) = self.rows.get_mut(idx) else {
            return;
        };
        r.favorite = next;
        let id = r.id.to_string();
        self.derived.rows_changed(); // the Starred filter reads it
        cx.background_executor()
            .spawn(async move {
                let _ = orrery_core::cache::set_favorite(&id, next);
            })
            .detach();
        cx.notify();
    }

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

    /// Dismiss whatever overlay is open.
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    /// Open the command palette and focus its query field.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder("Search repos, run a command…")
        });
        // On each edit: reset the selection, kick off a (debounced) code
        // search, and re-render. Subscribe to `Change` rather than observing
        // the input — it notifies on every cursor blink too, which would
        // reset the selection and re-run the searches twice a second.
        let sub = cx.subscribe(&query, |this, _q, ev: &InputEvent, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }
            if let Some(Overlay::Palette(d)) = &mut this.overlay {
                d.selected = 0;
                d.rev += 1;
            }
            this.trigger_code_search(cx);
            this.trigger_semantic_search(cx);
            cx.notify();
        });
        let fh = query.read(cx).focus_handle(cx);
        self.overlay = Some(Overlay::Palette(crate::palette::PaletteData {
            query,
            selected: 0,
            code: Vec::new(),
            semantic: Vec::new(),
            embeddings: None,
            query_vecs: std::collections::HashMap::new(),
            generation: 0,
            rev: 0,
            items: Default::default(),
            _sub: sub,
        }));
        window.focus(&fh, cx);
        // Load the embedding index once per palette session (a per-keystroke
        // load would dominate recall latency). Gated on AI: with it off the
        // index stays `None` and the palette is name-matching only.
        if self.services.ai_ready {
            cx.spawn(async move |this, cx| {
                let index = cx
                    .background_executor()
                    .spawn(async { orrery_core::semantic::all_embeddings() })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match &mut this.overlay {
                        Some(Overlay::Palette(d)) => {
                            d.embeddings = Some(std::sync::Arc::new(index))
                        }
                        _ => return, // palette closed while loading
                    }
                    // Rank whatever was typed while the index loaded.
                    this.trigger_semantic_search(cx);
                });
            })
            .detach();
        }
        cx.notify();
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

    /// The current palette result list (actions + repos + code hits). Cached
    /// on the palette and rebuilt only when its inputs change — the query or
    /// search results (`rev`), the rows, or whether a selection exists — so
    /// render, arrow keys, and Enter share one list.
    fn palette_items(&self, cx: &Context<Self>) -> Rc<Vec<crate::palette::PaletteItem>> {
        let Some(Overlay::Palette(d)) = &self.overlay else {
            return Rc::default();
        };
        let key = (d.rev, self.derived.rows_gen(), !self.selected.is_empty());
        if let Some((k, items)) = &*d.items.borrow()
            && *k == key
        {
            return items.clone();
        }
        let items = Rc::new(crate::palette::items(
            &self.rows,
            &d.code,
            &d.semantic,
            &d.query.read(cx).value(),
            key.2,
        ));
        *d.items.borrow_mut() = Some((key, items.clone()));
        items
    }

    /// Move the palette selection by `delta` (wrapping), if it's open.
    fn move_palette(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.palette_items(cx).len();
        if let Some(Overlay::Palette(d)) = &mut self.overlay
            && len > 0
        {
            let i = d.selected as isize + delta;
            d.selected = i.rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    /// Execute the currently-selected palette item (called on Enter).
    fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items(cx);
        if items.is_empty() {
            return;
        }
        let selected = match &self.overlay {
            Some(Overlay::Palette(d)) => d.selected.min(items.len() - 1),
            _ => return,
        };
        if let Some(item) = items.get(selected).cloned() {
            self.run_palette_item(item, cx);
            window.focus(&self.focus, cx);
        }
    }

    /// Debounced cross-repo code search for the current query.
    fn trigger_code_search(&mut self, cx: &mut Context<Self>) {
        let (query, generation) = match &mut self.overlay {
            Some(Overlay::Palette(d)) => {
                d.generation += 1;
                (d.query.read(cx).value().to_string(), d.generation)
            }
            _ => return,
        };
        let paths: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
        cx.spawn(async move |this, cx| {
            // Debounce keystrokes before doing the (expensive) ripgrep pass.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            // Bail if a newer keystroke superseded this search.
            let current = this
                .update(
                    cx,
                    |this, _| matches!(&this.overlay, Some(Overlay::Palette(d)) if d.generation == generation),
                )
                .unwrap_or(false);
            if !current {
                return;
            }
            let results = if query.trim().len() >= 2 {
                cx.background_executor()
                    .spawn(async move {
                        orrery_core::search::search(&query, &paths, 60).unwrap_or_default()
                    })
                    .await
            } else {
                Vec::new()
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(Overlay::Palette(d)) = &mut this.overlay
                    && d.generation == generation {
                        d.code = results.into_iter().map(crate::palette::code_hit).collect();
                        d.rev += 1;
                        cx.notify();
                    }
            });
        })
        .detach();
    }

    /// Debounced semantic recall for the current palette query: embed the
    /// query, rank it against the session-cached embedding index, and surface
    /// the best repo per match with its snippet. Gated on AI being ready (and
    /// the backend supporting embeddings); reuses the code-search generation
    /// for stale-drop. The query embedding is cached per palette session, so
    /// only genuinely new queries hit the backend — and never before the
    /// debounce window passes.
    fn trigger_semantic_search(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready || !orrery_core::ai::embeddings_supported() {
            return;
        }
        let (query, generation) = match &self.overlay {
            Some(Overlay::Palette(d)) => {
                (d.query.read(cx).value().trim().to_string(), d.generation)
            }
            _ => return,
        };
        if query.len() < 2 {
            if let Some(Overlay::Palette(d)) = &mut self.overlay {
                d.semantic.clear();
                d.rev += 1;
            }
            return;
        }
        // Session-cached query vector → rank right away, no debounce/backend.
        if let Some(Overlay::Palette(d)) = &self.overlay
            && let (Some(vec), Some(index)) =
                (d.query_vecs.get(&query).cloned(), d.embeddings.clone())
        {
            let hits = self.semantic_rank(&vec, &index);
            if let Some(Overlay::Palette(d)) = &mut self.overlay {
                d.semantic = hits;
                d.rev += 1;
            }
            cx.notify();
            return;
        }
        let model = self.config.embed_model.clone();
        cx.spawn(async move |this, cx| {
            // Debounce keystrokes before the (slow) embedding round-trip.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(260))
                .await;
            // Bail if a newer keystroke superseded this search.
            let current = this
                .update(cx, |this, _| {
                    matches!(&this.overlay, Some(Overlay::Palette(d)) if d.generation == generation)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let q = query.clone();
            let vec =
                crate::task::try_run(async move { orrery_core::ai::embed(&model, &q).await }).await;
            let _ = this.update(cx, |this, cx| {
                let Ok(vec) = vec else {
                    // Backend unreachable — recall just finds nothing.
                    if let Some(Overlay::Palette(d)) = &mut this.overlay
                        && d.generation == generation
                    {
                        d.semantic.clear();
                        d.rev += 1;
                        cx.notify();
                    }
                    return;
                };
                let vec = std::sync::Arc::new(vec);
                let index = match &mut this.overlay {
                    Some(Overlay::Palette(d)) => {
                        // Cache even when stale — a re-typed query reuses it.
                        d.query_vecs.insert(query, vec.clone());
                        d.embeddings.clone()
                    }
                    _ => return,
                };
                // Index still loading → its completion re-triggers ranking.
                let Some(index) = index else { return };
                let hits = this.semantic_rank(&vec, &index);
                if let Some(Overlay::Palette(d)) = &mut this.overlay
                    && d.generation == generation
                {
                    d.semantic = hits;
                    d.rev += 1;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Rank the loaded embedding index against a query vector and resolve the
    /// hits to repo rows: best chunk per repo (core `recall`), then
    /// (host, slug) store keys mapped back to repo ids for the palette.
    fn semantic_rank(
        &self,
        query: &[f32],
        index: &[orrery_core::semantic::EmbeddingRow],
    ) -> Vec<crate::palette::SemanticHit> {
        use orrery_core::semantic;
        let by_key: std::collections::HashMap<(String, String), &str> = self
            .repos
            .iter()
            .map(|r| (semantic::repo_key(r), r.id.as_str()))
            .collect();
        semantic::recall(query, index, semantic::MAX_HITS, semantic::MIN_SCORE)
            .into_iter()
            .filter_map(|h| {
                let id = by_key.get(&(h.host.clone(), h.slug.clone()))?;
                let snippet: String = crate::data::oneline(h.content).chars().take(120).collect();
                Some(crate::palette::SemanticHit {
                    id: SharedString::from(id.to_string()),
                    snippet: snippet.into(),
                })
            })
            .collect()
    }

    /// Close the palette and act on `item`.
    pub fn run_palette_item(&mut self, item: crate::palette::PaletteItem, cx: &mut Context<Self>) {
        use crate::palette::{PaletteAction, PaletteItem};
        // Resolve data living in the (about-to-close) palette first.
        let code_abs = match (&item, &self.overlay) {
            (PaletteItem::Code(i), Some(Overlay::Palette(d))) => {
                d.code.get(*i).map(|h| h.abs.to_string())
            }
            _ => None,
        };
        self.close_overlay();
        match item {
            PaletteItem::Action(PaletteAction::Rescan) => self.rescan(cx),
            PaletteItem::Action(PaletteAction::Settings) => self.view = View::Settings,
            // Fleet verbs (#184) — the same run plumbing as the fleet bar
            // (`fleet.rs`); one bulk run at a time is enforced there.
            PaletteItem::Action(PaletteAction::FetchAll) => {
                let repos: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
                self.run_fleet_repos(crate::fleet::FleetOp::Fetch, repos, cx);
            }
            PaletteItem::Action(PaletteAction::PullBehind) => {
                let repos: Vec<String> = self
                    .rows
                    .iter()
                    .filter(|r| r.behind > 0)
                    .map(|r| r.id.to_string())
                    .collect();
                if repos.is_empty() {
                    self.push_toast(
                        crate::toast::ToastKind::Info,
                        "Nothing behind",
                        Some("Every repo is up to date with its upstream.".into()),
                        cx,
                    );
                } else {
                    self.run_fleet_repos(crate::fleet::FleetOp::Pull, repos, cx);
                }
            }
            PaletteItem::Action(PaletteAction::FetchSelected) => {
                self.run_fleet(crate::fleet::FleetOp::Fetch, cx)
            }
            PaletteItem::Action(PaletteAction::PullSelected) => {
                self.run_fleet(crate::fleet::FleetOp::Pull, cx)
            }
            PaletteItem::Action(PaletteAction::SelectDirty) => {
                self.select_where(|r| r.dirty > 0, "dirty", cx)
            }
            PaletteItem::Action(PaletteAction::SelectBehind) => {
                self.select_where(|r| r.behind > 0, "behind their upstream", cx)
            }
            PaletteItem::Repo(i) | PaletteItem::Recall { row: i, .. } => {
                if let Some(r) = self.rows.get(i) {
                    let _ = orrery_core::launch::launch(&self.config.ide_command, &r.id);
                }
            }
            PaletteItem::Code(_) => {
                if let Some(abs) = code_abs {
                    let _ = orrery_core::launch::launch(&self.config.ide_command, &abs);
                }
            }
        }
        cx.notify();
    }

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
    fn maybe_load_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Start a settings editing session, seeding the field inputs from config,
    /// and kick off the live network checks (GitHub auth + AI reachability).
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Re-resolve whether a GitHub token is available (may shell out to `gh`, so
    /// off the UI thread).
    fn refresh_github_authed(&mut self, cx: &mut Context<Self>) {
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

    /// Re-check AI-backend reachability and list installed models.
    pub fn ai_refresh(&mut self, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if matches!(self.services.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.services.ai_status = AiStatus::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available())
                .await
                .unwrap_or(false);
            let status = if up {
                let models = crate::task::run(orrery_core::ai::installed_models())
                    .await
                    .unwrap_or_default();
                AiStatus::Ready(
                    models
                        .into_iter()
                        .map(|(n, sz)| (n.into(), crate::data::human_bytes(sz).into()))
                        .collect(),
                )
            } else {
                AiStatus::Offline
            };
            let _ = this.update(cx, |this, cx| {
                let ready = up && this.config.ai_enabled;
                this.services.ai_status = status;
                this.services.ai_ready = ready;
                // Reachable now → (re)build the semantic index so the palette can
                // search by meaning.
                if ready {
                    this.index_semantic(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Probe the AI backend with a tiny round-trip and report ok/latency in the
    /// Settings AI note. A quick way to confirm the model actually responds.
    pub fn ai_test(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.settings {
            s.ai_note = "Testing…".into();
        }
        cx.notify();
        let model = self.config.ai_model.clone();
        cx.spawn(async move |this, cx| {
            let note = crate::task::run(async move {
                let start = std::time::Instant::now();
                match orrery_core::ai::generate(&model, "Reply with the single word: pong.").await {
                    Ok(_) => format!("AI responded in {} ms", start.elapsed().as_millis()),
                    Err(e) => format!("AI test failed: {e}"),
                }
            })
            .await
            .unwrap_or_else(|| "AI test failed unexpectedly".into());
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Clear cached AI summaries + embeddings, reporting the counts in the
    /// Settings AI note. Frees the semantic index and stale summaries.
    pub fn ai_clear_cache(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { orrery_core::cache::clear_ai() })
                .await;
            let note = match result {
                Ok((summaries, embeddings)) => {
                    format!("Cleared {summaries} summaries, {embeddings} embeddings")
                }
                Err(e) => format!("Clear failed: {e}"),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                this.load_index_stats(cx);
                // Reload the rows so the cleared summaries leave the cards and
                // drawer (the reload re-derives summaries from the AI cache).
                this.request_snapshot(SnapKind::Reload, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh the semantic-index size line (chunks/repos/bytes) shown in the
    /// Settings AI section.
    pub fn load_index_stats(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_executor()
                .spawn(async { orrery_core::semantic::index_stats() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.index_stats = Some(stats);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Settings: drop the whole semantic index (rows + skip signatures) and
    /// re-embed the corpus from scratch, reporting in the AI note. Gated on
    /// `aiReady` like every AI affordance.
    pub fn rebuild_semantic_index(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        if let Some(s) = &mut self.settings {
            s.ai_note = "Rebuilding index…".into();
        }
        cx.notify();
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_executor()
                .spawn(async { orrery_core::semantic::clear_index() })
                .await;
            let note = match cleared {
                Ok(_) => {
                    let n =
                        crate::task::run(
                            async move { orrery_core::semantic::index_fleet(repos).await },
                        )
                        .await
                        .unwrap_or(0);
                    format!(
                        "Rebuilt index — embedded {n} repo{}",
                        if n == 1 { "" } else { "s" }
                    )
                }
                Err(e) => format!("Rebuild failed: {e}"),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                this.load_index_stats(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Download a GGUF model for the llama.cpp backend, streaming progress into
    /// the Settings AI note. The file lands in the app-data models dir; on
    /// success the model list refreshes so it can be selected. A non-empty
    /// `sha256` is verified before the file is installed.
    pub fn llama_download(&mut self, url: String, sha256: String, cx: &mut Context<Self>) {
        let url = url.trim().to_string();
        let sha256 = Some(sha256.trim().to_string()).filter(|s| !s.is_empty());
        let note = |this: &mut Self, msg: SharedString| {
            if let Some(s) = &mut this.settings {
                s.ai_note = msg;
            }
        };
        if url.is_empty() {
            note(self, "Enter a model URL to download.".into());
            cx.notify();
            return;
        }
        note(self, "Starting download…".into());
        cx.notify();

        // Progress messages flow from the (core-runtime) download callback to the
        // UI over a channel — the live-wiring pattern, since the callback can't
        // touch the app entity directly.
        let (tx, rx) = async_channel::unbounded::<SharedString>();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some(s) = &mut this.settings {
                            s.ai_note = msg;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let tx_progress = tx.clone();
            let result = crate::task::try_run(async move {
                let mut last_pct = u64::MAX;
                orrery_core::llama::download_model_verified(
                    &url,
                    sha256.as_deref(),
                    move |downloaded, total| {
                        let msg = match (downloaded * 100).checked_div(total) {
                            Some(pct) => {
                                if pct == last_pct {
                                    return; // throttle to whole-percent steps
                                }
                                last_pct = pct;
                                format!(
                                    "Downloading… {pct}% ({} / {})",
                                    crate::data::human_bytes(downloaded),
                                    crate::data::human_bytes(total)
                                )
                            }
                            None => {
                                format!("Downloading… {}", crate::data::human_bytes(downloaded))
                            }
                        };
                        let _ = tx_progress.try_send(msg.into());
                    },
                )
                .await
            })
            .await;
            let final_msg = match result {
                Ok(path) => format!("Downloaded {}", path.rsplit('/').next().unwrap_or(&path)),
                Err(e) => format!("Download failed: {e}"),
            };
            let _ = tx.send(final_msg.into()).await;
            drop(tx); // close the channel so the drain task ends
            // Pick up the new model in the installed list.
            let _ = this.update(cx, |this, cx| this.ai_refresh(cx));
        })
        .detach();
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

    /// Resolve an "Open PR" flow's keyed progress toast to its outcome: a
    /// success toast linking to the new PR, or the error.
    fn finish_pr_toast(
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

    /// One-shot at launch: if AI is enabled and reachable, mark it ready and
    /// kick off the semantic index (so Ctrl+K works without opening Settings).
    pub fn ai_startup(&mut self, cx: &mut Context<Self>) {
        if !self.config.ai_enabled {
            return;
        }
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available())
                .await
                .unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.services.ai_ready = up;
                if up {
                    this.index_semantic(cx);
                }
                // AI affordances (summaries, Summarize) appear with readiness.
                cx.notify();
            });
        })
        .detach();
    }

    /// (Re)index the semantic corpus (readme/description/topics/notes/commits
    /// per repo) from the current snapshot, on the shared tokio runtime.
    /// Incremental and self-pacing: unchanged (repo, source) pairs skip via
    /// signatures and core throttles the embed bursts, so post-rescan calls
    /// are cheap. Gated on `aiReady` — no model, no indexing, no errors.
    /// Single-flight: a request mid-pass reruns once afterwards, on the
    /// then-current snapshot.
    pub fn index_semantic(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready || !self.passes.index.begin() {
            return;
        }
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let _ = crate::task::run(orrery_core::semantic::index_fleet(repos)).await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.index.finish() {
                    this.index_semantic(cx);
                }
            });
        })
        .detach();
    }

    /// Pull (download) a model on the AI backend, then refresh the status.
    pub fn ai_pull(&mut self, model: String, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if model.trim().is_empty() || matches!(self.services.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.services.ai_status = AiStatus::Pulling(format!("{model} · starting…").into());
        cx.notify();

        // The pull runs on the tokio runtime and streams (status, done, total)
        // back over a channel; a gpui task drains it to update the live %. When
        // the pull finishes the sender drops, closing the channel — our cue to
        // refresh the model list. (The one-shot `task::run` can't stream, hence
        // the detached spawn + channel.)
        let (tx, rx) = async_channel::unbounded::<(String, u64, u64)>();
        let m = model.clone();
        crate::task::spawn_detached(async move {
            let mut last_pct = u64::MAX;
            let _ = orrery_core::ai::pull(&m, |status, done, total| {
                // Throttle to ~1% steps (and every status-only update, total==0).
                match (done * 100).checked_div(total) {
                    Some(pct) if pct == last_pct => {}
                    pct => {
                        last_pct = pct.unwrap_or(u64::MAX);
                        let _ = tx.try_send((status.to_string(), done, total));
                    }
                }
            })
            .await;
        });

        cx.spawn(async move |this, cx| {
            while let Ok((status, done, total)) = rx.recv().await {
                let label = match (done * 100).checked_div(total) {
                    Some(pct) => format!("{model} · {pct}%"),
                    None => format!("{model} · {status}"),
                };
                if this
                    .update(cx, |this, cx| {
                        this.services.ai_status = AiStatus::Pulling(label.into());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            // Channel closed → pull finished. Drop Pulling so the refresh isn't
            // short-circuited, then re-list models.
            let _ = this.update(cx, |this, cx| {
                this.services.ai_status = AiStatus::Unknown;
                this.ai_refresh(cx);
            });
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

    /// Load the starred-repo browser over the network.
    pub fn load_starred(&mut self, cx: &mut Context<Self>) {
        use crate::views::explore::{ExploreState, star_row};
        self.explore = ExploreState::Loading;
        self.passes.explore_seq += 1;
        let seq = self.passes.explore_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res =
                crate::task::try_run(async { orrery_core::inbox::github_starred().await }).await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.explore_seq != seq {
                    return; // a refresh superseded this load
                }
                this.explore = match res {
                    Ok(repos) => ExploreState::Ready(repos.into_iter().map(star_row).collect()),
                    Err(e) => ExploreState::Error(e.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Clone a starred repo into the first root, then rescan so it appears.
    pub fn clone_starred(
        &mut self,
        slug: SharedString,
        clone_url: SharedString,
        name: SharedString,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.config.roots.first().cloned() else {
            return;
        };
        self.explore_cloning.insert(slug.clone());
        self.explore_errors.remove(&slug);
        // A keyed Progress toast tracks the clone; the completion below upserts
        // the same key to Success/Error, so the op resolves its own toast.
        let toast_key = SharedString::from(format!("clone:{slug}"));
        self.upsert_toast(
            toast_key.clone(),
            crate::toast::ToastKind::Progress,
            format!("Cloning {slug}…"),
            None,
            cx,
        );
        let dest = orrery_core::scan::expand(&root)
            .join(name.as_ref())
            .to_string_lossy()
            .into_owned();
        let url = clone_url.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if std::path::Path::new(&dest).exists() {
                        Ok(())
                    } else {
                        orrery_core::git_ops::clone(&url, &dest).map(|_| ())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                use crate::toast::ToastKind;
                match result {
                    Ok(()) => {
                        this.upsert_toast(
                            toast_key,
                            ToastKind::Success,
                            format!("Cloned {slug}"),
                            None,
                            cx,
                        );
                        // Keep the card on "cloning" until a rescan has picked
                        // the repo up (then it shows as cloned), and watch the
                        // freshly cloned repo for live changes too.
                        this.passes.snapshot.settle_clones.push(slug.clone());
                        this.rescan(cx);
                    }
                    Err(e) => {
                        this.explore_cloning.remove(&slug);
                        let msg = SharedString::from(format!("Failed: {e}"));
                        // Keep the inline card error AND raise a toast, so the
                        // failure is visible even away from the Explore view.
                        this.explore_errors.insert(slug.clone(), msg.clone());
                        this.upsert_toast(
                            toast_key,
                            ToastKind::Error,
                            format!("Clone failed: {slug}"),
                            Some(msg),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scan all repos for prunable branches (sync git, off-thread).
    pub fn load_cleanup(&mut self, cx: &mut Context<Self>) {
        use crate::views::cleanup::CleanupState;
        self.cleanup = CleanupState::Loading;
        self.cleanup_confirm = None;
        cx.notify();
        let rows = self.rows.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = CleanupState::Ready(result);
                // Fresh prunable counts → refresh the attention surfaces.
                this.recompute_attention();
                cx.notify();
            });
        })
        .detach();
    }

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
    fn scan_agents(&mut self, loading: bool, cx: &mut Context<Self>) {
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
                if let Some(open) = &this.agents_review
                    && !result.dispatched.iter().any(|d| d.worktree_path == *open)
                {
                    this.agents_review = None;
                    this.agents_review_diff = None;
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
    fn start_agents_poll(&mut self, cx: &mut Context<Self>) {
        if self.agents_polling {
            return;
        }
        self.agents_polling = true;
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
                    let _ = this.update(cx, |this, _| this.agents_polling = false);
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

    /// Toggle the drawer's "fresh worktree" dispatch option.
    pub fn toggle_dispatch_fresh(&mut self, cx: &mut Context<Self>) {
        self.drawer.dispatch_fresh = !self.drawer.dispatch_fresh;
        cx.notify();
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
                            crate::drawer::load_overview(this, cx);
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
    /// two-stage confirm (same pattern as [`Self::arm_prune`]).
    pub fn arm_worktree_remove(&mut self, path: SharedString, cx: &mut Context<Self>) {
        self.agents_confirm = Some(path);
        self.agents_confirm_gen += 1;
        let generation = self.agents_confirm_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.agents_confirm_gen == generation && this.agents_confirm.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
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
        self.agents_confirm = None;
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
        if self.agents_review.as_ref() == Some(&path) {
            self.agents_review = None;
            self.agents_review_diff = None;
            cx.notify();
            return;
        }
        self.agents_review = Some(path.clone());
        self.agents_review_diff = None;
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
                if this.agents_review.as_ref() == Some(&path) {
                    this.agents_review_diff =
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
        if self.agents_pr_busy.contains(&path) {
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
        self.agents_pr_busy.insert(path.clone());
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
                        this.agents_pr_busy.remove(&path);
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
                this.agents_pr_busy.remove(&path);
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
        self.agents_confirm = None;
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
                if this.agents_review.as_ref() == Some(&path) {
                    this.agents_review = None;
                    this.agents_review_diff = None;
                }
                this.scan_agents(false, cx);
            });
        })
        .detach();
    }

    /// First click on a Cleanup "Prune" button: arm a two-stage confirm for that
    /// repo (the button flips to "Confirm prune {n}?"). Deleting branches is
    /// irreversible, so it only runs on the confirming second click. The armed
    /// state reverts after a few seconds, and arming another repo's button
    /// replaces it.
    pub fn arm_prune(&mut self, id: SharedString, cx: &mut Context<Self>) {
        self.cleanup_confirm = Some(id);
        self.cleanup_confirm_gen += 1;
        let generation = self.cleanup_confirm_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            // Revert only if this timer's arm is still the active one.
            let _ = this.update(cx, |this, cx| {
                if this.cleanup_confirm_gen == generation && this.cleanup_confirm.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Prune the given repo's stale branches, then refresh the Cleanup list.
    /// Only reached via the two-stage confirm in the Cleanup view (`arm_prune`).
    pub fn prune_repo(&mut self, id: SharedString, cx: &mut Context<Self>) {
        self.cleanup_confirm = None;
        cx.notify();
        let path = id.to_string();
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move { orrery_core::git_ops::prune_branches(&path) })
                .await;
            let Ok(rows) = this.update(cx, |this, cx| {
                // Surface what couldn't be pruned (a branch checked out in a
                // worktree, a permissions error, …) instead of dropping it.
                if let Some((title, detail)) = prune_failure(&report) {
                    this.push_toast(ToastKind::Error, title, Some(detail.into()), cx);
                }
                this.rows.clone()
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = crate::views::cleanup::CleanupState::Ready(result);
                // Pruning changed the prunable counts → refresh attention.
                this.recompute_attention();
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

    /// Generate local-AI one-line summaries for every repo (cached by commit, so
    /// repeats are cheap), then reload the grid so the cards show them. Gated on
    /// `ai_ready` — a no-op when AI is unavailable.
    pub fn summarize_all(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        cx.spawn(async move |this, cx| {
            let updated = crate::task::run(async { orrery_core::summarize::run_cached().await })
                .await
                .unwrap_or(0);
            if updated > 0 {
                let _ = this.update(cx, |this, cx| this.request_snapshot(SnapKind::Reload, cx));
            }
        })
        .detach();
    }
}

impl OrreryApp {
    /// Mission Control's nav badge: (count, any-urgent) over the items that
    /// need action — Urgent + Attention severities. Info items are ambient
    /// state and don't badge.
    fn grid_badge(&self) -> (usize, bool) {
        let mut n = 0;
        let mut urgent = false;
        for item in &self.attention_items {
            match item.severity {
                Severity::Urgent => {
                    n += 1;
                    urgent = true;
                }
                Severity::Attention => n += 1,
                Severity::Info => {}
            }
        }
        (n, urgent)
    }

    /// The Inbox nav badge: (count, any-urgent) over the inbox-derived
    /// attention items (review requests + your open PRs) once the inbox has
    /// loaded. Until then it falls back to the platform notifier's glance
    /// count — the poll already running from launch — so the badge is live
    /// before the Inbox view is ever opened.
    fn inbox_badge(&self) -> (usize, bool) {
        if !matches!(self.inbox, crate::views::inbox::InboxState::Ready(_)) {
            return (self.attention.len(), false);
        }
        let mut n = 0;
        let mut urgent = false;
        for item in &self.attention_items {
            match item.kind {
                AttentionKind::ReviewRequested => {
                    n += 1;
                    urgent = true;
                }
                AttentionKind::PrAssigned => n += 1,
                _ => {}
            }
        }
        (n, urgent)
    }

    fn header(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.))
            .h(px(52.))
            .px(px(16.))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.page))
            // brand
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(9.))
                    .child(lucide("orbit", 22., t.primary))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(t.text_brand))
                            .text_color(rgb(t.fg0))
                            .child("Orrery"),
                    ),
            )
            // roots · repos
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg2))
                    .child(lucide("folder", 14., t.fg2))
                    .child(SharedString::from(format!(
                        "{} roots · {} repos",
                        self.roots,
                        self.rows.len()
                    ))),
            )
            // spacer (ml-auto)
            .child(div().flex_1())
            // search box → opens the command palette
            .child(
                div()
                    .id("header-search")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(9.))
                    .w(px(380.))
                    .px(px(11.))
                    .py(px(7.))
                    .rounded(px(t.r_sm))
                    .bg(rgb(t.button_bg))
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_color(rgb(t.fg2))
                    .cursor_pointer()
                    .hover(|s| s.border_color(rgb(t.border_strong)))
                    .on_click(cx.listener(|this, _ev, window, cx| this.open_palette(window, cx)))
                    .child(lucide("search", 16., t.fg2))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(t.text_small))
                            .child("Search repos, run a command…"),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .rounded(px(t.r_xs))
                            .border_1()
                            .border_color(rgb(t.border))
                            .font_family("monospace")
                            .text_size(px(t.text_data_sm))
                            .child("⌘K"),
                    ),
            )
            .child(
                div()
                    .id("header-new")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded(px(t.r_sm))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.surface_hover)))
                    .child(lucide("plus", 16., t.fg1))
                    .on_click(
                        cx.listener(|this, _ev, window, cx| this.open_new_project(window, cx)),
                    ),
            )
            .child(
                div()
                    .id("header-rescan")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded(px(t.r_sm))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.surface_hover)))
                    .child(lucide("refresh-cw", 16., t.fg1))
                    .on_click(cx.listener(|this, _ev, _window, cx| this.rescan(cx))),
            )
    }

    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().flex().flex_col().gap(px(4.));
        for (view, icon_name, label) in NAV {
            let active = self.view == view;
            let fg = if active { t.accent_bright } else { t.fg1 };
            let mut item = div()
                .id(label)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .px(px(9.))
                .py(px(7.))
                .rounded(px(t.r_sm))
                .text_size(px(t.text_small))
                .text_color(rgb(fg))
                .hover(|s| s.bg(rgb(t.surface_hover)))
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    this.view = view;
                    this.view_filter = None; // contextual filters are per-view
                    this.maybe_load_view(view, window, cx);
                    cx.notify();
                }))
                .child(lucide(icon_name, 16., fg))
                .child(SharedString::from(label.to_string()));
            if active {
                item = item.bg(rgb(t.accent_wash));
            }
            // Attention-model count chips: Mission Control carries the total
            // urgent+attention items; the Inbox its inbox-derived items. A
            // zero count renders nothing (no empty-chip layout shift).
            let (n, urgent) = match view {
                View::Grid => self.grid_badge(),
                View::Inbox => self.inbox_badge(),
                _ => (0, false),
            };
            if n > 0 {
                item = item.child(div().flex_1()).child(badge(n, urgent, t));
            }
            nav = nav.child(item);
        }

        div()
            .flex()
            .flex_col()
            .w(px(236.))
            .h_full()
            .px(px(12.))
            .py(px(16.))
            .gap(px(16.))
            .border_r_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.page))
            // Primary nav stays put at the top…
            .child(nav)
            // …while the area below it is contextual: it swaps with the active
            // view (Mission Control shows the ROOTS / LANGUAGES filters). Scrolls
            // independently so the footer stays pinned.
            .child(
                div()
                    .id("sidebar-context")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(self.contextual_sidebar(t, cx)),
            )
            // footer pinned to the bottom
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child(lucide("hard-drive", 13., t.fg3))
                    .child(SharedString::from(scanned_label(
                        self.scanned_at,
                        crate::data::now_unix(),
                        self.passes.snapshot.scanning(),
                    ))),
            )
    }

    /// The view-specific sidebar content shown below the fixed nav. `None` for
    /// views that have no contextual panel yet (just the nav above).
    fn contextual_sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        match self.view {
            // Mission Control: the ROOTS / LANGUAGES quick-filters.
            View::Grid => Some(self.filter_sections(t, cx).into_any_element()),
            View::Feed => Some(self.feed_panel(t, cx)),
            View::Inbox => Some(self.inbox_panel(t, cx)),
            View::Tools => Some(self.devtools_panel(t, cx)),
            View::Settings => Some(self.settings_panel(t, cx)),
            View::Janitor => Some(self.cleanup_panel(t, cx)),
            View::Explore => Some(self.explore_panel(t, cx)),
            View::Agents => Some(self.agents_panel(t, cx)),
        }
    }

    /// A contextual filter list: a titled section of single-select category rows
    /// that drive `view_filter`. `cats` is `(key, icon, label, count)`; a `None`
    /// key is the "All" row.
    fn category_panel(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
        title: &'static str,
        cats: Vec<(
            Option<SharedString>,
            &'static str,
            SharedString,
            Option<usize>,
        )>,
    ) -> gpui::AnyElement {
        let mut sec = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(section_header(title, t));
        for (key, icon, label, count) in cats {
            let active = key == self.view_filter;
            let icon_fg = if active { t.accent_bright } else { t.fg2 };
            let pick = key.clone();
            sec = sec.child(sidebar_filter_item(
                SharedString::from(format!("cat-{title}-{label}")),
                lucide(icon, 14., icon_fg).into_any_element(),
                label,
                count,
                active,
                t,
                cx.listener(move |this, _e, _w, cx| this.set_view_filter(pick.clone(), cx)),
            ));
        }
        div().flex().flex_col().child(sec).into_any_element()
    }

    /// Feed: filter by activity type.
    fn feed_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::views::feed::FeedState;
        let rows = match &self.feed {
            FeedState::Ready(rows) => rows.as_slice(),
            _ => &[],
        };
        let total = rows.len();
        let count = |kind: &str| rows.iter().filter(|r| r.kind.as_ref() == kind).count();
        let new = rows.iter().filter(|r| r.is_new).count();
        self.category_panel(
            t,
            cx,
            "FILTER",
            vec![
                (None, "rss", "All".into(), Some(total)),
                (Some("new".into()), "sparkles", "New".into(), Some(new)),
                (
                    Some("release".into()),
                    "tag",
                    "Releases".into(),
                    Some(count("release")),
                ),
                (
                    Some("starred".into()),
                    "star",
                    "Stars".into(),
                    Some(count("starred")),
                ),
                (
                    Some("created".into()),
                    "box",
                    "New repos".into(),
                    Some(count("created")),
                ),
                (
                    Some("forked".into()),
                    "git-branch",
                    "Forks".into(),
                    Some(count("forked")),
                ),
                (
                    Some("public".into()),
                    "globe",
                    "Open-sourced".into(),
                    Some(count("public")),
                ),
            ],
        )
    }

    /// Inbox: filter by item category.
    fn inbox_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::views::inbox::InboxState;
        let count = |kind: &str| match &self.inbox {
            InboxState::Ready(d) => d.items.iter().filter(|i| i.kind.as_ref() == kind).count(),
            _ => 0,
        };
        let notifications = match &self.inbox {
            InboxState::Ready(d) => d.notifications.len(),
            _ => 0,
        };
        let total = match &self.inbox {
            InboxState::Ready(d) => d.items.len() + d.notifications.len(),
            _ => 0,
        };
        self.category_panel(
            t,
            cx,
            "FILTER",
            vec![
                (None, "inbox", "All".into(), Some(total)),
                (
                    Some("pr".into()),
                    "git-pull-request",
                    "Pull requests".into(),
                    Some(count("pr")),
                ),
                (
                    Some("review".into()),
                    "eye",
                    "Reviews".into(),
                    Some(count("review")),
                ),
                (
                    Some("issue".into()),
                    "circle-dot",
                    "Issues".into(),
                    Some(count("issue")),
                ),
                (
                    Some("notification".into()),
                    "bell",
                    "Notifications".into(),
                    Some(notifications),
                ),
            ],
        )
    }

    /// Dev Tools: filter the utility belt by category (composes with the search
    /// box). Counts are the number of tools in each category.
    fn devtools_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.category_panel(
            t,
            cx,
            "CATEGORY",
            vec![
                (None, "wrench", "All tools".into(), Some(11)),
                (
                    Some("generators".into()),
                    "box",
                    "Generators".into(),
                    Some(1),
                ),
                (
                    Some("encoding".into()),
                    "binary",
                    "Encoding".into(),
                    Some(2),
                ),
                (Some("hashing".into()), "hash", "Hashing".into(), Some(1)),
                (Some("data".into()), "braces", "Data".into(), Some(2)),
                (
                    Some("convert".into()),
                    "arrow-up-down",
                    "Convert".into(),
                    Some(3),
                ),
                (Some("text".into()), "type", "Text".into(), Some(2)),
            ],
        )
    }

    /// Settings: jump to a section (gates which section the view renders). No
    /// counts — these are section selectors, not filters.
    fn settings_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.category_panel(
            t,
            cx,
            "SECTIONS",
            vec![
                (None, "settings", "All".into(), None),
                (
                    Some("account".into()),
                    "user",
                    "GitHub account".into(),
                    None,
                ),
                (
                    Some("roots".into()),
                    "folder",
                    "Workspace roots".into(),
                    None,
                ),
                (Some("launchers".into()), "rocket", "Launchers".into(), None),
                (Some("ai".into()), "sparkles", "AI".into(), None),
                (
                    Some("notifications".into()),
                    "bell",
                    "Notifications".into(),
                    None,
                ),
            ],
        )
    }

    /// Cleanup: filter prunable branches by why they're prunable.
    fn cleanup_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::views::cleanup::CleanupState;
        let (mut merged, mut gone) = (0usize, 0usize);
        if let CleanupState::Ready(repos) = &self.cleanup {
            for repo in repos {
                for b in &repo.branches {
                    if b.why == "merged" {
                        merged += 1;
                    } else {
                        gone += 1;
                    }
                }
            }
        }
        self.category_panel(
            t,
            cx,
            "FILTER",
            vec![
                (None, "scissors", "All".into(), Some(merged + gone)),
                (
                    Some("merged".into()),
                    "git-merge",
                    "Merged".into(),
                    Some(merged),
                ),
                (
                    Some("gone".into()),
                    "circle-alert",
                    "Gone".into(),
                    Some(gone),
                ),
            ],
        )
    }

    /// Explore: filter starred results by language.
    fn explore_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::views::explore::ExploreState;
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let total = if let ExploreState::Ready(rows) = &self.explore {
            for r in rows {
                let l: &str = r.language.as_ref();
                if !l.is_empty() {
                    *counts.entry(l).or_default() += 1;
                }
            }
            rows.len()
        } else {
            0
        };
        let mut langs: Vec<(SharedString, usize)> = counts
            .into_iter()
            .map(|(k, n)| (SharedString::from(k.to_string()), n))
            .collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut cats: Vec<(
            Option<SharedString>,
            &'static str,
            SharedString,
            Option<usize>,
        )> = vec![(None, "compass", "All".into(), Some(total))];
        for (lang, n) in langs {
            cats.push((Some(lang.clone()), "box", lang, Some(n)));
        }
        self.category_panel(t, cx, "LANGUAGE", cats)
    }

    /// Agents: filter running sessions by repo.
    fn agents_panel(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::views::agents::AgentsState;
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let total = if let AgentsState::Ready(data) = &self.agents {
            for r in &data.sessions {
                *counts.entry(r.name.as_ref()).or_default() += 1;
            }
            // Dispatched worktrees file under their origin repo's name.
            for d in &data.dispatched {
                *counts.entry(d.origin_name.as_ref()).or_default() += 1;
            }
            data.sessions.len() + data.dispatched.len()
        } else {
            0
        };
        let mut repos: Vec<(SharedString, usize)> = counts
            .into_iter()
            .map(|(k, n)| (SharedString::from(k.to_string()), n))
            .collect();
        repos.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut cats: Vec<(
            Option<SharedString>,
            &'static str,
            SharedString,
            Option<usize>,
        )> = vec![(None, "square-terminal", "All".into(), Some(total))];
        for (name, n) in repos {
            cats.push((Some(name.clone()), "folder", name, Some(n)));
        }
        self.category_panel(t, cx, "REPO", cats)
    }

    /// The ROOTS and LANGUAGES filter lists, derived from the current rows.
    fn filter_sections(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        // Counts per root and per language — cached per rows generation.
        let facets = self.facets();

        // ── VIEWS (saved quick filters) ────────────────────────────────────
        let mut views_sec = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(section_header_action(
                "VIEWS",
                "plus",
                t,
                cx.listener(|this, _e, _w, cx| this.save_current_view(cx)),
            ));
        if self.grid.saved_views.is_empty() {
            views_sec = views_sec.child(
                div()
                    .px(px(9.))
                    .py(px(4.))
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg3))
                    .child("Save the current filters as a quick view."),
            );
        } else {
            let hov = t.surface_hover;
            for (i, v) in self.grid.saved_views.iter().enumerate() {
                let active = self.view_is_active(v);
                let fg = if active { t.accent_bright } else { t.fg1 };
                let mut row = div()
                    .id(SharedString::from(format!("view-{i}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(px(t.r_sm))
                    .text_size(px(t.text_small))
                    .text_color(rgb(fg))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(hov)))
                    .on_click(cx.listener(move |this, _e, _w, cx| this.apply_view(i, cx)))
                    .child(lucide("bookmark", 14., fg))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(SharedString::from(v.name.clone())),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("view-del-{i}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(18.))
                            .h(px(18.))
                            .rounded(px(t.r_xs))
                            .hover(move |s| s.bg(rgb(hov)))
                            .child(lucide("trash-2", 13., t.fg3))
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                // Don't let delete also apply the view.
                                cx.stop_propagation();
                                this.delete_view(i, cx);
                            })),
                    );
                if active {
                    row = row.bg(rgb(t.accent_wash));
                }
                views_sec = views_sec.child(row);
            }
        }

        // ── ROOTS ──────────────────────────────────────────────────────────
        let mut roots_sec = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(section_header("ROOTS", t));
        roots_sec = roots_sec.child(sidebar_filter_item(
            "root-all".into(),
            lucide("folder", 14., t.fg2).into_any_element(),
            "All repos".into(),
            Some(self.rows.len()),
            self.grid.root.is_none(),
            t,
            cx.listener(|this, _e, _w, cx| this.set_root(None, cx)),
        ));
        for (root, n) in facets.roots.iter().cloned() {
            let active = self.grid.root.as_ref() == Some(&root);
            let pick = root.clone();
            roots_sec = roots_sec.child(sidebar_filter_item(
                SharedString::from(format!("root-{root}")),
                lucide("folder", 14., t.fg2).into_any_element(),
                root,
                Some(n),
                active,
                t,
                cx.listener(move |this, _e, _w, cx| this.set_root(Some(pick.clone()), cx)),
            ));
        }

        // ── LANGUAGES ──────────────────────────────────────────────────────
        let mut langs_sec = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(section_header("LANGUAGES", t));
        for (lang, n) in facets.langs.iter().cloned() {
            let active = self.grid.language.as_ref() == Some(&lang);
            let pick = lang.clone();
            langs_sec = langs_sec.child(sidebar_filter_item(
                SharedString::from(format!("lang-{lang}")),
                crate::card::lang_mark(&lang, t),
                lang,
                Some(n),
                active,
                t,
                cx.listener(move |this, _e, _w, cx| this.toggle_language(pick.clone(), cx)),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(views_sec)
            .child(roots_sec)
            .child(langs_sec)
    }

    fn main_view(&self, t: &Theme, cx: &mut Context<Self>, cols: usize) -> gpui::AnyElement {
        match self.view {
            View::Grid => self.grid(t, cx, cols).into_any_element(),
            View::Inbox => crate::views::inbox::render(
                &self.inbox,
                self.view_filter.as_deref(),
                t,
                &cx.entity(),
            )
            .into_any_element(),
            View::Feed => {
                crate::views::feed::render(&self.feed, self.view_filter.as_deref(), t, &cx.entity())
                    .into_any_element()
            }
            View::Explore => {
                let cloned = self.cloned_slugs();
                crate::views::explore::render(
                    &self.explore,
                    crate::views::explore::CloneStatus {
                        cloned: &cloned,
                        cloning: &self.explore_cloning,
                        errors: &self.explore_errors,
                    },
                    self.view_filter.as_deref(),
                    self.config.roots.first().map(|s| s.as_str()),
                    t,
                    &cx.entity(),
                )
                .into_any_element()
            }
            View::Janitor => crate::views::cleanup::render(
                &self.cleanup,
                self.view_filter.as_deref(),
                self.cleanup_confirm.as_deref(),
                t,
                &cx.entity(),
            )
            .into_any_element(),
            View::Agents => crate::views::agents::render(
                &self.agents,
                self.view_filter.as_deref(),
                self.agents_confirm.as_deref(),
                self.agents_review
                    .as_deref()
                    .map(|p| (p, self.agents_review_diff.as_deref())),
                t,
                &cx.entity(),
            )
            .into_any_element(),
            View::Tools => match &self.devtools {
                Some(d) => crate::views::devtools::render(
                    d,
                    self.view_filter.as_deref(),
                    t,
                    &cx.entity(),
                    cx,
                )
                .into_any_element(),
                None => placeholder(View::Tools, t).into_any_element(),
            },
            View::Settings => match &self.settings {
                Some(s) => crate::views::settings::render(
                    s,
                    self.view_filter.as_deref(),
                    self.services.github_authed,
                    &self.services.github_device,
                    &self.services.ai_status,
                    self.services.ai_ready,
                    t,
                    &cx.entity(),
                )
                .into_any_element(),
                None => placeholder(View::Settings, t).into_any_element(),
            },
        }
    }
}

impl Render for OrreryApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        // Responsive grid columns from the current window width.
        let cols = columns(f32::from(window.viewport_size().width));
        let shell = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.page))
            .text_color(rgb(t.fg1))
            .font_family("sans-serif")
            .child(self.header(&t, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar(&t, cx))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.main_view(&t, cx, cols)),
                    ),
            );

        // The shell, with any overlay (drawer/palette/dialog) layered on top.
        // The root tracks focus + handles CloseOverlay so Esc dismisses overlays.
        let mut root = div()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::CloseOverlay, window, cx| {
                if this.overlay.is_some() {
                    this.close_overlay();
                    window.focus(&this.focus, cx);
                    cx.notify();
                } else if this.fleet_prune.is_some() {
                    // A pending prune confirm dismisses first, keeping the
                    // selection — a second Esc clears that.
                    this.cancel_fleet_prune(cx);
                } else if !this.selected.is_empty() {
                    // No overlay to dismiss — Esc clears the fleet selection.
                    this.clear_selection(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::OpenPalette, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteDown, _window, cx| {
                this.move_palette(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteUp, _window, cx| {
                this.move_palette(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteConfirm, window, cx| {
                this.confirm_palette(window, cx);
            }))
            .relative()
            .size_full()
            .child(shell);
        // Toasts layer over the active view; the modal overlay (drawer/palette/
        // dialog) is added after so it stays in front of them.
        if let Some(toasts) = self.toast_layer(&t, cx) {
            root = root.child(toasts);
        }
        if let Some(overlay) = self.overlay_element(&t, cx) {
            root = root.child(overlay);
        }
        root
    }
}

impl OrreryApp {
    /// Build the active overlay's element, if one is open. Returns `None` when
    /// the drawer's repo has vanished (e.g. a rescan dropped it) — which also
    /// leaves the stale overlay to be cleared on the next interaction.
    fn overlay_element(&self, t: &Theme, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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

/// An uppercase sidebar section header (ROOTS / LANGUAGES).
fn section_header(label: &'static str, t: &Theme) -> impl IntoElement {
    div()
        .px(px(9.))
        .pb(px(2.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(label)
}

/// A section header with a trailing icon action (e.g. VIEWS + to save a view).
fn section_header_action(
    label: &'static str,
    icon: &'static str,
    t: &Theme,
    on: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let hov = t.surface_hover;
    div()
        .flex()
        .flex_row()
        .items_center()
        .pl(px(9.))
        .pb(px(2.))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(label),
        )
        .child(
            div()
                .id(SharedString::from(format!("hdr-{label}")))
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.))
                .h(px(18.))
                .rounded(px(t.r_xs))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hov)))
                .child(lucide(icon, 13., t.fg3))
                .on_click(on),
        )
}

/// One clickable sidebar filter row: leading mark, label, and a right-aligned
/// count. Highlighted when `active`. `on` fires on click.
fn sidebar_filter_item(
    id: SharedString,
    leading: gpui::AnyElement,
    label: SharedString,
    count: Option<usize>,
    active: bool,
    t: &Theme,
    on: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let fg = if active { t.accent_bright } else { t.fg1 };
    let hov = t.surface_hover;
    let mut item = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.))
        .px(px(9.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .text_size(px(t.text_small))
        .text_color(rgb(fg))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(hov)))
        .on_click(on)
        .child(leading)
        .child(div().flex_1().min_w(px(0.)).truncate().child(label))
        // Count is right-aligned and optional (section selectors omit it).
        .children(count.map(|n| {
            div()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(n.to_string()))
        }));
    if active {
        item = item.bg(rgb(t.accent_wash));
    }
    item
}

/// A small count pill for the sidebar nav. Urgent counts get the danger
/// tint; everything else stays a neutral chip.
fn badge(n: usize, urgent: bool, t: &Theme) -> impl IntoElement {
    let (bg, fg) = if urgent {
        (t.danger_badge, t.behind)
    } else {
        (t.button_bg, t.fg2)
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .bg(rgb(bg))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(SharedString::from(n.to_string()))
}

/// Centered title + note, shown for a view whose state hasn't been created
/// yet (a frame or so, until its lazy load runs).
fn placeholder(view: View, t: &Theme) -> impl IntoElement {
    let (title, sub): (&str, &str) = match view {
        View::Inbox => ("Inbox", "Review queue — PRs & notifications awaiting you"),
        View::Feed => ("Feed", "Activity stream across your repos"),
        View::Explore => ("Explore", "Discover & search across hosts"),
        View::Agents => ("Agents", "Running terminal coding-agent sessions"),
        View::Tools => ("Dev Tools", "Utilities & quick actions"),
        View::Janitor => ("Cleanup", "Prunable branches & worktrees"),
        View::Settings => ("Settings", "Roots, AI, launchers, appearance"),
        View::Grid => ("Mission Control", ""),
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap(px(8.))
        .bg(rgb(t.page))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(t.text_display))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg2))
                .child(SharedString::from(sub.to_string())),
        )
}

/// The sidebar footer's scan line: "Scanning…" while a rescan runs, else how
/// long ago the roots were last scanned from disk.
fn scanned_label(scanned_at: Option<i64>, now: i64, scanning: bool) -> String {
    if scanning {
        return "Scanning…".into();
    }
    match scanned_at {
        None => "Not scanned yet".into(),
        Some(at) if now - at < 60 => "Scanned just now".into(),
        Some(at) => format!("Scanned {}", crate::data::rel_age(at, now)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::attention::RepoRef;

    #[test]
    fn prune_failure_reports_only_failures() {
        use orrery_core::git_ops::PruneReport;
        let clean = Ok(PruneReport {
            deleted: vec!["old".into()],
            failed: Vec::new(),
        });
        assert!(prune_failure(&clean).is_none());
        let partial = Ok(PruneReport {
            deleted: Vec::new(),
            failed: vec![("wip".into(), "checked out\nin a worktree".into())],
        });
        let (title, detail) = prune_failure(&partial).unwrap();
        assert_eq!(title, "Couldn't delete 1 branch");
        assert_eq!(detail, "wip: checked out in a worktree");
        let (title, _) = prune_failure(&Err("not a repo".into())).unwrap();
        assert_eq!(title, "Prune failed");
    }

    #[test]
    fn scanned_label_reflects_scan_state_and_age() {
        assert_eq!(scanned_label(Some(100), 200, true), "Scanning…");
        assert_eq!(scanned_label(None, 200, false), "Not scanned yet");
        assert_eq!(scanned_label(Some(170), 200, false), "Scanned just now");
        assert_eq!(
            scanned_label(Some(1), 1 + 3 * 3600, false),
            "Scanned 3h ago"
        );
    }
}
