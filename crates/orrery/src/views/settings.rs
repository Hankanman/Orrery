//! Settings view — edit the config in-app instead of hand-editing TOML.
//!
//! A draft `AppConfig` (toggles / scan depth / roots) lives on `OrreryApp`
//! alongside text-input entities for the string fields; "Save & rescan" reads
//! them back, persists via `config::save`, and re-scans. This first cut covers
//! roots, launchers, AI backend/endpoints, and notifications, plus the live
//! panels: GitHub device-flow login and AI status — Ollama (host + model pull)
//! or llama.cpp (server path + GGUF download), switchable in-app.

use gpui::{
    AppContext, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use gpui_component::input::{Input, InputState};
use orrery_core::model::AppConfig;

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// A GitHub device-flow login in progress: the code the user types at the
/// verification URL, plus a live status line.
pub struct GithubDevice {
    pub user_code: SharedString,
    pub verification_uri: SharedString,
    pub status: SharedString,
}

/// Live reachability of the AI backend, refreshed when Settings opens and on
/// demand. Drives the AI-status panel.
#[derive(Default, Clone)]
pub enum AiStatus {
    #[default]
    Unknown,
    Checking,
    Offline,
    /// Reachable — the installed models as (name, human size).
    Ready(Vec<(SharedString, SharedString)>),
    /// A model download is in flight.
    Pulling(SharedString),
}

/// The editable settings session: a draft config + the string-field inputs.
pub struct SettingsState {
    pub draft: AppConfig,
    pub ide: Entity<InputState>,
    pub agent: Entity<InputState>,
    pub ollama_host: Entity<InputState>,
    pub ai_model: Entity<InputState>,
    pub embed_model: Entity<InputState>,
    /// llama.cpp: path override for the `llama-server` binary.
    pub llama_server: Entity<InputState>,
    /// llama.cpp: a GGUF model URL to download.
    pub llama_url: Entity<InputState>,
    pub client_id: Entity<InputState>,
    pub ignore: Entity<InputState>,
    pub add_root: Entity<InputState>,
    /// Scan depth as a NumberInput (integer 1–8).
    pub scan_depth: Entity<InputState>,
    /// Flash a "Saved" confirmation after a successful save.
    pub saved: bool,
    /// Result line for the AI Test / Clear-cache actions.
    pub ai_note: SharedString,
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
            llama_server: field(
                window,
                cx,
                "llama-server (auto-detected)",
                &cfg.llama_server_path,
            ),
            llama_url: field(window, cx, "https://…/model.gguf", ""),
            client_id: field(window, cx, "GitHub OAuth client id", &cfg.github_client_id),
            ignore: field(window, cx, "node_modules, .cache", &cfg.ignore.join(", ")),
            add_root: field(window, cx, "~/dev", ""),
            scan_depth: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(cfg.scan_depth.to_string())
                    .validate(|s, _| s.chars().all(|c| c.is_ascii_digit()))
                    .min(1.)
                    .max(8.)
                    .step(1.)
            }),
            draft: cfg.clone(),
            saved: false,
            ai_note: SharedString::default(),
        }
    }
}

pub fn render(
    s: &SettingsState,
    filter: Option<&str>,
    authed: bool,
    device: &Option<GithubDevice>,
    ai: &AiStatus,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    // The sidebar SECTIONS panel gates which section shows (None = all).
    let show = |section: &str| filter.is_none() || filter == Some(section);
    let mut sections: Vec<gpui::AnyElement> = Vec::new();
    if show("account") {
        sections.push(account_section(authed, device, t, app).into_any_element());
    }
    if show("roots") {
        sections.push(roots_section(s, t, app).into_any_element());
    }
    if show("launchers") {
        sections.push(launchers_section(s, t).into_any_element());
    }
    if show("ai") {
        sections.push(ai_section(s, ai, t, app).into_any_element());
    }
    if show("notifications") {
        sections.push(notifications_section(s, t, app).into_any_element());
    }

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
                .children(sections),
        )
        // Save footer
        .child(save_footer(s, t, app))
}

/// GitHub account panel: connection status + Connect (device flow) / Sign out.
fn account_section(
    authed: bool,
    device: &Option<GithubDevice>,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut col = section(t, "GitHub account");

    if let Some(d) = device {
        // A device-flow login is in progress: show the code + verification URL.
        let uri = d.verification_uri.clone();
        col = col
            .child(field_label(
                "Enter this code at the verification page, then authorize:",
                t,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(t.r_sm))
                            .bg(rgb(t.button_bg))
                            .border_1()
                            .border_color(rgb(t.border))
                            .font_family("monospace")
                            .text_size(px(t.text_h3))
                            .text_color(rgb(t.fg0))
                            .child(d.user_code.clone()),
                    )
                    .child(button("Open page", t, move |_cx| {
                        let _ = orrery_core::launch::open(&uri);
                    })),
            )
            .child(note_line(d.status.clone(), t.fg3, t));
    } else if authed {
        col = col
            .child(status_row(
                "circle-check",
                "Connected to GitHub",
                t.clean,
                t,
            ))
            .child(div().child(button("Sign out", t, {
                let app = app.clone();
                move |cx| {
                    app.update(cx, |this, cx| this.github_sign_out(cx));
                }
            })));
    } else {
        col = col
            .child(status_row(
                "circle-alert",
                "Not connected — sign in to use Inbox, Feed, Explore and PR actions.",
                t.fg3,
                t,
            ))
            .child(div().child(button("Connect GitHub", t, {
                let app = app.clone();
                move |cx| {
                    app.update(cx, |this, cx| this.github_connect(cx));
                }
            })));
    }
    col
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
    col = col.child(number_row("Scan depth", &s.scan_depth, t));
    col.child(labeled("Ignore (comma-separated)", s.ignore.clone(), t))
}

fn launchers_section(s: &SettingsState, t: &Theme) -> impl IntoElement {
    section(t, "Launchers")
        .child(labeled("IDE command", s.ide.clone(), t))
        .child(labeled("Agent command", s.agent.clone(), t))
}

/// True when the configured backend string selects the llama.cpp sidecar.
fn backend_is_llama(backend: &str) -> bool {
    matches!(backend, "llamaCpp" | "llama_cpp" | "llamacpp")
}

fn ai_section(
    s: &SettingsState,
    ai: &AiStatus,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let is_llama = backend_is_llama(&s.draft.ai_backend);
    let mut sec = section(t, "AI & search")
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
        .child(backend_picker(is_llama, t, app));
    if is_llama {
        sec = sec
            .child(labeled("llama-server path", s.llama_server.clone(), t))
            .child(llama_download_row(s, t, app))
            .child(llama_model_line(s, t));
    } else {
        sec = sec
            .child(labeled("Ollama host", s.ollama_host.clone(), t))
            .child(labeled("Chat model", s.ai_model.clone(), t))
            .child(labeled("Embedding model", s.embed_model.clone(), t));
    }
    sec.child(labeled("GitHub OAuth client id", s.client_id.clone(), t))
        .child(ai_status_block(s, is_llama, ai, t, app))
}

/// Two-way Ollama / llama.cpp backend picker. Takes effect on Save.
fn backend_picker(is_llama: bool, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let opt = |label: &'static str, llama: bool| {
        let app = app.clone();
        let on = llama == is_llama;
        div()
            .id(label)
            .px(px(12.))
            .py(px(6.))
            .rounded(px(t.r_sm))
            .bg(rgb(if on { t.accent_wash } else { t.button_bg }))
            .border_1()
            .border_color(rgb(if on { t.primary } else { t.border }))
            .text_size(px(t.text_data_sm))
            .text_color(rgb(if on { t.fg0 } else { t.fg2 }))
            .cursor_pointer()
            .child(label)
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    if let Some(s) = &mut this.settings {
                        s.draft.ai_backend = if llama {
                            "llamaCpp".into()
                        } else {
                            "ollama".into()
                        };
                    }
                    cx.notify();
                });
            })
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(field_label("Backend", t))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(6.))
                .child(opt("Ollama", false))
                .child(opt("llama.cpp", true)),
        )
}

/// GGUF download row: a URL field + a Download button (progress shows in the AI
/// note line below). Only shown for the llama.cpp backend.
fn llama_download_row(s: &SettingsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let (app, url_input) = (app.clone(), s.llama_url.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(field_label("Download GGUF model", t))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().child(Input::new(&s.llama_url)))
                .child(button("Download", t, move |cx| {
                    let url = url_input.read(cx).value().to_string();
                    app.update(cx, |this, cx| this.llama_download(url, cx));
                })),
        )
}

/// Read-only display of the currently-selected GGUF model.
fn llama_model_line(s: &SettingsState, t: &Theme) -> impl IntoElement {
    let sel = s
        .draft
        .llama_model_path
        .rsplit('/')
        .next()
        .filter(|x| !x.is_empty())
        .unwrap_or("none — download or pick a model below");
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(field_label("Selected model", t))
        .child(
            div()
                .font_family("monospace")
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg1))
                .child(SharedString::from(sel.to_string())),
        )
}

/// The live AI-backend status row + installed models + refresh/pull actions.
fn ai_status_block(
    s: &SettingsState,
    is_llama: bool,
    ai: &AiStatus,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut block = div().flex().flex_col().gap(px(8.)).pt(px(4.));

    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().child(match ai {
            AiStatus::Unknown => status_row("clock", "Status unknown".to_string(), t.fg3, t),
            AiStatus::Checking => status_row("clock", "Checking…".to_string(), t.fg3, t),
            AiStatus::Offline => status_row(
                "circle-alert",
                "Backend unreachable".to_string(),
                t.behind,
                t,
            ),
            AiStatus::Ready(m) => status_row(
                "circle-check",
                format!(
                    "Reachable — {} model{}",
                    m.len(),
                    if m.len() == 1 { "" } else { "s" }
                ),
                t.clean,
                t,
            ),
            AiStatus::Pulling(name) => status_row("clock", format!("Pulling {name}…"), t.fg2, t),
        }))
        .child(button("Test", t, {
            let app = app.clone();
            move |cx| {
                app.update(cx, |this, cx| this.ai_test(cx));
            }
        }))
        .child(button("Clear cache", t, {
            let app = app.clone();
            move |cx| {
                app.update(cx, |this, cx| this.ai_clear_cache(cx));
            }
        }))
        .child(button("Refresh", t, {
            let app = app.clone();
            move |cx| {
                app.update(cx, |this, cx| this.ai_refresh(cx));
            }
        }));
    block = block.child(head);
    if !s.ai_note.is_empty() {
        block = block.child(note_line(s.ai_note.clone(), t.fg2, t));
    }

    if let AiStatus::Ready(models) = ai {
        if models.is_empty() {
            block = block.child(note_line("No models installed.".into(), t.fg3, t));
        } else {
            block = block.child(note_line("Click a model to use it.".into(), t.fg3, t));
            for (name, size) in models {
                // Clicking a model selects it: the GGUF path for llama.cpp, or the
                // chat-model name for Ollama (a lightweight picker over the list).
                let (app, ai_model, pick) = (app.clone(), s.ai_model.clone(), name.clone());
                block = block.child(
                    div()
                        .id(SharedString::from(format!("ai-model-{name}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .px(px(6.))
                        .py(px(3.))
                        .rounded(px(t.r_sm))
                        .font_family("monospace")
                        .text_size(px(t.text_data_sm))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.surface_hover)))
                        .child(lucide("box", 12., t.fg3))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(rgb(t.fg1))
                                .child(name.clone()),
                        )
                        .child(super::muted_mono(size.clone(), t))
                        .on_click(move |_ev, window, cx| {
                            let pick = pick.clone();
                            if is_llama {
                                let path = orrery_core::llama::models_dir()
                                    .map(|d| d.join(pick.as_ref()).to_string_lossy().into_owned());
                                app.update(cx, |this, cx| {
                                    if let (Some(s), Some(path)) = (&mut this.settings, path) {
                                        s.draft.llama_model_path = path;
                                    }
                                    cx.notify();
                                });
                            } else {
                                ai_model
                                    .update(cx, |st, cx| st.set_value(pick.clone(), window, cx));
                                app.update(cx, |this, _cx| {
                                    if let Some(s) = &mut this.settings {
                                        s.draft.ai_model = pick.to_string();
                                    }
                                });
                            }
                        }),
                );
            }
        }
        // Ollama can pull the configured chat model from the registry; the
        // llama.cpp backend downloads GGUFs via the field above instead.
        let model = s.draft.ai_model.clone();
        if !is_llama && !model.trim().is_empty() {
            block = block.child(div().child(button(&format!("Pull \"{model}\""), t, {
                let app = app.clone();
                move |cx| {
                    let model = model.clone();
                    app.update(cx, |this, cx| this.ai_pull(model.clone(), cx));
                }
            })));
        }
    }
    block
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

/// A status icon + label row (connection / reachability).
fn status_row(
    icon: &str,
    label: impl Into<SharedString>,
    color: u32,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(lucide(icon, 14., color))
        .child(
            div()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(label.into()),
        )
}

/// A small coloured note line (status / hint).
fn note_line(text: SharedString, color: u32, t: &Theme) -> impl IntoElement {
    div()
        .text_size(px(t.text_data_sm))
        .text_color(rgb(color))
        .child(text)
}

/// A label + gpui-component on/off [`Switch`] row. `flip` toggles the bound
/// draft field; the switch reflects `on`.
fn toggle(
    label: &str,
    on: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
    flip: impl Fn(&mut OrreryApp) + 'static,
) -> impl IntoElement {
    use gpui_component::switch::Switch;
    let app = app.clone();
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
        .child(
            Switch::new(SharedString::from(format!("tg-{label}")))
                .checked(on)
                .color(rgb(t.primary))
                .on_click(move |_checked, _window, cx| {
                    app.update(cx, |this, cx| {
                        flip(this);
                        cx.notify();
                    });
                }),
        )
}

/// A label + gpui-component [`NumberInput`] row (used for scan depth). Stepping +
/// validation live in the bound `InputState`; the value is read back on save.
fn number_row(label: &str, input: &Entity<InputState>, t: &Theme) -> impl IntoElement {
    use gpui_component::input::NumberInput;
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
        .child(div().w(px(120.)).child(NumberInput::new(input)))
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
