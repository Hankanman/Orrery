//! Acting on an [`AppRequest`]: raise the window, then focus a repo (its
//! drawer on Mission Control) or open the palette on a search. Requests come
//! from `orrery open` / `orrery://` (handed over by a second process, or this
//! one's own launch arguments) and the other desktop integrations.

use gpui::{Context, Window};
use orrery_platform::request::AppRequest;

use crate::shell::{OrreryApp, Overlay, View};

impl OrreryApp {
    /// Carry out `req`. A repo id that isn't in the grid (the cache changed
    /// under us) just raises the window.
    pub(crate) fn apply_request(
        &mut self,
        req: AppRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.activate_window();
        cx.activate(true);
        match req {
            AppRequest::Show { .. } => {}
            AppRequest::ShowRepo { id, .. } => {
                let Some(row) = self.rows.iter().find(|r| r.id.as_ref() == id) else {
                    return;
                };
                let id = row.id.clone();
                self.view = View::Grid;
                self.view_filter = None;
                self.open_drawer(id, window, cx);
            }
            AppRequest::Search { query, .. } => {
                self.view = View::Grid;
                self.open_palette(window, cx);
                if let Some(Overlay::Palette(d)) = &mut self.overlay {
                    // `set_value` doesn't emit a change event, so bump the
                    // palette's revision for its repo matches to pick the
                    // query up.
                    d.query
                        .update(cx, |s, cx| s.set_value(query.clone(), window, cx));
                    d.rev += 1;
                }
            }
        }
        cx.notify();
    }
}
