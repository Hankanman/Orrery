//! Focusing a repo for an `AppRequest::ShowRepo` (a notification's "Show in
//! Orrery", `orrery open`, an `orrery://` link). The request handler in
//! `live.rs` raises the window, then calls [`OrreryApp::show_repo`].

use gpui::{Context, Window};

use crate::shell::{OrreryApp, View};

impl OrreryApp {
    /// Switch to Mission Control and open the drawer of the repo with this
    /// id. Only a repo already in the grid is opened; any other id (the
    /// cache changed under us, or a caller made it up) does nothing.
    pub(crate) fn show_repo(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|r| r.id.as_ref() == id) else {
            return;
        };
        let id = row.id.clone();
        self.view = View::Grid;
        self.view_filter = None;
        self.open_drawer(id, window, cx);
        cx.notify();
    }
}
