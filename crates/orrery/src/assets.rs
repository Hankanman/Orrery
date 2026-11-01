//! Embedded SVG assets served to GPUI. The icon files under `assets/icons/`
//! (generated from lucide-react + simple-icons by `assets/generate-icons.mjs`)
//! are baked into the binary by `rust-embed`, so there are no runtime file
//! dependencies. GPUI's `svg()` element loads them by path through this source.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/icons"]
struct Icons;

/// True if an asset is embedded at `path` (e.g. `"devicon/rust.svg"`). Used to
/// fall back gracefully when a language has no bundled devicon.
pub fn has_icon(path: &str) -> bool {
    Icons::get(path).is_some()
}

/// AssetSource registered on the `Application`. Asset paths are relative to
/// `assets/icons/`, e.g. `"lucide/git-branch.svg"`, `"brand/github.svg"`.
pub struct Assets;

/// gpui-component's own icons (`IconName`) load from `icons/<name>.svg`.
/// Serve the ones the components we use need from our lucide set: the
/// Checkbox tick and the notification close button (lucide's `x`). Others
/// stay unresolved, as before — e.g. NumberInput's +/- would otherwise show
/// a `+` with no matching `-`, which our icon set lacks.
fn component_icon(path: &str) -> Option<&'static str> {
    match path {
        "icons/check.svg" => Some("lucide/check.svg"),
        "icons/close.svg" => Some("lucide/x.svg"),
        _ => None,
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let file = Icons::get(path).or_else(|| component_icon(path).and_then(Icons::get));
        Ok(file.map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icons::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_icons_resolve_to_lucide() {
        let load = |p: &str| Assets.load(p).unwrap().is_some();
        assert!(load("icons/check.svg"), "the Checkbox tick");
        assert!(load("icons/close.svg"), "close maps to lucide's x");
        assert!(load("lucide/check.svg"), "our own paths are unchanged");
        assert!(!load("icons/plus.svg"), "only the icons we use are mapped");
    }
}
