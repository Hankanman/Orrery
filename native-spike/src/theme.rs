//! Orrery's `--orr-*` design tokens (dark theme) ported to GPUI. Values come
//! straight from `src/index.css`: hex tokens verbatim, and the handful of
//! oklch/`color-mix` surfaces pre-blended to opaque sRGB (the flat-design
//! contract means we don't layer translucency anyway).
//!
//! Colors are `u32` 0xRRGGBB so call sites use `rgb(theme.fg0)`. Sizes are
//! logical px matching the CSS.

#![allow(dead_code)]

/// Dark-theme token set. One value per `--orr-*`/shadcn var the grid touches.
pub struct Theme {
    // Surfaces (deep blue-black void the system orbits in).
    pub page: u32,           // --background #0a0e16
    pub surface: u32,        // --orr-glass over page (card background)
    pub surface_hover: u32,  // --orr-glass-hover over page
    pub button_bg: u32,      // --orr-glass-2 over page
    pub border: u32,         // --orr-border (white 7.5%) over page
    pub border_strong: u32,  // --orr-border-strong (white 14%) over page
    pub border_accent: u32,  // --orr-border-accent (primary 40%) over page

    // Text ramp (--orr-fg-0..3).
    pub fg0: u32, // primary
    pub fg1: u32, // body
    pub fg2: u32, // secondary / data
    pub fg3: u32, // faint

    // Identity + semantics.
    pub primary: u32,        // --primary orbit cyan
    pub accent_bright: u32,  // --orr-accent-bright (primary + white 22%)
    pub accent_wash: u32,    // --orr-accent-wash (primary 12% over page) — active nav bg
    pub accent_badge: u32,   // accent 20% over page — nav count badge bg
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
}

/// Language → brand dot color (stand-in for `LangIcon`; the real devicon SVGs
/// land in Phase 2). Falls back to the faint text color for unknowns.
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
