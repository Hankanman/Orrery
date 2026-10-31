//! The 236px left rail: the primary nav with its badges, and below it the
//! active view's contextual panel (Mission Control's VIEWS / ROOTS /
//! LANGUAGES filters, or a view's category list), plus the scan footer.

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use orrery_core::attention::{AttentionKind, Severity};

use super::{OrreryApp, View};
use crate::icon::lucide;
use crate::theme::Theme;

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

    pub(super) fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
