//! Drawer Overview: the project-tags editor. The repo's tags as removable
//! chips, then a field + Add button (Enter works too) to add one.

use super::*;

pub(super) fn tags_editor(
    row: &Row,
    tags: &[SharedString],
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut s = section(t, "Tags", Some(tags.len()));

    let mut chips = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.));
    if tags.is_empty() {
        chips = chips.child(placeholder("No tags yet.", t));
    }
    for tag in tags {
        let remove = {
            let (app, repo, tag) = (app.clone(), row.id.clone(), tag.clone());
            div()
                .id(SharedString::from(format!("tag-rm-{tag}")))
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .h(px(16.))
                .rounded(px(t.r_xs))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.surface_hover)))
                .child(lucide("x", 11., t.fg3))
                .on_click(move |_ev, _win, cx| {
                    let (repo, tag) = (repo.clone(), tag.clone());
                    app.update(cx, |this, cx| this.remove_tag(repo, tag, cx));
                })
        };
        chips = chips.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .pl(px(7.))
                .pr(px(3.))
                .py(px(2.))
                .rounded(px(t.r_xs))
                .border_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.surface))
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.accent_bright))
                .child(tag.clone())
                .child(remove),
        );
    }
    s = s.child(chips);

    if let Some(input) = &data.tag_input {
        let (app, input2) = (app.clone(), input.clone());
        s = s.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .mt(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(gpui_component::input::Input::new(input)),
                )
                .child(
                    widgets::button("tag-add", Tone::Neutral, t)
                        .px(px(12.))
                        .py(px(5.))
                        .child(widgets::label("Add", t.text_data_sm, false))
                        .on_click(move |_ev, window, cx| {
                            let input = input2.clone();
                            app.update(cx, |this, cx| this.drawer_submit_tag(input, window, cx));
                        }),
                ),
        );
    }
    s
}
