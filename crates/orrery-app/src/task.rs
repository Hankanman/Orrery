//! Bridge async (network / AI) core calls onto the GPUI foreground.
//!
//! GPUI's background executor can poll futures but provides no tokio reactor, so
//! reqwest-backed core calls (`forge` / `inbox` / `ai` / `oauth`) — which need
//! one — can't run on it directly. We own a single shared multi-threaded tokio
//! runtime and hand each result back through a one-shot channel that the calling
//! gpui task awaits.
//!
//! This is the one spot `orrery-app` owns a runtime (everything else lives in
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
/// (gpui) executor. Call inside `cx.spawn` for network/AI core calls. Our core
/// futures return `Result` rather than panicking, so the channel always delivers.
pub async fn run<F, T>(fut: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        let _ = tx.try_send(fut.await);
    });
    rx.recv()
        .await
        .expect("tokio task dropped before sending its result")
}
