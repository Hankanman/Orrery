//! App shell — the chrome that wraps every view: the 52px header (brand,
//! roots·repos, search, new/rescan), the 236px left rail with the 8 primary nav
//! items, and the main column.
//!
//! The nav is live: clicking an item switches the active `View`; each view loads
//! its data lazily on first selection.

use std::rc::Rc;

use gpui::{
    AppContext, Context, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

use orrery_core::model::AppConfig;

use crate::card::card;
use crate::data::Row;
use crate::icon::lucide;
use crate::theme::Theme;

/// Grid row height without / with AI summary lines (the launcher row is the
/// bottom of the card, so the row must be tall enough not to clip it).
const ROW_H: f32 = 260.;
const ROW_H_AI: f32 = 288.;

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

/// A Mission Control quick filter. Single-select (radio): one is active at a
/// time, `All` meaning no filtering.
#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RepoFilter {
    #[default]
    All,
    Public,
    Private,
    Dirty,
    Ahead,
    Starred,
    Stale,
}

impl RepoFilter {
    /// The chip order shown in the toolbar.
    pub const ORDER: [RepoFilter; 7] = [
        RepoFilter::All,
        RepoFilter::Public,
        RepoFilter::Private,
        RepoFilter::Dirty,
        RepoFilter::Ahead,
        RepoFilter::Starred,
        RepoFilter::Stale,
    ];

    fn label(self) -> &'static str {
        match self {
            RepoFilter::All => "All",
            RepoFilter::Public => "Public",
            RepoFilter::Private => "Private",
            RepoFilter::Dirty => "Dirty",
            RepoFilter::Ahead => "Ahead",
            RepoFilter::Starred => "Starred",
            RepoFilter::Stale => "Stale",
        }
    }

    /// Lucide icon for the chip, if any (the visibility chips carry one).
    fn icon(self) -> Option<&'static str> {
        match self {
            RepoFilter::Public => Some("globe"),
            RepoFilter::Private => Some("lock"),
            RepoFilter::Dirty => Some("circle-dot"),
            RepoFilter::Ahead => Some("arrow-up"),
            RepoFilter::Starred => Some("star"),
            RepoFilter::Stale => Some("clock"),
            RepoFilter::All => None,
        }
    }

    /// Does `row` pass this filter?
    fn matches(self, r: &Row) -> bool {
        use orrery_core::model::Activity;
        match self {
            RepoFilter::All => true,
            RepoFilter::Public => !r.private,
            RepoFilter::Private => r.private,
            RepoFilter::Dirty => r.dirty > 0,
            RepoFilter::Ahead => r.ahead > 0,
            RepoFilter::Starred => r.favorite,
            RepoFilter::Stale => r.activity == Activity::Stale,
        }
    }
}

/// Card ordering for Mission Control.
#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortMode {
    /// Most recently committed first.
    #[default]
    Activity,
    /// Alphabetical by name.
    Name,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            SortMode::Activity => "Activity",
            SortMode::Name => "Name",
        }
    }

    fn next(self) -> SortMode {
        match self {
            SortMode::Activity => SortMode::Name,
            SortMode::Name => SortMode::Activity,
        }
    }
}

/// A persisted Mission Control "quick view": a named snapshot of the active
/// filter combo, restorable from the sidebar's VIEWS section.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedView {
    pub name: String,
    pub filter: RepoFilter,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub sort: SortMode,
}

/// SQLite meta key holding the saved-views JSON array.
const SAVED_VIEWS_KEY: &str = "saved_views";

/// Load persisted saved views from the cache (empty if none / unparseable).
pub fn load_saved_views() -> Vec<SavedView> {
    orrery_core::cache::get_meta(SAVED_VIEWS_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn persist_saved_views(views: &[SavedView]) {
    if let Ok(json) = serde_json::to_string(views) {
        orrery_core::cache::set_meta(SAVED_VIEWS_KEY, &json);
    }
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
    pub theme: Rc<Theme>,
    pub config: AppConfig,
    /// Current attention glance lines (PRs/reviews/CI) from the background
    /// poller — drives the Inbox nav badge. Empty until the first poll lands.
    pub attention: Vec<String>,
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
    /// Agents view state (lazy; detected agent sessions on the machine).
    pub agents: crate::views::agents::AgentsState,
    /// Slugs currently being cloned from the Explore view.
    pub explore_cloning: std::collections::HashSet<SharedString>,
    /// Settings editing session (draft config + field inputs); created on first
    /// open, kept so edits survive navigating away.
    pub settings: Option<crate::views::settings::SettingsState>,
    /// Dev Tools fields (created on first open).
    pub devtools: Option<crate::views::devtools::DevToolsState>,
    /// Whether a GitHub token is currently resolvable (Settings account panel).
    pub github_authed: bool,
    /// An in-progress GitHub device-flow login, if any.
    pub github_device: Option<crate::views::settings::GithubDevice>,
    /// Live AI-backend reachability (Settings AI panel).
    pub ai_status: crate::views::settings::AiStatus,
    /// AI is enabled and reachable — gates semantic search + AI affordances.
    pub ai_ready: bool,
    /// Whether the system tray came up — gates close-to-tray.
    pub tray_active: bool,
    /// Contribution-graph data (commits/day across repos), computed in the
    /// background; `None` until the first pass lands.
    pub activity: Option<orrery_core::activity::Activity>,
    /// Whether the Mission Control activity graph is shown (dismissible).
    pub activity_open: bool,
    /// Active Mission Control quick filter (All = no filtering).
    pub filter: RepoFilter,
    /// Active scanned-root filter (sidebar ROOTS); `None` = all roots.
    pub root: Option<SharedString>,
    /// Active language filter (sidebar LANGUAGES); `None` = all languages.
    pub language: Option<SharedString>,
    /// Persisted quick views (sidebar VIEWS), loaded from the cache at launch.
    pub saved_views: Vec<SavedView>,
    /// The active contextual sub-filter for the current non-Grid view (e.g. the
    /// Feed/Inbox category panels). Ephemeral: reset when the view changes so
    /// filters don't bleed across views.
    pub view_filter: Option<SharedString>,
    /// Mission Control card ordering.
    pub sort: SortMode,
    /// App-root focus handle, so global key bindings (Esc) dispatch here.
    pub focus: FocusHandle,
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
        // The new-worktree field lives in Overview, shown immediately on open.
        self.drawer.worktree_input = Some(cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("new-worktree-name")
        }));
        crate::drawer::load_overview(repo, cx);
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
        // On each keystroke: reset the selection, kick off a (debounced) code
        // search, and re-render.
        let sub = cx.observe(&query, |this, _q, cx| {
            if let Some(Overlay::Palette(d)) = &mut this.overlay {
                d.selected = 0;
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
            generation: 0,
            _sub: sub,
        }));
        window.focus(&fh, cx);
        cx.notify();
    }

    /// Open the new-project dialog (clone / init into a workspace root).
    pub fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::views::newproject::{NewMode, NewProjectData};
        use gpui_component::input::InputState;
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/owner/repo"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("repo"));
        let subs = vec![
            cx.observe(&url, |_this, _e, cx| cx.notify()),
            cx.observe(&name, |_this, _e, cx| cx.notify()),
        ];
        self.overlay = Some(Overlay::NewProject(NewProjectData {
            mode: NewMode::Clone,
            url,
            name,
            root: 0,
            status: "".into(),
            busy: false,
            _subs: subs,
        }));
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
                            None,
                            None,
                            Some("Initial commit"),
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

    /// The current palette result list (actions + repos + code hits).
    fn palette_items(&self, cx: &Context<Self>) -> Vec<crate::palette::PaletteItem> {
        match &self.overlay {
            Some(Overlay::Palette(d)) => {
                crate::palette::items(&self.rows, &d.code, &d.semantic, &d.query.read(cx).value())
            }
            _ => Vec::new(),
        }
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
                        cx.notify();
                    }
            });
        })
        .detach();
    }

    /// Debounced semantic (embedding) repo search for the current query. Gated
    /// on AI being ready; reuses the code-search generation for stale-drop.
    fn trigger_semantic_search(&mut self, cx: &mut Context<Self>) {
        if !self.ai_ready {
            return;
        }
        let (query, generation) = match &self.overlay {
            Some(Overlay::Palette(d)) => (d.query.read(cx).value().to_string(), d.generation),
            _ => return,
        };
        if query.trim().len() < 2 {
            if let Some(Overlay::Palette(d)) = &mut self.overlay {
                d.semantic.clear();
            }
            return;
        }
        cx.spawn(async move |this, cx| {
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
            let hits =
                crate::task::run(async move { orrery_core::semantic::search(&query).await }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Overlay::Palette(d)) = &mut this.overlay
                    && d.generation == generation
                {
                    d.semantic = hits.into_iter().map(|(id, _)| id.into()).collect();
                    cx.notify();
                }
            });
        })
        .detach();
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
            PaletteItem::Repo(i) => {
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
        cx.notify();
        cx.spawn(async move |this, cx| {
            let items = crate::task::run(async { orrery_core::inbox::github_inbox().await }).await;
            let notes =
                crate::task::run(async { orrery_core::inbox::github_notifications().await }).await;
            let _ = this.update(cx, |this, cx| {
                this.inbox = match items {
                    Ok(i) => InboxState::Ready(InboxData {
                        items: i.into_iter().map(inbox_row).collect(),
                        notifications: notes
                            .unwrap_or_default()
                            .into_iter()
                            .map(notice_row)
                            .collect(),
                    }),
                    Err(e) => InboxState::Error(e.into()),
                };
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
            View::Agents if matches!(self.agents, views::agents::AgentsState::Idle) => {
                self.load_agents(cx)
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
        let mut subs = Vec::new();
        for input in [&search, &base64, &hash, &json, &base_conv, &case_conv, &url] {
            subs.push(cx.observe(input, |_this, _e, cx| cx.notify()));
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
                this.github_authed = authed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Begin the GitHub device-flow login: request a code, show it, then poll
    /// until the user authorizes (or it fails / expires).
    pub fn github_connect(&mut self, cx: &mut Context<Self>) {
        use crate::views::settings::GithubDevice;
        if self.github_device.is_some() {
            return;
        }
        self.github_device = Some(GithubDevice {
            user_code: "…".into(),
            verification_uri: "https://github.com/login/device".into(),
            status: "Requesting a device code…".into(),
        });
        cx.notify();

        let client_id = orrery_core::oauth::github_client_id();
        cx.spawn(async move |this, cx| {
            let id = client_id.clone();
            let started =
                crate::task::run(async move { orrery_core::oauth::device_start(&id).await }).await;
            let start = match started {
                Ok(d) => d,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(d) = &mut this.github_device {
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
                    this.github_device = Some(GithubDevice {
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
                    .update(cx, |this, _| this.github_device.is_none())
                    .unwrap_or(true)
                {
                    return;
                }
                let id = client_id.clone();
                let code = device_code.clone();
                let poll =
                    crate::task::run(
                        async move { orrery_core::oauth::device_poll(&id, &code).await },
                    )
                    .await;
                let status = match poll {
                    Ok(p) => p.status,
                    Err(e) => e,
                };
                match status.as_str() {
                    "authorized" => {
                        let _ = this.update(cx, |this, cx| {
                            this.github_device = None;
                            this.github_authed = true;
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
                            if let Some(d) = &mut this.github_device {
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
        self.github_device = None;
        self.github_authed = orrery_core::oauth::github_authed();
        cx.notify();
    }

    /// Re-check AI-backend reachability and list installed models.
    pub fn ai_refresh(&mut self, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if matches!(self.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.ai_status = AiStatus::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available()).await;
            let status = if up {
                let models = crate::task::run(orrery_core::ai::installed_models()).await;
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
                this.ai_status = status;
                this.ai_ready = ready;
                // Reachable now → (re)build the semantic index so the palette can
                // search by meaning.
                if ready {
                    this.index_semantic();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One-shot at launch: if AI is enabled and reachable, mark it ready and
    /// kick off the semantic index (so Ctrl+K works without opening Settings).
    pub fn ai_startup(&mut self, cx: &mut Context<Self>) {
        if !self.config.ai_enabled {
            return;
        }
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available()).await;
            let _ = this.update(cx, |this, _cx| {
                this.ai_ready = up;
                if up {
                    this.index_semantic();
                }
            });
        })
        .detach();
    }

    /// (Re)build the semantic embedding index from the current rows, off the UI
    /// thread. Cheap when nothing changed (core skips unchanged repos).
    pub fn index_semantic(&self) {
        if !self.ai_ready {
            return;
        }
        let items: Vec<(String, String)> = self
            .rows
            .iter()
            .map(|r| {
                (
                    r.id.to_string(),
                    format!("{} {} {} {}", r.name, r.slug, r.language, r.description),
                )
            })
            .collect();
        crate::task::spawn_detached(async move {
            let _ = orrery_core::semantic::index(&items).await;
        });
    }

    /// Pull (download) a model on the AI backend, then refresh the status.
    pub fn ai_pull(&mut self, model: String, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if model.trim().is_empty() || matches!(self.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.ai_status = AiStatus::Pulling(format!("{model} · starting…").into());
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
                        this.ai_status = AiStatus::Pulling(label.into());
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
                this.ai_status = AiStatus::Unknown;
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
        draft.ollama_host = s.ollama_host.read(cx).value().to_string();
        draft.ai_model = s.ai_model.read(cx).value().to_string();
        draft.embed_model = s.embed_model.read(cx).value().to_string();
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

        let _ = orrery_core::config::save(&draft);
        self.config = draft.clone();
        if let Some(s) = &mut self.settings {
            s.draft = draft;
            s.saved = true;
        }
        self.rescan(cx);
        cx.notify();
    }

    /// Load the activity/release feed over the network.
    pub fn load_feed(&mut self, cx: &mut Context<Self>) {
        use crate::views::feed::{FeedState, feed_row};
        self.feed = FeedState::Loading;
        cx.notify();
        let now = crate::data::now_unix();
        cx.spawn(async move |this, cx| {
            let res = crate::task::run(async { orrery_core::inbox::github_feed().await }).await;
            let _ = this.update(cx, |this, cx| {
                this.feed = match res {
                    Ok(items) => {
                        FeedState::Ready(items.into_iter().map(|f| feed_row(f, now)).collect())
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
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = crate::task::run(async { orrery_core::inbox::github_starred().await }).await;
            let _ = this.update(cx, |this, cx| {
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
        cx.notify();
        let dest = orrery_core::scan::expand(&root)
            .join(name.as_ref())
            .to_string_lossy()
            .into_owned();
        let url = clone_url.to_string();
        cx.spawn(async move |this, cx| {
            let (rows, roots) = cx
                .background_executor()
                .spawn(async move {
                    if !std::path::Path::new(&dest).exists() {
                        let _ = orrery_core::git_ops::clone(&url, &dest);
                    }
                    crate::data::rescan()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.roots = roots;
                this.explore_cloning.remove(&slug);
                cx.notify();
            });
        })
        .detach();
    }

    /// Scan all repos for prunable branches (sync git, off-thread).
    pub fn load_cleanup(&mut self, cx: &mut Context<Self>) {
        use crate::views::cleanup::CleanupState;
        self.cleanup = CleanupState::Loading;
        cx.notify();
        let rows = self.rows.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = CleanupState::Ready(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Scan the machine for running agent sessions (off the UI thread).
    pub fn load_agents(&mut self, cx: &mut Context<Self>) {
        use crate::views::agents::AgentsState;
        self.agents = AgentsState::Loading;
        cx.notify();
        let rows = self.rows.clone();
        let agent_command = self.config.agent_command.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::agents::scan(&rows, &agent_command) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.agents = AgentsState::Ready(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Terminate an agent process by pid, then re-scan the list.
    pub fn terminate_agent(&mut self, pid: u32, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { orrery_platform::agents::terminate(pid) })
                .await;
            let Ok((rows, agent_command)) = this.update(cx, |this, _| {
                (this.rows.clone(), this.config.agent_command.clone())
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::agents::scan(&rows, &agent_command) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.agents = crate::views::agents::AgentsState::Ready(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Prune the given repo's stale branches, then refresh the Cleanup list.
    pub fn prune_repo(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let path = id.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    let _ = orrery_core::git_ops::prune_branches(&path);
                })
                .await;
            let Ok(rows) = this.update(cx, |this, _| this.rows.clone()) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::views::cleanup::scan(&rows) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cleanup = crate::views::cleanup::CleanupState::Ready(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-scan the roots from disk (off the UI thread) and reload the grid, then
    /// refresh host enrichment.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (rows, roots) = cx
                .background_executor()
                .spawn(async { crate::data::rescan() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.roots = roots;
                this.enrich_hosts(cx);
                this.load_activity(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Recompute the contribution graph (commits/day across all repos) on the
    /// background pool — git history walking is slow — then store it. Cheap to
    /// call on rescan; the revwalk stops past the one-year window.
    pub fn load_activity(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
        cx.spawn(async move |this, cx| {
            let activity = cx
                .background_executor()
                .spawn(async move { orrery_core::activity::compute(&paths) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.activity = Some(activity);
                cx.notify();
            });
        })
        .detach();
    }

    /// Set the active Mission Control quick filter.
    pub fn set_filter(&mut self, f: RepoFilter, cx: &mut Context<Self>) {
        self.filter = f;
        cx.notify();
    }

    /// Cycle the Mission Control sort order.
    pub fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = self.sort.next();
        cx.notify();
    }

    /// Show/hide the contribution graph.
    pub fn toggle_activity(&mut self, cx: &mut Context<Self>) {
        self.activity_open = !self.activity_open;
        cx.notify();
    }

    /// Force-refresh host enrichment for every repo (ignores the TTL), then
    /// reload the grid. The toolbar's "Fetch all".
    pub fn fetch_all_hosts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let updated =
                crate::task::run(async move { orrery_core::enrich::refresh_cached_all(now).await })
                    .await;
            if updated == 0 {
                return;
            }
            let (rows, roots) = cx
                .background_executor()
                .spawn(async { crate::data::load(crate::data::now_unix()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.roots = roots;
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

    /// Select the scanned root to filter by (sidebar ROOTS); `None` = all.
    pub fn set_root(&mut self, root: Option<SharedString>, cx: &mut Context<Self>) {
        self.root = root;
        cx.notify();
    }

    /// Toggle the language filter (sidebar LANGUAGES) — clicking the active one
    /// clears it.
    pub fn toggle_language(&mut self, lang: SharedString, cx: &mut Context<Self>) {
        self.language = if self.language.as_ref() == Some(&lang) {
            None
        } else {
            Some(lang)
        };
        cx.notify();
    }

    /// The current filter combo as a `SavedView` (with a generated name).
    fn current_view(&self) -> SavedView {
        let root = self.root.as_ref().map(|r| r.to_string());
        let language = self.language.as_ref().map(|l| l.to_string());
        // Name from the active facets, e.g. "Dirty · Rust · Orrery"; "All repos"
        // when nothing is narrowed.
        let mut parts: Vec<String> = Vec::new();
        if self.filter != RepoFilter::All {
            parts.push(self.filter.label().to_string());
        }
        if let Some(l) = &language {
            parts.push(l.clone());
        }
        if let Some(r) = &root {
            parts.push(r.rsplit('/').next().unwrap_or(r).to_string());
        }
        let name = if parts.is_empty() {
            "All repos".to_string()
        } else {
            parts.join(" · ")
        };
        SavedView {
            name,
            filter: self.filter,
            root,
            language,
            sort: self.sort,
        }
    }

    /// Whether `v` matches the live filter combo (drives the active highlight).
    fn view_is_active(&self, v: &SavedView) -> bool {
        v.filter == self.filter
            && v.sort == self.sort
            && v.root.as_deref() == self.root.as_deref()
            && v.language.as_deref() == self.language.as_deref()
    }

    /// Save the current filter combo as a quick view (deduped by combo), persist,
    /// and refresh.
    pub fn save_current_view(&mut self, cx: &mut Context<Self>) {
        let view = self.current_view();
        if !self.saved_views.iter().any(|v| self.view_is_active(v)) {
            self.saved_views.push(view);
            persist_saved_views(&self.saved_views);
            cx.notify();
        }
    }

    /// Apply a saved quick view's filter combo.
    pub fn apply_view(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(v) = self.saved_views.get(idx) {
            self.filter = v.filter;
            self.sort = v.sort;
            self.root = v.root.clone().map(SharedString::from);
            self.language = v.language.clone().map(SharedString::from);
            cx.notify();
        }
    }

    /// Delete a saved quick view.
    pub fn delete_view(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.saved_views.len() {
            self.saved_views.remove(idx);
            persist_saved_views(&self.saved_views);
            cx.notify();
        }
    }

    /// Absolute row indices passing every active filter (chip AND root AND
    /// language), in the active sort order.
    fn visible_rows(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| self.filter.matches(r))
            .filter(|(_, r)| self.root.as_ref().is_none_or(|root| &r.root == root))
            .filter(|(_, r)| {
                self.language
                    .as_ref()
                    .is_none_or(|lang| &r.language == lang)
            })
            .map(|(i, _)| i)
            .collect();
        match self.sort {
            SortMode::Activity => v.sort_by(|&a, &b| {
                self.rows[b]
                    .last_commit_unix
                    .cmp(&self.rows[a].last_commit_unix)
            }),
            SortMode::Name => v.sort_by(|&a, &b| {
                self.rows[a]
                    .name
                    .to_lowercase()
                    .cmp(&self.rows[b].name.to_lowercase())
            }),
        }
        v
    }

    /// Refresh host enrichment (stars/topics/issues/release/visibility) from
    /// GitHub/GitLab on the tokio runtime, then reload the grid from the freshly
    /// written cache. A no-op when every repo's cache is still within the TTL
    /// (so repeated rescans cost nothing) or when offline. Network failures are
    /// silent by design — stale enrichment simply persists.
    pub fn enrich_hosts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let updated =
                crate::task::run(async move { orrery_core::enrich::refresh_cached(now).await })
                    .await;
            if updated == 0 {
                return;
            }
            // Rebuild rows from the enriched cache, off the UI thread.
            let (rows, roots) = cx
                .background_executor()
                .spawn(async { crate::data::load(crate::data::now_unix()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rows = rows;
                this.roots = roots;
                cx.notify();
            });
        })
        .detach();
    }
}

impl OrreryApp {
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
                            .text_size(px(15.))
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
            // The Inbox carries a count badge for items awaiting attention.
            if view == View::Inbox && !self.attention.is_empty() {
                item = item
                    .child(div().flex_1())
                    .child(badge(self.attention.len(), t));
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
                    .child("Scanned just now"),
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
        let (total, releases) = match &self.feed {
            FeedState::Ready(rows) => (
                rows.len(),
                rows.iter().filter(|r| r.kind.as_ref() == "release").count(),
            ),
            _ => (0, 0),
        };
        self.category_panel(
            t,
            cx,
            "FILTER",
            vec![
                (None, "rss", "All".into(), Some(total)),
                (
                    Some("release".into()),
                    "tag",
                    "Releases".into(),
                    Some(releases),
                ),
                (
                    Some("activity".into()),
                    "star",
                    "Activity".into(),
                    Some(total - releases),
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
        let total = match &self.inbox {
            InboxState::Ready(d) => d.items.len(),
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
                (None, "wrench", "All tools".into(), Some(7)),
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
                (Some("text".into()), "type", "Text".into(), Some(1)),
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
        let total = if let AgentsState::Ready(rows) = &self.agents {
            for r in rows {
                *counts.entry(r.name.as_ref()).or_default() += 1;
            }
            rows.len()
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
        use std::collections::HashMap;

        // Aggregate counts per root and per language.
        let mut root_counts: HashMap<&str, usize> = HashMap::new();
        let mut lang_counts: HashMap<&str, usize> = HashMap::new();
        for r in &self.rows {
            *root_counts.entry(r.root.as_ref()).or_default() += 1;
            let lang: &str = r.language.as_ref();
            if !lang.is_empty() {
                *lang_counts.entry(lang).or_default() += 1;
            }
        }
        // Sort by descending count, then name, for a stable order.
        let sorted = |m: HashMap<&str, usize>| {
            let mut v: Vec<(SharedString, usize)> = m
                .into_iter()
                .map(|(k, n)| (SharedString::from(k.to_string()), n))
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            v
        };
        let roots = sorted(root_counts);
        let langs = sorted(lang_counts);

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
        if self.saved_views.is_empty() {
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
            for (i, v) in self.saved_views.iter().enumerate() {
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
            self.root.is_none(),
            t,
            cx.listener(|this, _e, _w, cx| this.set_root(None, cx)),
        ));
        for (root, n) in roots {
            let active = self.root.as_ref() == Some(&root);
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
        for (lang, n) in langs {
            let active = self.language.as_ref() == Some(&lang);
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
                let cloned: std::collections::HashSet<SharedString> =
                    self.rows.iter().map(|r| r.slug.clone()).collect();
                crate::views::explore::render(
                    &self.explore,
                    &cloned,
                    &self.explore_cloning,
                    self.view_filter.as_deref(),
                    t,
                    &cx.entity(),
                )
                .into_any_element()
            }
            View::Janitor => crate::views::cleanup::render(
                &self.cleanup,
                self.view_filter.as_deref(),
                t,
                &cx.entity(),
            )
            .into_any_element(),
            View::Agents => crate::views::agents::render(
                &self.agents,
                self.view_filter.as_deref(),
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
                    self.github_authed,
                    &self.github_device,
                    &self.ai_status,
                    t,
                    &cx.entity(),
                )
                .into_any_element(),
                None => placeholder(View::Settings, t).into_any_element(),
            },
        }
    }

    fn grid(&self, t: &Theme, cx: &mut Context<Self>, cols: usize) -> impl IntoElement {
        // The contribution graph sits pinned above the toolbar + scrolling cards.
        let band = match (self.activity_open, &self.activity) {
            (true, Some(activity)) => {
                Some(crate::heatmap::render(activity, t, &cx.entity()).into_any_element())
            }
            _ => None,
        };
        let visible = self.visible_rows();
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.page))
            .children(band)
            .child(self.toolbar(t, cx, visible.len()))
            .child(self.filter_chips(t, cx))
            .child(self.card_list(t, cx, cols, visible))
    }

    /// The "All repos · N repos" heading + right-aligned action buttons.
    fn toolbar(&self, t: &Theme, cx: &mut Context<Self>, count: usize) -> impl IntoElement {
        let title = if self.filter == RepoFilter::All {
            "All repos".to_string()
        } else {
            format!("{} repos", self.filter.label())
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .pt(px(14.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(t.text_h3))
                    .text_color(rgb(t.fg0))
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg2))
                    .child(SharedString::from(format!("{count} repos"))),
            )
            .child(div().flex_1())
            // Contribution-graph toggle (active when shown).
            .child(tool_btn(
                "tb-activity",
                "activity",
                None,
                self.activity_open,
                t,
                cx.listener(|this, _ev, _w, cx| this.toggle_activity(cx)),
            ))
            // Force-refresh host enrichment.
            .child(tool_btn(
                "tb-fetch",
                "cloud-download",
                Some("Fetch all"),
                false,
                t,
                cx.listener(|this, _ev, _w, cx| this.fetch_all_hosts(cx)),
            ))
            // Sort order (cycles Activity ↔ Name).
            .child(tool_btn(
                "tb-sort",
                "arrow-up-down",
                Some(self.sort.label()),
                false,
                t,
                cx.listener(|this, _ev, _w, cx| this.cycle_sort(cx)),
            ))
    }

    /// The single-select quick-filter chips (All / Public / … / Stale).
    fn filter_chips(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(7.))
            .px(px(16.))
            .py(px(12.));
        let hov = t.border_strong;
        for f in RepoFilter::ORDER {
            let active = self.filter == f;
            let (bg, border, fg) = if active {
                (t.accent_wash, t.border_accent, t.accent_bright)
            } else {
                (t.button_bg, t.border, t.fg1)
            };
            let mut chip = div()
                .id(SharedString::from(format!("chip-{}", f.label())))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .px(px(11.))
                .py(px(5.))
                .rounded_full()
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(border))
                .text_size(px(t.text_small))
                .text_color(rgb(fg))
                .cursor_pointer()
                .hover(move |s| s.border_color(rgb(hov)))
                .on_click(cx.listener(move |this, _ev, _w, cx| this.set_filter(f, cx)));
            if let Some(icon) = f.icon() {
                chip = chip.child(lucide(icon, 13., fg));
            }
            row = row.child(chip.child(SharedString::from(f.label())));
        }
        row
    }

    fn card_list(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
        cols: usize,
        visible: Vec<usize>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let theme = self.theme.clone();
        let ide = self.config.ide_command.clone();
        let agent = self.config.agent_command.clone();
        let grid_rows = visible.len().div_ceil(cols);
        // uniform_list needs one row height, so size it to the tallest card. The
        // AI-summary line is all-or-nothing per user (gated on aiReady), so pick
        // the taller height only when summaries are present — keeping cards snug
        // either way rather than clipping the launcher row at the bottom.
        let has_ai = visible.iter().any(|&i| !self.rows[i].ai_summary.is_empty());
        let row_h = if has_ai { ROW_H_AI } else { ROW_H };

        gpui::uniform_list("repo-grid", grid_rows, move |range, _win, cx| {
            let app = entity.read(cx);
            range
                .map(|gi| {
                    let start = gi * cols;
                    let end = (start + cols).min(visible.len());
                    // Map each grid slot to its absolute row index (so the card's
                    // favorite toggle keeps editing the right `rows[idx]`).
                    let mut cells: Vec<gpui::AnyElement> = visible[start..end]
                        .iter()
                        .map(|&i| {
                            card(&app.rows[i], i, &theme, &entity, &ide, &agent).into_any_element()
                        })
                        .collect();
                    while cells.len() < cols {
                        cells.push(div().flex_1().min_w(px(0.)).into_any_element());
                    }
                    // w_full so the row fills the list width and the flex_1 cells
                    // divide it equally — otherwise the row shrink-wraps to the
                    // cards' content width and overflows horizontally.
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_stretch()
                        .h(px(row_h))
                        .gap(px(12.))
                        .px(px(16.))
                        .py(px(8.))
                        .children(cells)
                        .into_any_element()
                })
                .collect()
        })
        .flex_1()
        .size_full()
        .bg(rgb(t.page))
    }
}

/// A toolbar action button: a lucide icon with an optional label, highlighted
/// when `active`. `on` fires on click.
fn tool_btn(
    id: &'static str,
    icon: &'static str,
    label: Option<&str>,
    active: bool,
    t: &Theme,
    on: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let (bg, border, fg) = if active {
        (t.accent_wash, t.border_accent, t.accent_bright)
    } else {
        (t.button_bg, t.border, t.fg1)
    };
    let hov = t.border_strong;
    let mut b = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(border))
        .text_size(px(t.text_small))
        .text_color(rgb(fg))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov)))
        .on_click(on)
        .child(lucide(icon, 15., fg));
    if let Some(label) = label {
        b = b.child(SharedString::from(label.to_string()));
    }
    b
}

/// Responsive column count from the window width: aim for ~340px-wide cards
/// (after the 236px sidebar), clamped to a sensible range.
fn columns(viewport_width: f32) -> usize {
    (((viewport_width - 236.) / 340.).floor() as usize).clamp(1, 6)
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
                    )
                    .into_any_element(),
                )
            }
            Some(Overlay::Palette(data)) => {
                let query = data.query.read(cx).value();
                let items = crate::palette::items(&self.rows, &data.code, &data.semantic, &query);
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

/// A small count pill for the sidebar (e.g. Inbox attention items).
fn badge(n: usize, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .bg(rgb(t.accent_badge))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.accent_bright))
        .child(SharedString::from(n.to_string()))
}

/// Scaffold for a not-yet-ported view: centered title + note.
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
                .text_size(px(22.))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg2))
                .child(SharedString::from(sub.to_string())),
        )
        .child(
            div()
                .mt(px(6.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(t.r_xs))
                .border_1()
                .border_color(rgb(t.border))
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child("Phase 2 scaffold"),
        )
}
