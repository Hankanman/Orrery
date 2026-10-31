//! Native desktop notifications over D-Bus (`org.freedesktop.Notifications`).
//! Works on any freedesktop desktop.
//!
//! Notifications can carry buttons ([`NotifyAction`]): "Open" opens a PR or
//! issue page, "Show in Orrery" raises the app on a repo. The server reports
//! clicks with the `ActionInvoked` signal, which a listener on the shared
//! session connection maps back to the action. Servers without the `actions`
//! capability just get a plain notification, and "Show in Orrery" is only
//! offered once the app has registered a [`crate::request`] handler to serve it.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use futures_util::StreamExt;
use orrery_core::launch;
use zbus::zvariant::Value;
use zbus::Connection;

use crate::request::{self, AppRequest};

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, &Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;

    /// Spec 1.2: sent just before `ActionInvoked`, with a token the invoked
    /// app can use to take focus on Wayland.
    #[zbus(signal)]
    fn activation_token(&self, id: u32, activation_token: String) -> zbus::Result<()>;
}

/// A notification button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotifyAction {
    /// "Open": open this URL (a PR or issue page) in the default browser.
    OpenUrl(String),
    /// "Show in Orrery": raise the app on the repo with this id (its path).
    /// Dropped from the notification unless a [`crate::request`] handler is
    /// registered.
    ShowRepo(String),
}

impl NotifyAction {
    /// The action key sent to the server and echoed back in `ActionInvoked`.
    fn key(&self) -> &'static str {
        match self {
            NotifyAction::OpenUrl(_) => "open",
            NotifyAction::ShowRepo(_) => "show",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            NotifyAction::OpenUrl(_) => "Open",
            NotifyAction::ShowRepo(_) => "Show in Orrery",
        }
    }
}

/// The spec's key for clicking the notification itself rather than a button.
const DEFAULT_KEY: &str = "default";

/// How many notifications' actions are remembered. Servers emit
/// `NotificationClosed` when one goes away, which forgets it; this cap only
/// bounds the map if a server never does.
const PENDING_CAP: usize = 64;

/// A notification we sent with actions, awaiting a click.
struct Pending {
    id: u32,
    actions: Vec<NotifyAction>,
    /// The activation token from the server's `ActivationToken` signal.
    token: Option<String>,
}

static PENDING: Mutex<VecDeque<Pending>> = Mutex::new(VecDeque::new());

/// The session-bus connection shared by every notification, opened lazily on
/// the first send, plus whether the server supports actions. zbus connections
/// are cheap to clone (an `Arc`) and run on their own executor, so one serves
/// callers on any thread/runtime.
#[derive(Clone)]
struct Session {
    conn: Connection,
    actions: bool,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

async fn session() -> zbus::Result<Session> {
    if let Some(session) = SESSION.lock().ok().and_then(|c| c.clone()) {
        return Ok(session);
    }
    // Connect without holding the lock across the await. Two first sends
    // racing just open two connections; the later one wins the slot (and
    // the loser's listener ends when its connection drops).
    let conn = Connection::session().await?;
    let proxy = NotificationsProxy::new(&conn).await?;
    let actions = proxy
        .get_capabilities()
        .await
        .map(|caps| caps.iter().any(|c| c == "actions"))
        .unwrap_or(false);
    if actions {
        listen(proxy);
    }
    let session = Session { conn, actions };
    if let Ok(mut slot) = SESSION.lock() {
        *slot = Some(session.clone());
    }
    Ok(session)
}

/// Forget the shared connection (after a failure), so the next send reconnects.
fn reset_session() {
    if let Ok(mut slot) = SESSION.lock() {
        *slot = None;
    }
}

/// Show a desktop notification; returns the server-assigned id. Best-effort —
/// the caller can ignore the result on a desktop without a notification daemon.
pub async fn send(summary: &str, body: &str) -> zbus::Result<u32> {
    send_with(summary, body, &[]).await
}

/// Show a desktop notification with buttons. Actions the server or app can't
/// serve are left off (see the module docs), degrading to a plain
/// notification. Clicking the notification body runs the first action.
pub async fn send_with(summary: &str, body: &str, actions: &[NotifyAction]) -> zbus::Result<u32> {
    let res = async {
        let session = session().await?;
        let actions: Vec<NotifyAction> = if session.actions {
            usable(actions, request::has_handler())
        } else {
            Vec::new()
        };
        let id = send_on(&session.conn, summary, body, &actions).await?;
        if !actions.is_empty() {
            remember(id, actions);
        }
        Ok(id)
    }
    .await;
    if res.is_err() {
        reset_session();
    }
    res
}

/// The actions worth offering: "Show in Orrery" only with an app handler to
/// serve it, and at most one of each kind.
fn usable(actions: &[NotifyAction], has_handler: bool) -> Vec<NotifyAction> {
    let mut out: Vec<NotifyAction> = Vec::new();
    for a in actions {
        let servable = match a {
            NotifyAction::OpenUrl(url) => !url.is_empty(),
            NotifyAction::ShowRepo(_) => has_handler,
        };
        if servable && !out.iter().any(|o| o.key() == a.key()) {
            out.push(a.clone());
        }
    }
    out
}

/// The flat `[key, label, key, label, …]` list the spec wants, led by the
/// body-click `default` (bound to the first action).
fn action_list(actions: &[NotifyAction]) -> Vec<&'static str> {
    let mut out = Vec::with_capacity(actions.len() * 2 + 2);
    if let Some(first) = actions.first() {
        out.extend([DEFAULT_KEY, first.label()]);
    }
    for a in actions {
        out.extend([a.key(), a.label()]);
    }
    out
}

async fn send_on(
    conn: &Connection,
    summary: &str,
    body: &str,
    actions: &[NotifyAction],
) -> zbus::Result<u32> {
    let proxy = NotificationsProxy::new(conn).await?;
    proxy
        .notify(
            "Orrery",
            0,
            "orrery",
            summary,
            body,
            &action_list(actions),
            HashMap::new(),
            -1,
        )
        .await
}

fn remember(id: u32, actions: Vec<NotifyAction>) {
    if let Ok(mut pending) = PENDING.lock() {
        while pending.len() >= PENDING_CAP {
            pending.pop_front();
        }
        pending.push_back(Pending {
            id,
            actions,
            token: None,
        });
    }
}

/// One signal from the notification server, about some notification (not
/// necessarily ours: ids not in [`PENDING`] are ignored).
enum Event {
    Invoked(u32, String),
    Closed(u32),
    Token(u32, String),
}

/// Listen for clicks and closes on the shared connection, on its own thread
/// (like the other platform services). Ends when the connection does.
fn listen(proxy: NotificationsProxy<'static>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        rt.block_on(async move {
            let (Ok(invoked), Ok(closed), Ok(tokens)) = (
                proxy.receive_action_invoked().await,
                proxy.receive_notification_closed().await,
                proxy.receive_activation_token().await,
            ) else {
                eprintln!("[orrery notify] can't listen for notification actions");
                return;
            };
            // Signals whose body doesn't parse map to `None` and are skipped.
            let invoked = invoked.map(|s| {
                s.args()
                    .ok()
                    .map(|a| Event::Invoked(a.id, a.action_key.clone()))
            });
            let closed = closed.map(|s| s.args().ok().map(|a| Event::Closed(a.id)));
            let tokens = tokens.map(|s| {
                s.args()
                    .ok()
                    .map(|a| Event::Token(a.id, a.activation_token.clone()))
            });
            let mut events =
                futures_util::stream::select(invoked, futures_util::stream::select(closed, tokens));
            while let Some(event) = events.next().await {
                if let Some(event) = event {
                    handle(event);
                }
            }
        });
    });
}

fn handle(event: Event) {
    let Ok(mut pending) = PENDING.lock() else {
        return;
    };
    match event {
        Event::Token(id, token) => {
            if let Some(p) = pending.iter_mut().find(|p| p.id == id) {
                p.token = Some(token);
            }
        }
        Event::Closed(id) => pending.retain(|p| p.id != id),
        Event::Invoked(id, key) => {
            let Some(p) = pending.iter().find(|p| p.id == id) else {
                return;
            };
            let action = resolve(&p.actions, &key).cloned();
            let token = p.token.clone();
            // Act without the lock held.
            drop(pending);
            if let Some(action) = action {
                run(action, token);
            }
        }
    }
}

/// The action an `ActionInvoked` key names (`default` → the first).
fn resolve<'a>(actions: &'a [NotifyAction], key: &str) -> Option<&'a NotifyAction> {
    if key == DEFAULT_KEY {
        actions.first()
    } else {
        actions.iter().find(|a| a.key() == key)
    }
}

fn run(action: NotifyAction, token: Option<String>) {
    match action {
        NotifyAction::OpenUrl(url) => {
            // Through `launch::open`, which only opens http(s) URLs and local
            // paths. It can't pass the activation token on, so on Wayland
            // the browser may flash rather than take focus.
            if let Err(e) = launch::open(&url) {
                eprintln!("[orrery notify] failed to open {url}: {e}");
            }
        }
        NotifyAction::ShowRepo(id) => {
            request::dispatch(AppRequest::ShowRepo {
                id,
                activation_token: token,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> NotifyAction {
        NotifyAction::OpenUrl("https://github.com/o/r/pull/1".into())
    }

    fn show() -> NotifyAction {
        NotifyAction::ShowRepo("/home/me/dev/r".into())
    }

    #[test]
    fn show_needs_an_app_handler() {
        assert_eq!(usable(&[open(), show()], false), vec![open()]);
        assert_eq!(usable(&[open(), show()], true), vec![open(), show()]);
    }

    #[test]
    fn empty_urls_and_duplicates_are_dropped() {
        let none = NotifyAction::OpenUrl(String::new());
        assert!(usable(&[none], true).is_empty());
        assert_eq!(usable(&[open(), open()], true), vec![open()]);
    }

    #[test]
    fn action_list_leads_with_default() {
        assert_eq!(
            action_list(&[open(), show()]),
            ["default", "Open", "open", "Open", "show", "Show in Orrery"]
        );
        assert!(action_list(&[]).is_empty());
    }

    #[test]
    fn keys_resolve_back_to_actions() {
        let actions = [open(), show()];
        assert_eq!(resolve(&actions, "default"), Some(&open()));
        assert_eq!(resolve(&actions, "show"), Some(&show()));
        assert_eq!(resolve(&actions, "inline-reply"), None);
    }

    /// Sends a real notification with both buttons and waits a while so a
    /// human can click one (the server's `ActionInvoked` can't be faked).
    /// Run with `cargo test -p orrery-platform -- --ignored live_actions --nocapture`.
    #[test]
    #[ignore = "needs a notification server and a human"]
    fn live_actions() {
        request::set_handler(|r| eprintln!("app request: {r:?}"));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let id = rt
            .block_on(send_with(
                "New pull request",
                "Hankanman/Orrery #1 · test notification",
                &[open(), show()],
            ))
            .expect("notification server");
        eprintln!("sent notification {id}");
        // The action listener runs on its own thread; just keep the process up.
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn closing_forgets_and_others_ids_are_ignored() {
        remember(4_000_001, vec![open()]);
        handle(Event::Invoked(4_000_999, "open".into())); // not ours: no-op
        handle(Event::Token(4_000_001, "tok".into()));
        assert!(PENDING
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.id == 4_000_001 && p.token.as_deref() == Some("tok")));
        handle(Event::Closed(4_000_001));
        assert!(!PENDING.lock().unwrap().iter().any(|p| p.id == 4_000_001));
    }
}
