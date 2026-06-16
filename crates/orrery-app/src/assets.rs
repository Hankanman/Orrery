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

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icons::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icons::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}
