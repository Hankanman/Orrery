//! System tray, native (no Tauri, no GTK). A `StatusNotifierItem` over D-Bus via
//! `ksni` — the protocol KDE/Plasma and most freedesktop panels speak directly,
//! so we avoid pulling in a GTK event loop (which would reintroduce the very CPU
//! cost the native rewrite left WebKitGTK to escape).
//!
//! The tray owns its own thread + async runtime, like the other platform
//! integrations. It's UI-agnostic: menu activations are reported through an
//! `on_action` callback, and the app pushes glance/appearance updates back via
//! [`TrayHandle`]. Mirrors the old Tauri tray: an attention header, the
//! recent-repos quick-open list, and show/rescan/quit actions.

use std::sync::Arc;
use std::sync::OnceLock;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip};

use orrery_core::{cache, config, launch};

/// Cap on attention lines / recent repos shown, to keep the menu compact.
const MAX_ATTENTION: usize = 8;
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

/// Lets the app update the live tray. `ksni` runs the tray on its own thread; an
/// update is a short, synchronous round-trip to it.
pub struct TrayHandle {
    handle: Handle<Model>,
}

impl TrayHandle {
    /// Replace the attention glance lines shown in the menu + tooltip.
    pub fn set_glance(&self, lines: Vec<String>) {
        self.handle.update(|m| m.attention = lines);
    }

    /// Tell the tray whether the panel is dark, so it picks the right glyph tint.
    pub fn set_panel_dark(&self, dark: bool) {
        self.handle.update(|m| m.panel_dark = dark);
    }
}

/// The SNI item model. `ksni` calls these methods to paint the icon + menu and
/// to dispatch clicks (which we forward through `on_action`).
struct Model {
    attention: Vec<String>,
    panel_dark: bool,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
}

impl Model {
    fn fire(&self, action: TrayAction) {
        (self.on_action)(action);
    }
}

/// Decode a PNG glyph to an SNI pixmap (ARGB32, network byte order), cached on
/// first use. Returns empty on any decode failure (the host then shows nothing
/// rather than crashing).
fn glyph(panel_dark: bool) -> Vec<Icon> {
    static LIGHT: OnceLock<Option<Icon>> = OnceLock::new();
    static DARK: OnceLock<Option<Icon>> = OnceLock::new();
    let cell = if panel_dark { &LIGHT } else { &DARK };
    cell.get_or_init(|| decode_argb(if panel_dark { TRAY_LIGHT } else { TRAY_DARK }))
        .clone()
        .into_iter()
        .collect()
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

    fn icon_pixmap(&self) -> Vec<Icon> {
        glyph(self.panel_dark)
    }

    fn tool_tip(&self) -> ToolTip {
        let description = match self.attention.len() {
            0 => String::new(),
            n => format!("{n} need attention"),
        };
        ToolTip {
            title: "Orrery".into(),
            description,
            ..Default::default()
        }
    }

    /// Left-click raises the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.fire(TrayAction::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Model>> {
        let mut items = Vec::new();

        // Attention header + per-item lines (informational).
        items.push(label(match self.attention.len() {
            0 => "All clear".to_string(),
            n => format!("{n} need attention"),
        }));
        items.push(MenuItem::Separator);
        for line in self.attention.iter().take(MAX_ATTENTION) {
            items.push(label(format!("● {line}")));
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
        attention: Vec::new(),
        panel_dark: true,
        on_action,
    };
    // Degrade silently (like the rest of platform) if there's no SNI host.
    let handle = model.spawn().ok()?;
    Some(TrayHandle { handle })
}
