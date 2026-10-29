//! Bridge async (network / AI) core calls onto the GPUI foreground.
//!
//! GPUI's background executor can poll futures but provides no tokio reactor, so
//! reqwest-backed core calls (`forge` / `inbox` / `ai` / `oauth`) — which need
//! one — can't run on it directly. We own a single shared multi-threaded tokio
//! runtime and hand each result back through a one-shot channel that the calling
//! gpui task awaits.
//!
//! This is the one spot `orrery` (the app) owns a runtime (everything else lives in
//! `orrery-platform`); on-demand UI data fundamentally needs an in-app reactor.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// The shared runtime, built on first use.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build the shared tokio runtime")
    })
}

/// Run `fut` on the shared tokio runtime and await its result on the caller's
/// (gpui) executor. Call inside `cx.spawn` for network/AI core calls.
///
/// Returns `None` if the future panicked (tokio tears the task down before it
/// sends) instead of re-raising the panic on the GPUI foreground: a bug in one
/// core call must not take the whole app down. The failure is logged.
pub async fn run<F, T>(fut: F) -> Option<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        let _ = tx.try_send(fut.await);
    });
    match rx.recv().await {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!("[native] background task ended without a result (panicked?)");
            None
        }
    }
}

/// [`run`] for the common `Result<T, String>` core call: a panicked task folds
/// into an `Err`, so callers keep their single error path.
pub async fn try_run<F, T>(fut: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    run(fut)
        .await
        .unwrap_or_else(|| Err("background task failed unexpectedly".into()))
}

/// Spawn `fut` on the shared runtime, detached (fire-and-forget). Use for work
/// that reports back through its own channel rather than a single return value —
/// e.g. a model download streaming progress. The future must be `Send`.
pub fn spawn_detached<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(fut);
}

/// Persist a cache-meta value (`orrery_core::cache::set_meta`) off the UI
/// thread. Writes go through one lazily started writer thread, so they land in
/// the order they were issued — a later value is never overwritten by an
/// earlier one. Fire-and-forget, like the synchronous `set_meta` it replaces.
pub fn persist_meta(key: &'static str, value: String) {
    use std::sync::mpsc;
    static WRITER: OnceLock<mpsc::Sender<(&'static str, String)>> = OnceLock::new();
    let tx = WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(&'static str, String)>();
        let spawned = std::thread::Builder::new()
            .name("orrery-meta-writer".into())
            .spawn(move || {
                while let Ok((key, value)) = rx.recv() {
                    orrery_core::cache::set_meta(key, &value);
                }
            });
        if let Err(e) = spawned {
            eprintln!("[native] couldn't start the cache writer thread: {e}");
        }
        tx
    });
    if tx.send((key, value)).is_err() {
        eprintln!("[native] cache writer unavailable; dropped a write to {key}");
    }
}
