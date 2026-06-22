//! Settings view — edit the config in-app instead of hand-editing TOML.
//!
//! A draft `AppConfig` (toggles / scan depth / roots) lives on `OrreryApp`
//! alongside text-input entities for the string fields; "Save & rescan" reads
//! them back, persists via `config::save`, and re-scans. This first cut covers
//! roots, launchers, AI toggles/endpoints, and notifications; the GitHub
//! device-flow login and live AI status/model-pull (network) come next.

use gpui::{
    AppContext, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use gpui_component::input::{Input, InputState};
use orrery_core::model::AppConfig;

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// The editable settings session: a draft config + the string-field inputs.
pub struct SettingsState {
    pub draft: AppConfig,
    pub ide: Entity<InputState>,
    pub agent: Entity<InputState>,
    pub ollama_host: Entity<InputState>,
    pub ai_model: Entity<InputState>,
    pub embed_model: Entity<InputState>,
    pub client_id: Entity<InputState>,
    pub ignore: Entity<InputState>,
    pub add_root: Entity<InputState>,
    /// Flash a "Saved" confirmation after a successful save.
    pub saved: bool,
}

impl SettingsState {
    /// Seed a session from the live config.
    pub fn new(cfg: &AppConfig, window: &mut Window, cx: &mut gpui::App) -> Self {
        let field = |window: &mut Window, cx: &mut gpui::App, ph: &'static str, val: &str| {
            let val = val.to_string();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(ph)
                    .default_value(val)
            })
        };
        SettingsState {
            ide: field(window, cx, "code {path}", &cfg.ide_command),
            agent: field(window, cx, "agent command", &cfg.agent_command),
            ollama_host: field(window, cx, "http://localhost:11434", &cfg.ollama_host),
            ai_model: field(window, cx, "model name", &cfg.ai_model),
            embed_model: field(window, cx, "embed model", &cfg.embed_model),
            client_id: field(window, cx, "GitHub OAuth client id", &cfg.github_client_id),
            ignore: field(window, cx, "node_modules, .cache", &cfg.ignore.join(", ")),
            add_root: field(window, cx, "~/dev", ""),
            draft: cfg.clone(),
            saved: false,
        }
    }
}

pub fn render(s: &SettingsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(t.page))
        // Header
        .child(
            div()
                .flex()
                .items_center()
                .h(px(52.))
                .px(px(20.))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(t.text_h3))
                        .text_color(rgb(t.fg0))
                        .child("Settings"),
                ),
        )
        // Scrolling sections
        .child(
            div()
                .id("settings-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(20.))
                .gap(px(16.))
                .child(roots_section(s, t, app))
                .child(launchers_section(s, t))
                .child(ai_section(s, t, app))
                .child(notifications_section(s, t, app)),
        )
        // Save footer
        .child(save_footer(s, t, app))
}

// ── sections ────────────────────────────────────────────────────────────────

fn roots_section(s: &SettingsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let mut col = section(t, "Workspace roots");
    for (i, root) in s.draft.roots.iter().enumerate() {
        let remove = icon_btn("x", t, app, move |a| {
            if let Some(s) = &mut a.settings
                && i < s.draft.roots.len()
            {
                s.draft.roots.remove(i);
            }
        });
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(lucide("folder", 13., t.fg3))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .font_family("monospace")
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg1))
                        .child(SharedString::from(root.clone())),
                )
                .child(remove),
        );
    }
    // Add a root.
    let add = {
        let app = app.clone();
        button("Add", t, move |cx| {
            app.update(cx, |this, cx| this.settings_add_root(cx));
        })
    };
    col = col.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w(px(0.)).child(Input::new(&s.add_root)))
            .child(add),
    );

    // Scan depth stepper + ignore field.
    col = col.child(stepper("Scan depth", s.draft.scan_depth, t, app));
    col.child(labeled("Ignore (comma-separated)", s.ignore.clone(), t))
}

fn launchers_section(s: &SettingsState, t: &Theme) -> impl IntoElement {
    section(t, "Launchers")
        .child(labeled("IDE command", s.ide.clone(), t))
        .child(labeled("Agent command", s.agent.clone(), t))
}

fn ai_section(s: &SettingsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    section(t, "AI & search")
        .child(toggle(
            "Enable AI features",
            s.draft.ai_enabled,
            t,
            app,
            |a| {
                if let Some(s) = &mut a.settings {
                    s.draft.ai_enabled = !s.draft.ai_enabled;
                }
            },
        ))
        .child(labeled("Ollama host", s.ollama_host.clone(), t))
        .child(labeled("Chat model", s.ai_model.clone(), t))
        .child(labeled("Embedding model", s.embed_model.clone(), t))
        .child(labeled("GitHub OAuth client id", s.client_id.clone(), t))
}

fn notifications_section(
    s: &SettingsState,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    section(t, "Notifications")
        .child(toggle(
            "Background notifications",
            s.draft.notify_enabled,
            t,
            app,
            |a| {
                if let Some(s) = &mut a.settings {
                    s.draft.notify_enabled = !s.draft.notify_enabled;
                }
            },
        ))
        .child(toggle(
            "New pull request",
            s.draft.notify_new_pr,
            t,
            app,
            |a| {
                if let Some(s) = &mut a.settings {
                    s.draft.notify_new_pr = !s.draft.notify_new_pr;
                }
            },
        ))
        .child(toggle(
            "Review requested",
            s.draft.notify_review_requested,
            t,
            app,
            |a| {
                if let Some(s) = &mut a.settings {
                    s.draft.notify_review_requested = !s.draft.notify_review_requested;
                }
            },
        ))
        .child(toggle(
            "CI failure",
            s.draft.notify_ci_failure,
            t,
            app,
            |a| {
                if let Some(s) = &mut a.settings {
                    s.draft.notify_ci_failure = !s.draft.notify_ci_failure;
                }
            },
        ))
}

fn save_footer(s: &SettingsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let app2 = app.clone();
    let mut bar = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .h(px(56.))
        .px(px(20.))
        .border_t_1()
        .border_color(rgb(t.border))
        .child(div().flex_1());
    if s.saved {
        bar = bar.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.clean))
                .child(lucide("check", 14., t.clean))
                .child("Saved"),
        );
    }
    bar.child(button("Save & rescan", t, move |cx| {
        app2.update(cx, |this, cx| this.settings_save(cx));
    }))
}

// ── building blocks ─────────────────────────────────────────────────────────

fn section(t: &Theme, title: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(16.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(title.to_string())),
        )
}

/// A labelled text-input row.
fn labeled(label: &str, input: Entity<InputState>, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(field_label(label, t))
        .child(Input::new(&input))
}

fn field_label(label: &str, t: &Theme) -> impl IntoElement {
    div()
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(label.to_string()))
}

/// A label + on/off switch row.
fn toggle(
    label: &str,
    on: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
    flip: impl Fn(&mut OrreryApp) + 'static,
) -> impl IntoElement {
    let app = app.clone();
    let knob_x = if on { px(16.) } else { px(2.) };
    div()
        .id(SharedString::from(format!("tg-{label}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .cursor_pointer()
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| {
                flip(this);
                cx.notify();
            });
        })
        .child(
            div()
                .flex_1()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .w(px(34.))
                .h(px(18.))
                .rounded(px(9.))
                .bg(rgb(if on { t.primary } else { t.button_bg }))
                .border_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .mt(px(1.))
                        .ml(knob_x)
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(7.))
                        .bg(rgb(t.fg0)),
                ),
        )
}

/// A label + −/value/+ stepper for scan depth.
fn stepper(label: &str, value: usize, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let dec = step_btn("−", t, app, |a| {
        if let Some(s) = &mut a.settings {
            s.draft.scan_depth = s.draft.scan_depth.saturating_sub(1).max(1);
        }
    });
    let inc = step_btn("+", t, app, |a| {
        if let Some(s) = &mut a.settings {
            s.draft.scan_depth = (s.draft.scan_depth + 1).min(8);
        }
    });
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .flex_1()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(SharedString::from(label.to_string())),
        )
        .child(dec)
        .child(
            div()
                .w(px(24.))
                .text_center()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(value.to_string())),
        )
        .child(inc)
}

fn step_btn(
    label: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    on: impl Fn(&mut OrreryApp) + 'static,
) -> impl IntoElement {
    let app = app.clone();
    div()
        .id(SharedString::from(format!("step-{label}")))
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .font_family("monospace")
        .text_color(rgb(t.fg1))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(t.border_strong)))
        .child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| {
                on(this);
                cx.notify();
            });
        })
}

fn icon_btn(
    icon: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    on: impl Fn(&mut OrreryApp) + 'static,
) -> impl IntoElement {
    let app = app.clone();
    div()
        .id(SharedString::from(format!("ib-{icon}")))
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(t.r_xs))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.surface_hover)))
        .child(lucide(icon, 13., t.fg3))
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| {
                on(this);
                cx.notify();
            });
        })
}

fn button(label: &str, t: &Theme, on: impl Fn(&mut gpui::App) + 'static) -> impl IntoElement {
    let (hb, hf) = (t.border_strong, t.fg0);
    div()
        .id(SharedString::from(format!("btn-{label}")))
        .px(px(14.))
        .py(px(7.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hb)).text_color(rgb(hf)))
        .child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| on(cx))
}
