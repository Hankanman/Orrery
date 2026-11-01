//! RepoDrawer — the right-anchored detail panel. Opens
//! over the shell when a card is clicked; a scrim backdrop or the close button
//! dismisses it. Tabs: Overview / Changes / PR / Notes / Readme.
//!
//! This is the workhorse primitive — most journeys (catch-up, dive, commit, PR
//! triage) live here. All five tabs are in: Overview (Row facts + async
//! branches/commits/worktrees, with worktree add/remove), Readme (gpui-component
//! markdown), PR (lazy, GitHub-only, via the `task` bridge — rollups, per-check
//! breakdown + inline approve/merge), Changes (Unstaged/Staged file lists with
//! per-file stage/unstage, a per-file diff pane, and a commit composer acting
//! on the index, plus AI commit-message + changelog generation gated on
//! aiReady), and Notes (catch-up + AI "what changed" narrative + an editable
//! markdown note via gpui-component's multiline input).
//!
//! This module holds the drawer's data, its chrome (header, tab bar, footer)
//! and the helpers the tabs share; each tab renders from its own submodule.

use gpui::{
    Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb, rgba,
};
use orrery_core::{git_ops, launch};

use gpui_component::tab::{Tab, TabBar};

use crate::data::{self, Row};
use crate::icon::{brand, lucide};
use crate::shell::{DrawerTab, OrreryApp};
use crate::theme::Theme;
use crate::widgets::{self, Header, Tone, tag};

mod actions;
mod branches;
mod changes;
mod commits;
mod notes;
mod overview;
mod pr;
mod readme;

const MONO: &str = "monospace";
const PANEL_W: f32 = 560.;
/// Recent commits shown in the Overview.
const LOG_LIMIT: usize = 8;

const TABS: [(DrawerTab, &str); 5] = [
    (DrawerTab::Overview, "Overview"),
    (DrawerTab::Changes, "Changes"),
    (DrawerTab::Pr, "PR"),
    (DrawerTab::Notes, "Notes"),
    (DrawerTab::Readme, "Readme"),
];

// ── async per-repo data ────────────────────────────────────────────────────
// The drawer's git data is loaded off the UI thread and marshalled back onto the
// foreground (the live-wiring pattern). `None` = still loading; `Some(vec)` =
// loaded (possibly empty). `repo` guards against a result landing after the
// drawer moved to a different repo or closed.

pub struct BranchRow {
    pub name: SharedString,
    pub current: bool,
    pub gone: bool,
    pub merged: bool,
}

pub struct CommitRow {
    pub summary: SharedString,
    pub author: SharedString,
    pub age: SharedString,
}

pub struct WorktreeRow {
    pub name: SharedString,
    pub path: SharedString,
}

/// Lazy-loaded state for a drawer tab whose data is fetched on first view.
#[derive(Default, PartialEq)]
pub enum ReadmeState {
    /// Not requested yet.
    #[default]
    Idle,
    Loading,
    /// Loaded — `None` means the repo has no README file.
    Ready(Option<SharedString>),
}

/// A render-ready open pull request (flattened from `inbox::PrDetail`).
pub struct PrRow {
    pub number: u64,
    pub title: SharedString,
    pub url: SharedString,
    pub draft: bool,
    pub author: SharedString,
    pub refs: SharedString,         // "head → base"
    pub mergeable: SharedString,    // clean | conflicting | unknown
    pub review: SharedString,       // approved | changes_requested | review_required | none
    pub checks: SharedString,       // success | failure | pending | none (rollup)
    pub check_runs: Vec<CheckItem>, // individual status checks / CI contexts
}

/// One status check on a PR, for the per-check breakdown under the rollup.
pub struct CheckItem {
    pub name: SharedString,
    pub state: SharedString, // success | failure | pending | neutral
    pub url: Option<SharedString>,
}

/// Lazy-loaded PR panel state (network).
#[derive(Default)]
pub enum PrState {
    #[default]
    Idle,
    Loading,
    Ready {
        methods: Vec<SharedString>,
        prs: Vec<PrRow>,
    },
    /// Not applicable (non-GitHub) or the fetch failed.
    Error(SharedString),
}

/// Lazy-loaded diff state (sync git, but loaded off the UI thread).
#[derive(Default)]
pub enum DiffState {
    #[default]
    Idle,
    Loading,
    /// The diff's hunks (empty when the selected file has no diff on its side).
    Ready(Vec<HunkRow>),
}

/// One hunk of the selected file's diff, render-ready
/// ([`git_ops::Hunk`] flattened; the vec position is the stage/unstage index).
pub struct HunkRow {
    /// The `@@ -a,b +c,d @@ …` header line.
    pub header: SharedString,
    /// Body lines, origin-char prefixed (single lines by construction).
    pub lines: Vec<SharedString>,
}

/// One pending change in the Changes tab's Unstaged/Staged lists (render-ready
/// [`git_ops::FileChange`]). A path staged and then further edited appears in
/// both lists.
pub struct ChangeRow {
    pub path: SharedString,
    pub kind: git_ops::ChangeKind,
    pub staged: bool,
}

/// Notes tab data: the "resume where I left off" catch-up line (the note text
/// itself lives in `DrawerData::note_input`).
pub struct NotesData {
    pub catchup: SharedString,
    /// Commits since last seen — drives whether "Mark caught up" is offered.
    pub count: usize,
    pub first_visit: bool,
    /// AI "what changed" narrative, once generated (gated on `aiReady`) —
    /// stored already reflowed for the markdown renderer.
    pub resume: Option<SharedString>,
}

/// Async-loaded data for the currently open repo's drawer. Overview loads on
/// open; Readme/PR/Changes/Notes load lazily when their tabs are first shown.
#[derive(Default)]
pub struct DrawerData {
    pub repo: SharedString,
    pub branches: Option<Vec<BranchRow>>,
    pub commits: Option<Vec<CommitRow>>,
    pub worktrees: Option<Vec<WorktreeRow>>,
    pub readme: ReadmeState,
    pub pr: PrState,
    /// The selected file's diff (per-file, capped render — see
    /// `changes::diff_block`).
    pub diff: DiffState,
    /// The Changes tab's file lists; `None` until the tab first loads them.
    pub changes: Option<Vec<ChangeRow>>,
    /// The file whose diff the pane shows, as (path, staged-side).
    pub change_sel: Option<(SharedString, bool)>,
    /// AI-generated commit message for the staged diff, once requested — the
    /// body may be empty. Errors land in the subject.
    pub commit_suggestion: Option<CommitSuggestion>,
    /// AI-generated changelog from recent commits, once requested — stored
    /// already reflowed for the markdown renderer.
    pub changelog: Option<SharedString>,
    /// Notes tab (catch-up + saved note); `None` until first shown.
    pub notes: Option<NotesData>,
    /// The editable note field (gpui-component multiline input), seeded from the
    /// saved note when the Notes tab first opens.
    pub note_input: Option<Entity<gpui_component::input::InputState>>,
    /// The commit-subject field for the Changes tab, created when it first opens.
    pub commit_input: Option<Entity<gpui_component::input::InputState>>,
    /// The optional multi-line commit-body field, created with `commit_input`.
    pub commit_body_input: Option<Entity<gpui_component::input::InputState>>,
    /// The new-worktree name field (Overview), created when the drawer opens.
    pub worktree_input: Option<Entity<gpui_component::input::InputState>>,
    /// The repo's default branch (origin/HEAD else main/master), loaded with
    /// the Overview — the base for "Open PR" and its visibility check.
    pub default_branch: Option<SharedString>,
    /// The repo's cached default-branch CI state (central pass, #183), looked
    /// up from the app's `ci_states` when the drawer opens — the Overview's
    /// CI line. `None` for non-GitHub repos or before the first pass.
    pub ci: Option<orrery_core::cache::CiEntry>,
    /// The repo's cached security-advisory report (Dependabot or a local
    /// audit), looked up when the drawer opens — the Overview's badge.
    pub advisory: Option<orrery_core::advisories::AdvisoryReport>,
    /// The Overview's worktree list is expanded (the card's "N worktrees"
    /// affordance opens the drawer with it expanded).
    pub worktrees_open: bool,
    /// A commit landed in this drawer session — keeps Push offered even while
    /// the grid row's ahead count is stale (no upstream / pre-rescan).
    pub committed: bool,
    /// A push kicked off from this drawer is still running.
    pub push_busy: bool,
    /// An "Open PR" flow kicked off from this drawer is still running.
    pub pr_busy: bool,
    /// The PR created from this drawer, for the "View PR" affordance.
    pub pr_url: Option<SharedString>,
    /// The agent-dispatch task field (Overview), created when the drawer opens.
    pub dispatch_input: Option<Entity<gpui_component::input::InputState>>,
    /// Whether "Dispatch" runs the agent on a fresh worktree (#185).
    pub dispatch_fresh: bool,
    /// Identifies this drawer session (unique per open), so a load started for
    /// an earlier session of the same repo can't land in this one.
    epoch: u64,
    /// Latest issued load ticket per [`Slot`]; only the newest load of a slot
    /// may store its result, so overlapping reloads can't land out of order.
    seqs: [u64; SLOTS],
}

/// The independently reloaded parts of the drawer, for stale-result guards.
#[derive(Clone, Copy)]
enum Slot {
    Overview,
    Readme,
    Changes,
    Diff,
    Notes,
    Pr,
}
const SLOTS: usize = 6;

/// Issued when a drawer load starts; the result is stored only while the
/// ticket is still current (same session, no newer load of the same slot).
#[derive(Clone, Copy)]
struct Ticket {
    epoch: u64,
    slot: Slot,
    seq: u64,
}

/// The AI commit-message suggestion (Changes tab).
pub struct CommitSuggestion {
    pub subject: SharedString,
    /// The raw body — what "Commit this" commits.
    pub body: SharedString,
    /// `body` reflowed for the markdown renderer, computed once here rather
    /// than on every frame (see [`data::unwrap_soft_breaks`]).
    body_md: SharedString,
}

impl CommitSuggestion {
    pub fn new(subject: impl Into<SharedString>, body: impl Into<SharedString>) -> Self {
        let body = body.into();
        CommitSuggestion {
            subject: subject.into(),
            body_md: data::unwrap_soft_breaks(&body).into(),
            body,
        }
    }
}

impl DrawerData {
    /// Fresh, all-loading state for a newly opened repo.
    pub fn loading(repo: SharedString) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
        DrawerData {
            repo,
            epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
            ..Default::default()
        }
    }

    /// Start a load of `slot`, superseding any in-flight one.
    fn ticket(&mut self, slot: Slot) -> Ticket {
        let seq = &mut self.seqs[slot as usize];
        *seq += 1;
        Ticket {
            epoch: self.epoch,
            slot,
            seq: *seq,
        }
    }

    /// Is `t` still the newest load of its slot in this drawer session?
    fn is_current(&self, t: Ticket) -> bool {
        self.epoch == t.epoch && self.seqs[t.slot as usize] == t.seq
    }
}

/// Join a commit subject and optional body with the conventional blank line.
/// Both sides are trimmed; an empty body yields just the subject.
fn full_commit_message(subject: &str, body: &str) -> String {
    let (subject, body) = (subject.trim(), body.trim());
    if body.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n\n{body}")
    }
}

#[allow(clippy::too_many_arguments)]
pub fn drawer(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    app: &Entity<OrreryApp>,
    data: &DrawerData,
    ide_cmd: &str,
    agent_cmd: &str,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    // Scrim: click anywhere outside the panel to dismiss.
    let backdrop = {
        let app = app.clone();
        div()
            .id("drawer-backdrop")
            .flex_1()
            .h_full()
            .bg(rgba(t.scrim))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let panel = div()
        .flex()
        .flex_col()
        .w(px(PANEL_W))
        .h_full()
        .bg(rgb(t.page))
        .border_l_1()
        .border_color(rgb(t.border))
        .child(header(row, t, app))
        .child(tab_bar(tab, t, app))
        .child(body(row, tab, t, data, app, ai_ready, github_authed))
        .child(footer(row, t, ide_cmd, agent_cmd));

    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .flex()
        .flex_row()
        // Modal: block all mouse interaction with the grid behind, so clicks on
        // the panel don't also fall through to a card.
        .occlude()
        .child(backdrop)
        .child(panel)
}

fn header(row: &Row, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let close = {
        let app = app.clone();
        div()
            .id("drawer-close")
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(30.))
            .rounded(px(t.r_sm))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide("x", 17., t.fg1))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.close_overlay();
                    cx.notify();
                });
            })
    };

    let mut title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .text_size(px(t.text_h3))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.fg0))
        .child(div().min_w(px(0.)).truncate().child(row.name.clone()));
    if !row.host.is_empty() {
        title = title.child(brand(&row.host, 15., t.fg2));
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .px(px(18.))
        .py(px(15.))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(4.))
                .child(title)
                .child(
                    div()
                        .truncate()
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(SharedString::from(format!("{} · {}", row.slug, row.path))),
                ),
        )
        .child(close)
}

/// The owner/name slug if this repo is a usable GitHub remote, else `None`.
pub(crate) fn github_slug(row: &Row) -> Option<SharedString> {
    (row.host.as_ref() == "github" && row.slug.as_ref() != "no remote").then(|| row.slug.clone())
}

/// The tab strip: gpui-component's underline [`TabBar`]. Selecting a tab
/// lazy-loads its data (see [`OrreryApp::drawer_select_tab`]).
fn tab_bar(active: DrawerTab, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let app = app.clone();
    let selected = TABS.iter().position(|(tab, _)| *tab == active).unwrap_or(0);
    TabBar::new("drawer-tabs")
        .underline()
        // Tab labels sit where the hand-rolled ones did: 23px in, 24px apart
        // (the underline variant's 16px gap + 4px each side).
        .pl(px(19.))
        .selected_index(selected)
        .children(TABS.iter().map(|(_, label)| {
            Tab::new().px(px(4.)).cursor_pointer().child(
                div()
                    .text_size(px(t.text_small))
                    .child(SharedString::from(*label)),
            )
        }))
        .on_click(move |ix, window, cx| {
            let tab = TABS[*ix].0;
            app.update(cx, |this, cx| this.drawer_select_tab(tab, window, cx));
        })
}

#[allow(clippy::too_many_arguments)]
fn body(
    row: &Row,
    tab: DrawerTab,
    t: &Theme,
    data: &DrawerData,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
    github_authed: bool,
) -> impl IntoElement {
    let content = match tab {
        DrawerTab::Overview => overview::overview(row, t, data, app, ai_ready).into_any_element(),
        DrawerTab::Readme => readme::readme_view(data, t).into_any_element(),
        DrawerTab::Pr => pr::pr_view(row, data, t, app).into_any_element(),
        DrawerTab::Changes => {
            changes::changes_view(row, data, t, app, ai_ready, github_authed).into_any_element()
        }
        DrawerTab::Notes => notes::notes_view(row, data, t, app, ai_ready).into_any_element(),
    };
    div()
        .id("drawer-body")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(px(18.))
        .gap(px(16.))
        .child(content)
}

/// Section wrapper: an uppercase label (+ optional count) over a list.
fn section(t: &Theme, title: &str, count: Option<usize>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(widgets::section_header(title, Header::Caps { count }, t).mb(px(3.)))
}

/// A muted "Loading…" / empty / error placeholder line.
fn placeholder(text: impl Into<SharedString>, t: &Theme) -> impl IntoElement {
    widgets::note(text, t)
        .py(px(3.))
        .text_size(px(t.text_data_sm))
}

/// A small PR action button.
fn pr_btn(
    id: SharedString,
    label: &str,
    t: &Theme,
    on: impl Fn(&mut gpui::App) + 'static,
) -> impl IntoElement {
    widgets::button(id, Tone::Neutral, t)
        .px(px(10.))
        .py(px(5.))
        .child(widgets::label(label.to_string(), t.text_data_sm, true))
        .on_click(move |_ev, _win, cx| on(cx))
}

/// A [`pr_btn`]-shaped button in its disabled state: muted, inert, no hover.
fn disabled_btn(label: &str, t: &Theme) -> impl IntoElement {
    widgets::button(
        SharedString::from(format!("off-{label}")),
        Tone::Disabled,
        t,
    )
    .px(px(10.))
    .py(px(5.))
    .child(widgets::label(label.to_string(), t.text_data_sm, true))
}

fn footer(row: &Row, t: &Theme, ide_cmd: &str, agent_cmd: &str) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .px(px(18.))
        .py(px(14.))
        .border_t_1()
        .border_color(rgb(t.border))
        .child(launch_btn(
            "drawer-ide",
            SharedString::from("Open in IDE"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
                move || {
                    let _ = launch::launch(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-agent",
            SharedString::from("Agent"),
            true,
            t,
            {
                let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
                move || {
                    let _ = launch::spawn(&cmd, &path);
                }
            },
        ))
        .child(launch_btn(
            "drawer-folder",
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            {
                let path = row.id.clone();
                move || {
                    let _ = launch::open(&path);
                }
            },
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        bar = bar.child(launch_btn(
            "drawer-host",
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move || {
                let _ = launch::open(&url);
            },
        ));
    }
    bar
}

/// A drawer launcher button. `on` runs a side-effecting launch (no app state).
fn launch_btn(
    id: &'static str,
    content: impl IntoElement,
    wide: bool,
    t: &Theme,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let b = widgets::button(id, Tone::Neutral, t)
        .py(px(9.))
        .px(px(0.))
        .on_click(move |_ev, _win, _cx| on())
        .child(
            div()
                .text_size(px(t.text_data_sm))
                .font_family(MONO)
                .child(content),
        );
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(40.))
    }
}

/// Inline icon+label segment (shared shape with the card's status segs).
fn seg(icon: &str, label: SharedString, color: u32) -> impl IntoElement {
    widgets::seg(icon, label, 13., color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_load_of_a_slot_supersedes_older() {
        let mut d = DrawerData::loading("/a".into());
        let first = d.ticket(Slot::Changes);
        let second = d.ticket(Slot::Changes);
        assert!(!d.is_current(first), "the older load is stale");
        assert!(d.is_current(second));
        // Other slots are independent.
        let pr = d.ticket(Slot::Pr);
        assert!(d.is_current(pr) && d.is_current(second));
    }

    #[test]
    fn reopening_the_drawer_drops_earlier_sessions_loads() {
        let mut old = DrawerData::loading("/a".into());
        let t = old.ticket(Slot::Overview);
        // Same repo, new session: the old session's ticket must not match.
        let mut new = DrawerData::loading("/a".into());
        assert!(!new.is_current(t));
        let fresh = new.ticket(Slot::Overview);
        assert!(new.is_current(fresh));
    }

    #[test]
    fn commit_suggestion_keeps_raw_body_and_reflows_display() {
        let s = CommitSuggestion::new("Subject", "line one\nline two");
        assert_eq!(s.body.as_ref(), "line one\nline two");
        assert_eq!(s.body_md.as_ref(), "line one line two");
    }
}
