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

/// gpui-component's own icons (`IconName`, e.g. the Checkbox tick or the
/// notification close button) load from `icons/<name>.svg`. Those are lucide
/// icons too, so serve them from our lucide set; `close` is lucide's `x`.
fn component_icon(path: &str) -> Option<String> {
    let name = path.strip_prefix("icons/")?;
    Some(match name {
        "close.svg" => "lucide/x.svg".to_string(),
        _ => format!("lucide/{name}"),
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let file = Icons::get(path).or_else(|| component_icon(path).and_then(|p| Icons::get(&p)));
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
        assert!(!load("icons/no-such-icon.svg"));
    }
}
