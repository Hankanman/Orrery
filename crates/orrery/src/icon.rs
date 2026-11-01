//! Icon helpers — sized, tinted GPUI `svg()` elements backed by the embedded
//! asset source. lucide for chrome/status, simple-icons for host brand marks.
//! SVGs are monochrome (alpha mask), tinted by `text_color`.

use gpui::{Img, SharedString, Styled, Svg, img, px, rgb, svg};

/// A lucide icon (e.g. `"git-branch"`), `size` px square, tinted with `color`.
pub fn lucide(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("lucide/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// A host brand mark (`"github"` / `"gitlab"` / `"gitea"` / `"codeberg"`),
/// `size` px square, tinted. See [`host_brand`].
pub fn brand(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("brand/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// The brand-mark name for a host instance: GitHub, GitLab, or Gitea, with
/// Codeberg (a Forgejo instance) getting its own mark. `domain` is the
/// instance's hostname, when known.
pub fn host_brand(host: orrery_core::model::Host, domain: Option<&str>) -> &'static str {
    use orrery_core::model::Host;
    match host {
        Host::Github => "github",
        Host::Gitlab => "gitlab",
        Host::Gitea if domain.is_some_and(|d| d.eq_ignore_ascii_case("codeberg.org")) => "codeberg",
        Host::Gitea => "gitea",
    }
}

/// A language mark — the multicolor devicon for `stem`, rendered in full color
/// via `img()` (not tinted). `size` px square.
pub fn langicon(stem: &str, size: f32) -> Img {
    img(SharedString::from(format!("devicon/{stem}.svg")))
        .size(px(size))
        .flex_none()
}
