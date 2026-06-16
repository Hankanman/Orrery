//! Icon helpers — sized, tinted GPUI `svg()` elements backed by the embedded
//! asset source. lucide for chrome/status, simple-icons for host brand marks.
//! SVGs are monochrome (alpha mask), tinted by `text_color`.

use gpui::{img, px, rgb, svg, Img, SharedString, Styled, Svg};

/// A lucide icon (e.g. `"git-branch"`), `size` px square, tinted with `color`.
pub fn lucide(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("lucide/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// A host brand mark (`"github"` / `"gitlab"`), `size` px square, tinted.
pub fn brand(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("brand/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// A language mark — the multicolor devicon for `stem`, rendered in full color
/// via `img()` (not tinted). `size` px square.
pub fn langicon(stem: &str, size: f32) -> Img {
    img(SharedString::from(format!("devicon/{stem}.svg")))
        .size(px(size))
        .flex_none()
}
