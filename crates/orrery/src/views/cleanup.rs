//! Cleanup view — fleet hygiene in three sections:
//!
//! - **Disk usage**: each repo's build-artefact directories (`target/`,
//!   `node_modules/`, …) and their sizes, with a fleet total. Cached sizes
//!   show at once; stale repos are re-measured off the UI thread. One kind,
//!   or all, can be purged per repo behind the two-step confirm.
//! - **At-risk work**: unpushed branches, old stashes, long-dirty trees, repos
//!   with no remote and detached commits on no branch, each with an action
//!   where one exists (Push, Review changes, Open in IDE, Open folder).
//! - **Prunable branches**: merged / upstream-gone branches, bulk-deleted per
//!   repo behind the two-step confirm. Never touches the current/default
//!   branch (the core `prunable` already excludes those).
//!
//! The sidebar FILTER narrows to one section (`disk`, `at-risk`) or one kind
//! of prunable branch (`merged`, `gone`).

use gpui::{Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px, rgb};
use orrery_core::at_risk::AtRisk;
use orrery_core::git_ops;
use orrery_core::janitor::{self, ArtifactKind, DiskUsage};
use orrery_core::model::Repo;

use crate::data::Row;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::shell::hygiene::{HygieneState, purge_key};
use crate::theme::Theme;
use crate::widgets::{self, Header, Tone};

#[derive(Default)]
pub enum CleanupState {
    #[default]
    Idle,
    Loading,
    Ready(Vec<CleanupRepo>),
}

/// The Cleanup view's UI state, alongside its loaded [`CleanupState`].
#[derive(Default)]
pub(crate) struct CleanupUi {
    /// Repo id whose "Prune" button is armed, awaiting a confirming second
    /// click before the (irreversible) bulk branch delete runs.
    pub(crate) confirm: crate::shell::ArmedConfirm,
    /// The armed disk-cleaner Purge button (`hygiene::purge_key`).
    pub(crate) purge_confirm: crate::shell::ArmedConfirm,
}

/// Everything the Cleanup view renders from.
pub(crate) struct CleanupView<'a> {
    pub state: &'a CleanupState,
    pub ui: &'a CleanupUi,
    pub hygiene: &'a HygieneState,
    /// The raw snapshot, worktrees included (names + current branches).
    pub repos: &'a [Repo],
    pub ide_cmd: &'a str,
}

/// A repo with prunable branches.
pub struct CleanupRepo {
    pub id: SharedString,
    pub name: SharedString,
    pub branches: Vec<BranchRow>,
}

pub struct BranchRow {
    pub name: SharedString,
    /// "merged" or "gone".
    pub why: &'static str,
}

/// Scan every repo for prunable branches (sync git — runs off the UI thread).
pub fn scan(rows: &[Row]) -> Vec<CleanupRepo> {
    rows.iter()
        .filter_map(|r| {
            let branches: Vec<BranchRow> = git_ops::prunable(&r.id)
                .unwrap_or_default()
                .into_iter()
                .map(|b| BranchRow {
                    name: b.name.into(),
                    why: if b.gone { "gone" } else { "merged" },
                })
                .collect();
            (!branches.is_empty()).then(|| CleanupRepo {
                id: r.id.clone(),
                name: r.name.clone(),
                branches,
            })
        })
        .collect()
}

/// Does a branch's prune reason pass the sidebar filter (None = all)?
fn passes(why: &str, filter: Option<&str>) -> bool {
    filter.is_none() || filter == Some(why)
}

/// Which sections a sidebar filter shows: (disk, at-risk, branches).
fn sections(filter: Option<&str>) -> (bool, bool, bool) {
    match filter {
        None => (true, true, true),
        Some("disk") => (true, false, false),
        Some("at-risk") => (false, true, false),
        Some(_) => (false, false, true),
    }
}

pub(crate) fn render(
    v: CleanupView<'_>,
    filter: Option<&str>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let (disk, risk, branches) = sections(filter);
    let mut col = div().flex().flex_col().gap(px(24.));
    if disk {
        col = col.child(disk_section(&v, t, app));
    }
    if risk {
        col = col.child(at_risk_section(&v, t, app));
    }
    if branches {
        col = col.child(branches_section(&v, filter, t, app));
    }
    super::frame(
        "Cleanup",
        t,
        app,
        OrreryApp::load_cleanup,
        "cleanup-scroll",
        col,
    )
}

/// A section: its group header (icon, title, count, optional trailing
/// elements) over its body.
fn section(
    icon: &str,
    title: &str,
    count: usize,
    trailing: Vec<gpui::AnyElement>,
    t: &Theme,
) -> gpui::Div {
    div().flex().flex_col().gap(px(12.)).child(
        widgets::section_header(title, Header::Group { icon, count }, t)
            .mb(px(0.))
            .child(div().flex_1())
            .children(trailing),
    )
}

/// A bordered card for one repo's entries.
fn card(t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .p(px(12.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
}

/// A card's title line: icon, repo name, and the worktree branch when the
/// entry is a linked worktree.
fn card_head(icon: &str, repo: Option<&Repo>, id: &str, t: &Theme) -> gpui::Div {
    let name = repo
        .map(|r| r.display_name.clone())
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id).to_string());
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .min_w(px(0.))
        .child(lucide(icon, 14., t.fg2))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(crate::data::oneline(name))),
        );
    if let Some(branch) = repo.and_then(|r| {
        r.worktree_of
            .as_ref()
            .map(|_| r.worktree_branch.as_deref().unwrap_or("detached"))
    }) {
        head = head
            .child(lucide("folder-tree", 12., t.fg3))
            .child(super::tag(&format!("worktree · {branch}"), t.fg3, t));
    }
    head
}

/// A small text action button with a unique id.
fn action(
    id: String,
    label: &str,
    tone: Tone,
    t: &Theme,
    on: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    widgets::button(SharedString::from(id), tone, t)
        .px(px(10.))
        .py(px(4.))
        .child(widgets::label(label.to_string(), t.text_data_sm, true))
        .on_click(on)
}

/// A muted inert button (an action in flight).
fn busy(id: String, label: &str, t: &Theme) -> impl IntoElement {
    widgets::button(SharedString::from(id), Tone::Disabled, t)
        .px(px(10.))
        .py(px(4.))
        .child(widgets::label(label.to_string(), t.text_data_sm, true))
}

fn mono(text: impl Into<SharedString>, color: u32, t: &Theme) -> gpui::Div {
    div()
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(color))
        .child(text.into())
}

// ── Disk usage ─────────────────────────────────────────────────────────────

/// A repo's artefact dirs grouped by kind: (kind, dir count, bytes, the
/// single dir's relative path when there's one, any walk truncated).
fn by_kind(u: &DiskUsage) -> Vec<(ArtifactKind, usize, u64, Option<String>, bool)> {
    let mut out: Vec<(ArtifactKind, usize, u64, Option<String>, bool)> = Vec::new();
    for d in &u.dirs {
        match out.iter_mut().find(|e| e.0 == d.kind) {
            Some(e) => {
                e.1 += 1;
                e.2 += d.bytes;
                e.3 = None;
                e.4 |= d.truncated;
            }
            None => out.push((d.kind, 1, d.bytes, Some(d.rel.clone()), d.truncated)),
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.2));
    out
}

/// The fleet's measured repos with at least one artefact dir, largest first.
fn disk_entries<'a>(
    disk: &'a std::collections::HashMap<String, DiskUsage>,
    repos: &[Repo],
) -> Vec<&'a DiskUsage> {
    let mut out: Vec<&DiskUsage> = repos
        .iter()
        .filter_map(|r| disk.get(&r.id))
        .filter(|u| !u.dirs.is_empty())
        .collect();
    out.sort_by_key(|u| std::cmp::Reverse(u.total_bytes));
    out
}

fn disk_section(v: &CleanupView<'_>, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let h = v.hygiene;
    let entries = h
        .disk
        .as_ref()
        .map(|d| disk_entries(d, v.repos))
        .unwrap_or_default();
    let total: u64 = entries.iter().map(|u| u.total_bytes).sum();
    let mut trailing: Vec<gpui::AnyElement> = Vec::new();
    if h.disk.is_some() {
        trailing.push(
            mono(format!("{} total", janitor::format_bytes(total)), t.fg1, t).into_any_element(),
        );
    }
    trailing.push(if h.measuring {
        mono("Measuring…", t.fg3, t).into_any_element()
    } else {
        let app = app.clone();
        action(
            "disk-remeasure".into(),
            "Re-measure",
            Tone::Neutral,
            t,
            move |_, _, cx| {
                app.update(cx, |this, cx| this.load_disk_usage(true, cx));
            },
        )
        .into_any_element()
    });
    let mut s = section("hard-drive", "Disk usage", entries.len(), trailing, t);
    match &h.disk {
        None => return s.child(super::note("Loading…", t)),
        Some(_) if entries.is_empty() && !h.measuring => {
            return s.child(super::note(
                "No build artefacts found: no ignored target/, node_modules/ or virtualenv directories.",
                t,
            ));
        }
        Some(_) => {}
    }
    let armed = v.ui.purge_confirm.armed();
    for u in entries {
        let repo = v.repos.iter().find(|r| r.id == u.repo_id);
        let id: SharedString = u.repo_id.clone().into();
        let purging = h.purging.contains(&id);
        let all_key = purge_key(&id, None);
        let head = card_head("hard-drive", repo, &u.repo_id, t)
            .child(div().flex_1())
            .child(mono(size_label(u.total_bytes, u.truncated), t.fg0, t))
            .child(purge_button(
                &id,
                None,
                "Purge all",
                armed == Some(all_key.as_ref()),
                purging,
                t,
                app,
            ));
        let mut c = card(t).child(head);
        for (kind, n, bytes, rel, truncated) in by_kind(u) {
            let key = purge_key(&id, Some(kind));
            let what = match rel {
                Some(rel) => rel,
                None => format!("{n} dirs"),
            };
            c = c.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(22.))
                    .child(mono(kind.label(), t.fg1, t).flex_none())
                    .child(mono(what, t.fg3, t).flex_1().min_w(px(0.)).truncate())
                    .child(mono(size_label(bytes, truncated), t.fg1, t).flex_none())
                    .child(purge_button(
                        &id,
                        Some(kind),
                        "Purge",
                        armed == Some(key.as_ref()),
                        purging,
                        t,
                        app,
                    )),
            );
        }
        s = s.child(c);
    }
    s
}

/// "1.4 GB", or "≥ 1.4 GB" when the walk was cut short.
fn size_label(bytes: u64, truncated: bool) -> String {
    let size = janitor::format_bytes(bytes);
    if truncated {
        format!("≥ {size}")
    } else {
        size
    }
}

/// A two-step Purge button: the first click arms it ("Confirm?"), the second
/// purges. Inert while a purge of the repo is running.
fn purge_button(
    repo: &SharedString,
    kind: Option<ArtifactKind>,
    label: &str,
    armed: bool,
    purging: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> gpui::AnyElement {
    let key = purge_key(repo, kind);
    let id = format!("purge-{key}");
    if purging {
        return busy(id, "Purging…", t).into_any_element();
    }
    let (text, tone) = if armed {
        (format!("Confirm {}?", label.to_lowercase()), Tone::Armed)
    } else {
        (label.to_string(), Tone::Danger)
    };
    let (app, repo) = (app.clone(), repo.clone());
    action(id, &text, tone, t, move |_, _, cx| {
        let (repo, key) = (repo.clone(), key.clone());
        app.update(cx, |this, cx| {
            if armed {
                let kinds = match kind {
                    Some(k) => vec![k],
                    None => ArtifactKind::ALL.to_vec(),
                };
                this.purge_artifacts(repo, kinds, cx);
            } else {
                this.arm_purge(key, cx);
            }
        });
    })
    .into_any_element()
}

// ── At-risk work ───────────────────────────────────────────────────────────

fn at_risk_section(v: &CleanupView<'_>, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let reports = &v.hygiene.at_risk;
    let mut s = section("circle-alert", "At-risk work", reports.len(), Vec::new(), t);
    if reports.is_empty() {
        return s.child(super::note(
            "Nothing at risk: every branch is pushed, no old stashes or long-dirty trees.",
            t,
        ));
    }
    for r in reports {
        s = s.child(at_risk_card(r, v, t, app));
    }
    s
}

/// One entry line: icon, text, and an optional action.
fn risk_line(
    icon: &str,
    color: u32,
    text: String,
    act: Option<gpui::AnyElement>,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .pl(px(22.))
        .child(lucide(icon, 12., color))
        .child(
            mono(crate::data::oneline(text), t.fg1, t)
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        )
        .children(act)
}

fn at_risk_card(
    r: &AtRisk,
    v: &CleanupView<'_>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let repo = v.repos.iter().find(|x| x.id == r.repo_id);
    let worktree = repo.is_some_and(|x| x.worktree_of.is_some());
    let current = repo.map(|x| x.git.branch.as_str()).unwrap_or("");
    let id: SharedString = r.repo_id.clone().into();
    let key = |what: &str| format!("risk-{what}-{id}");

    // Reused launch actions.
    let ide = |what: &str| -> gpui::AnyElement {
        let (path, cmd) = (id.to_string(), v.ide_cmd.to_string());
        action(
            key(what),
            "Open in IDE",
            Tone::Neutral,
            t,
            move |_, _, _| {
                let _ = orrery_core::launch::launch(&cmd, &path);
            },
        )
        .into_any_element()
    };
    let drawer = |what: &str, label: &str, tab: Option<crate::shell::DrawerTab>| {
        let (app, id) = (app.clone(), id.clone());
        action(key(what), label, Tone::Neutral, t, move |_, window, cx| {
            let id = id.clone();
            app.update(cx, |this, cx| {
                this.open_drawer(id, window, cx);
                if let Some(tab) = tab {
                    this.drawer_select_tab(tab, window, cx);
                }
            });
        })
        .into_any_element()
    };

    let mut c = card(t).child(card_head("circle-alert", repo, &r.repo_id, t));

    if !r.unpushed_branches.is_empty() {
        // Push when the checked-out branch is one of them (`git_ops::push`
        // pushes HEAD); otherwise the drawer, whose branch list can switch.
        let act = if r.unpushed_branches.iter().any(|b| b.name == current) {
            if v.hygiene.pushing.contains(&id) {
                busy(key("push"), "Pushing…", t).into_any_element()
            } else {
                let (app, id) = (app.clone(), id.clone());
                action(key("push"), "Push", Tone::Neutral, t, move |_, _, cx| {
                    let id = id.clone();
                    app.update(cx, |this, cx| this.push_at_risk(id, cx));
                })
                .into_any_element()
            }
        } else if worktree {
            ide("branches")
        } else {
            drawer("branches", "Open branches", None)
        };
        let n = r.unpushed_branches.len();
        c = c.child(risk_line(
            "upload",
            t.dirty,
            format!(
                "{n} branch{} with unpushed commits",
                if n == 1 { "" } else { "es" }
            ),
            Some(act),
            t,
        ));
        for b in &r.unpushed_branches {
            let what = match &b.upstream {
                Some(up) => format!("{} ahead of {up}", plural(b.commits, "commit")),
                None => format!("{}, never pushed", plural(b.commits, "commit")),
            };
            c = c.child(risk_line(
                "git-branch",
                t.fg3,
                format!("{} · {what}", b.name),
                None,
                t,
            ));
        }
    }
    if !r.old_stashes.is_empty() {
        let n = r.old_stashes.len();
        c = c.child(risk_line(
            "archive",
            t.fg2,
            format!("{n} old stash{}", if n == 1 { "" } else { "es" }),
            Some(ide("stash")),
            t,
        ));
        for st in &r.old_stashes {
            c = c.child(risk_line(
                "clock",
                t.fg3,
                format!(
                    "stash@{{{}}} · {} · {}d old",
                    st.index, st.message, st.age_days
                ),
                None,
                t,
            ));
        }
    }
    if let Some(days) = r.dirty_days {
        let act = if worktree {
            ide("dirty")
        } else {
            drawer(
                "dirty",
                "Review changes",
                Some(crate::shell::DrawerTab::Changes),
            )
        };
        c = c.child(risk_line(
            "circle-dot",
            t.dirty,
            format!("Uncommitted changes for {}", plural(days, "day")),
            Some(act),
            t,
        ));
    }
    if r.orphan_commits > 0 {
        c = c.child(risk_line(
            "git-merge",
            t.behind,
            format!(
                "{} on a detached HEAD, on no branch",
                plural(r.orphan_commits, "commit")
            ),
            Some(ide("orphans")),
            t,
        ));
    }
    if r.no_remote {
        let path = id.to_string();
        c = c.child(risk_line(
            "globe",
            t.fg2,
            "No remote: nothing here is backed up".into(),
            Some(
                action(
                    key("remote"),
                    "Open folder",
                    Tone::Neutral,
                    t,
                    move |_, _, _| {
                        let _ = orrery_core::launch::open(&path);
                    },
                )
                .into_any_element(),
            ),
            t,
        ));
    }
    c
}

fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

// ── Prunable branches ──────────────────────────────────────────────────────

fn branches_section(
    v: &CleanupView<'_>,
    filter: Option<&str>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let count = match v.state {
        CleanupState::Ready(repos) => repos
            .iter()
            .map(|r| r.branches.iter().filter(|b| passes(b.why, filter)).count())
            .sum(),
        _ => 0,
    };
    let s = section("scissors", "Prunable branches", count, Vec::new(), t);
    match v.state {
        CleanupState::Idle | CleanupState::Loading => s.child(super::note("Loading…", t)),
        CleanupState::Ready(repos) if repos.is_empty() => s.child(super::note(
            "Nothing to prune: no merged or gone branches.",
            t,
        )),
        CleanupState::Ready(repos) => {
            // Only repos with at least one branch matching the filter.
            let shown: Vec<&CleanupRepo> = repos
                .iter()
                .filter(|r| r.branches.iter().any(|b| passes(b.why, filter)))
                .collect();
            if shown.is_empty() {
                return s.child(super::note("Nothing in this filter.", t));
            }
            let confirm = v.ui.confirm.armed();
            shown.into_iter().fold(s, |s, r| {
                let armed = confirm == Some(r.id.as_ref());
                s.child(repo_card(r, filter, armed, t, app))
            })
        }
    }
}

fn repo_card(
    r: &CleanupRepo,
    filter: Option<&str>,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let n = r.branches.len();
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide("scissors", 14., t.fg2))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(r.name.clone()),
        )
        .child(div().flex_1())
        .child(prune_button(r.id.clone(), n, armed, t, app));

    let mut card = card(t).child(head);
    for b in r.branches.iter().filter(|b| passes(b.why, filter)) {
        card = card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .child(lucide("git-branch", 12., t.fg3))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(rgb(t.fg1))
                        .child(b.name.clone()),
                )
                .child(super::tag(
                    b.why,
                    if b.why == "gone" { t.behind } else { t.fg3 },
                    t,
                )),
        );
    }
    card
}

/// The per-repo prune button. Two-stage: the first click arms a confirm (the
/// button flips to a danger-styled "Confirm prune {n}?"), the second actually
/// deletes — branch deletion is irreversible. The armed state reverts on a
/// timeout or when another repo's button is armed (see `OrreryApp::arm_prune`).
fn prune_button(
    id: SharedString,
    n: usize,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let label = if armed {
        format!("Confirm prune {n}?")
    } else {
        format!("Prune {n}")
    };
    let tone = if armed { Tone::Armed } else { Tone::Danger };
    widgets::button(SharedString::from(format!("prune-{id}")), tone, t)
        .px(px(12.))
        .py(px(6.))
        .child(widgets::label(label, t.text_data_sm, true))
        .on_click(move |_ev, _win, cx| {
            let id = id.clone();
            app.update(cx, |this, cx| {
                if armed {
                    this.prune_repo(id, cx)
                } else {
                    this.arm_prune(id, cx)
                }
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::janitor::ArtifactDir;

    fn dir(kind: ArtifactKind, rel: &str, bytes: u64) -> ArtifactDir {
        ArtifactDir {
            kind,
            path: format!("/r/{rel}"),
            rel: rel.into(),
            bytes,
            files: 1,
            truncated: false,
        }
    }

    #[test]
    fn dirs_group_by_kind_largest_first() {
        let u = DiskUsage {
            repo_id: "/r".into(),
            dirs: vec![
                dir(ArtifactKind::PyCache, "a/__pycache__", 10),
                dir(ArtifactKind::CargoTarget, "target", 5_000),
                dir(ArtifactKind::PyCache, "b/__pycache__", 20),
            ],
            total_bytes: 5_030,
            truncated: false,
            measured_at: 0,
        };
        let g = by_kind(&u);
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].0, g[0].1, g[0].2),
            (ArtifactKind::CargoTarget, 1, 5_000)
        );
        assert_eq!(g[0].3.as_deref(), Some("target"));
        // Two dirs of one kind: summed, no single path to show.
        assert_eq!(
            (g[1].0, g[1].1, g[1].2, g[1].3.clone()),
            (ArtifactKind::PyCache, 2, 30, None)
        );
    }

    #[test]
    fn filter_picks_sections() {
        assert_eq!(sections(None), (true, true, true));
        assert_eq!(sections(Some("disk")), (true, false, false));
        assert_eq!(sections(Some("at-risk")), (false, true, false));
        assert_eq!(sections(Some("merged")), (false, false, true));
        assert_eq!(sections(Some("gone")), (false, false, true));
    }

    #[test]
    fn truncated_sizes_read_as_a_lower_bound() {
        assert_eq!(size_label(1_400_000_000, false), "1.4 GB");
        assert_eq!(size_label(1_400_000_000, true), "≥ 1.4 GB");
    }
}
