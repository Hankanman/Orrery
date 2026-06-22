//! Dev Tools view — an offline utility belt: UUID, SHA-256, Base64, URL
//! encode/decode, JSON format/minify, base converter, case converter. Everything
//! runs locally (no network), so secrets never leave the machine. Each tool's
//! output recomputes live from its input field.

use base64::Engine;
use gpui::{
    Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, div, px, rgb,
};
use gpui_component::input::{Input, InputState};
use sha2::{Digest, Sha256};

use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Per-tool input fields + the search box. Created on first open; the `_subs`
/// keep the per-input observations alive so outputs recompute on each keystroke.
pub struct DevToolsState {
    pub search: Entity<InputState>,
    pub uuid: SharedString,
    pub base64: Entity<InputState>,
    pub hash: Entity<InputState>,
    pub json: Entity<InputState>,
    pub base_conv: Entity<InputState>,
    pub case_conv: Entity<InputState>,
    pub url: Entity<InputState>,
    pub _subs: Vec<Subscription>,
}

/// A fresh UUID v4 string.
pub fn new_uuid() -> SharedString {
    uuid::Uuid::new_v4().to_string().into()
}

pub fn render(
    s: &DevToolsState,
    t: &Theme,
    app: &Entity<OrreryApp>,
    cx: &Context<OrreryApp>,
) -> impl IntoElement {
    let q = s.search.read(cx).value().to_lowercase();
    let show = |name: &str, keys: &str| {
        q.is_empty() || name.to_lowercase().contains(&q) || keys.contains(&q)
    };

    let mut grid = div().flex().flex_col().gap(px(12.));
    if show("UUID", "uuid guid id") {
        grid = grid.child(uuid_tool(s, t, app));
    }
    if show("Base64", "base64 encode decode") {
        grid = grid.child(base64_tool(s, t, cx));
    }
    if show("SHA-256", "hash sha sha256 digest") {
        grid = grid.child(hash_tool(s, t, cx));
    }
    if show("URL encode", "url percent encode decode") {
        grid = grid.child(url_tool(s, t, cx));
    }
    if show("JSON", "json format minify pretty") {
        grid = grid.child(json_tool(s, t, cx));
    }
    if show("Base converter", "base hex octal binary radix number") {
        grid = grid.child(base_tool(s, t, cx));
    }
    if show("Case converter", "case upper lower snake kebab camel title") {
        grid = grid.child(case_tool(s, t, cx));
    }

    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(t.page))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(14.))
                .h(px(52.))
                .px(px(20.))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(t.text_h3))
                        .text_color(rgb(t.fg0))
                        .child("Dev Tools"),
                )
                .child(div().w(px(280.)).child(Input::new(&s.search))),
        )
        .child(
            div()
                .id("devtools-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(20.))
                .child(grid),
        )
}

// ── tools ───────────────────────────────────────────────────────────────────

fn uuid_tool(s: &DevToolsState, t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let app = app.clone();
    card("UUID v4", t).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .child(mono(s.uuid.clone(), t).flex_1())
            .child(super::button("Generate", t, move |cx| {
                app.update(cx, |this, cx| {
                    if let Some(d) = &mut this.devtools {
                        d.uuid = new_uuid();
                    }
                    cx.notify();
                });
            })),
    )
}

fn base64_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.base64.read(cx).value();
    let encoded = base64::engine::general_purpose::STANDARD.encode(input.as_bytes());
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(input.trim().as_bytes())
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    card("Base64", t)
        .child(Input::new(&s.base64))
        .child(out("Encoded", encoded, t))
        .child(out("Decoded", decoded, t))
}

fn hash_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.hash.read(cx).value();
    let digest = if input.is_empty() {
        String::new()
    } else {
        let mut h = Sha256::new();
        h.update(input.as_bytes());
        hex(&h.finalize())
    };
    card("SHA-256", t)
        .child(Input::new(&s.hash))
        .child(out("Digest", digest, t))
}

fn url_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.url.read(cx).value();
    let encoded = urlencoding::encode(&input).into_owned();
    let decoded = urlencoding::decode(input.trim())
        .map(|c| c.into_owned())
        .unwrap_or_default();
    card("URL encode / decode", t)
        .child(Input::new(&s.url))
        .child(out("Encoded", encoded, t))
        .child(out("Decoded", decoded, t))
}

fn json_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.json.read(cx).value();
    let (pretty, minified) = match serde_json::from_str::<serde_json::Value>(&input) {
        Ok(v) => (
            serde_json::to_string_pretty(&v).unwrap_or_default(),
            serde_json::to_string(&v).unwrap_or_default(),
        ),
        Err(_) if input.trim().is_empty() => (String::new(), String::new()),
        Err(e) => (format!("invalid JSON: {e}"), String::new()),
    };
    card("JSON", t)
        .child(Input::new(&s.json).h_full())
        .child(out_block("Formatted", pretty, t))
        .child(out("Minified", minified, t))
}

fn base_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.base_conv.read(cx).value();
    let n = input.trim().parse::<i128>().ok();
    let (hex, oct, bin) = match n {
        Some(n) => (format!("0x{n:X}"), format!("0o{n:o}"), format!("0b{n:b}")),
        None => (String::new(), String::new(), String::new()),
    };
    card("Base converter (decimal →)", t)
        .child(Input::new(&s.base_conv))
        .child(out("Hex", hex, t))
        .child(out("Octal", oct, t))
        .child(out("Binary", bin, t))
}

fn case_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.case_conv.read(cx).value();
    let words = split_words(&input);
    card("Case converter", t)
        .child(Input::new(&s.case_conv))
        .child(out("UPPER", input.to_uppercase(), t))
        .child(out("lower", input.to_lowercase(), t))
        .child(out("snake_case", words.join("_"), t))
        .child(out("kebab-case", words.join("-"), t))
        .child(out("camelCase", camel(&words), t))
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn card(title: &str, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(14.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(SharedString::from(title.to_string())),
        )
}

/// A labelled single-line output row.
fn out(label: &str, value: String, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .w(px(78.))
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg3))
                .child(SharedString::from(label.to_string())),
        )
        .child(mono(SharedString::from(value), t).flex_1().truncate())
}

/// A labelled multi-line output block (e.g. pretty JSON).
fn out_block(label: &str, value: String, t: &Theme) -> impl IntoElement {
    let mut block = div().flex().flex_col().gap(px(3.)).child(
        div()
            .text_size(px(t.text_data_sm))
            .text_color(rgb(t.fg3))
            .child(SharedString::from(label.to_string())),
    );
    let body = div()
        .flex()
        .flex_col()
        .p(px(8.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1));
    let body = value.lines().take(40).fold(body, |b, line| {
        b.child(SharedString::from(line.to_string()))
    });
    block = block.child(body);
    block
}

fn mono(value: SharedString, t: &Theme) -> gpui::Div {
    div()
        .min_w(px(0.))
        .font_family("monospace")
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .child(value)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Split text into lowercase words across spaces, `_`, `-`, and camelCase humps.
fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_whitespace() || ch == '_' || ch == '-' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
        } else {
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(ch.to_ascii_lowercase());
            prev_lower = ch.is_lowercase();
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn camel(words: &[String]) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(w);
        } else {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                out.extend(f.to_uppercase());
                out.push_str(c.as_str());
            }
        }
    }
    out
}
