//! A lightweight line-based Markdown renderer for GPUI — enough for repo READMEs
//! and AI summaries: headings, bullet lists, fenced code blocks, and paragraphs.
//! Inline spans (bold/italic/links) are rendered as plain text for now; a richer
//! renderer can replace this without touching callers.

use gpui::{div, px, rgb, Div, FontWeight, IntoElement, ParentElement, SharedString, Styled};

use crate::theme::Theme;

const MONO: &str = "monospace";

/// Render `src` as a vertical column of styled blocks.
pub fn render(src: &str, t: &Theme) -> Div {
    let mut col = div().flex().flex_col().gap(px(8.));
    let mut code: Option<Vec<String>> = None;

    for line in src.lines() {
        // Fenced code blocks toggle on ``` and capture everything between.
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => col = col.child(code_block(&lines, t)),
                None => code = Some(Vec::new()),
            }
            continue;
        }
        if let Some(buf) = code.as_mut() {
            buf.push(line.to_string());
            continue;
        }

        let l = line.trim_end();
        if let Some(h) = l.strip_prefix("### ") {
            col = col.child(heading(h, 13., FontWeight::SEMIBOLD, t.fg1, t));
        } else if let Some(h) = l.strip_prefix("## ") {
            col = col.child(heading(h, 15., FontWeight::SEMIBOLD, t.fg0, t));
        } else if let Some(h) = l.strip_prefix("# ") {
            col = col.child(heading(h, 18., FontWeight::BOLD, t.fg0, t));
        } else if let Some(b) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
            col = col.child(bullet(b, t));
        } else if !l.is_empty() {
            col = col.child(para(l, t));
        }
        // blank lines just produce the column gap
    }
    // Unterminated fence: still render what we captured.
    if let Some(lines) = code {
        if !lines.is_empty() {
            col = col.child(code_block(&lines, t));
        }
    }
    col
}

fn heading(text: &str, size: f32, weight: FontWeight, color: u32, t: &Theme) -> impl IntoElement {
    let _ = t;
    div()
        .mt(px(4.))
        .text_size(px(size))
        .font_weight(weight)
        .text_color(rgb(color))
        .child(SharedString::from(text.to_string()))
}

fn para(text: &str, t: &Theme) -> impl IntoElement {
    div()
        .text_size(px(t.text_small))
        .line_height(px(20.))
        .text_color(rgb(t.fg1))
        .child(SharedString::from(text.to_string()))
}

fn bullet(text: &str, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .text_size(px(t.text_small))
        .line_height(px(20.))
        .text_color(rgb(t.fg1))
        .child(div().text_color(rgb(t.fg3)).child("•"))
        .child(div().flex_1().child(SharedString::from(text.to_string())))
}

fn code_block(lines: &[String], t: &Theme) -> impl IntoElement {
    let mut block = div()
        .flex()
        .flex_col()
        .p(px(10.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1));
    for line in lines {
        block = block.child(div().child(SharedString::from(line.clone())));
    }
    block
}
