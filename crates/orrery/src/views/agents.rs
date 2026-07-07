//! Agents view — terminal coding-agent sessions running on the machine, detected
//! by scanning `/proc` (not just ones Orrery launched): any process whose program
//! is a known agent CLI and whose working directory sits inside one of your repos
//! or a dispatched agent worktree. Each session row shows the repo, command, pid,
//! and uptime, with open-in-IDE / open-folder / terminate actions. Dispatched
//! worktrees (drawer "Dispatch" with the fresh-worktree toggle, #185) render as
//! their own cards — origin repo + branch + task prompt, live or exited — with a
//! two-stage "Remove worktree" that refuses while uncommitted changes exist.
//! The scan doubles as the outcome detector: a dispatched session that ends
//! with commits on its branch flips the card to "finished · N commits" and
//! raises `AgentFinished` attention; the card then offers the landing path —
//! Review (inline branch diff), Open PR, and a two-stage Discard.
//! Loaded off the UI thread when the nav item is selected; the refresh button
//! re-scans.

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};

use crate::data::Row;
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Known terminal coding-agent CLIs to detect by program name.
const KNOWN: &[&str] = &[
    "claude",
    "aider",
    "cursor-agent",
    "goose",
    "codex",
    "cody",
    "amp",
    "opencode",
    "gemini",
    "qwen",
    "cline",
    "gptme",
];

#[derive(Default)]
pub enum AgentsState {
    #[default]
    Idle,
    Loading,
    Ready(AgentsData),
}

/// Everything one agents scan produces: live sessions inside repos, plus the
/// recorded dispatched worktrees (running or not).
#[derive(Default)]
pub struct AgentsData {
    pub sessions: Vec<AgentRow>,
    pub dispatched: Vec<DispatchRow>,
    /// Dispatched sessions this very scan observed finishing *with commits to
    /// review* — the one-shot outcome events the shell turns into a toast +
    /// (config-gated) desktop notification. Notifying at the transition (not
    /// by diffing attention items) means it fires exactly once: the persisted
    /// `finished_at` makes later scans — and restarts — classify the row as
    /// already-finished.
    pub finished_now: Vec<FinishedDispatch>,
}

/// One "agent finished with work" outcome event (see `AgentsData::finished_now`).
pub struct FinishedDispatch {
    /// Origin repo display name (for the notification title).
    pub origin_name: SharedString,
    /// The `agent/…` branch holding the work.
    pub branch: SharedString,
    /// Commits ahead of the origin's default branch.
    pub commits: u32,
}

/// A detected agent session.
pub struct AgentRow {
    pub pid: u32,
    /// Absolute repo path (the action target).
    pub repo: SharedString,
    /// Repo display name.
    pub name: SharedString,
    /// Full command line (collapsed to one line).
    pub command: SharedString,
    pub started_unix: i64,
}

/// A dispatched agent worktree (from the SQLite pairing record), joined with
/// whether an agent process is currently running inside it.
pub struct DispatchRow {
    /// The worktree's working directory (the action target).
    pub worktree_path: SharedString,
    /// git worktree name in the origin repo (needed to prune it).
    pub worktree_name: SharedString,
    /// Origin repo id (absolute path).
    pub origin: SharedString,
    /// Origin repo display name.
    pub origin_name: SharedString,
    /// The `agent/…` branch the agent works on.
    pub branch: SharedString,
    /// The task prompt the agent was dispatched with.
    pub prompt: SharedString,
    pub created_unix: i64,
    /// The live agent process inside the worktree, if any.
    pub pid: Option<u32>,
    /// The agent's program label — the live process's when running, else the
    /// configured agent command's (the dispatched agent was launched with it).
    /// Feeds the attention model's `AgentFact::program` for the origin repo.
    pub program: SharedString,
    /// The session was observed to have finished (persisted `finished_at`).
    pub finished: bool,
    /// Commits ahead of the origin's default branch, measured at finish.
    pub commits_ahead: u32,
    /// URL of the PR opened from this branch ("" = none). Set → the outcome
    /// is handed off; the card shows "View PR" and attention stays quiet.
    pub pr_url: SharedString,
}

impl AgentRow {
    /// The agent program's display label — the command line's first token,
    /// basename only ("/usr/bin/claude --resume" → "claude"). Feeds the
    /// attention model's `AgentFact::program`.
    pub fn program(&self) -> String {
        program_label(&self.command)
    }
}

/// First token's basename ("/usr/bin/claude --resume" → "claude").
fn program_label(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .map(|tok| tok.rsplit('/').next().unwrap_or(tok))
        .unwrap_or("agent")
        .to_string()
}

/// Agent CLI basenames to match: the curated list plus whatever the user's
/// configured agent command resolves to (so a custom agent is detected too).
pub fn programs(agent_command: &str) -> Vec<String> {
    let mut progs: Vec<String> = KNOWN.iter().map(|s| s.to_string()).collect();
    if let Some(p) = agent_program(agent_command)
        && !progs.contains(&p)
    {
        progs.push(p);
    }
    progs
}

/// Best-effort extraction of the agent program from a `{path}`-templated command
/// like `kitty -e claude {path}` → `claude` (skips the terminal + flags).
fn agent_program(cmd: &str) -> Option<String> {
    const SKIP: &[&str] = &[
        "kitty",
        "wezterm",
        "alacritty",
        "gnome-terminal",
        "konsole",
        "xterm",
        "foot",
        "st",
        "terminator",
        "tilix",
        "xfce4-terminal",
        "urxvt",
        "ghostty",
        "start",
    ];
    cmd.split_whitespace()
        .filter(|tok| !tok.starts_with('-') && !tok.contains('{'))
        .map(|tok| tok.rsplit('/').next().unwrap_or(tok).to_string())
        .rfind(|b| !SKIP.contains(&b.as_str()))
}

/// Throttle for the `last_seen_alive` cache write: the transition logic only
/// needs the value to be nonzero, so refreshing the timestamp every poll (5s)
/// would be pointless churn; once a minute keeps it usefully fresh.
const ALIVE_WRITE_INTERVAL: i64 = 60;

/// Scan running processes for agent sessions (sync — runs off the UI thread).
/// Watches both the scanned repos and the recorded dispatched worktrees: a
/// process inside a dispatched worktree becomes that worktree's live session
/// rather than a plain repo session, and every recorded worktree gets a card
/// even after its agent exits (so it can still be inspected / removed).
///
/// This is also where dispatched-session *outcomes* are detected (#185): each
/// scan classifies every recorded worktree via `dispatch::session_transition`
/// and persists the resulting state, so a session that was seen alive and is
/// now gone gets its branch measured against the origin's default branch
/// (`git_ops::commits_ahead_of`, sync git — we're already off the UI thread)
/// exactly once.
pub fn scan(rows: &[Row], agent_command: &str) -> AgentsData {
    let mut recorded = orrery_core::cache::agent_worktrees();
    let mut paths: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    paths.extend(recorded.iter().map(|w| w.worktree_path.clone()));

    let detected = orrery_platform::agents::detect(&paths, &programs(agent_command));

    let mut sessions = Vec::new();
    // Live process per worktree path; detect returns newest-first, so the
    // first hit per path (the youngest process) wins.
    let mut live: std::collections::HashMap<String, (u32, String)> =
        std::collections::HashMap::new();
    for a in detected {
        if recorded.iter().any(|w| w.worktree_path == a.repo) {
            live.entry(a.repo).or_insert((a.pid, a.command));
            continue;
        }
        let name = rows
            .iter()
            .find(|r| r.id.as_ref() == a.repo)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| a.repo.rsplit('/').next().unwrap_or(&a.repo).into());
        sessions.push(AgentRow {
            pid: a.pid,
            repo: a.repo.into(),
            name,
            command: crate::data::oneline(a.command).into(),
            started_unix: a.started_unix,
        });
    }

    // Outcome detection: classify each recorded worktree against the live
    // set and persist the transition before building the rows.
    let now = crate::data::now_unix();
    let mut finished_now = Vec::new();
    for w in &mut recorded {
        use orrery_core::dispatch::SessionTransition;
        let alive = live.contains_key(&w.worktree_path);
        match orrery_core::dispatch::session_transition(alive, w.last_seen_alive, w.finished_at) {
            SessionTransition::SeenAlive => {
                // Throttled: the timestamp only needs to be nonzero.
                if now - w.last_seen_alive >= ALIVE_WRITE_INTERVAL {
                    w.last_seen_alive = now;
                    let _ = orrery_core::cache::mark_agent_worktree_alive(&w.worktree_path, now);
                }
            }
            SessionTransition::Resumed => {
                w.last_seen_alive = now;
                w.finished_at = 0;
                w.commits_ahead = 0;
                let _ = orrery_core::cache::mark_agent_worktree_alive(&w.worktree_path, now);
            }
            SessionTransition::Finished => {
                // Size the outcome: commits the agent branch is ahead of the
                // origin's default branch. Measured in the worktree (its HEAD
                // is the agent branch; branches + remote refs are shared).
                let commits = orrery_core::git_ops::default_branch(&w.repo_id)
                    .and_then(|base| {
                        orrery_core::git_ops::commits_ahead_of(&w.worktree_path, &base, 500).ok()
                    })
                    .map(|c| c.len() as u32)
                    .unwrap_or(0);
                w.finished_at = now;
                w.commits_ahead = commits;
                let _ = orrery_core::cache::mark_agent_worktree_finished(
                    &w.worktree_path,
                    now,
                    commits,
                );
                if commits > 0 && w.pr_url.is_empty() {
                    let origin_name = rows
                        .iter()
                        .find(|r| r.id.as_ref() == w.repo_id)
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| {
                            w.repo_id.rsplit('/').next().unwrap_or(&w.repo_id).into()
                        });
                    finished_now.push(FinishedDispatch {
                        origin_name,
                        branch: w.branch.clone().into(),
                        commits,
                    });
                }
            }
            SessionTransition::None => {}
        }
    }

    // The exited-session fallback label: the dispatched agent was launched
    // with the configured command, so its program is the best description.
    let dispatched_label: SharedString = agent_program(agent_command)
        .unwrap_or_else(|| "agent".into())
        .into();
    let dispatched = recorded
        .into_iter()
        .map(|w| {
            let origin_name = rows
                .iter()
                .find(|r| r.id.as_ref() == w.repo_id)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| w.repo_id.rsplit('/').next().unwrap_or(&w.repo_id).into());
            let (pid, program) = match live.get(&w.worktree_path) {
                Some((pid, command)) => (Some(*pid), program_label(command).into()),
                None => (None, dispatched_label.clone()),
            };
            DispatchRow {
                worktree_path: w.worktree_path.into(),
                worktree_name: w.worktree_name.into(),
                origin: w.repo_id.into(),
                origin_name,
                branch: w.branch.into(),
                prompt: crate::data::oneline(w.prompt).into(),
                created_unix: w.created_at,
                pid,
                program,
                finished: w.finished_at > 0,
                commits_ahead: w.commits_ahead,
                pr_url: w.pr_url.into(),
            }
        })
        .collect();

    AgentsData {
        sessions,
        dispatched,
        finished_now,
    }
}

/// Map one scan's results into the attention model's agent facts (pure — see
/// the tests). Live sessions raise running facts; a live dispatched-worktree
/// session counts against its *origin* repo (the worktree isn't a scanned
/// repo). A finished dispatched session raises a finished fact only while it
/// has work to review and hasn't been handed off: commits ahead and no PR yet.
/// Finished-without-commits stays card-only — there's nothing to act on.
pub fn agent_facts(data: &AgentsData) -> Vec<orrery_core::attention::AgentFact> {
    use orrery_core::attention::AgentFact;
    let mut facts: Vec<AgentFact> = data
        .sessions
        .iter()
        .map(|a| AgentFact {
            repo_id: a.repo.to_string(),
            program: a.program(),
            running: true,
            branch: None,
            commits: 0,
        })
        .collect();
    for d in &data.dispatched {
        if d.pid.is_some() {
            facts.push(AgentFact {
                repo_id: d.origin.to_string(),
                program: d.program.to_string(),
                running: true,
                branch: Some(d.branch.to_string()),
                commits: 0,
            });
        } else if d.finished && d.commits_ahead > 0 && d.pr_url.is_empty() {
            facts.push(AgentFact {
                repo_id: d.origin.to_string(),
                program: d.program.to_string(),
                running: false,
                branch: Some(d.branch.to_string()),
                commits: d.commits_ahead,
            });
        }
    }
    facts
}

/// Elapsed runtime as a compact string ("3h", "2d", "12m").
fn uptime(started_unix: i64, now: i64) -> String {
    if started_unix <= 0 {
        return "—".into();
    }
    let secs = (now - started_unix).max(0);
    let days = secs / 86_400;
    if days >= 1 {
        format!("{days}d")
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}m", (secs / 60).max(1))
    }
}

pub fn render(
    state: &AgentsState,
    filter: Option<&str>,
    confirm: Option<&str>,
    review: Option<(&str, Option<&str>)>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let now = crate::data::now_unix();
    let body = match state {
        AgentsState::Idle | AgentsState::Loading => {
            super::note("Scanning for running agents…", t).into_any_element()
        }
        AgentsState::Ready(data) if data.sessions.is_empty() && data.dispatched.is_empty() => {
            super::note(
                "No agent sessions running. Launch or dispatch one from a repo's drawer.",
                t,
            )
            .into_any_element()
        }
        AgentsState::Ready(data) => {
            let sessions: Vec<&AgentRow> = data
                .sessions
                .iter()
                .filter(|a| filter.is_none_or(|repo| a.name.as_ref() == repo))
                .collect();
            let dispatched: Vec<&DispatchRow> = data
                .dispatched
                .iter()
                .filter(|d| filter.is_none_or(|repo| d.origin_name.as_ref() == repo))
                .collect();
            if sessions.is_empty() && dispatched.is_empty() {
                super::note("Nothing in this filter.", t).into_any_element()
            } else {
                let mut col = div().flex().flex_col().gap(px(12.));
                for d in dispatched {
                    // The two-stage confirms share one slot; the discard arm is
                    // key-prefixed so the two buttons can't confirm each other.
                    let armed = confirm == Some(d.worktree_path.as_ref());
                    let armed_discard =
                        confirm == Some(format!("discard:{}", d.worktree_path).as_str());
                    // This card's expanded review diff: collapsed / loading /
                    // loaded.
                    let card_review = match review {
                        Some((path, diff)) if path == d.worktree_path.as_ref() => Some(diff),
                        _ => None,
                    };
                    col = col.child(dispatch_card(
                        d,
                        now,
                        armed,
                        armed_discard,
                        card_review,
                        t,
                        app,
                    ));
                }
                for a in sessions {
                    col = col.child(agent_card(a, now, t, app));
                }
                col.into_any_element()
            }
        }
    };
    super::frame(
        "Agents",
        t,
        app,
        OrreryApp::load_agents,
        "agents-scroll",
        body,
    )
}

fn agent_card(a: &AgentRow, now: i64, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide("square-terminal", 14., t.clean))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(a.name.clone()),
        )
        .child(super::tag(&format!("pid {}", a.pid), t.fg3, t))
        .child(super::muted_mono(uptime(a.started_unix, now), t))
        .child(div().flex_1());

    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(head)
        .child(
            div()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg2))
                .truncate()
                .child(a.command.clone()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(action(
                    "agent",
                    "Re-open",
                    a.repo.clone(),
                    t,
                    app,
                    Act::Agent,
                ))
                .child(action("ide", "Open IDE", a.repo.clone(), t, app, Act::Ide))
                .child(action(
                    "folder",
                    "Open folder",
                    a.repo.clone(),
                    t,
                    app,
                    Act::Folder,
                ))
                .child(div().flex_1())
                .child(terminate_button(a.pid, t, app)),
        )
}

/// A dispatched-worktree card: origin repo + `agent/…` branch + the task
/// prompt, with live/exited/finished status and worktree-scoped actions. A
/// finished session with commits offers the landing path (#185): Review
/// (inline branch diff vs the origin's default branch), Open PR (push +
/// forge PR from the worktree), and a two-stage Discard. `review` is this
/// card's expanded diff — `None` collapsed, `Some(None)` loading,
/// `Some(Some(diff))` loaded.
fn dispatch_card(
    d: &DispatchRow,
    now: i64,
    armed: bool,
    armed_discard: bool,
    review: Option<Option<&str>>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    // The finished-with-work state that drives the landing affordances.
    let reviewable = d.pid.is_none() && d.finished && d.commits_ahead > 0 && d.pr_url.is_empty();
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide(
            "git-branch",
            14.,
            if d.pid.is_some() { t.clean } else { t.fg2 },
        ))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(d.origin_name.clone()),
        )
        .child(super::tag(&d.branch, t.primary, t));
    head = match d.pid {
        Some(pid) => head.child(super::tag(&format!("pid {pid}"), t.clean, t)),
        None if !d.pr_url.is_empty() => head.child(super::tag("PR opened", t.primary, t)),
        None if reviewable => head.child(super::tag(
            &if d.commits_ahead == 1 {
                "finished · 1 commit".to_string()
            } else {
                format!("finished · {} commits", d.commits_ahead)
            },
            t.dirty,
            t,
        )),
        None if d.finished => head.child(super::tag("finished · no commits", t.fg3, t)),
        None => head.child(super::tag("no session", t.fg3, t)),
    };
    head = head
        .child(super::muted_mono(
            crate::data::rel_age(d.created_unix, now),
            t,
        ))
        .child(div().flex_1());

    let mut actions = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(action(
            "agent",
            if d.pid.is_some() { "Re-open" } else { "Resume" },
            d.worktree_path.clone(),
            t,
            app,
            Act::Agent,
        ))
        .child(action(
            "ide",
            "Open IDE",
            d.worktree_path.clone(),
            t,
            app,
            Act::Ide,
        ))
        .child(action(
            "folder",
            "Open folder",
            d.worktree_path.clone(),
            t,
            app,
            Act::Folder,
        ));
    if reviewable {
        // Review: toggle the inline branch diff.
        let (path, origin) = (d.worktree_path.clone(), d.origin.clone());
        let app2 = app.clone();
        actions = actions.child(wt_button(
            format!("wt-review-{}", d.worktree_path),
            if review.is_some() {
                "Hide review"
            } else {
                "Review"
            },
            false,
            t,
            move |cx| {
                let (path, origin) = (path.clone(), origin.clone());
                app2.update(cx, |this, cx| this.toggle_agent_review(path, origin, cx));
            },
        ));
        // Open PR: push the branch from the worktree, then a forge PR.
        let (path, origin, branch) = (d.worktree_path.clone(), d.origin.clone(), d.branch.clone());
        let app3 = app.clone();
        actions = actions.child(wt_button(
            format!("wt-pr-{}", d.worktree_path),
            "Open PR",
            false,
            t,
            move |cx| {
                let (path, origin, branch) = (path.clone(), origin.clone(), branch.clone());
                app3.update(cx, |this, cx| {
                    this.open_worktree_pr(path, origin, branch, cx)
                });
            },
        ));
    }
    if !d.pr_url.is_empty() {
        let url = d.pr_url.clone();
        actions = actions.child(wt_button(
            format!("wt-view-pr-{}", d.worktree_path),
            "View PR",
            false,
            t,
            move |_cx| {
                let _ = orrery_core::launch::open(&url);
            },
        ));
    }
    actions = actions.child(div().flex_1());
    if let Some(pid) = d.pid {
        actions = actions.child(terminate_button(pid, t, app));
    }
    if reviewable {
        // Discard: throw the outcome away — remove the worktree AND delete
        // the agent branch (unlike "Remove worktree", which keeps it).
        let (path, origin, name, branch) = (
            d.worktree_path.clone(),
            d.origin.clone(),
            d.worktree_name.clone(),
            d.branch.clone(),
        );
        let app4 = app.clone();
        actions = actions.child(wt_button(
            format!("wt-discard-{}", d.worktree_path),
            if armed_discard {
                "Confirm discard?"
            } else {
                "Discard"
            },
            true,
            t,
            move |cx| {
                let (path, origin, name, branch) =
                    (path.clone(), origin.clone(), name.clone(), branch.clone());
                app4.update(cx, |this, cx| {
                    if armed_discard {
                        this.discard_dispatch_worktree(path, origin, name, branch, cx);
                    } else {
                        this.arm_worktree_remove(format!("discard:{path}").into(), cx);
                    }
                });
            },
        ));
    }
    actions = actions.child(remove_worktree_button(d, armed, t, app));

    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(head)
        .child(
            div()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg1))
                .truncate()
                .child(d.prompt.clone()),
        )
        .child(
            div()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .truncate()
                .child(d.worktree_path.clone()),
        )
        .child(actions);
    if let Some(diff) = review {
        card = card.child(match diff {
            None => super::note("Loading branch diff…", t).into_any_element(),
            Some(d) if d.trim().is_empty() => {
                super::note("No changes vs the default branch.", t).into_any_element()
            }
            Some(d) => review_diff_block(d, t).into_any_element(),
        });
    }
    card
}

/// Cap on rendered review-diff lines — same rationale as the drawer's diff
/// pane: the whole app re-renders on any `cx.notify()` (agents poll, attention
/// poll, appearance signals), so an unbounded diff would rebuild thousands of
/// elements on every background tick.
const REVIEW_DIFF_MAX_LINES: usize = 500;

/// Render a unified diff with per-line sentiment colouring, truncated to
/// [`REVIEW_DIFF_MAX_LINES`] with a muted "… n more lines" footer. Read-only —
/// the drawer's `diff_block` grew per-hunk staging (#201), which doesn't apply
/// to reviewing a finished agent branch.
fn review_diff_block(diff: &str, t: &Theme) -> impl IntoElement {
    let mut block = div()
        .flex()
        .flex_col()
        .p(px(10.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .font_family("monospace")
        .text_size(px(t.text_data_sm));
    let mut lines = diff.lines();
    for line in lines.by_ref().take(REVIEW_DIFF_MAX_LINES) {
        let color = match line.as_bytes().first() {
            Some(b'+') => t.clean,
            Some(b'-') => t.behind,
            Some(b'@') => t.accent_bright,
            _ => t.fg2,
        };
        block = block.child(
            div()
                .text_color(rgb(color))
                .child(SharedString::from(line.to_string())),
        );
    }
    let hidden = lines.count();
    if hidden > 0 {
        block = block.child(
            div()
                .pt(px(6.))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(format!("… {hidden} more lines"))),
        );
    }
    block
}

/// A closure-driven card button, styled like [`action`]; `danger` uses the
/// destructive hover/armed palette.
fn wt_button(
    id: String,
    label: &str,
    danger: bool,
    t: &Theme,
    on: impl Fn(&mut gpui::App) + 'static,
) -> impl IntoElement {
    let btn = div()
        .id(SharedString::from(id))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .text_size(px(t.text_data_sm))
        .cursor_pointer();
    let btn = if danger && label.starts_with("Confirm") {
        btn.border_color(rgb(t.behind)).text_color(rgb(t.behind))
    } else if danger {
        btn.border_color(rgb(t.border))
            .text_color(rgb(t.fg1))
            .hover(|s| s.border_color(rgb(t.behind)).text_color(rgb(t.behind)))
    } else {
        btn.border_color(rgb(t.border))
            .text_color(rgb(t.fg1))
            .hover(|s| s.border_color(rgb(t.border_strong)).text_color(rgb(t.fg0)))
    };
    btn.child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| on(cx))
}

#[derive(Clone, Copy)]
enum Act {
    Agent,
    Ide,
    Folder,
}

fn action(
    key: &str,
    label: &str,
    repo: SharedString,
    t: &Theme,
    app: &Entity<OrreryApp>,
    act: Act,
) -> impl IntoElement {
    let app = app.clone();
    div()
        .id(SharedString::from(format!("agent-{key}-{repo}")))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(t.border_strong)).text_color(rgb(t.fg0)))
        .child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| {
            let repo = repo.clone();
            app.update(cx, |this, _cx| match act {
                Act::Agent => {
                    let _ = orrery_core::launch::spawn(&this.config.agent_command, &repo);
                }
                Act::Ide => {
                    let _ = orrery_core::launch::launch(&this.config.ide_command, &repo);
                }
                Act::Folder => {
                    let _ = orrery_core::launch::open(&repo);
                }
            });
        })
}

fn terminate_button(pid: u32, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let app = app.clone();
    div()
        .id(SharedString::from(format!("agent-kill-{pid}")))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(t.behind)).text_color(rgb(t.behind)))
        .child(SharedString::from("Terminate"))
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| this.terminate_agent(pid, cx));
        })
}

/// The per-worktree remove button. Two-stage like Cleanup's prune: the first
/// click arms a danger-styled "Confirm remove?", the second unlinks the
/// worktree (which is refused with a toast if it has uncommitted changes).
fn remove_worktree_button(
    d: &DispatchRow,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let (path, origin, name) = (
        d.worktree_path.clone(),
        d.origin.clone(),
        d.worktree_name.clone(),
    );
    let label = if armed {
        "Confirm remove?"
    } else {
        "Remove worktree"
    };
    let btn = div()
        .id(SharedString::from(format!("wt-remove-{path}")))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .text_size(px(t.text_data_sm))
        .cursor_pointer();
    let btn = if armed {
        btn.border_1()
            .border_color(rgb(t.behind))
            .text_color(rgb(t.behind))
    } else {
        btn.border_1()
            .border_color(rgb(t.border))
            .text_color(rgb(t.fg1))
            .hover(|s| s.border_color(rgb(t.behind)).text_color(rgb(t.behind)))
    };
    btn.child(SharedString::from(label))
        .on_click(move |_ev, _win, cx| {
            let (path, origin, name) = (path.clone(), origin.clone(), name.clone());
            app.update(cx, |this, cx| {
                if armed {
                    this.remove_dispatch_worktree(path, origin, name, cx);
                } else {
                    this.arm_worktree_remove(path, cx);
                }
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_agent_from_terminal_wrapper() {
        assert_eq!(
            agent_program("kitty -e claude {path}").as_deref(),
            Some("claude")
        );
        assert_eq!(agent_program("claude").as_deref(), Some("claude"));
        assert_eq!(
            agent_program("wezterm start -- aider {path}").as_deref(),
            Some("aider")
        );
        assert_eq!(
            agent_program("/usr/bin/ghostty -e goose").as_deref(),
            Some("goose")
        );
    }

    #[test]
    fn program_label_is_first_token_basename() {
        let row = |command: &str| AgentRow {
            pid: 1,
            repo: "/r".into(),
            name: "r".into(),
            command: command.to_string().into(),
            started_unix: 0,
        };
        assert_eq!(row("/usr/bin/claude --resume").program(), "claude");
        assert_eq!(row("aider").program(), "aider");
        assert_eq!(row("").program(), "agent");
    }

    fn dispatch_row(
        path: &str,
        pid: Option<u32>,
        finished: bool,
        commits_ahead: u32,
        pr_url: &str,
    ) -> DispatchRow {
        DispatchRow {
            worktree_path: path.to_string().into(),
            worktree_name: "agent-x-1111".into(),
            origin: "/dev/repo".into(),
            origin_name: "repo".into(),
            branch: "agent/x-1111".into(),
            prompt: "fix x".into(),
            created_unix: 0,
            pid,
            program: "claude".into(),
            finished,
            commits_ahead,
            pr_url: pr_url.to_string().into(),
        }
    }

    #[test]
    fn agent_facts_map_sessions_and_dispatch_outcomes() {
        let data = AgentsData {
            sessions: vec![AgentRow {
                pid: 1,
                repo: "/dev/other".into(),
                name: "other".into(),
                command: "/usr/bin/claude --resume".into(),
                started_unix: 0,
            }],
            dispatched: vec![
                // Live dispatched session → running fact against the origin.
                dispatch_row("/wt/a", Some(2), false, 0, ""),
                // Finished with work, no PR yet → the AgentFinished fact.
                dispatch_row("/wt/b", None, true, 3, ""),
                // Finished without commits → card-only, no fact.
                dispatch_row("/wt/c", None, true, 0, ""),
                // PR already opened → handed off, no fact.
                dispatch_row("/wt/d", None, true, 3, "https://github.com/o/r/pull/1"),
                // Never observed alive → no fact.
                dispatch_row("/wt/e", None, false, 0, ""),
            ],
            finished_now: Vec::new(),
        };
        let facts = agent_facts(&data);
        assert_eq!(facts.len(), 3);

        assert!(facts[0].running);
        assert_eq!(facts[0].repo_id, "/dev/other");
        assert_eq!(facts[0].program, "claude");
        assert_eq!(facts[0].branch, None);

        assert!(facts[1].running, "live dispatched session runs");
        assert_eq!(facts[1].repo_id, "/dev/repo", "counts against the origin");
        assert_eq!(facts[1].branch.as_deref(), Some("agent/x-1111"));

        assert!(!facts[2].running, "finished-with-work raises the fact");
        assert_eq!(facts[2].repo_id, "/dev/repo");
        assert_eq!(facts[2].commits, 3);
        assert_eq!(facts[2].branch.as_deref(), Some("agent/x-1111"));
    }

    #[test]
    fn programs_includes_known_and_custom() {
        let p = programs("kitty -e claude {path}");
        assert!(p.iter().any(|s| s == "claude"));
        assert!(p.iter().any(|s| s == "aider"));
        // A custom agent not in the curated list is still detected.
        let p = programs("xterm -e mycoolagent {path}");
        assert!(p.iter().any(|s| s == "mycoolagent"));
    }
}
