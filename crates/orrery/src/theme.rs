//! Orrery's `--orr-*` design tokens, in a dark and a light variant with the
//! same token set. The handful of surfaces that were `color-mix`/translucent
//! are pre-blended to opaque sRGB here — the flat-design contract means we
//! don't layer translucency anyway.
//!
//! Colors are `u32` 0xRRGGBB so call sites use `rgb(theme.fg0)`. Sizes are
//! logical px. See `docs/design-system/` for the full spec.

use orrery_core::model::ThemePreference;

/// Which palette a [`Theme`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    /// The palette to paint: the Settings choice, or for "System" the
    /// desktop's portal `color-scheme` ("dark" / "light" / `None`). With no
    /// desktop preference Orrery stays dark, its native look.
    pub fn resolve(pref: ThemePreference, system_scheme: Option<&str>) -> Mode {
        match pref {
            ThemePreference::Dark => Mode::Dark,
            ThemePreference::Light => Mode::Light,
            ThemePreference::System => match system_scheme {
                Some("light") => Mode::Light,
                _ => Mode::Dark,
            },
        }
    }
}

/// The token set. One value per `--orr-*`/shadcn var the UI touches; both
/// [`Theme::dark`] and [`Theme::light`] fill every field.
pub struct Theme {
    /// Which palette this is. Picks the gpui-component mode and how the
    /// system accent is adapted ([`Theme::with_system_accent`]).
    pub mode: Mode,

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
    pub danger_badge: u32,  // danger 20% over page — urgent nav badge bg
    pub star: u32,
    pub ai: u32,
    pub clean: u32,
    pub dirty: u32,
    pub behind: u32,
    // Launcher action hues (design-system §3). Documented tokens, but the
    // launch buttons are currently untinted, so nothing reads them yet.
    #[allow(dead_code)]
    pub act_ide: u32,
    #[allow(dead_code)]
    pub act_agent: u32,
    #[allow(dead_code)]
    pub act_folder: u32,
    #[allow(dead_code)]
    pub act_host: u32,

    /// Modal backdrop behind overlays (drawer / palette / dialog), as
    /// 0xRRGGBBAA for `rgba()` — the one translucent token.
    pub scrim: u32,

    /// Contribution-heatmap ramp: `[0]` is the empty/no-commit cell; `[1..=4]`
    /// are increasing accent intensities ("Less → More"). Recomputed from the
    /// system accent in [`Theme::with_system_accent`].
    pub heat: [u32; 5],

    // Radii (px): --r-xs/sm/md/lg.
    pub r_xs: f32,
    pub r_sm: f32,
    pub r_md: f32,
    pub r_lg: f32,

    // Type sizes (px): --text-h3 / --text-small / --text-data-sm.
    pub text_h3: f32,
    pub text_small: f32,
    pub text_data_sm: f32,
    /// The header wordmark ("Orrery").
    pub text_brand: f32,
    /// Large standalone titles (empty / placeholder states).
    pub text_display: f32,
}

impl Theme {
    /// The palette for `mode`, before any system accent.
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Theme::dark(),
            Mode::Light => Theme::light(),
        }
    }

    pub fn dark() -> Self {
        Theme {
            mode: Mode::Dark,
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
            // danger (--orr-behind #ff6b6b) 20% over page; static — the danger
            // hue doesn't follow the system accent.
            danger_badge: over(0x0a0e16, (0xff, 0x6b, 0x6b), 0.20),
            star: 0xffc24b,
            ai: 0xa78bfa,
            clean: 0x3dd68c,
            dirty: 0xff9e45,
            behind: 0xff6b6b,
            act_ide: 0x5b9dff,
            act_agent: 0xa78bfa,
            act_folder: 0xf5b94b,
            act_host: 0x46c8a0,
            scrim: 0x00000066,

            heat: heat_ramp(
                0x0a0e16,
                over(0x0a0e16, (0xff, 0xff, 0xff), 0.05),
                (0x38, 0xdb, 0xf0),
            ),

            ..METRICS
        }
    }

    /// The light palette. Designed rather than inverted: a near-white page
    /// with a slight cool (blue-grey) bias so the orbit-cyan identity still
    /// reads, white cards lifted off it by visible hairline borders (no
    /// shadows: flat design), a slate text ramp where even the faint tier
    /// passes WCAG AA for body text, and an accent and semantic hues darkened
    /// until they pass as text on the page. See the contrast test below.
    pub fn light() -> Self {
        const PAGE: u32 = 0xf4f7fa;
        const PRIMARY: (u8, u8, u8) = (0x08, 0x7a, 0x8c);
        const BEHIND: (u8, u8, u8) = (0xb9, 0x2f, 0x38);
        Theme {
            mode: Mode::Light,
            page: PAGE,
            surface: 0xffffff,
            // Hover and fills sink slightly below the white card, the light
            // counterpart of dark mode's lift.
            surface_hover: 0xeff3f7,
            button_bg: 0xebf0f5,
            border: 0xd8dfe7,
            border_strong: 0xbac5d1,
            border_accent: over(PAGE, PRIMARY, LIGHT_BORDER_ACCENT),

            // Slate ink, cool like the page: fg0 for titles, fg3 still
            // readable for the notes and labels that use it.
            fg0: 0x0e1726,
            fg1: 0x334155,
            fg2: 0x4b596c,
            fg3: 0x5d6a7e,

            // Orbit cyan deepened to a teal that holds 4.5:1 on the page;
            // "bright" is the emphasised accent (links, active labels), which
            // on a light page means darker still.
            primary: pack(PRIMARY),
            accent_bright: darken(PRIMARY, ACCENT_EMPHASIS),
            accent_wash: over(PAGE, PRIMARY, LIGHT_WASH),
            accent_badge: over(PAGE, PRIMARY, LIGHT_BADGE),
            danger_badge: over(PAGE, BEHIND, 0.12),
            // The semantic hues keep their meaning and hue family, deepened
            // from the dark theme's neon so they pass as text on white.
            star: 0x9a6408,
            ai: 0x7444d6,
            clean: 0x157a47,
            dirty: 0xb0520a,
            behind: pack(BEHIND),
            act_ide: 0x2560d8,
            act_agent: 0x7444d6,
            act_folder: 0x9a6408,
            act_host: 0x0f766e,
            // A cool slate scrim, lighter than dark mode's black: enough to
            // set the overlay apart without dimming the page to grey.
            scrim: 0x0e17264d,

            // The empty cell is the slate ink at a whisper, so the no-commit
            // grid still shows on the near-white page.
            heat: heat_ramp(PAGE, over(PAGE, (0x0e, 0x17, 0x26), 0.07), PRIMARY),

            ..METRICS
        }
    }

    /// Whether this is the light palette.
    pub fn is_light(&self) -> bool {
        self.mode == Mode::Light
    }

    /// Override the built-in orbit-cyan accent with the desktop's system accent
    /// when one is present, recomputing the derived accent tokens (bright/wash/
    /// badge/border/heat). Matches the design system's "primary overridden by
    /// the system accent at runtime". On the light palette the accent is first
    /// deepened until it holds 4.5:1 against the page (a desktop accent is
    /// usually tuned for dark panels: KDE's default blue is 2.6:1 on white).
    /// No-op when `accent` is `None`.
    pub fn with_system_accent(mut self, accent: Option<(u8, u8, u8)>) -> Self {
        let Some(a) = accent else {
            return self;
        };
        match self.mode {
            Mode::Dark => {
                self.primary = pack(a);
                self.accent_bright = lighten(a, ACCENT_EMPHASIS);
                self.accent_wash = over(self.page, a, 0.12);
                self.accent_badge = over(self.page, a, 0.20);
                self.border_accent = over(self.page, a, 0.40);
            }
            Mode::Light => {
                let a = with_contrast(a, unpack(self.page), 4.5);
                self.primary = pack(a);
                self.accent_bright = darken(a, ACCENT_EMPHASIS);
                self.accent_wash = over(self.page, a, LIGHT_WASH);
                self.accent_badge = over(self.page, a, LIGHT_BADGE);
                self.border_accent = over(self.page, a, LIGHT_BORDER_ACCENT);
            }
        }
        self.heat = heat_ramp(self.page, self.heat[0], unpack(self.primary));
        self
    }
}

/// The theme-independent tokens (radii and type sizes), shared by both
/// palettes through struct-update syntax. The colour fields here are
/// placeholders that every constructor overrides.
const METRICS: Theme = Theme {
    mode: Mode::Dark,
    page: 0,
    surface: 0,
    surface_hover: 0,
    button_bg: 0,
    border: 0,
    border_strong: 0,
    border_accent: 0,
    fg0: 0,
    fg1: 0,
    fg2: 0,
    fg3: 0,
    primary: 0,
    accent_bright: 0,
    accent_wash: 0,
    accent_badge: 0,
    danger_badge: 0,
    star: 0,
    ai: 0,
    clean: 0,
    dirty: 0,
    behind: 0,
    act_ide: 0,
    act_agent: 0,
    act_folder: 0,
    act_host: 0,
    scrim: 0,
    heat: [0; 5],

    r_xs: 4.,
    r_sm: 6.,
    r_md: 10.,
    r_lg: 14.,

    text_h3: 16.,
    text_small: 13.,
    text_data_sm: 12.,
    text_brand: 15.,
    text_display: 22.,
};

/// How far the emphasised accent (`accent_bright`) moves from `primary`:
/// toward white on the dark page, toward black on the light one.
const ACCENT_EMPHASIS: f32 = 0.22;
/// Light-palette accent blends over the page. A touch lighter than dark
/// mode's 12% / 20%, since a tint reads more strongly on white.
const LIGHT_WASH: f32 = 0.10;
const LIGHT_BADGE: f32 = 0.18;
const LIGHT_BORDER_ACCENT: f32 = 0.40;

/// Build the contribution-heatmap ramp from an accent: the `empty`
/// (no-commit) cell, then four increasing blends of the accent over the page
/// background, up to the full accent at the top.
fn heat_ramp(page: u32, empty: u32, accent: (u8, u8, u8)) -> [u32; 5] {
    [
        empty,
        over(page, accent, 0.22),
        over(page, accent, 0.42),
        over(page, accent, 0.68),
        pack(accent),
    ]
}

fn pack((r, g, b): (u8, u8, u8)) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn unpack(c: u32) -> (u8, u8, u8) {
    ((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Mix an sRGB colour toward white by fraction `f`.
fn lighten((r, g, b): (u8, u8, u8), f: f32) -> u32 {
    let m = |c: u8| (c as f32 + (255.0 - c as f32) * f).round() as u8;
    pack((m(r), m(g), m(b)))
}

/// Mix an sRGB colour toward black by fraction `f`.
fn darken((r, g, b): (u8, u8, u8), f: f32) -> u32 {
    let m = |c: u8| (c as f32 * (1.0 - f)).round() as u8;
    pack((m(r), m(g), m(b)))
}

/// Blend an sRGB colour over the opaque `base` (a page colour) at `alpha`.
fn over(base: u32, (r, g, b): (u8, u8, u8), alpha: f32) -> u32 {
    let (br, bg, bb) = unpack(base);
    let blend = |c: u8, base: u8| (base as f32 + (c as f32 - base as f32) * alpha).round() as u8;
    pack((blend(r, br), blend(g, bg), blend(b, bb)))
}

/// WCAG 2 relative luminance of an sRGB colour.
fn luminance((r, g, b): (u8, u8, u8)) -> f32 {
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG 2 contrast ratio between two colours (1.0 – 21.0).
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(unpack(a)), luminance(unpack(b)));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Deepen `c` (toward black, keeping its hue) until it reaches `target`
/// contrast against the light background `bg`. Already-dark colours come
/// back unchanged.
fn with_contrast(c: (u8, u8, u8), bg: (u8, u8, u8), target: f32) -> (u8, u8, u8) {
    let mut out = c;
    for step in 1..=50 {
        if contrast(pack(out), pack(bg)) >= target {
            break;
        }
        out = unpack(darken(c, step as f32 * 0.02));
    }
    out
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
/// components (buttons, tabs, tags, notifications, markdown, inputs, …) match
/// the rest of the UI without per-call-site restyling. Call after
/// `gpui_component::init`, before opening the window, and again whenever the
/// theme changes (then refresh the windows).
pub fn apply_gpui_component_theme(t: &Theme, cx: &mut gpui::App) {
    use gpui::{Anchor, px, rgb};
    use gpui_component::ThemeMode;
    use gpui_component::highlighter::HighlightTheme;

    let (mode, highlight) = match t.mode {
        Mode::Dark => (ThemeMode::Dark, HighlightTheme::default_dark()),
        Mode::Light => (ThemeMode::Light, HighlightTheme::default_light()),
    };
    // Start from gpui-component's own palette for this mode, so any colour
    // we don't map below (tables, sliders, …) matches the mode rather than
    // whichever one was loaded before.
    gpui_component::Theme::change(mode, None, cx);
    let c = gpui_component::Theme::global_mut(cx);
    c.mode = mode;
    // Code blocks in rendered markdown (the drawer's Readme).
    c.highlight_theme = highlight;
    // Flat design: no drop shadows on buttons, inputs or checkboxes.
    c.shadow = false;

    // Surfaces.
    c.background = rgb(t.page).into();
    c.popover = rgb(t.surface).into();
    c.secondary = rgb(t.surface).into();
    c.muted = rgb(t.surface).into();
    c.sidebar = rgb(t.page).into();
    c.title_bar = rgb(t.page).into();
    // The input border, which is also the default button's border.
    c.input = rgb(t.border).into();
    c.tab_active = rgb(t.surface).into();

    // Buttons (see `widgets::button`): the default variant is the plain
    // bordered button, whose hover lifts the fill and brightens the label;
    // the danger variant hovers into the danger wash.
    c.button = rgb(t.button_bg).into();
    c.button_foreground = rgb(t.fg0).into();
    // Dark mode lifts the hover fill to the strong-border grey; on the
    // light page that would be a heavy slab, so the hover sinks one step
    // to the hairline grey instead.
    c.button_hover = rgb(if t.is_light() {
        t.border
    } else {
        t.border_strong
    })
    .into();
    // Pressed sinks below hover: darker in dark mode, deeper grey in light.
    c.button_active = rgb(if t.is_light() {
        t.border_strong
    } else {
        t.surface_hover
    })
    .into();
    c.button_danger = rgb(t.behind).into();
    c.button_danger_foreground = rgb(t.behind).into();
    c.button_danger_hover = rgb(t.danger_badge).into();
    c.button_danger_active = rgb(t.danger_badge).into();

    // Tabs (the drawer's tab bar).
    c.tab_foreground = rgb(t.fg2).into();
    c.tab_active_foreground = rgb(t.fg0).into();

    // Notifications (toasts) stack bottom-right, as the hand-rolled ones did.
    c.notification.placement = Anchor::BottomRight;

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

    // Components paint from the resolved tokens, so refresh them from the
    // colours set above.
    c.tokens = gpui_component::theme::ThemeTokens::from(&c.colors);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key text/background pair of the light palette meets WCAG AA:
    /// 4.5:1 for anything used as body text (the whole fg ramp, the accent
    /// and the semantic hues, which all colour small labels), 3:1 for UI
    /// marks. Run with `--nocapture` to see the ratios.
    #[test]
    fn light_palette_meets_wcag_aa() {
        let t = Theme::light();
        let backgrounds = [
            ("page", t.page),
            ("surface", t.surface),
            ("surface_hover", t.surface_hover),
            ("button_bg", t.button_bg),
        ];
        let text = [
            ("fg0", t.fg0),
            ("fg1", t.fg1),
            ("fg2", t.fg2),
            ("fg3", t.fg3),
            ("primary", t.primary),
            ("accent_bright", t.accent_bright),
            ("clean", t.clean),
            ("dirty", t.dirty),
            ("behind", t.behind),
            ("ai", t.ai),
        ];
        for (bn, bg) in backgrounds {
            for (fname, fg) in text {
                let r = contrast(fg, bg);
                println!("{fname:>13} on {bn:<13} {r:5.2}");
                // The page and cards carry body text; the fills carry button
                // labels and hovered rows, where 4.5 still holds for the
                // text tiers but a hue may dip to the UI bar.
                let min = if bn == "page" || bn == "surface" || fname.starts_with("fg") {
                    4.5
                } else {
                    3.0
                };
                assert!(r >= min, "{fname} on {bn}: {r:.2} < {min}");
            }
            // Stars are icons (a UI mark), never running text.
            assert!(contrast(t.star, bg) >= 3.0, "star on {bn}");
        }
        // Labels on accent / danger washes (active nav, danger hover).
        assert!(contrast(t.fg0, t.accent_wash) >= 4.5);
        assert!(contrast(t.accent_bright, t.accent_wash) >= 4.5);
        assert!(contrast(t.fg0, t.accent_badge) >= 4.5);
        assert!(contrast(t.behind, t.danger_badge) >= 4.5);
        // Primary-filled buttons label in the page colour.
        assert!(contrast(t.page, t.primary) >= 4.5);
        // The text tiers step down in strength.
        let ramp = [t.fg0, t.fg1, t.fg2, t.fg3].map(|c| contrast(c, t.page));
        assert!(ramp.windows(2).all(|w| w[0] > w[1]), "{ramp:?}");
    }

    /// A pale desktop accent is deepened on the light palette until it
    /// passes on the page; dark mode takes it as is.
    #[test]
    fn system_accent_is_adapted_to_the_light_page() {
        let kde_blue = (0x3d, 0xae, 0xe9);
        let dark = Theme::dark().with_system_accent(Some(kde_blue));
        assert_eq!(dark.primary, 0x3daee9);
        let light = Theme::light().with_system_accent(Some(kde_blue));
        assert!(contrast(light.primary, light.page) >= 4.5);
        assert!(contrast(light.accent_bright, light.page) >= 4.5);
        assert!(contrast(light.page, light.primary) >= 4.5);
        // Still recognisably the same blue: the hue's dominant channel leads.
        let (r, g, b) = unpack(light.primary);
        assert!(b > g && g > r, "{:06x}", light.primary);
        // The heat ramp follows the adapted accent and keeps its empty cell.
        assert_eq!(light.heat[4], light.primary);
        assert_eq!(light.heat[0], Theme::light().heat[0]);
        // An accent that already passes is left alone.
        let deep = (0x1d, 0x4e, 0xd8);
        assert_eq!(
            Theme::light().with_system_accent(Some(deep)).primary,
            0x1d4ed8
        );
    }

    #[test]
    fn dark_accent_tokens_unchanged() {
        // The refactor onto shared helpers must not move the dark palette.
        let t = Theme::dark();
        assert_eq!(t.accent_wash, 0x102730);
        assert_eq!(t.heat[0], over(0x0a0e16, (0xff, 0xff, 0xff), 0.05));
        let cyan = Theme::dark().with_system_accent(Some((0x38, 0xdb, 0xf0)));
        assert_eq!(cyan.accent_bright, t.accent_bright);
        assert_eq!(cyan.accent_badge, t.accent_badge);
    }

    #[test]
    fn system_preference_follows_the_desktop_scheme() {
        use ThemePreference::*;
        assert_eq!(Mode::resolve(System, Some("light")), Mode::Light);
        assert_eq!(Mode::resolve(System, Some("dark")), Mode::Dark);
        // No desktop preference: Orrery's native dark.
        assert_eq!(Mode::resolve(System, None), Mode::Dark);
        assert_eq!(Mode::resolve(Light, Some("dark")), Mode::Light);
        assert_eq!(Mode::resolve(Dark, Some("light")), Mode::Dark);
    }
}
