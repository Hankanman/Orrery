//! Command palette (Ctrl+K) — a centered overlay with a query field over a
//! filtered list of actions, repositories (by substring and, when AI is ready,
//! by meaning), and cross-repo code-search hits. Arrow keys move the selection,
//! Enter runs it, Esc closes.
//!
//! State lives in `Overlay::Palette(PaletteData)`; the action handlers + executor
//! are methods on `OrreryApp` (shell.rs). This module owns the item model + the
//! rendering.

use gpui::{
    Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, div, px, rgb, rgba,
};

use gpui_component::input::{Input, InputState};

use crate::data::Row;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

const PANEL_W: f32 = 640.;
/// Cap on repo rows shown, to keep the list quick.
const MAX_REPOS: usize = 40;

/// A cross-repo code-search result (flattened from `search::SearchHit`).
pub struct CodeHit {
    pub file: SharedString, // path relative to its repo, for display
    pub line: u32,
    pub text: SharedString, // the matching line
    pub abs: SharedString,  // absolute path, for launching the editor
}

/// A semantic recall hit: a repo (by id) plus the chunk text that matched,
/// shown as context under the repo name.
#[derive(Clone)]
pub struct SemanticHit {
    pub id: SharedString,
    pub snippet: SharedString,
}

/// Live palette state.
pub struct PaletteData {
    pub query: Entity<InputState>,
    pub selected: usize,
    /// Cross-repo ripgrep results for the current query (debounced).
    pub code: Vec<CodeHit>,
    /// Repos ranked by semantic similarity to the query (debounced; empty
    /// unless AI is ready), surfaced below the name matches with their
    /// matching snippet.
    pub semantic: Vec<SemanticHit>,
    /// The stored embedding index, loaded once per palette open (never per
    /// keystroke); `None` until the load lands or when AI is off.
    pub embeddings: Option<std::sync::Arc<Vec<orrery_core::semantic::EmbeddingRow>>>,
    /// Session cache of query → embedding, so re-typing a query (or ranking
    /// after the index load) never re-hits the backend.
    pub query_vecs: std::collections::HashMap<String, std::sync::Arc<Vec<f32>>>,
    /// Query generation, for debouncing/dropping stale searches.
    pub generation: u64,
    /// Keeps the query-observation alive (re-renders the app on each keystroke).
    pub _sub: Subscription,
}

/// Flatten a core search hit into a render-ready [`CodeHit`].
pub fn code_hit(h: orrery_core::search::SearchHit) -> CodeHit {
    CodeHit {
        file: h.file.into(),
        line: h.line,
        text: h.text.into(),
        abs: h.abs.into(),
    }
}

/// A standing command (not tied to a single repo). The fleet verbs (#184)
/// drive the same run/selection plumbing as the fleet bar (`fleet.rs`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Rescan,
    Settings,
    /// Bulk fetch across every repo — no selection needed.
    FetchAll,
    /// Bulk pull across every repo with behind > 0 — no selection needed.
    PullBehind,
    /// Bulk fetch across the current selection (listed only when one exists).
    FetchSelected,
    /// Bulk pull across the current selection (listed only when one exists).
    PullSelected,
    /// Replace the selection with the repos with uncommitted changes.
    SelectDirty,
    /// Replace the selection with the repos behind their upstream.
    SelectBehind,
}

impl PaletteAction {
    fn label(self) -> &'static str {
        match self {
            PaletteAction::Rescan => "Rescan repositories",
            PaletteAction::Settings => "Open settings",
            PaletteAction::FetchAll => "Fetch all repositories",
            PaletteAction::PullBehind => "Pull all repositories behind upstream",
            PaletteAction::FetchSelected => "Fetch selected repositories",
            PaletteAction::PullSelected => "Pull selected repositories",
            PaletteAction::SelectDirty => "Select dirty repositories",
            PaletteAction::SelectBehind => "Select repositories behind upstream",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            PaletteAction::Rescan => "refresh-cw",
            PaletteAction::Settings => "settings",
            PaletteAction::FetchAll | PaletteAction::FetchSelected => "refresh-cw",
            PaletteAction::PullBehind | PaletteAction::PullSelected => "cloud-download",
            PaletteAction::SelectDirty | PaletteAction::SelectBehind => "circle-check",
        }
    }
}

/// One palette result.
#[derive(Clone)]
pub enum PaletteItem {
    Action(PaletteAction),
    /// Index into `OrreryApp::rows`.
    Repo(usize),
    /// A semantic recall hit: the repo's index into `OrreryApp::rows` plus its
    /// index into `PaletteData::semantic` (for the snippet).
    Recall {
        row: usize,
        hit: usize,
    },
    /// Index into `PaletteData::code`.
    Code(usize),
}

/// The standing commands offered for the current app state. The
/// selection-scoped fleet verbs only list while a selection exists — with
/// none they'd be dead rows.
fn actions(has_selection: bool) -> Vec<PaletteAction> {
    let mut out = vec![
        PaletteAction::Rescan,
        PaletteAction::Settings,
        PaletteAction::FetchAll,
        PaletteAction::PullBehind,
        PaletteAction::SelectDirty,
        PaletteAction::SelectBehind,
    ];
    if has_selection {
        out.push(PaletteAction::FetchSelected);
        out.push(PaletteAction::PullSelected);
    }
    out
}

/// How strongly a repo's name matches `q` (already lowercased): 0 exact (name,
/// slug, or the slug's repo tail equals), 1 prefix, 2 substring (incl. path),
/// `None` no match. Lower ranks list first; semantic recall comes after all of
/// them, so a repo literally named like the query always beats a meaning match.
pub fn name_rank(name: &str, slug: &str, path: &str, q: &str) -> Option<u8> {
    let (name, slug, path) = (
        name.to_lowercase(),
        slug.to_lowercase(),
        path.to_lowercase(),
    );
    let tail = slug.rsplit('/').next().unwrap_or(&slug);
    if name == q || slug == q || tail == q {
        Some(0)
    } else if name.starts_with(q) || slug.starts_with(q) || tail.starts_with(q) {
        Some(1)
    } else if name.contains(q) || slug.contains(q) || path.contains(q) {
        Some(2)
    } else {
        None
    }
}

/// Build the filtered result list for `query`: actions (selection-scoped
/// fleet verbs only when `has_selection`), name-matching repos (exact/prefix
/// first, then substring), semantic recall hits the name pass missed
/// (deduped, similarity-ranked, with snippets), then code-search hits.
/// Must be deterministic — the executor rebuilds it to resolve the selected
/// index. With AI off `semantic` is always empty and this reduces to the
/// name-only palette.
pub fn items(
    rows: &[Row],
    code: &[CodeHit],
    semantic: &[SemanticHit],
    query: &str,
    has_selection: bool,
) -> Vec<PaletteItem> {
    use std::collections::HashSet;
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();
    let mut shown: HashSet<usize> = HashSet::new();

    for a in actions(has_selection) {
        if q.is_empty() || a.label().to_lowercase().contains(&q) {
            out.push(PaletteItem::Action(a));
        }
    }

    // Name matches, exact/prefix before substring; the sort is stable so grid
    // order is preserved within each rank.
    let mut matched: Vec<(u8, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if q.is_empty() {
                return Some((2, i));
            }
            name_rank(&r.name, &r.slug, &r.path, &q).map(|rank| (rank, i))
        })
        .collect();
    matched.sort_by_key(|(rank, _)| *rank);
    for (_, i) in matched.into_iter().take(MAX_REPOS) {
        if shown.insert(i) {
            out.push(PaletteItem::Repo(i));
        }
    }

    // Semantic recall the name pass didn't surface, best match first.
    if !q.is_empty() {
        for (hit, h) in semantic.iter().enumerate() {
            if let Some(row) = rows.iter().position(|r| r.id == h.id)
                && shown.insert(row)
            {
                out.push(PaletteItem::Recall { row, hit });
            }
        }
    }

    for i in 0..code.len() {
        out.push(PaletteItem::Code(i));
    }
    out
}

/// Render the palette overlay.
pub fn render(
    data: &PaletteData,
    items: &[PaletteItem],
    rows: &[Row],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let selected = data.selected.min(items.len().saturating_sub(1));

    let mut list = div().flex().flex_col().gap(px(1.)).p(px(6.));
    if items.is_empty() {
        list = list.child(
            div()
                .p(px(12.))
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg3))
                .child("No matches."),
        );
    }
    for (i, item) in items.iter().enumerate() {
        list = list.child(row_view(
            item,
            i,
            i == selected,
            rows,
            &data.code,
            &data.semantic,
            t,
            app,
        ));
    }

    let panel = div()
        .key_context("Palette")
        .flex()
        .flex_col()
        .w(px(PANEL_W))
        .max_h(px(520.))
        .rounded(px(t.r_md))
        .bg(rgb(t.page))
        .border_1()
        .border_color(rgb(t.border_strong))
        // Query field.
        .child(
            div()
                .p(px(8.))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(Input::new(&data.query).appearance(false)),
        )
        // Results.
        .child(
            div()
                .id("palette-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(list),
        );

    // Backdrop + top-centered panel.
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .occlude()
        .flex()
        .flex_col()
        .items_center()
        .bg(rgba(0x00000066))
        .child(div().h(px(80.)))
        .child(panel)
}

#[allow(clippy::too_many_arguments)]
fn row_view(
    item: &PaletteItem,
    idx: usize,
    selected: bool,
    rows: &[Row],
    code: &[CodeHit],
    semantic: &[SemanticHit],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    // `context` renders as a second line under the primary text (the semantic
    // hit's matching snippet); `secondary` stays a right-aligned single line.
    let (icon, primary, secondary, context) = match item {
        PaletteItem::Action(a) => (
            a.icon(),
            SharedString::from(a.label()),
            SharedString::default(),
            None,
        ),
        PaletteItem::Repo(i) => {
            let r = &rows[*i];
            ("box", r.name.clone(), r.slug.clone(), None)
        }
        PaletteItem::Recall { row, hit } => {
            let r = &rows[*row];
            let snippet = semantic.get(*hit).map(|h| h.snippet.clone());
            ("sparkles", r.name.clone(), r.slug.clone(), snippet)
        }
        PaletteItem::Code(i) => {
            let h = &code[*i];
            (
                "file-search",
                SharedString::from(format!("{}:{}", h.file, h.line)),
                h.text.clone(),
                None,
            )
        }
    };

    let mut main = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(div().truncate().text_size(px(t.text_small)).child(primary));
    if let Some(snippet) = context {
        main = main.child(
            div()
                .truncate()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(snippet),
        );
    }

    let item = item.clone();
    let app = app.clone();
    let mut row = div()
        .id(SharedString::from(format!("pal-{idx}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(t.r_sm))
        .cursor_pointer()
        .text_color(rgb(t.fg1))
        .child(lucide(
            icon,
            15.,
            if selected { t.accent_bright } else { t.fg2 },
        ))
        .child(main)
        .on_click(move |_ev, window, cx| {
            let fh = app.read(cx).focus.clone();
            let item = item.clone();
            app.update(cx, |this, cx| this.run_palette_item(item, cx));
            window.focus(&fh, cx);
        });
    if !secondary.is_empty() {
        row = row.child(
            div()
                .max_w(px(280.))
                .truncate()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(secondary),
        );
    }
    if selected {
        row = row.bg(rgb(t.accent_wash));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, name: &str, slug: &str) -> Row {
        Row {
            id: id.into(),
            url: "".into(),
            name: name.into(),
            slug: slug.into(),
            root: "".into(),
            path: id.into(),
            description: "".into(),
            language: "".into(),
            branch: "".into(),
            age: "".into(),
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
        }
    }

    fn hit(id: &str) -> SemanticHit {
        SemanticHit {
            id: id.into(),
            snippet: format!("snippet for {id}").into(),
        }
    }

    /// Repo indices in result order (name matches and recall hits together).
    fn repo_order(items: &[PaletteItem]) -> Vec<usize> {
        items
            .iter()
            .filter_map(|it| match it {
                PaletteItem::Repo(i) => Some(*i),
                PaletteItem::Recall { row, .. } => Some(*row),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn name_rank_orders_exact_prefix_substring() {
        // name / slug-tail exact
        assert_eq!(name_rank("Orrery", "o/orrery", "/x", "orrery"), Some(0));
        assert_eq!(name_rank("Dash", "seb/orrery", "/x", "orrery"), Some(0));
        // prefix
        assert_eq!(name_rank("orrery-docs", "o/other", "/x", "orrery"), Some(1));
        // substring anywhere (incl. path)
        assert_eq!(name_rank("my-orrery-fork", "o/f", "/x", "orrery"), Some(2));
        assert_eq!(name_rank("f", "o/f", "/dev/orrery-old", "orrery"), Some(2));
        // no match
        assert_eq!(name_rank("gamma", "o/gamma", "/g", "orrery"), None);
    }

    #[test]
    fn items_ranks_exact_and_prefix_names_before_substrings() {
        // Grid order deliberately puts the substring match first: the exact
        // match must still list first, then the prefix, then the substring.
        let rows = vec![
            row("/1", "not-alpha", "o/not-alpha"), // substring
            row("/2", "alphabet", "o/alphabet"),   // prefix
            row("/3", "alpha", "o/alpha"),         // exact
        ];
        let out = items(&rows, &[], &[], "alpha", false);
        assert_eq!(repo_order(&out), vec![2, 1, 0]);
        // No actions match "alpha".
        assert!(!out.iter().any(|i| matches!(i, PaletteItem::Action(_))));
    }

    #[test]
    fn items_appends_semantic_after_names_and_dedups() {
        let rows = vec![
            row("/a", "alpha", "o/alpha"),
            row("/b", "beta", "o/beta"),
            row("/c", "gamma", "o/gamma"),
        ];
        // Semantic ranked beta-then-alpha; alpha already matched by name, so
        // only beta surfaces as a recall row — after the name match.
        let semantic = vec![hit("/b"), hit("/a")];
        let out = items(&rows, &[], &semantic, "alpha", false);
        assert_eq!(repo_order(&out), vec![0, 1]);
        assert!(matches!(out[0], PaletteItem::Repo(0)));
        assert!(matches!(out[1], PaletteItem::Recall { row: 1, hit: 0 }));

        // A semantic hit whose repo vanished from the grid is skipped.
        let out = items(&rows, &[], &[hit("/gone")], "alpha", false);
        assert_eq!(repo_order(&out), vec![0]);
    }

    #[test]
    fn items_without_semantic_matches_todays_palette() {
        // AI off → `semantic` is empty; empty query lists everything in grid
        // order, and a query filters by substring.
        let rows = vec![row("/a", "alpha", "o/alpha"), row("/b", "beta", "o/beta")];
        let out = items(&rows, &[], &[], "", false);
        assert_eq!(repo_order(&out), vec![0, 1]);
        assert_eq!(
            out.iter()
                .filter(|i| matches!(i, PaletteItem::Action(_)))
                .count(),
            actions(false).len()
        );
        let out = items(&rows, &[], &[], "bet", false);
        assert_eq!(repo_order(&out), vec![1]);
        assert!(!out.iter().any(|i| matches!(i, PaletteItem::Recall { .. })));
    }

    #[test]
    fn selection_scoped_fleet_verbs_gate_on_a_selection() {
        let rows = vec![row("/a", "alpha", "o/alpha")];
        let listed = |has_selection: bool, action: PaletteAction| {
            items(&rows, &[], &[], "", has_selection)
                .iter()
                .any(|i| matches!(i, PaletteItem::Action(a) if *a == action))
        };
        // No selection → the fleet-wide verbs list, the selection-scoped don't.
        assert!(listed(false, PaletteAction::FetchAll));
        assert!(listed(false, PaletteAction::PullBehind));
        assert!(listed(false, PaletteAction::SelectDirty));
        assert!(listed(false, PaletteAction::SelectBehind));
        assert!(!listed(false, PaletteAction::FetchSelected));
        assert!(!listed(false, PaletteAction::PullSelected));
        // With a selection, the selection-scoped verbs appear too.
        assert!(listed(true, PaletteAction::FetchSelected));
        assert!(listed(true, PaletteAction::PullSelected));

        // And the query filters them like any action.
        let out = items(&rows, &[], &[], "pull all", true);
        assert!(
            out.iter()
                .any(|i| matches!(i, PaletteItem::Action(PaletteAction::PullBehind)))
        );
        assert!(
            !out.iter()
                .any(|i| matches!(i, PaletteItem::Action(PaletteAction::PullSelected)))
        );
    }

    #[test]
    fn items_ignores_semantic_when_query_is_empty() {
        // A stale recall list must not leak into the empty-query view.
        let rows = vec![row("/a", "alpha", "o/alpha")];
        let out = items(&rows, &[], &[hit("/a")], "", false);
        assert!(out.iter().all(|i| !matches!(i, PaletteItem::Recall { .. })));
    }
}
