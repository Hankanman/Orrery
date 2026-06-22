//! New-project dialog — the header "+" action. A centered modal to either clone
//! a remote repository or initialise a fresh one into a chosen workspace root.
//! On success it rescans so the new repo appears in the grid. Sync git
//! (clone/init) runs off the UI thread.

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, div, px, rgb, rgba,
};
use gpui_component::input::{Input, InputState};

use crate::shell::OrreryApp;
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewMode {
    Clone,
    Create,
}

/// State for the new-project dialog: the two text fields, the chosen mode and
/// destination root, plus a status line for validation / progress.
pub struct NewProjectData {
    pub mode: NewMode,
    pub url: Entity<InputState>,
    pub name: Entity<InputState>,
    /// Index into `config.roots` — the destination root.
    pub root: usize,
    pub status: SharedString,
    pub busy: bool,
    pub _subs: Vec<Subscription>,
}

pub fn render(
    d: &NewProjectData,
    roots: &[String],
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let dest_root = roots.get(d.root).cloned().unwrap_or_default();

    let mut panel = div()
        .occlude()
        .w(px(520.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(20.))
        .rounded(px(t.r_lg))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(t.text_h3))
                .text_color(rgb(t.fg0))
                .child("New project"),
        )
        // Mode tabs.
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(8.))
                .child(mode_tab("Clone repository", NewMode::Clone, d.mode, t, app))
                .child(mode_tab("New repository", NewMode::Create, d.mode, t, app)),
        );

    if d.mode == NewMode::Clone {
        panel = panel.child(field("Repository URL", &d.url, t));
    }
    panel = panel.child(field("Folder name", &d.name, t));

    // Destination root.
    if roots.is_empty() {
        panel = panel.child(
            div()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.behind))
                .child("Add a workspace root in Settings first."),
        );
    } else {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(field_label("Destination", t))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family("monospace")
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg1))
                    .child(SharedString::from(dest_root)),
            );
        if roots.len() > 1 {
            row = row.child(small_btn("Change root", t, app, |this, cx| {
                this.new_project_cycle_root(cx)
            }));
        }
        panel = panel.child(row);
    }

    if !d.status.is_empty() {
        panel = panel.child(
            div()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(if d.busy { t.fg3 } else { t.behind }))
                .child(d.status.clone()),
        );
    }

    // Actions.
    let submit_label = if d.mode == NewMode::Clone {
        "Clone"
    } else {
        "Create"
    };
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1())
            .child(small_btn("Cancel", t, app, |this, _cx| {
                this.close_overlay();
            }))
            .child(primary_btn(submit_label, d.busy, t, app)),
    );

    // Centered over a dimmed, click-to-dismiss backdrop.
    let app = app.clone();
    div()
        .id("np-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x00000088))
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, _cx| this.close_overlay());
        })
        .child(panel)
}

// ── building blocks ─────────────────────────────────────────────────────────

fn mode_tab(
    label: &str,
    mode: NewMode,
    active: NewMode,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let on = mode == active;
    div()
        .id(SharedString::from(format!("npmode-{label}")))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .bg(rgb(if on { t.accent_wash } else { t.button_bg }))
        .border_1()
        .border_color(rgb(if on { t.primary } else { t.border }))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(if on { t.fg0 } else { t.fg2 }))
        .cursor_pointer()
        .child(SharedString::from(label.to_string()))
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| this.new_project_set_mode(mode, cx));
        })
}

fn field(label: &str, input: &Entity<InputState>, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(field_label(label, t))
        .child(Input::new(input))
}

fn field_label(label: &str, t: &Theme) -> impl IntoElement {
    div()
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child(SharedString::from(label.to_string()))
}

fn small_btn(
    label: &str,
    t: &Theme,
    app: &Entity<OrreryApp>,
    on: impl Fn(&mut OrreryApp, &mut gpui::Context<OrreryApp>) + 'static,
) -> impl IntoElement {
    let app = app.clone();
    let (hb, hf) = (t.border_strong, t.fg0);
    div()
        .id(SharedString::from(format!("npbtn-{label}")))
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
        .on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| {
                on(this, cx);
                cx.notify();
            });
        })
}

fn primary_btn(label: &str, busy: bool, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let app = app.clone();
    let mut btn = div()
        .id("np-submit")
        .px(px(16.))
        .py(px(7.))
        .rounded(px(t.r_sm))
        .bg(rgb(if busy { t.button_bg } else { t.primary }))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(if busy { t.fg3 } else { t.page }))
        .child(SharedString::from(if busy { "Working…" } else { label }));
    if !busy {
        btn = btn.cursor_pointer().on_click(move |_ev, _win, cx| {
            app.update(cx, |this, cx| this.submit_new_project(cx));
        });
    }
    btn
}
