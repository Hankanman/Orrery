//! System tray, native (no Tauri, no GTK). A `StatusNotifierItem` over D-Bus via
//! `ksni` — the protocol KDE/Plasma and most freedesktop panels speak directly,
//! so we avoid pulling in a GTK event loop (which would reintroduce the very CPU
//! cost the native rewrite left WebKitGTK to escape).
//!
//! The tray owns its own thread + async runtime, like the other platform
//! integrations. It's UI-agnostic: menu activations are reported through an
//! `on_action` callback, and the app pushes attention/appearance updates back
//! via [`TrayHandle`]. The menu shows the attention-model summary (#183) — a
//! count header plus the top items, each raising the window — then the
//! recent-repos quick-open list and show/rescan/quit actions. While anything
//! urgent is pending the item reports `NeedsAttention` and badges its glyph.

use std::sync::Arc;
use std::sync::OnceLock;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip};

use orrery_core::{cache, config, launch};

/// Cap on recent repos shown, to keep the menu compact.
const MAX_RECENT: usize = 5;

// Monochrome symbolic glyphs (transparent background) so the icon reads like the
// other symbolic icons in the panel. We can't hand the SNI host a themed name,
// so we ship both tints and pick by the panel colour-scheme: a light glyph for
// dark panels, a dark glyph for light ones.
const TRAY_LIGHT: &[u8] = include_bytes!("../assets/tray-light.png");
const TRAY_DARK: &[u8] = include_bytes!("../assets/tray-dark.png");

/// A tray menu activation, handed to the app to act on.
pub enum TrayAction {
    /// Show / raise the main window (left-click or "Show Orrery").
    Show,
    /// Re-scan the repos.
    Rescan,
    /// Quit the application.
    Quit,
    /// Open a repo (by id / path) in the configured IDE.
    Open(String),
}

/// Compact attention-model summary for the tray, pushed by the app after each
/// `recompute_attention`. Counts cover the *actionable* severities (Urgent +
/// Attention — Info is ambient state and stays off the tray); `top` carries the
/// highest-ranked item lines for the menu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayAttention {
    /// Actionable items (Urgent + Attention severities).
    pub total: usize,
    /// Urgent subset — drives the `NeedsAttention` icon state.
    pub urgent: usize,
    /// Top item lines, already ranked (the app caps these at a handful).
    pub top: Vec<String>,
}

/// Lets the app update the live tray. `ksni` runs the tray on its own thread; an
/// update is a short, synchronous round-trip to it. Clonable so the app entity
/// and the live-signal task can each hold one.
#[derive(Clone)]
pub struct TrayHandle {
    handle: Handle<Model>,
}

impl TrayHandle {
    /// Replace the attention summary (menu top items, tooltip count, urgent
    /// icon state).
    pub fn set_attention(&self, attention: TrayAttention) {
        self.handle.update(|m| m.attention = attention);
    }

    /// Tell the tray whether the panel is dark, so it picks the right glyph tint.
    pub fn set_panel_dark(&self, dark: bool) {
        self.handle.update(|m| m.panel_dark = dark);
    }
}

/// The SNI item model. `ksni` calls these methods to paint the icon + menu and
/// to dispatch clicks (which we forward through `on_action`).
struct Model {
    attention: TrayAttention,
    panel_dark: bool,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
}

impl Model {
    fn fire(&self, action: TrayAction) {
        (self.on_action)(action);
    }
}

/// Decode a PNG glyph to an SNI pixmap (ARGB32, network byte order), cached on
/// first use; the `urgent` variant gets a corner dot overlaid. Returns empty on
/// any decode failure (the host then shows nothing rather than crashing).
fn glyph(panel_dark: bool, urgent: bool) -> Vec<Icon> {
    static LIGHT: OnceLock<Option<Icon>> = OnceLock::new();
    static DARK: OnceLock<Option<Icon>> = OnceLock::new();
    static LIGHT_URGENT: OnceLock<Option<Icon>> = OnceLock::new();
    static DARK_URGENT: OnceLock<Option<Icon>> = OnceLock::new();
    let cell = match (panel_dark, urgent) {
        (true, false) => &LIGHT,
        (false, false) => &DARK,
        (true, true) => &LIGHT_URGENT,
        (false, true) => &DARK_URGENT,
    };
    cell.get_or_init(|| {
        let base = decode_argb(if panel_dark { TRAY_LIGHT } else { TRAY_DARK })?;
        Some(if urgent { with_urgent_dot(base) } else { base })
    })
    .clone()
    .into_iter()
    .collect()
}

/// Overlay a filled dot in the glyph's bottom-right corner — the urgent badge.
/// The tint is `--orr-behind` (#ff6b6b), the design system's danger role, so the
/// tray matches the in-app urgent badges.
fn with_urgent_dot(mut icon: Icon) -> Icon {
    const DOT: (f32, f32, f32) = (255., 107., 107.);
    let (w, h) = (icon.width as f32, icon.height as f32);
    let r = w.min(h) * 0.28;
    let (cx, cy) = (w - r, h - r);
    for y in 0..icon.height {
        for x in 0..icon.width {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            // Coverage of this pixel by the dot, with a ~1px anti-aliased edge.
            let sa = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0., 1.);
            if sa == 0. {
                continue;
            }
            let i = ((y * icon.width + x) * 4) as usize;
            let px = &mut icon.data[i..i + 4];
            // src-over composite of the dot onto the (non-premultiplied) glyph.
            let da = px[0] as f32 / 255.;
            let oa = sa + da * (1. - sa);
            for (c, s) in [(1, DOT.0), (2, DOT.1), (3, DOT.2)] {
                let d = px[c] as f32;
                px[c] = ((s * sa + d * da * (1. - sa)) / oa).round() as u8;
            }
            px[0] = (oa * 255.).round() as u8;
        }
    }
    icon
}

fn decode_argb(png_bytes: &[u8]) -> Option<Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    // SNI wants ARGB32 in network (big-endian) byte order, i.e. bytes A,R,G,B.
    let data = match info.color_type {
        png::ColorType::Rgba => px
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect(),
        png::ColorType::Rgb => px
            .chunks_exact(3)
            .flat_map(|p| [255, p[0], p[1], p[2]])
            .collect(),
        _ => return None,
    };
    Some(Icon {
        width: info.width as i32,
        height: info.height as i32,
        data,
    })
}

/// The most-recently-active repos (by last commit) as `(id, display_name)`, for
/// the quick-open list. The id is the repo path — what the IDE launcher expects.
fn recent_repos() -> Vec<(String, String)> {
    let mut repos = cache::load_repos();
    repos.sort_by_key(|r| std::cmp::Reverse(r.last_commit_unix));
    repos
        .into_iter()
        .take(MAX_RECENT)
        .map(|r| (r.id, r.display_name))
        .collect()
}

/// "needs" / "need" — the attention header + tooltip verb.
fn need(n: usize) -> &'static str {
    if n == 1 {
        "needs"
    } else {
        "need"
    }
}

/// One-line readout of the attention counts, or `None` when all clear.
fn attention_line(a: &TrayAttention) -> Option<String> {
    match (a.total, a.urgent) {
        (0, _) => None,
        (n, 0) => Some(format!("{n} {} attention", need(n))),
        (n, u) => Some(format!("{n} {} attention · {u} urgent", need(n))),
    }
}

/// A non-interactive label row (header / attention line).
fn label(text: impl Into<String>) -> MenuItem<Model> {
    StandardItem {
        label: text.into(),
        enabled: false,
        ..Default::default()
    }
    .into()
}

/// An actionable row that fires `action` when clicked.
fn action_item(text: impl Into<String>, action: fn(&mut Model)) -> MenuItem<Model> {
    StandardItem {
        label: text.into(),
        activate: Box::new(action),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for Model {
    fn id(&self) -> String {
        "orrery".into()
    }

    fn title(&self) -> String {
        "Orrery".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    /// `NeedsAttention` tells the host to emphasize the item (and to show the
    /// attention icon) while anything urgent is pending.
    fn status(&self) -> ksni::Status {
        if self.attention.urgent > 0 {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    /// The badged glyph doubles as the base icon while urgent, so hosts that
    /// ignore `status`/attention icons still show the dot.
    fn icon_pixmap(&self) -> Vec<Icon> {
        glyph(self.panel_dark, self.attention.urgent > 0)
    }

    fn attention_icon_pixmap(&self) -> Vec<Icon> {
        glyph(self.panel_dark, true)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Orrery".into(),
            description: attention_line(&self.attention).unwrap_or_default(),
            ..Default::default()
        }
    }

    /// Left-click raises the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.fire(TrayAction::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Model>> {
        let mut items = Vec::new();

        // Attention header + the top items. Activating an item raises the
        // window — the app is where it gets dealt with.
        items.push(label(
            attention_line(&self.attention).unwrap_or_else(|| "All clear".to_string()),
        ));
        items.push(MenuItem::Separator);
        for line in &self.attention.top {
            items.push(
                StandardItem {
                    label: format!("● {line}"),
                    activate: Box::new(|m: &mut Model| m.fire(TrayAction::Show)),
                    ..Default::default()
                }
                .into(),
            );
        }

        // Recent repos quick-open.
        let recent = recent_repos();
        if !recent.is_empty() {
            items.push(MenuItem::Separator);
            items.push(label("Recent"));
            for (id, name) in recent {
                items.push(
                    StandardItem {
                        label: format!("  {name}"),
                        activate: Box::new(move |m: &mut Model| {
                            m.fire(TrayAction::Open(id.clone()))
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        // Standing actions.
        items.push(MenuItem::Separator);
        items.push(action_item("Show Orrery", |m| m.fire(TrayAction::Show)));
        items.push(action_item("Rescan repos", |m| m.fire(TrayAction::Rescan)));
        items.push(action_item("Quit", |m| m.fire(TrayAction::Quit)));
        items
    }
}

/// Start the tray (ksni spawns and drives it on its own thread). `on_action` is
/// invoked for every menu activation; `Open` is handled here so the caller only
/// deals with app-level actions. Returns a [`TrayHandle`] for live updates, or
/// `None` if the tray couldn't start (no SNI host, etc.).
pub fn spawn(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Option<TrayHandle> {
    // Opening a repo needs no foreground hop — it just spawns a process — so
    // handle it here and forward the rest to the app.
    let on_action: Arc<dyn Fn(TrayAction) + Send + Sync> = Arc::new(move |action| {
        if let TrayAction::Open(id) = &action {
            let _ = launch::launch(&config::load().ide_command, id);
        } else {
            on_action(action);
        }
    });

    let model = Model {
        attention: TrayAttention::default(),
        panel_dark: true,
        on_action,
    };
    // Degrade silently (like the rest of platform) if there's no SNI host.
    let handle = model.spawn().ok()?;
    Some(TrayHandle { handle })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_line_reads_counts() {
        let a = |total, urgent| TrayAttention {
            total,
            urgent,
            top: Vec::new(),
        };
        assert_eq!(attention_line(&a(0, 0)), None);
        assert_eq!(
            attention_line(&a(1, 0)).as_deref(),
            Some("1 needs attention")
        );
        assert_eq!(
            attention_line(&a(3, 2)).as_deref(),
            Some("3 need attention · 2 urgent")
        );
    }

    #[test]
    fn urgent_dot_badges_the_corner_only() {
        // A fully transparent 16×16 glyph: after badging, the bottom-right
        // corner is opaque-ish red and the top-left stays untouched.
        let icon = with_urgent_dot(Icon {
            width: 16,
            height: 16,
            data: vec![0; 16 * 16 * 4],
        });
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(icon.data.len(), 16 * 16 * 4);
        assert_eq!(&icon.data[..4], [0, 0, 0, 0], "top-left untouched");
        // Dot centre = (w - r, h - r) with r = 16 × 0.28 ≈ 4.5 → pixel (11, 11).
        let i = (11 * 16 + 11) * 4;
        assert_eq!(
            &icon.data[i..i + 4],
            [255, 255, 107, 107],
            "dot centre is opaque --orr-behind red"
        );
    }
}
