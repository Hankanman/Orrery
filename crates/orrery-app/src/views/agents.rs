//! Agents view — terminal coding-agent sessions running on the machine, detected
//! by scanning `/proc` (not just ones Orrery launched): any process whose program
//! is a known agent CLI and whose working directory sits inside one of your repos.
//! Each row shows the repo, command, pid, and uptime, with open-in-IDE / open-
//! folder / terminate actions. Loaded off the UI thread when the nav item is
//! selected; the refresh button re-scans.

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
    Ready(Vec<AgentRow>),
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
pub fn scan(rows: &[Row], agent_command: &str) -> Vec<AgentRow> {
    let paths: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    orrery_platform::agents::detect(&paths, &programs(agent_command))
        .into_iter()
        .map(|a| {
            let name = rows
                .iter()
                .find(|r| r.id.as_ref() == a.repo)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| a.repo.rsplit('/').next().unwrap_or(&a.repo).into());
            AgentRow {
                pid: a.pid,
                repo: a.repo.into(),
                name,
                command: crate::data::oneline(a.command).into(),
                started_unix: a.started_unix,
            }
        })
        .collect()
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

pub fn render(state: &AgentsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let now = crate::data::now_unix();
    let body = match state {
        AgentsState::Idle | AgentsState::Loading => {
            super::note("Scanning for running agents…", t).into_any_element()
        }
        AgentsState::Ready(agents) if agents.is_empty() => super::note(
            "No agent sessions running. Launch one from a repo card's Agent button.",
            t,
        )
        .into_any_element(),
        AgentsState::Ready(agents) => {
            let mut col = div().flex().flex_col().gap(px(12.));
            for a in agents {
                col = col.child(agent_card(a, now, t, app));
            }
            col.into_any_element()
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

#[derive(Clone, Copy)]
enum Act {
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
                Act::Ide => {
                    let _ = orrery_core::launch::launch(&this.config.ide_command, &repo);
                }
                Act::Folder => {
                    let _ = orrery_core::launch::open(&repo);
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
    fn programs_includes_known_and_custom() {
        let p = programs("kitty -e claude {path}");
        assert!(p.iter().any(|s| s == "claude"));
        assert!(p.iter().any(|s| s == "aider"));
        // A custom agent not in the curated list is still detected.
        let p = programs("xterm -e mycoolagent {path}");
        assert!(p.iter().any(|s| s == "mycoolagent"));
    }
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
