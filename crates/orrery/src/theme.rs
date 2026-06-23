//! Orrery's `--orr-*` design tokens (dark theme). The handful of surfaces that
//! were `color-mix`/translucent are pre-blended to opaque sRGB here — the
//! flat-design contract means we don't layer translucency anyway.
//!
//! Colors are `u32` 0xRRGGBB so call sites use `rgb(theme.fg0)`. Sizes are
//! logical px. See `docs/design-system/` for the full spec.

#![allow(dead_code)]

/// Dark-theme token set. One value per `--orr-*`/shadcn var the grid touches.
pub struct Theme {
    // Surfaces (deep blue-black void the system orbits in).
    pub page: u32,          // --background #0a0e16
    pub surface: u32,       // --orr-glass over page (card background)
    pub surface_hover: u32, // --orr-glass-hover over page
    pub button_bg: u32,     // --orr-glass-2 over page
    pub border: u32,        // --orr-border (white 7.5%) over page
    pub border_strong: u32, // --orr-border-strong (white 14%) over page
    pub border_accent: u32, // --orr-border-accent (primary 40%) over page

    // Text ramp (--orr-fg-0..3).
    pub fg0: u32, // primary
    pub fg1: u32, // body
    pub fg2: u32, // secondary / data
    pub fg3: u32, // faint

    // Identity + semantics.
    pub primary: u32,       // --primary orbit cyan
    pub accent_bright: u32, // --orr-accent-bright (primary + white 22%)
    pub accent_wash: u32,   // --orr-accent-wash (primary 12% over page) — active nav bg
    pub accent_badge: u32,  // accent 20% over page — nav count badge bg
    pub star: u32,
    pub ai: u32,
    pub clean: u32,
    pub dirty: u32,
    pub behind: u32,
    pub act_ide: u32,
    pub act_agent: u32,
    pub act_folder: u32,
    pub act_host: u32,

    // Radii (px): --r-xs/sm/md/lg.
    pub r_xs: f32,
    pub r_sm: f32,
    pub r_md: f32,
    pub r_lg: f32,

    // Type sizes (px): --text-h3 / --text-small / --text-data-sm.
    pub text_h3: f32,
    pub text_small: f32,
    pub text_data_sm: f32,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            page: 0x0a0e16,
            // glass (white @3.5%) / hover (@6%) / glass-2 (@8.5%) / borders
            // (@7.5%, @14%) pre-blended onto #0a0e16.
            surface: 0x13161e,
            surface_hover: 0x191c24,
            button_bg: 0x1f222a,
            border: 0x1c2028,
            border_strong: 0x2c3037,
            // --primary 40% over page (≈ orbit-cyan-tinted edge).
            border_accent: 0x1b4b56,

            fg0: 0xeef2f8,
            fg1: 0xafb9cb,
            fg2: 0x6c778c,
            fg3: 0x434e63,

            primary: 0x38dbf0,
            accent_bright: 0x64e3f3,
            accent_wash: 0x102730,
            accent_badge: 0x133742,
            star: 0xffc24b,
            ai: 0xa78bfa,
            clean: 0x3dd68c,
            dirty: 0xff9e45,
            behind: 0xff6b6b,
            act_ide: 0x5b9dff,
            act_agent: 0xa78bfa,
            act_folder: 0xf5b94b,
            act_host: 0x46c8a0,

            r_xs: 4.,
            r_sm: 6.,
            r_md: 10.,
            r_lg: 14.,

            text_h3: 16.,
            text_small: 13.,
            text_data_sm: 12.,
        }
    }

    /// Override the built-in orbit-cyan accent with the desktop's system accent
    /// when one is present, recomputing the derived accent tokens (bright/wash/
    /// badge/border). Matches the design system's "primary overridden by the
    /// system accent at runtime". No-op when `accent` is `None`.
    pub fn with_system_accent(mut self, accent: Option<(u8, u8, u8)>) -> Self {
        if let Some(a) = accent {
            self.primary = pack(a);
            self.accent_bright = lighten(a, 0.22);
            self.accent_wash = over_page(a, 0.12);
            self.accent_badge = over_page(a, 0.20);
            self.border_accent = over_page(a, 0.40);
        }
        self
    }
}

fn pack((r, g, b): (u8, u8, u8)) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Mix an sRGB colour toward white by fraction `f`.
fn lighten((r, g, b): (u8, u8, u8), f: f32) -> u32 {
    let m = |c: u8| (c as f32 + (255.0 - c as f32) * f).round() as u8;
    pack((m(r), m(g), m(b)))
}

/// Blend an sRGB colour over the page background (#0a0e16) at `alpha`.
fn over_page((r, g, b): (u8, u8, u8), alpha: f32) -> u32 {
    let blend = |c: u8, base: u8| (base as f32 + (c as f32 - base as f32) * alpha).round() as u8;
    pack((blend(r, 0x0a), blend(g, 0x0e), blend(b, 0x16)))
}

/// Language → embedded devicon asset stem (`assets/icons/devicon/<stem>.svg`),
/// or `None` when there's no devicon (card falls back to the color dot).
/// Mirrors the `DEVICON` table in `assets/generate-icons.mjs`.
pub fn devicon_stem(language: &str) -> Option<&'static str> {
    Some(match language.to_ascii_lowercase().as_str() {
        "rust" => "rust",
        "typescript" | "tsx" => "typescript",
        "javascript" | "jsx" => "javascript",
        "python" => "python",
        "go" => "go",
        "ruby" => "ruby",
        "java" => "java",
        "c" => "c",
        "c++" | "cpp" => "cpp",
        "c#" | "csharp" => "csharp",
        "html" => "html",
        "css" => "css",
        "shell" | "bash" | "sh" => "shell",
        "vue" => "vue",
        "svelte" => "svelte",
        "kotlin" => "kotlin",
        "swift" => "swift",
        "php" => "php",
        "scala" => "scala",
        "elixir" => "elixir",
        "haskell" => "haskell",
        "lua" => "lua",
        "dart" => "dart",
        "zig" => "zig",
        "nix" => "nix",
        "markdown" => "markdown",
        _ => return None,
    })
}

/// Language → brand dot color, the fallback language mark when no devicon
/// exists. Falls back to the faint text color for unknowns.
pub fn lang_color(language: &str, fallback: u32) -> u32 {
    match language.to_ascii_lowercase().as_str() {
        "rust" => 0xdea584,
        "typescript" | "tsx" => 0x3178c6,
        "javascript" | "jsx" => 0xf1e05a,
        "python" => 0x3572a5,
        "go" => 0x00add8,
        "ruby" => 0xcc342d,
        "java" => 0xb07219,
        "c" => 0x90a4ae,
        "c++" | "cpp" => 0xf34b7d,
        "c#" | "csharp" => 0x178600,
        "html" => 0xe34c26,
        "css" => 0x563d7c,
        "shell" | "bash" | "sh" => 0x89e051,
        "vue" => 0x41b883,
        "svelte" => 0xff3e00,
        "kotlin" => 0xa97bff,
        "swift" => 0xf05138,
        "php" => 0x4f5d95,
        "scala" => 0xc22d40,
        "elixir" => 0x6e4a7e,
        "haskell" => 0x5e5086,
        "lua" => 0x51a0cf,
        "dart" => 0x00b4ab,
        "zig" => 0xf7a41d,
        "nix" => 0x7e7eff,
        "markdown" => 0x6a9fb5,
        "toml" => 0x9c4221,
        _ => fallback,
    }
}

/// Map our `--orr-*` tokens onto gpui-component's shadcn-style theme, so its
/// components (markdown, inputs, dropdowns, …) match the rest of the UI. Call
/// once after `gpui_component::init`, before opening the window.
pub fn apply_gpui_component_theme(t: &Theme, cx: &mut gpui::App) {
    use gpui::{px, rgb};
    use gpui_component::ThemeMode;

    let c = gpui_component::Theme::global_mut(cx);
    c.mode = ThemeMode::Dark;

    // Surfaces.
    c.background = rgb(t.page).into();
    c.popover = rgb(t.surface).into();
    c.secondary = rgb(t.surface).into();
    c.muted = rgb(t.surface).into();
    c.sidebar = rgb(t.page).into();
    c.title_bar = rgb(t.page).into();
    c.input = rgb(t.button_bg).into();
    c.tab_active = rgb(t.surface).into();

    // Text.
    c.foreground = rgb(t.fg1).into();
    c.popover_foreground = rgb(t.fg1).into();
    c.secondary_foreground = rgb(t.fg1).into();
    c.sidebar_foreground = rgb(t.fg1).into();
    c.muted_foreground = rgb(t.fg3).into();
    c.accent_foreground = rgb(t.fg0).into();
    c.primary_foreground = rgb(t.page).into();

    // Borders / accents / state.
    c.border = rgb(t.border).into();
    c.drag_border = rgb(t.border_accent).into();
    c.ring = rgb(t.primary).into();
    c.primary = rgb(t.primary).into();
    c.accent = rgb(t.surface_hover).into();
    c.list_active = rgb(t.accent_wash).into();
    c.selection = rgb(t.accent_wash).into();
    c.link = rgb(t.accent_bright).into();
    c.scrollbar = rgb(t.border).into();
    c.danger = rgb(t.behind).into();
    c.success = rgb(t.clean).into();
    c.info = rgb(t.primary).into();
    c.warning = rgb(t.star).into();

    // Shape / fonts.
    c.radius = px(t.r_md);
    c.mono_font_family = "monospace".into();
}
