//! Agents view — terminal coding-agent sessions running on the machine, detected
//! by scanning `/proc` (not just ones Orrery launched): any process whose program
//! is a known agent CLI and whose working directory sits inside one of your repos
//! or a dispatched agent worktree. Each session row shows the repo, command, pid,
//! and uptime, with open-in-IDE / open-folder / terminate actions. Dispatched
//! worktrees (drawer "Dispatch" with the fresh-worktree toggle, #185) render as
//! their own cards — origin repo + branch + task prompt, live or exited — with a
//! two-stage "Remove worktree" that refuses while uncommitted changes exist.
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
    /// The running agent's program label ("" when not running) — feeds the
    /// attention model's `AgentFact::program` for the origin repo.
    pub program: SharedString,
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

/// Scan running processes for agent sessions (sync — runs off the UI thread).
/// Watches both the scanned repos and the recorded dispatched worktrees: a
/// process inside a dispatched worktree becomes that worktree's live session
/// rather than a plain repo session, and every recorded worktree gets a card
/// even after its agent exits (so it can still be inspected / removed).
pub fn scan(rows: &[Row], agent_command: &str) -> AgentsData {
    let recorded = orrery_core::cache::agent_worktrees();
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
                None => (None, SharedString::default()),
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
            }
        })
        .collect();

    AgentsData {
        sessions,
        dispatched,
    }
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
                    let armed = confirm == Some(d.worktree_path.as_ref());
                    col = col.child(dispatch_card(d, now, armed, t, app));
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
/// prompt, with live/exited status and worktree-scoped actions.
fn dispatch_card(
    d: &DispatchRow,
    now: i64,
    armed: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
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
        ))
        .child(div().flex_1());
    if let Some(pid) = d.pid {
        actions = actions.child(terminate_button(pid, t, app));
    }
    actions = actions.child(remove_worktree_button(d, armed, t, app));

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
        .child(actions)
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
