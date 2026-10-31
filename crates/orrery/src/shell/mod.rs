//! App shell — the chrome that wraps every view: the 52px header (brand,
//! roots·repos, search, new/rescan), the 236px left rail with the 8 primary nav
//! items, and the main column.
//!
//! The nav is live: clicking an item switches the active `View`; each view loads
//! its data lazily on first selection.

use std::rc::Rc;

use gpui::{
    Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div, px, rgb,
};
use gpui_component::input::{InputEvent, InputState};

use orrery_core::attention::{AttentionItem, Severity};
use orrery_core::model::AppConfig;

use crate::data::Row;
use crate::theme::Theme;

mod actions;
mod attention;
mod confirm;
mod grid;
mod header;
mod overlay;
mod passes;
mod sidebar;

use grid::{Facets, GridState, VisibleKey, columns};
use passes::{Passes, SnapKind};

pub(crate) use actions::draft_and_create_pr;
pub(crate) use confirm::ArmedConfirm;

pub(crate) use overlay::{DrawerTab, Overlay};

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
    /// Cleanup view UI state (the armed Prune confirm).
    pub(crate) cleanup_ui: crate::views::cleanup::CleanupUi,
    /// Explore view UI state (clones in flight, clone errors).
    pub(crate) explore_ui: crate::views::explore::ExploreUi,
    /// Agents view state (lazy; detected agent sessions on the machine).
    pub agents: crate::views::agents::AgentsState,
    /// Agents view UI state (poll guard, armed confirm, review panel, PR
    /// flows in flight).
    pub(crate) agents_ui: crate::views::agents::AgentsUi,
    /// Repo ids (paths) with a live agent session — drives the card indicator.
    /// Refreshed on rescan and by the Agents-view poll.
    pub active_agents: std::collections::HashSet<SharedString>,
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
                        cloning: &self.explore_ui.cloning,
                        errors: &self.explore_ui.errors,
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
                self.cleanup_ui.confirm.armed(),
                t,
                &cx.entity(),
            )
            .into_any_element(),
            View::Agents => crate::views::agents::render(
                &self.agents,
                self.view_filter.as_deref(),
                self.agents_ui.confirm.armed(),
                self.agents_ui
                    .review
                    .as_deref()
                    .map(|p| (p, self.agents_ui.review_diff.as_deref())),
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
