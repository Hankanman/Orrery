//! Dev Tools view — an offline utility belt: UUID (v4/v7), hashes (SHA-1/256/
//! 384/512), Base64, URL encode/decode, JSON format/minify, JWT decode, base
//! converter, timestamp converter, colour converter, case converter, regex
//! tester. Everything runs locally (no network), so secrets never leave the
//! machine. Each tool's output recomputes live from its input field.

use base64::Engine;
use chrono::{DateTime, Local, Utc};
use gpui::{
    Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, div, px, rgb,
};
use gpui_component::input::{Input, InputState};
use regex::Regex;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

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
    pub jwt: Entity<InputState>,
    pub timestamp: Entity<InputState>,
    pub colour: Entity<InputState>,
    pub regex_pat: Entity<InputState>,
    pub regex_text: Entity<InputState>,
    pub _subs: Vec<Subscription>,
}

/// A fresh UUID v4 string.
pub fn new_uuid() -> SharedString {
    uuid::Uuid::new_v4().to_string().into()
}

/// A fresh UUID v7 string (time-ordered).
pub fn new_uuid_v7() -> SharedString {
    uuid::Uuid::now_v7().to_string().into()
}

pub fn render(
    s: &DevToolsState,
    filter: Option<&str>,
    t: &Theme,
    app: &Entity<OrreryApp>,
    cx: &Context<OrreryApp>,
) -> impl IntoElement {
    let q = s.search.read(cx).value().to_lowercase();
    // A tool shows when it matches the search box AND the sidebar category.
    let show = |name: &str, keys: &str, category: &str| {
        let matches_search = q.is_empty() || name.to_lowercase().contains(&q) || keys.contains(&q);
        let in_category = filter.is_none() || filter == Some(category);
        matches_search && in_category
    };

    let mut grid = div().flex().flex_col().gap(px(12.));
    if show("UUID", "uuid guid id", "generators") {
        grid = grid.child(uuid_tool(s, t, app));
    }
    if show("Base64", "base64 encode decode", "encoding") {
        grid = grid.child(base64_tool(s, t, cx));
    }
    if show("SHA-256", "hash sha sha256 digest", "hashing") {
        grid = grid.child(hash_tool(s, t, cx));
    }
    if show("URL encode", "url percent encode decode", "encoding") {
        grid = grid.child(url_tool(s, t, cx));
    }
    if show("JSON", "json format minify pretty", "data") {
        grid = grid.child(json_tool(s, t, cx));
    }
    if show("JWT decoder", "jwt token decode auth bearer", "data") {
        grid = grid.child(jwt_tool(s, t, cx));
    }
    if show(
        "Base converter",
        "base hex octal binary radix number",
        "convert",
    ) {
        grid = grid.child(base_tool(s, t, cx));
    }
    if show(
        "Timestamp converter",
        "timestamp epoch unix date time",
        "convert",
    ) {
        grid = grid.child(timestamp_tool(s, t, cx));
    }
    if show("Colour converter", "colour color hex rgb hsl", "convert") {
        grid = grid.child(colour_tool(s, t, cx));
    }
    if show(
        "Case converter",
        "case upper lower snake kebab camel title",
        "text",
    ) {
        grid = grid.child(case_tool(s, t, cx));
    }
    if show("Regex tester", "regex regexp pattern match test", "text") {
        grid = grid.child(regex_tool(s, t, cx));
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
    let regen = |make: fn() -> SharedString, app: Entity<OrreryApp>| {
        move |cx: &mut gpui::App| {
            app.update(cx, |this, cx| {
                if let Some(d) = &mut this.devtools {
                    d.uuid = make();
                }
                cx.notify();
            });
        }
    };
    card("UUID", t).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .child(mono(s.uuid.clone(), t).flex_1())
            .child(super::button("v4", t, regen(new_uuid, app.clone())))
            .child(super::button("v7", t, regen(new_uuid_v7, app.clone()))),
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
    let bytes = input.as_bytes();
    let (sha1, sha256, sha384, sha512) = if input.is_empty() {
        Default::default()
    } else {
        (
            sha::<Sha1>(bytes),
            sha::<Sha256>(bytes),
            sha::<Sha384>(bytes),
            sha::<Sha512>(bytes),
        )
    };
    card("Hash (SHA)", t)
        .child(Input::new(&s.hash))
        .child(out("SHA-1", sha1, t))
        .child(out("SHA-256", sha256, t))
        .child(out("SHA-384", sha384, t))
        .child(out("SHA-512", sha512, t))
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

fn jwt_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.jwt.read(cx).value();
    let token = input.trim();
    let (header, payload) = if token.is_empty() {
        (String::new(), String::new())
    } else {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            (
                "not a JWT (expected header.payload.signature)".to_string(),
                String::new(),
            )
        } else {
            (decode_jwt_segment(parts[0]), decode_jwt_segment(parts[1]))
        }
    };
    card("JWT decoder", t)
        .child(Input::new(&s.jwt))
        .child(out_block("Header", header, t))
        .child(out_block("Payload", payload, t))
}

/// Base64url-decode one JWT segment and pretty-print it as JSON. Decode-only —
/// the signature is never verified (no secret leaves the machine).
fn decode_jwt_segment(seg: &str) -> String {
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(seg.as_bytes()) else {
        return "invalid base64url".to_string();
    };
    let text = String::from_utf8_lossy(&bytes);
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.into_owned()),
        Err(_) => text.into_owned(),
    }
}

fn timestamp_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.timestamp.read(cx).value();
    let raw = input.trim();
    // Number → treat as a unix epoch (seconds, or milliseconds when too large to
    // be a plausible second count). Otherwise try to parse it as an RFC 3339 date.
    let dt: Option<DateTime<Utc>> = if let Ok(n) = raw.parse::<i64>() {
        if n.abs() > 100_000_000_000 {
            DateTime::from_timestamp_millis(n)
        } else {
            DateTime::from_timestamp(n, 0)
        }
    } else {
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let (utc, local, unix) = match dt {
        Some(dt) => (
            dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string(),
            dt.timestamp().to_string(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    card("Timestamp converter", t)
        .child(Input::new(&s.timestamp))
        .child(out("UTC", utc, t))
        .child(out("Local", local, t))
        .child(out("Unix", unix, t))
}

fn colour_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let input = s.colour.read(cx).value();
    let rgb_val = parse_colour(input.trim());
    let card = card("Colour converter", t).child(Input::new(&s.colour));
    let Some((r, g, b)) = rgb_val else {
        return card
            .child(out("HEX", String::new(), t))
            .child(out("RGB", String::new(), t))
            .child(out("HSL", String::new(), t));
    };
    let (h, sat, l) = rgb_to_hsl(r, g, b);
    let swatch = div()
        .w(px(40.))
        .h(px(20.))
        .rounded(px(t.r_sm))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(((r as u32) << 16) | ((g as u32) << 8) | (b as u32)));
    card.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .child(swatch)
            .child(mono(SharedString::from(format!("#{r:02x}{g:02x}{b:02x}")), t).flex_1()),
    )
    .child(out("RGB", format!("rgb({r}, {g}, {b})"), t))
    .child(out("HSL", format!("hsl({h}, {sat}%, {l}%)"), t))
}

fn regex_tool(s: &DevToolsState, t: &Theme, cx: &Context<OrreryApp>) -> impl IntoElement {
    let pat = s.regex_pat.read(cx).value();
    let text = s.regex_text.read(cx).value();
    let summary = if pat.is_empty() {
        String::new()
    } else {
        match Regex::new(&pat) {
            Err(e) => format!("invalid pattern: {e}"),
            Ok(re) => {
                let mut lines = Vec::new();
                for (i, caps) in re.captures_iter(&text).take(20).enumerate() {
                    let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                    let groups: Vec<String> = (1..caps.len())
                        .map(|g| caps.get(g).map(|m| m.as_str()).unwrap_or("").to_string())
                        .collect();
                    if groups.is_empty() {
                        lines.push(format!("{}: {whole}", i + 1));
                    } else {
                        lines.push(format!("{}: {whole}  [{}]", i + 1, groups.join(", ")));
                    }
                }
                if lines.is_empty() {
                    "no matches".to_string()
                } else {
                    format!("{} match(es)\n{}", lines.len(), lines.join("\n"))
                }
            }
        }
    };
    card("Regex tester", t)
        .child(Input::new(&s.regex_pat))
        .child(Input::new(&s.regex_text))
        .child(out_block("Matches", summary, t))
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

/// Lowercase hex digest of `bytes` under any RustCrypto `Digest` (SHA-1/2 here).
fn sha<D: Digest>(bytes: &[u8]) -> String {
    let mut h = D::new();
    h.update(bytes);
    hex(&h.finalize())
}

/// Parse `#rgb`, `#rrggbb` (with or without `#`), or `r,g,b` into 8-bit RGB.
fn parse_colour(input: &str) -> Option<(u8, u8, u8)> {
    if input.is_empty() {
        return None;
    }
    if let Some((r, g, b)) = input
        .split(',')
        .map(|p| p.trim().parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .filter(|v| v.len() == 3)
        .map(|v| (v[0], v[1], v[2]))
    {
        return Some((r, g, b));
    }
    let h = input.trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let expand = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        3 => {
            let c: Vec<char> = h.chars().collect();
            Some((
                expand(&format!("{0}{0}", c[0]))?,
                expand(&format!("{0}{0}", c[1]))?,
                expand(&format!("{0}{0}", c[2]))?,
            ))
        }
        6 => Some((expand(&h[0..2])?, expand(&h[2..4])?, expand(&h[4..6])?)),
        _ => None,
    }
}

/// 8-bit RGB → HSL, rounded to whole degrees / percents.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u16, u8, u8) {
    let (rf, gf, bf) = (r as f64 / 255., g as f64 / 255., b as f64 / 255.);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.;
    let d = max - min;
    let (mut h, sat) = if d.abs() < f64::EPSILON {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == rf {
            ((gf - bf) / d).rem_euclid(6.0)
        } else if max == gf {
            (bf - rf) / d + 2.0
        } else {
            (rf - gf) / d + 4.0
        };
        (h * 60.0, s)
    };
    if h < 0.0 {
        h += 360.0;
    }
    (
        h.round() as u16,
        (sat * 100.0).round() as u8,
        (l * 100.0).round() as u8,
    )
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

#[cfg(test)]
mod tests {
    use super::{decode_jwt_segment, parse_colour, rgb_to_hsl, sha};
    use sha2::Sha256;

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha::<Sha256>(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_colour_accepts_hex_shorthand_and_rgb() {
        assert_eq!(parse_colour("#1f6feb"), Some((31, 111, 235)));
        assert_eq!(parse_colour("1f6feb"), Some((31, 111, 235)));
        assert_eq!(parse_colour("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_colour("31, 111, 235"), Some((31, 111, 235)));
        assert_eq!(parse_colour("not a colour"), None);
        assert_eq!(parse_colour("#12"), None);
        assert_eq!(parse_colour("300,0,0"), None, "out of u8 range");
    }

    #[test]
    fn rgb_to_hsl_known_colours() {
        assert_eq!(rgb_to_hsl(255, 0, 0), (0, 100, 50), "pure red");
        assert_eq!(rgb_to_hsl(0, 0, 0), (0, 0, 0), "black");
        assert_eq!(rgb_to_hsl(255, 255, 255), (0, 0, 100), "white");
    }

    #[test]
    fn jwt_segment_decodes_base64url_json() {
        // {"alg":"HS256","typ":"JWT"} base64url-encoded, no padding.
        let header = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let decoded = decode_jwt_segment(header);
        assert!(decoded.contains("\"alg\""));
        assert!(decoded.contains("HS256"));
        assert_eq!(decode_jwt_segment("!!!notbase64!!!"), "invalid base64url");
    }
}
