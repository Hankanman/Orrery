//! The Mission Control contribution graph — a GitHub-style commit heatmap.
//!
//! Data (per-day counts, total, window) comes from `orrery_core::activity`; this
//! module is pure layout: a 53-week × 7-day grid of themed squares, month labels,
//! a Less→More legend, and a dismiss button. Colors are the theme's `heat` ramp,
//! so the graph follows the system accent like the rest of the UI.

use chrono::{Datelike, Duration};
use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};
use orrery_core::activity::{Activity, WEEKS};

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Square size and inter-square gap (px). One column is `CELL + GAP` wide.
const CELL: f32 = 11.0;
const GAP: f32 = 3.0;
const COL_W: f32 = CELL + GAP;

/// Color bucket for a day's commit count. `None` (a future day in the trailing
/// partial week) blends into the page so no square shows.
fn heat_color(count: Option<u32>, t: &Theme) -> u32 {
    match count {
        None => t.page,
        Some(0) => t.heat[0],
        Some(1..=2) => t.heat[1],
        Some(3..=5) => t.heat[2],
        Some(6..=9) => t.heat[3],
        Some(_) => t.heat[4],
    }
}

/// Group digits with thousands separators: `3046` → `"3,046"`.
fn commas(n: u32) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A single 11px heat square.
fn square(color: u32) -> impl IntoElement {
    div().w(px(CELL)).h(px(CELL)).rounded(px(2.)).bg(rgb(color))
}

/// The Less→More legend: the five ramp swatches between two labels.
fn legend(t: &Theme) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg3))
        .child("Less");
    for level in t.heat {
        row = row.child(square(level));
    }
    row.child("More")
}

/// Render the contribution graph band. `app` is captured so the dismiss button
/// can hide it.
pub fn render(activity: &Activity, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let grid_w = px(WEEKS as f32 * COL_W);

    // Month labels: one per column where the month changes, absolutely placed so
    // the text can overflow its 14px column to the right (GitHub-style).
    let mut labels = div().relative().h(px(15.)).w(grid_w);
    let mut prev_month = 0u32;
    for col in 0..WEEKS {
        let date = activity.start + Duration::days((col * 7) as i64);
        if date.month() != prev_month {
            prev_month = date.month();
            labels = labels.child(
                div()
                    .absolute()
                    .left(px(col as f32 * COL_W))
                    .top_0()
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(t.fg2))
                    .child(SharedString::from(date.format("%b").to_string())),
            );
        }
    }

    // The 53 week-columns, each a vertical run of 7 day squares.
    let mut grid = div().flex().flex_row().gap(px(GAP));
    for col in 0..WEEKS {
        let mut column = div().flex().flex_col().gap(px(GAP));
        for row in 0..7 {
            let count = activity.cells.get(col * 7 + row).copied().flatten();
            column = column.child(square(heat_color(count, t)));
        }
        grid = grid.child(column);
    }

    let dismiss = {
        let app = app.clone();
        div()
            .id("activity-dismiss")
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(t.r_xs))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.surface_hover)))
            .child(lucide("x", 15., t.fg3))
            .on_click(move |_ev, _win, cx| {
                app.update(cx, |this, cx| {
                    this.activity_open = false;
                    cx.notify();
                });
            })
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .px(px(16.))
        .py(px(14.))
        .border_b_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.page))
        // header: ACTIVITY · total — legend · dismiss
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(t.text_data_sm))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.fg2))
                        .child("ACTIVITY"),
                )
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(SharedString::from(format!(
                            "{} commits in the last year",
                            commas(activity.total)
                        ))),
                )
                .child(div().flex_1())
                .child(legend(t))
                .child(dismiss),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(labels)
                .child(grid),
        )
}

#[cfg(test)]
mod tests {
    use super::commas;

    #[test]
    fn commas_groups_thousands() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(42), "42");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(3046), "3,046");
        assert_eq!(commas(1234567), "1,234,567");
    }
}
