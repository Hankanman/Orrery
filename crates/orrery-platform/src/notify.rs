//! Native desktop notifications over D-Bus (`org.freedesktop.Notifications`),
//! replacing the Tauri notification plugin. Works on any freedesktop desktop.

use std::collections::HashMap;

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

/// Show a desktop notification; returns the server-assigned id. Best-effort —
/// the caller can ignore the result on a desktop without a notification daemon.
pub async fn send(summary: &str, body: &str) -> zbus::Result<u32> {
    let conn = Connection::session().await?;
    let proxy = NotificationsProxy::new(&conn).await?;
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
