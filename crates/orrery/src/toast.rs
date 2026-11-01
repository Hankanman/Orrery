//! Toast surface — the shared "something succeeded / failed / is running"
//! feedback channel. Bulk ops, clone, the commit flow, and the attention
//! engine all report through here so long-running operations never fail
//! silently.
//!
//! A thin wrapper over gpui-component's notifications (the `Root`'s
//! notification layer, stacked bottom-right; see `theme.rs`). The wrapper
//! keeps the app's toast API, which runs from plain `Context`s, often in
//! async callbacks without a `Window`: a toast is queued here and handed to
//! the notification layer on the next render ([`OrreryApp::flush_toasts`]).
//!
//! Lifecycle: Success/Info toasts auto-hide after a few seconds. Error toasts
//! persist until clicked. Progress toasts persist until the owning operation
//! replaces them through its stable caller-supplied key
//! ([`OrreryApp::upsert_toast`]), flipping "Cloning…" to done/failed.
//!
//! With reduce motion on, the same toasts go to a still stack the app draws
//! itself ([`OrreryApp::still_toast_layer`]), since gpui-component's
//! notifications always animate in and out.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use gpui_component::WindowExt as _;
use gpui_component::notification::Notification;

use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Toast width (px) — comfortably narrower than the drawer so it never
/// competes with an open overlay.
const TOAST_W: f32 = 340.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
    Progress,
}

impl ToastKind {
    /// Success/Info are transient; Error waits for a click, Progress for its
    /// owning operation to resolve it.
    fn auto_dismisses(self) -> bool {
        matches!(self, ToastKind::Success | ToastKind::Info)
    }

    /// (lucide icon, semantic color) for the leading mark: ok/danger tokens
    /// for Success/Error, the accent for Info/Progress.
    fn style(self, t: &Theme) -> (&'static str, u32) {
        match self {
            ToastKind::Success => ("circle-check", t.clean),
            ToastKind::Error => ("circle-alert", t.behind),
            ToastKind::Info => ("bell", t.primary),
            ToastKind::Progress => ("refresh-cw", t.primary),
        }
    }
}

/// One toast, queued until the next render hands it to the notification
/// layer. `key` is the caller's stable handle for updating an in-flight
/// operation's toast (e.g. `"clone:owner/repo"`): a toast with the same key
/// replaces the one before it.
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub title: SharedString,
    pub detail: Option<SharedString>,
    pub key: Option<SharedString>,
    /// A link the toast opens when clicked (e.g. a freshly created PR's URL);
    /// clicking still dismisses it afterwards.
    pub url: Option<SharedString>,
}

/// Notification id type: keyed toasts replace each other by (this, key).
struct ToastKey;

impl OrreryApp {
    /// Show a toast. Success/Info auto-hide; Error stays until clicked;
    /// Progress stays until the caller resolves it (use
    /// [`Self::upsert_toast`] for that). Returns the toast's id.
    pub fn push_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<SharedString>,
        detail: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.insert_toast(None, kind, title.into(), detail, None, cx)
    }

    /// Show or update the toast owned by `key` — the in-flight-operation
    /// channel: push a Progress toast when the op starts, then upsert the same
    /// key to Success/Error when it resolves. An update replaces the keyed
    /// toast; if the toast was dismissed meanwhile, the resolution appears as
    /// a fresh toast.
    pub fn upsert_toast(
        &mut self,
        key: impl Into<SharedString>,
        kind: ToastKind,
        title: impl Into<SharedString>,
        detail: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.insert_toast(Some(key.into()), kind, title.into(), detail, None, cx)
    }

    /// Like [`Self::upsert_toast`] but the resolved toast is a link: clicking
    /// it opens `url` (then dismisses). Used by "PR opened" to jump to the PR.
    pub fn upsert_toast_link(
        &mut self,
        key: impl Into<SharedString>,
        kind: ToastKind,
        title: impl Into<SharedString>,
        detail: Option<SharedString>,
        url: SharedString,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.insert_toast(Some(key.into()), kind, title.into(), detail, Some(url), cx)
    }

    fn insert_toast(
        &mut self,
        key: Option<SharedString>,
        kind: ToastKind,
        title: SharedString,
        detail: Option<SharedString>,
        url: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.toast_seq += 1;
        let id = self.toast_seq;
        // A queued update for the same key supersedes the queued original.
        if let Some(k) = &key {
            self.toasts.retain(|x| x.key.as_ref() != Some(k));
        }
        self.toasts.push(Toast {
            id,
            kind,
            title,
            detail,
            key,
            url,
        });
        cx.notify();
        id
    }

    /// Hand the queued toasts to gpui-component's notification layer. Called
    /// from render, which has the `Window` the layer needs; the push itself is
    /// deferred to after the frame, since it updates the window's `Root`.
    pub(crate) fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.toasts.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.toasts);
        // gpui-component's notifications always slide in and fade out, so
        // with reduce motion on the app draws its own still toasts instead.
        if self.reduce_motion() {
            for toast in queued {
                self.show_still_toast(toast, cx);
            }
            return;
        }
        let theme = self.theme.clone();
        window.defer(cx, move |window, cx| {
            for toast in queued {
                window.push_notification(notification(toast, &theme), cx);
            }
        });
    }

    /// Put `toast` on the still (unanimated) stack, replacing the toast with
    /// the same key in place, and schedule its auto-hide like the animated
    /// path's.
    fn show_still_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let id = toast.id;
        let autohide = toast.kind.auto_dismisses();
        let same_key = toast.key.as_ref().and_then(|k| {
            self.still_toasts
                .iter()
                .position(|x| x.key.as_ref() == Some(k))
        });
        match same_key {
            Some(i) => self.still_toasts[i] = toast,
            None => {
                self.still_toasts.push(toast);
                // Bounded like the notification layer: oldest drop first.
                if self.still_toasts.len() > MAX_STILL_TOASTS {
                    self.still_toasts.remove(0);
                }
            }
        }
        if autohide {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AUTOHIDE).await;
                let _ = this.update(cx, |this, cx| this.dismiss_still_toast(id, cx));
            })
            .detach();
        }
        cx.notify();
    }

    fn dismiss_still_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.still_toasts.len();
        self.still_toasts.retain(|t| t.id != id);
        if self.still_toasts.len() != before {
            cx.notify();
        }
    }

    /// The still toasts, stacked bottom-right; `None` when there are none.
    /// Clicking one dismisses it, after opening its link if it has one.
    pub(crate) fn still_toast_layer(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.still_toasts.is_empty() {
            return None;
        }
        let mut col = div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.));
        for toast in &self.still_toasts {
            let (id, url) = (toast.id, toast.url.clone());
            col = col.child(
                div()
                    .id(SharedString::from(format!("still-toast-{id}")))
                    .w(px(TOAST_W))
                    .p(px(12.))
                    .rounded(px(t.r_md))
                    .bg(rgb(t.surface))
                    .border_1()
                    .border_color(rgb(t.border_strong))
                    .cursor_pointer()
                    .child(toast_body(toast, t))
                    .on_click(cx.listener(move |this, _ev, _win, cx| {
                        if let Some(url) = &url {
                            let _ = orrery_core::launch::open(url);
                        }
                        this.dismiss_still_toast(id, cx);
                    })),
            );
        }
        Some(col.into_any_element())
    }
}

/// How long a Success/Info toast stays up (gpui-component's autohide delay).
const AUTOHIDE: std::time::Duration = std::time::Duration::from_secs(5);

/// Most still toasts on screen at once.
const MAX_STILL_TOASTS: usize = 6;

/// One flat toast card: leading semantic icon, title + optional detail, on the
/// surface token with an elevation border (gpui-component's drop shadow off).
/// Clicking anywhere dismisses it — and first opens the toast's link when it
/// carries one.
fn notification(toast: Toast, t: &Rc<Theme>) -> Notification {
    let key: SharedString = match &toast.key {
        Some(k) => k.clone(),
        None => format!("toast-{}", toast.id).into(),
    };
    let url = toast.url.clone();
    let autohide = toast.kind.auto_dismisses();
    let body = {
        let t = t.clone();
        let toast = Rc::new(toast);
        move |_: &mut Notification, _: &mut Window, _: &mut Context<Notification>| {
            toast_body(&toast, &t)
        }
    };
    Notification::new()
        .id1::<ToastKey>(key)
        .autohide(autohide)
        .content(body)
        .on_click(move |_ev, _win, _cx| {
            if let Some(url) = &url {
                let _ = orrery_core::launch::open(url);
            }
        })
        .w(px(TOAST_W))
        .p(px(12.))
        .gap(px(10.))
        .rounded(px(t.r_md))
        .bg(rgb(t.surface))
        .border_color(rgb(t.border_strong))
        .shadow_none()
}

fn toast_body(toast: &Toast, t: &Theme) -> AnyElement {
    let (icon, color) = toast.kind.style(t);
    let mut text = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .gap(px(2.))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(toast.title.clone()),
        );
    if let Some(detail) = &toast.detail {
        text = text.child(
            div()
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg2))
                .child(detail.clone()),
        );
    }
    let mut row = div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .child(lucide(icon, 16., color))
        .child(text);
    if toast.url.is_some() {
        row = row.child(lucide("external-link", 13., t.fg3));
    }
    row.into_any_element()
}
