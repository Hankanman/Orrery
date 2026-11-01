//! Drawer Notes tab: the "resume where I left off" catch-up and an editable
//! markdown note.

use super::*;

/// Notes tab: a "resume where I left off" catch-up + an editable markdown note
/// (gpui-component multiline input) with Save.
pub(super) fn notes_view(
    row: &Row,
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ai_ready: bool,
) -> Div {
    let mut col = div().flex().flex_col().gap(px(14.));
    let Some(n) = &data.notes else {
        return col.child(placeholder("Loading…", t));
    };

    // Catch-up row.
    let mut catch = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.))
        .p(px(11.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(lucide("history", 15., t.accent_bright))
        .child(
            div()
                .flex_1()
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg1))
                .child(n.catchup.clone()),
        );
    // AI "what changed" catch-up (gated on aiReady; only when there are commits).
    if ai_ready && n.count > 0 {
        let app2 = app.clone();
        catch = catch.child(pr_btn(
            SharedString::from("gen-resume"),
            "Catch me up",
            t,
            move |cx| {
                app2.update(cx, |this, cx| this.drawer_generate_resume(cx));
            },
        ));
    }
    if n.count > 0 || n.first_visit {
        let app = app.clone();
        catch = catch.child(pr_btn(
            SharedString::from("mark-seen"),
            "Mark caught up",
            t,
            move |cx| {
                app.update(cx, |this, cx| this.drawer_mark_seen(cx));
            },
        ));
    }
    col = col.child(catch);

    // The AI narrative, once generated.
    if let Some(resume) = &n.resume {
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(11.))
                .rounded(px(t.r_sm))
                .bg(rgb(t.surface))
                .border_1()
                .border_color(rgb(t.border))
                .child(seg("sparkles", "What changed".into(), t.fg3))
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(
                            // Render as markdown (reflowed when stored, like the Readme): the
                            // prompt asks for plain sentences but models still emit
                            // multi-line text, which plain text elements panic on.
                            gpui_component::text::markdown(resume.clone()),
                        ),
                ),
        );
    }

    // Editable note field + Save.
    let mut note = section(t, "Note", None);
    if let Some(input) = &data.note_input {
        let (app, repo, input2) = (app.clone(), row.id.clone(), input.clone());
        note = note
            .child(
                div()
                    .min_h(px(160.))
                    .child(gpui_component::input::Input::new(input).h_full()),
            )
            .child(div().flex().flex_row().justify_end().child(pr_btn(
                SharedString::from("save-note"),
                "Save note",
                t,
                move |cx| {
                    let (repo, text) = (repo.clone(), input2.read(cx).value());
                    app.update(cx, |this, cx| {
                        this.drawer_save_note(repo, text.to_string(), cx)
                    });
                },
            )));
    }
    col.child(note)
}
