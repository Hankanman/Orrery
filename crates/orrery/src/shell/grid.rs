//! Mission Control: the grid's filter/sort/layout state and saved views,
//! the cached visible-rows and facet derivations, and the toolbar, filter
//! chips and card grid/list it renders.

use std::rc::Rc;

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use orrery_core::attention::Severity;

use super::{OrreryApp, SnapKind};
use crate::card::card;
use crate::data::Row;
use crate::icon::lucide;
use crate::theme::Theme;
use crate::widgets::{self, Tone};

/// Grid row height without / with AI summary lines (the launcher row is the
/// bottom of the card, so the row must be tall enough not to clip it).
const ROW_H: f32 = 260.;

const ROW_H_AI: f32 = 288.;

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
    /// Repos with at least one attention item (`orrery_core::attention`) —
    /// dirty/unpushed, review requests, prunable branches, agent sessions, ….
    /// Toolbar-only (not a chip), driven by the "Attention" button.
    Attention,
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
            RepoFilter::Attention => "Attention",
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
            RepoFilter::Attention => Some("circle-alert"),
            RepoFilter::All => None,
        }
    }

    /// Does `row` pass this filter? `attention` is the app's per-repo severity
    /// lookup (`OrreryApp::attention_by_repo`) — only the Attention filter
    /// consults it.
    fn matches(
        self,
        r: &Row,
        attention: &std::collections::HashMap<SharedString, Severity>,
    ) -> bool {
        use orrery_core::model::Activity;
        match self {
            RepoFilter::All => true,
            RepoFilter::Public => !r.private,
            RepoFilter::Private => r.private,
            RepoFilter::Dirty => r.dirty > 0,
            RepoFilter::Ahead => r.ahead > 0,
            RepoFilter::Starred => r.favorite,
            RepoFilter::Stale => r.activity == Activity::Stale,
            // Repos that need action now (Urgent / Attention), matching the
            // toolbar count; Info-only state (behind, hygiene) is left out.
            RepoFilter::Attention => attention.get(&r.id).is_some_and(|s| *s != Severity::Info),
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
    /// Most host stars first; ties (including every unstarred repo) by name.
    Stars,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            SortMode::Activity => "Activity",
            SortMode::Name => "Name",
            SortMode::Stars => "Stars",
        }
    }

    fn next(self) -> SortMode {
        match self {
            SortMode::Activity => SortMode::Name,
            SortMode::Name => SortMode::Stars,
            SortMode::Stars => SortMode::Activity,
        }
    }
}

/// Order the visible row indices `v` by `mode`. `stars(i)` is row `i`'s host
/// star count (the flat [`Row`] only carries it pre-formatted). Stable, so
/// equal keys keep their incoming order.
fn sort_visible(v: &mut [usize], rows: &[Row], stars: impl Fn(usize) -> u32, mode: SortMode) {
    match mode {
        SortMode::Activity => {
            v.sort_by(|&a, &b| rows[b].last_commit_unix.cmp(&rows[a].last_commit_unix))
        }
        // Lowercase each name once (not twice per comparison).
        SortMode::Name => v.sort_by_cached_key(|&i| rows[i].name.to_lowercase()),
        SortMode::Stars => {
            v.sort_by_cached_key(|&i| (std::cmp::Reverse(stars(i)), rows[i].name.to_lowercase()))
        }
    }
}

/// Mission Control card layout: the multi-column card grid, or a compact
/// single-column list.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Grid,
    List,
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
    /// The sidebar TAGS filter, if the view narrows to one tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// SQLite meta key holding the saved-views JSON array.
const SAVED_VIEWS_KEY: &str = "saved_views";

/// Load persisted saved views from the cache (empty if none / unparseable).
pub fn load_saved_views() -> Vec<SavedView> {
    orrery_core::cache::get_meta(SAVED_VIEWS_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Persist the saved views (off the UI thread, in order).
fn persist_saved_views(views: &[SavedView]) {
    if let Ok(json) = serde_json::to_string(views) {
        crate::task::persist_meta(SAVED_VIEWS_KEY, json);
    }
}

/// Mission Control's UI state, grouped out of [`OrreryApp`]: the quick filter,
/// root/language facets, sort + layout, persisted saved views, and the
/// contribution graph.
pub struct GridState {
    /// Active quick filter (All = no filtering).
    pub filter: RepoFilter,
    /// Active scanned-root filter (sidebar ROOTS); `None` = all roots.
    pub root: Option<SharedString>,
    /// Active language filter (sidebar LANGUAGES); `None` = all languages.
    pub language: Option<SharedString>,
    /// Active tag filter (sidebar TAGS); `None` = any tags.
    pub tag: Option<SharedString>,
    /// Every repo's project tags (see `tags.rs`), loaded at launch.
    pub tags: super::tags::Tags,
    /// Card ordering.
    pub sort: SortMode,
    /// Card layout (grid vs. compact list).
    pub layout: Layout,
    /// Persisted quick views (sidebar VIEWS), loaded from the cache at launch.
    pub saved_views: Vec<SavedView>,
    /// Contribution-graph data (commits/day across repos), computed in the
    /// background; `None` until the first pass lands.
    pub activity: Option<orrery_core::activity::Activity>,
    /// Whether the contribution graph is shown (dismissible).
    pub activity_open: bool,
}

impl Default for GridState {
    fn default() -> Self {
        GridState {
            filter: RepoFilter::default(),
            root: None,
            language: None,
            tag: None,
            tags: super::tags::Tags::load(),
            sort: SortMode::default(),
            layout: Layout::default(),
            saved_views: load_saved_views(),
            activity: None,
            activity_open: true,
        }
    }
}

/// The cache key for [`Derived::visible`]: everything `visible_rows` reads.
#[derive(Clone, PartialEq)]
pub(super) struct VisibleKey {
    rows_gen: u64,
    attention_gen: u64,
    filter: RepoFilter,
    root: Option<SharedString>,
    language: Option<SharedString>,
    tag: Option<SharedString>,
    tags_gen: u64,
    sort: SortMode,
}

/// The sidebar ROOTS / LANGUAGES facets: (value, repo count), most repos first.
pub struct Facets {
    pub(super) roots: Vec<(SharedString, usize)>,
    pub(super) langs: Vec<(SharedString, usize)>,
}

/// Aggregate the sidebar facets: repos per root and per (known) language,
/// most repos first, then by name for a stable order.
fn compute_facets(rows: &[Row]) -> Facets {
    use std::collections::HashMap;
    let mut root_counts: HashMap<&str, usize> = HashMap::new();
    let mut lang_counts: HashMap<&str, usize> = HashMap::new();
    for r in rows {
        *root_counts.entry(r.root.as_ref()).or_default() += 1;
        let lang: &str = r.language.as_ref();
        if !lang.is_empty() {
            *lang_counts.entry(lang).or_default() += 1;
        }
    }
    let sorted = |m: HashMap<&str, usize>| {
        let mut v: Vec<(SharedString, usize)> = m
            .into_iter()
            .map(|(k, n)| (SharedString::from(k.to_string()), n))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    Facets {
        roots: sorted(root_counts),
        langs: sorted(lang_counts),
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
    let (tone, fg) = if active {
        (Tone::Active, t.accent_bright)
    } else {
        (Tone::Neutral, t.fg1)
    };
    let mut content = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .text_size(px(t.text_small))
        .child(lucide(icon, 15., fg));
    if let Some(label) = label {
        content = content.child(SharedString::from(label.to_string()));
    }
    widgets::button(id, tone, t)
        .px(px(10.))
        .py(px(6.))
        .on_click(on)
        .child(content)
}

/// Responsive column count from the window width: aim for ~340px-wide cards
/// (after the 236px sidebar), clamped to a sensible range.
pub(super) fn columns(viewport_width: f32) -> usize {
    (((viewport_width - 236.) / 340.).floor() as usize).clamp(1, 6)
}

impl OrreryApp {
    /// Set the active Mission Control quick filter.
    pub fn set_filter(&mut self, f: RepoFilter, cx: &mut Context<Self>) {
        self.grid.filter = f;
        cx.notify();
    }

    /// Cycle the Mission Control sort order.
    pub fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        self.grid.sort = self.grid.sort.next();
        cx.notify();
    }

    /// Switch the Mission Control card layout (grid ↔ list).
    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        self.grid.layout = layout;
        cx.notify();
    }

    /// Toggle the "Attention" filter (repos with attention items).
    pub fn toggle_attention(&mut self, cx: &mut Context<Self>) {
        self.grid.filter = if self.grid.filter == RepoFilter::Attention {
            RepoFilter::All
        } else {
            RepoFilter::Attention
        };
        cx.notify();
    }

    /// How many repos need action now — the same count as the nav badge.
    fn attention_count(&self) -> usize {
        self.needs_you().0
    }

    /// Show/hide the contribution graph.
    pub fn toggle_activity(&mut self, cx: &mut Context<Self>) {
        self.grid.activity_open = !self.grid.activity_open;
        cx.notify();
    }

    /// Force-refresh host enrichment for every repo (ignores the TTL), then
    /// reload the grid. The toolbar's "Fetch all".
    pub fn fetch_all_hosts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let now = crate::data::now_unix();
            let updated =
                crate::task::run(async move { orrery_core::enrich::refresh_cached_all(now).await })
                    .await
                    .unwrap_or(0);
            if updated > 0 {
                let _ = this.update(cx, |this, cx| this.request_snapshot(SnapKind::Reload, cx));
            }
        })
        .detach();
    }

    /// Select the scanned root to filter by (sidebar ROOTS); `None` = all.
    pub fn set_root(&mut self, root: Option<SharedString>, cx: &mut Context<Self>) {
        self.grid.root = root;
        cx.notify();
    }

    /// Toggle the language filter (sidebar LANGUAGES) — clicking the active one
    /// clears it.
    pub fn toggle_language(&mut self, lang: SharedString, cx: &mut Context<Self>) {
        self.grid.language = if self.grid.language.as_ref() == Some(&lang) {
            None
        } else {
            Some(lang)
        };
        cx.notify();
    }

    /// The current filter combo as a `SavedView` (with a generated name).
    fn current_view(&self) -> SavedView {
        let root = self.grid.root.as_ref().map(|r| r.to_string());
        let language = self.grid.language.as_ref().map(|l| l.to_string());
        let tag = self.grid.tag.as_ref().map(|t| t.to_string());
        // Name from the active facets, e.g. "Dirty · #work · Rust · Orrery";
        // "All repos" when nothing is narrowed.
        let mut parts: Vec<String> = Vec::new();
        if self.grid.filter != RepoFilter::All {
            parts.push(self.grid.filter.label().to_string());
        }
        if let Some(t) = &tag {
            parts.push(format!("#{t}"));
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
            filter: self.grid.filter,
            root,
            language,
            sort: self.grid.sort,
            tag,
        }
    }

    /// Whether `v` matches the live filter combo (drives the active highlight).
    pub(super) fn view_is_active(&self, v: &SavedView) -> bool {
        v.filter == self.grid.filter
            && v.sort == self.grid.sort
            && v.root.as_deref() == self.grid.root.as_deref()
            && v.language.as_deref() == self.grid.language.as_deref()
            && v.tag.as_deref() == self.grid.tag.as_deref()
    }

    /// Save the current filter combo as a quick view (deduped by combo), persist,
    /// and refresh.
    pub fn save_current_view(&mut self, cx: &mut Context<Self>) {
        let view = self.current_view();
        if !self.grid.saved_views.iter().any(|v| self.view_is_active(v)) {
            self.grid.saved_views.push(view);
            persist_saved_views(&self.grid.saved_views);
            cx.notify();
        }
    }

    /// Apply a saved quick view's filter combo.
    pub fn apply_view(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(v) = self.grid.saved_views.get(idx) {
            self.grid.filter = v.filter;
            self.grid.sort = v.sort;
            self.grid.root = v.root.clone().map(SharedString::from);
            self.grid.language = v.language.clone().map(SharedString::from);
            self.grid.tag = v.tag.clone().map(SharedString::from);
            cx.notify();
        }
    }

    /// Delete a saved quick view.
    pub fn delete_view(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.grid.saved_views.len() {
            self.grid.saved_views.remove(idx);
            persist_saved_views(&self.grid.saved_views);
            cx.notify();
        }
    }

    /// Absolute row indices passing every active filter (chip AND root AND
    /// language), in the active sort order. Cached until the rows, the
    /// attention model, or the filters/sort change (it used to be rebuilt on
    /// every frame).
    pub(crate) fn visible_rows(&self) -> Rc<Vec<usize>> {
        let key = VisibleKey {
            rows_gen: self.derived.rows_gen,
            attention_gen: self.derived.attention_gen,
            filter: self.grid.filter,
            root: self.grid.root.clone(),
            language: self.grid.language.clone(),
            tag: self.grid.tag.clone(),
            tags_gen: self.grid.tags.generation(),
            sort: self.grid.sort,
        };
        if let Some((k, v)) = &*self.derived.visible.borrow()
            && *k == key
        {
            return v.clone();
        }
        let v = Rc::new(self.compute_visible_rows());
        *self.derived.visible.borrow_mut() = Some((key, v.clone()));
        v
    }

    fn compute_visible_rows(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| self.grid.filter.matches(r, &self.attention_by_repo))
            .filter(|(_, r)| self.grid.root.as_ref().is_none_or(|root| &r.root == root))
            .filter(|(_, r)| {
                self.grid
                    .language
                    .as_ref()
                    .is_none_or(|lang| &r.language == lang)
            })
            .filter(|(_, r)| {
                self.grid
                    .tag
                    .as_ref()
                    .is_none_or(|tag| self.grid.tags.of(&r.id).contains(tag))
            })
            .map(|(i, _)| i)
            .collect();
        // Stars come from the raw snapshot, which `apply_snapshot` keeps in
        // lockstep with `rows` (same order, same length).
        let stars = |i: usize| self.repos.get(i).map_or(0, |r| r.stars);
        sort_visible(&mut v, &self.rows, stars, self.grid.sort);
        // The Attention filter ranks the most urgent repos first; the sort is
        // stable, so the active sort still orders repos within each severity
        // tier.
        if self.grid.filter == RepoFilter::Attention {
            v.sort_by_key(|&i| {
                self.attention_by_repo
                    .get(&self.rows[i].id)
                    .copied()
                    .unwrap_or(Severity::Info)
            });
        }
        v
    }

    /// The sidebar ROOTS / LANGUAGES facet counts, cached per rows generation.
    pub(super) fn facets(&self) -> Rc<Facets> {
        let generation = self.derived.rows_gen;
        if let Some((g, f)) = &*self.derived.facets.borrow()
            && *g == generation
        {
            return f.clone();
        }
        let f = Rc::new(compute_facets(&self.rows));
        *self.derived.facets.borrow_mut() = Some((generation, f.clone()));
        f
    }

    /// Slugs of the repos already in the fleet (Explore's "cloned" marks),
    /// cached per rows generation.
    pub(super) fn cloned_slugs(&self) -> Rc<std::collections::HashSet<SharedString>> {
        let generation = self.derived.rows_gen;
        if let Some((g, s)) = &*self.derived.slugs.borrow()
            && *g == generation
        {
            return s.clone();
        }
        let s: Rc<std::collections::HashSet<SharedString>> =
            Rc::new(self.rows.iter().map(|r| r.slug.clone()).collect());
        *self.derived.slugs.borrow_mut() = Some((generation, s.clone()));
        s
    }

    /// The card state flags for `rows[idx]`: live agent session, urgent
    /// attention, and fleet selection. Cheap map lookups — fine inside the
    /// `uniform_list` closures.
    fn card_state(&self, idx: usize, selecting: bool) -> crate::card::CardState {
        let id = &self.rows[idx].id;
        crate::card::CardState {
            active: self.active_agents.contains(id),
            urgent: self.attention_by_repo.get(id).copied() == Some(Severity::Urgent),
            selected: selecting && self.selected.contains(id),
            selecting,
            show_ai: self.services.ai_ready,
            advisory: self.hygiene.badge(id),
        }
    }

    pub(super) fn grid(&self, t: &Theme, cx: &mut Context<Self>, cols: usize) -> impl IntoElement {
        // The contribution graph sits pinned above the toolbar + scrolling cards.
        let band = match (self.grid.activity_open, &self.grid.activity) {
            (true, Some(activity)) => {
                Some(crate::heatmap::render(activity, t, &cx.entity()).into_any_element())
            }
            _ => None,
        };
        let visible = self.visible_rows();
        // The fleet bar (multi-select bulk ops) pins under the scrolling cards;
        // `None` (no element at all) until a selection exists or a run is live.
        let fleet_bar = self.fleet_bar(t, cx, visible.len());
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.page))
            .children(band)
            .child(self.toolbar(t, cx, visible.len()))
            .child(self.filter_chips(t, cx))
            .child(self.card_list(t, cx, cols, visible))
            .children(fleet_bar)
    }

    /// The "All repos · N repos" heading + right-aligned action buttons.
    fn toolbar(&self, t: &Theme, cx: &mut Context<Self>, count: usize) -> impl IntoElement {
        let title = if self.grid.filter == RepoFilter::All {
            "All repos".to_string()
        } else {
            format!("{} repos", self.grid.filter.label())
        };
        let attention_label = format!("Attention {}", self.attention_count());
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
                self.grid.activity_open,
                t,
                cx.listener(|this, _ev, _w, cx| this.toggle_activity(cx)),
            ))
            // Attention filter: repos that are dirty / ahead / behind.
            .child(tool_btn(
                "tb-attention",
                "circle-alert",
                Some(attention_label.as_str()),
                self.grid.filter == RepoFilter::Attention,
                t,
                cx.listener(|this, _ev, _w, cx| this.toggle_attention(cx)),
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
            // Summarize (local AI) — hidden unless a backend is ready.
            .children(self.services.ai_ready.then(|| {
                tool_btn(
                    "tb-summarize",
                    "sparkles",
                    Some("Summarize"),
                    false,
                    t,
                    cx.listener(|this, _ev, _w, cx| this.summarize_all(cx)),
                )
                .into_any_element()
            }))
            // Sort order (cycles Activity ↔ Name).
            .child(tool_btn(
                "tb-sort",
                "arrow-up-down",
                Some(self.grid.sort.label()),
                false,
                t,
                cx.listener(|this, _ev, _w, cx| this.cycle_sort(cx)),
            ))
            // Layout toggle: grid vs. compact list.
            .child(tool_btn(
                "tb-grid",
                "layout-grid",
                None,
                self.grid.layout == Layout::Grid,
                t,
                cx.listener(|this, _ev, _w, cx| this.set_layout(Layout::Grid, cx)),
            ))
            .child(tool_btn(
                "tb-list",
                "list",
                None,
                self.grid.layout == Layout::List,
                t,
                cx.listener(|this, _ev, _w, cx| this.set_layout(Layout::List, cx)),
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
            let active = self.grid.filter == f;
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
        visible: Rc<Vec<usize>>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let theme = self.theme.clone();
        let ide = self.config.ide_command.clone();
        let agent = self.config.agent_command.clone();

        let list = match self.grid.layout {
            // Compact single-column list: one repo per row.
            Layout::List => {
                gpui::uniform_list("repo-list", visible.len(), move |range, _win, cx| {
                    let app = entity.read(cx);
                    let selecting = !app.selected.is_empty();
                    range
                        .map(|i| {
                            let abs = visible[i];
                            crate::card::list_item(
                                &app.rows[abs],
                                abs,
                                &theme,
                                &entity,
                                &ide,
                                &agent,
                                app.card_state(abs, selecting),
                                app.grid.tags.of(&app.rows[abs].id),
                            )
                            .into_any_element()
                        })
                        .collect()
                })
            }
            // Multi-column card grid (one uniform_list row = `cols` cards).
            Layout::Grid => {
                let grid_rows = visible.len().div_ceil(cols);
                // uniform_list needs one row height, so size it to the tallest
                // card. A card only renders its AI-summary line while AI is
                // ready, so take the taller height only then, and only when a
                // visible card actually has a summary — keeping cards snug
                // either way rather than clipping the launcher row at the
                // bottom.
                let has_ai = self.services.ai_ready
                    && visible.iter().any(|&i| !self.rows[i].ai_summary.is_empty());
                // Likewise the tag-chip line, only while a visible card has tags.
                let has_tags = visible
                    .iter()
                    .any(|&i| !self.grid.tags.of(&self.rows[i].id).is_empty());
                let row_h = if has_ai { ROW_H_AI } else { ROW_H }
                    + if has_tags {
                        crate::card::TAGS_LINE_H
                    } else {
                        0.
                    };
                gpui::uniform_list("repo-grid", grid_rows, move |range, _win, cx| {
                    let app = entity.read(cx);
                    let selecting = !app.selected.is_empty();
                    range
                        .map(|gi| {
                            let start = gi * cols;
                            let end = (start + cols).min(visible.len());
                            // Map each grid slot to its absolute row index (so the
                            // card's favorite toggle keeps editing the right row).
                            let mut cells: Vec<gpui::AnyElement> = visible[start..end]
                                .iter()
                                .map(|&i| {
                                    let state = app.card_state(i, selecting);
                                    let tags = app.grid.tags.of(&app.rows[i].id);
                                    card(
                                        &app.rows[i],
                                        i,
                                        &theme,
                                        &entity,
                                        &ide,
                                        &agent,
                                        state,
                                        tags,
                                    )
                                    .into_any_element()
                                })
                                .collect();
                            while cells.len() < cols {
                                cells.push(div().flex_1().min_w(px(0.)).into_any_element());
                            }
                            // w_full so the row fills the list width and the flex_1
                            // cells divide it equally — otherwise the row shrink-
                            // wraps to content width and overflows horizontally.
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
            }
        };
        list.flex_1().size_full().bg(rgb(t.page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_sort_most_first_then_by_name() {
        let rows: Vec<Row> = ["beta", "Alpha", "gamma", "delta"]
            .iter()
            .map(|n| {
                let mut r = row_named(n);
                r.last_commit_unix = n.len() as i64;
                r
            })
            .collect();
        let stars = [5u32, 0, 5, 12];
        let mut v: Vec<usize> = (0..rows.len()).collect();
        sort_visible(&mut v, &rows, |i| stars[i], SortMode::Stars);
        // 12 first, then the two 5s by name (beta < gamma), then the 0.
        assert_eq!(v, vec![3, 0, 2, 1]);
        sort_visible(&mut v, &rows, |i| stars[i], SortMode::Name);
        assert_eq!(v, vec![1, 0, 3, 2], "Alpha, beta, delta, gamma");
        // The toolbar cycles through all three.
        assert!(SortMode::Activity.next() == SortMode::Name);
        assert!(SortMode::Name.next() == SortMode::Stars);
        assert!(SortMode::Stars.next() == SortMode::Activity);
    }

    #[test]
    fn saved_views_without_a_tag_still_load() {
        let old =
            r#"[{"name":"Dirty","filter":"Dirty","root":null,"language":null,"sort":"Name"}]"#;
        let views: Vec<SavedView> = serde_json::from_str(old).unwrap();
        assert_eq!(views[0].tag, None);
        let json = serde_json::to_string(&SavedView {
            tag: Some("work".into()),
            ..views[0].clone()
        })
        .unwrap();
        let back: Vec<SavedView> = serde_json::from_str(&format!("[{json}]")).unwrap();
        assert_eq!(back[0].tag.as_deref(), Some("work"));
    }

    fn row_named(name: &str) -> Row {
        Row {
            id: name.to_string().into(),
            url: "".into(),
            name: name.to_string().into(),
            slug: "".into(),
            root: "".into(),
            path: "".into(),
            description: "".into(),
            language: "".into(),
            branch: "".into(),
            release: "".into(),
            ai_summary: "".into(),
            ahead: 0,
            behind: 0,
            dirty: 0,
            stars: "".into(),
            host: "".into(),
            private: false,
            favorite: false,
            activity: orrery_core::model::Activity::Active,
            last_commit_unix: 0,
            worktrees: Vec::new(),
        }
    }

    #[test]
    fn facets_count_and_order_by_size_then_name() {
        let row = |root: &str, lang: &str| Row {
            root: root.to_string().into(),
            language: lang.to_string().into(),
            ..row_named("")
        };
        let rows = [
            row("/b", "Rust"),
            row("/a", ""),
            row("/b", "Go"),
            row("/a", "Rust"),
        ];
        let f = compute_facets(&rows);
        let names = |v: &[(SharedString, usize)]| -> Vec<(String, usize)> {
            v.iter().map(|(k, n)| (k.to_string(), *n)).collect()
        };
        assert_eq!(names(&f.roots), vec![("/a".into(), 2), ("/b".into(), 2)]);
        // Unknown language ("") is left out.
        assert_eq!(names(&f.langs), vec![("Rust".into(), 2), ("Go".into(), 1)]);
    }
}
