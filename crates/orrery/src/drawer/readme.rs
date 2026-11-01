//! Drawer Readme tab: the repo's README rendered as markdown.

use super::*;

/// Readme tab: the rendered README, or a placeholder while loading / when absent.
pub(super) fn readme_view(data: &DrawerData, t: &Theme) -> impl IntoElement {
    match &data.readme {
        ReadmeState::Ready(Some(src)) => {
            // Already reflowed at load — gpui-component's renderer panics on a
            // paragraph's embedded newlines (see data::unwrap_soft_breaks).
            gpui_component::text::markdown(src.clone()).into_any_element()
        }
        ReadmeState::Ready(None) => placeholder("No README in this repo.", t).into_any_element(),
        _ => placeholder("Loading…", t).into_any_element(),
    }
}
