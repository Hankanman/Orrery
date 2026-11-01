//! Drawer Overview's Recent commits section.

use super::*;

pub(super) fn commits_section(data: &DrawerData, t: &Theme) -> impl IntoElement {
    let mut s = section(t, "Recent commits", data.commits.as_ref().map(|c| c.len()));
    match &data.commits {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No commits.", t)),
        Some(list) => {
            for c in list {
                s = s.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .py(px(4.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(t.text_small))
                                .text_color(rgb(t.fg1))
                                .child(c.summary.clone()),
                        )
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(t.text_data_sm))
                                .text_color(rgb(t.fg3))
                                .child(SharedString::from(format!("{} · {}", c.author, c.age))),
                        ),
                );
            }
        }
    }
    s
}
