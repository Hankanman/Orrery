//! Native desktop notifications over D-Bus (`org.freedesktop.Notifications`).
//! Works on any freedesktop desktop.

use std::collections::HashMap;
use std::sync::Mutex;

use zbus::zvariant::Value;
use zbus::Connection;

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
}

/// The session-bus connection shared by every notification, opened lazily on
/// the first send. zbus connections are cheap to clone (an `Arc`) and run on
/// their own executor, so one serves callers on any thread/runtime.
static SESSION: Mutex<Option<Connection>> = Mutex::new(None);

async fn session() -> zbus::Result<Connection> {
    if let Some(conn) = SESSION.lock().ok().and_then(|c| c.clone()) {
        return Ok(conn);
    }
    // Connect without holding the lock across the await. Two first sends
    // racing just open two connections; the later one wins the slot.
    let conn = Connection::session().await?;
    if let Ok(mut slot) = SESSION.lock() {
        *slot = Some(conn.clone());
    }
    Ok(conn)
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
    let res = send_on(&session().await?, summary, body).await;
    if res.is_err() {
        reset_session();
    }
    res
}

async fn send_on(conn: &Connection, summary: &str, body: &str) -> zbus::Result<u32> {
    let proxy = NotificationsProxy::new(conn).await?;
    proxy
        .notify(
            "Orrery",
            0,
            "orrery",
            summary,
            body,
            &[],
            HashMap::new(),
            -1,
        )
        .await
}
