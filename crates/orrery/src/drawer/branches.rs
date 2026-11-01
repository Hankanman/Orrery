//! Drawer Overview's Branches section: local branches, switchable by click.

use super::*;

pub(super) fn branches_section(
    data: &DrawerData,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let mut s = section(t, "Branches", data.branches.as_ref().map(|b| b.len()));
    match &data.branches {
        None => s = s.child(placeholder("Loading…", t)),
        Some(list) if list.is_empty() => s = s.child(placeholder("No branches.", t)),
        Some(list) => {
            for b in list {
                s = s.child(branch_item(b, t, app));
            }
        }
    }
    s
}

fn branch_item(b: &BranchRow, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let fg = if b.current { t.accent_bright } else { t.fg1 };
    let icon = if b.current { "check" } else { "git-branch" };
    let mut item = div()
        .id(SharedString::from(format!("br-{}", b.name)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(t.r_sm))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(lucide(icon, 13., fg))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(b.name.clone()),
        );
    if b.merged {
        item = item.child(tag("merged", t.fg3, t));
    }
    if b.gone {
        item = item.child(tag("gone", t.behind, t));
    }
    // Only non-current branches are switchable.
    if !b.current {
        let name = b.name.clone();
        let app = app.clone();
        item = item
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .on_click(move |_ev, _win, cx| {
                let name = name.clone();
                app.update(cx, |this, cx| {
                    this.drawer.branches = None; // optimistic loading state
                    this.drawer_switch_branch(name, cx);
                    cx.notify();
                });
            });
    }
    item
}
