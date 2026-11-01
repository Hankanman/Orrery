//! Single instance over D-Bus. The first Orrery process owns the session-bus
//! name [`BUS_NAME`] and serves `com.orrery.Instance1`; a later process (a
//! second launch, `orrery open <query>`, an `orrery://` link) finds the name
//! taken, hands its request to the running app and exits.
//!
//! Requests arrive as the same [`AppRequest`]s the notification buttons and
//! the GNOME search provider raise, and go to the app through the one handler
//! registered with [`crate::request::set_handler`].
//!
//! The session bus is unauthenticated, so the service trusts nothing it is
//! sent: `ShowRepo` only acts on an id that is a known scanned repo, and a
//! search query is cleaned and capped before it reaches the palette.

use std::sync::mpsc;
use std::time::Duration;

use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::open_request;
use crate::repo_search;
use crate::request::{self, AppRequest};

/// The well-known name the running instance owns.
pub const BUS_NAME: &str = "com.orrery.Instance";
const OBJECT_PATH: &str = "/com/orrery/Instance";
const INTERFACE: &str = "com.orrery.Instance1";

/// How long startup waits for the bus before carrying on as the only
/// instance. A wedged bus mustn't stop the app from opening.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(3);

/// The outcome of [`claim`].
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This process is the instance (or there's no session bus): start the UI.
    Primary,
    /// Another instance took the request: exit.
    HandedOff,
    /// Another instance is running but refused the request (an unknown repo
    /// id, or its UI isn't ready): exit with an error.
    Refused,
}

/// An empty token travels as "" over D-Bus.
fn token(t: String) -> Option<String> {
    (!t.is_empty()).then_some(t)
}

struct Instance;

#[zbus::interface(name = "com.orrery.Instance1")]
impl Instance {
    /// Raise the window. Returns whether the app took the request.
    async fn show(&self, activation_token: String) -> bool {
        request::dispatch(AppRequest::Show {
            activation_token: token(activation_token),
        })
    }

    /// Raise the window on the repo with this id (its path). Refused unless
    /// the id is a known scanned repo.
    async fn show_repo(&self, id: String, activation_token: String) -> bool {
        if repo_search::known(&id).is_none() {
            eprintln!("[orrery instance] ShowRepo refused: not a known repo: {id:?}");
            return false;
        }
        request::dispatch(AppRequest::ShowRepo {
            id,
            activation_token: token(activation_token),
        })
    }

    /// Raise the window with this query in the command palette.
    async fn search(&self, query: String, activation_token: String) -> bool {
        let Some(query) = open_request::clean_query(&query) else {
            return false;
        };
        request::dispatch(AppRequest::Search {
            query,
            activation_token: token(activation_token),
        })
    }
}

/// Send `request` to the instance that owns [`BUS_NAME`]; `Ok(accepted)`.
async fn hand_off(conn: &zbus::Connection, request: &AppRequest) -> zbus::Result<bool> {
    let tok = |t: &Option<String>| t.clone().unwrap_or_default();
    let reply = match request {
        AppRequest::Show { activation_token } => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                "Show",
                &(tok(activation_token),),
            )
            .await?
        }
        AppRequest::ShowRepo {
            id,
            activation_token,
        } => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                "ShowRepo",
                &(id.as_str(), tok(activation_token)),
            )
            .await?
        }
        AppRequest::Search {
            query,
            activation_token,
        } => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                "Search",
                &(query.as_str(), tok(activation_token)),
            )
            .await?
        }
    };
    reply.body().deserialize::<bool>()
}

/// Become the running instance, or hand `request` (a plain launch sends
/// [`AppRequest::Show`]) to the one already running. Blocks for at most
/// [`CLAIM_TIMEOUT`]. When this process wins, the service keeps running on its
/// own thread for the life of the process.
pub fn claim(request: Option<AppRequest>) -> Claim {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
            let _ = tx.send(Claim::Primary);
            return;
        };
        rt.block_on(async move {
            let conn = match zbus::connection::Builder::session()
                .and_then(|b| b.serve_at(OBJECT_PATH, Instance))
            {
                Ok(b) => b.build().await,
                Err(e) => Err(e),
            };
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[orrery instance] no session bus, running unclaimed: {e}");
                    let _ = tx.send(Claim::Primary);
                    return;
                }
            };
            let reply = conn
                .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
                .await;
            match reply {
                Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                    let _ = tx.send(Claim::Primary);
                    // Keep the connection (and with it the name) alive.
                    std::future::pending::<()>().await;
                }
                // zbus reports a taken name (with DoNotQueue) as `NameTaken`.
                Ok(RequestNameReply::Exists | RequestNameReply::InQueue)
                | Err(zbus::Error::NameTaken) => {
                    let request = request.unwrap_or(AppRequest::Show {
                        activation_token: None,
                    });
                    let claim = match hand_off(&conn, &request).await {
                        Ok(true) => Claim::HandedOff,
                        Ok(false) => Claim::Refused,
                        Err(e) => {
                            // The owner is wedged or an older build without
                            // the interface: start normally rather than not
                            // at all.
                            eprintln!("[orrery instance] hand-off failed, starting anyway: {e}");
                            Claim::Primary
                        }
                    };
                    let _ = tx.send(claim);
                }
                Err(e) => {
                    eprintln!("[orrery instance] couldn't claim {BUS_NAME}: {e}");
                    let _ = tx.send(Claim::Primary);
                }
            }
        });
    });
    rx.recv_timeout(CLAIM_TIMEOUT).unwrap_or(Claim::Primary)
}
