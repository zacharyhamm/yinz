//! The runtime that enforces forward-slot deadlines.
//!
//! A forwarded query has to be released even if the host terminal never
//! answers, or the server would wait on it forever. The deadline is driven
//! here rather than by the parser, which is deliberately deadline-agnostic.

use crate::stdin_ansi_parser::{ForwardKind, StdinAnsiParser};
use std::sync::{Arc, Mutex, OnceLock};

/// Dedicated, lazily-initialised runtime for driving forward-slot
/// timeouts. A single current-thread executor runs on its own OS
/// thread; timer tasks are `spawn`-ed onto it from the synchronous
/// `ClientInstruction::ForwardQueryToHost` handler. One-thread model
/// because timer tasks do no CPU work — they just sleep and perform a
/// millisecond-scale mutex check on wake-up.
static FORWARD_TIMEOUT_RUNTIME: OnceLock<Arc<tokio::runtime::Runtime>> = OnceLock::new();

pub fn forward_timeout_runtime() -> &'static Arc<tokio::runtime::Runtime> {
    FORWARD_TIMEOUT_RUNTIME.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("failed to build forward-timeout runtime");
        let rt = Arc::new(rt);
        let rt_for_driver = rt.clone();
        // `block_on(pending())` keeps the executor loop alive forever
        // on this thread; spawned timer tasks are polled as they
        // become ready (on spawn, on wake from the time driver).
        std::thread::Builder::new()
            .name("yinz-client-forward-timeout".into())
            .spawn(move || {
                rt_for_driver.block_on(std::future::pending::<()>());
            })
            .expect("failed to spawn forward-timeout driver thread");
        rt
    })
}

/// Spawn a timer task that closes a forward slot after `deadline` and
/// invokes `on_timeout(token, reply_bytes)` with whatever the slot
/// accumulated. Token-guard idempotent: if the barrier (or a
/// replacement forward) has already cleared the slot by the time the
/// timer wakes, `close_forward_on_timeout(token)` returns `None` and
/// `on_timeout` is never called — no explicit cancellation path
/// required.
///
/// Extracted as a free function so tests can drive it against a
/// `tokio::time::pause()`-backed paused runtime without instantiating
/// the full client.
pub fn schedule_timeout<F>(
    runtime: &tokio::runtime::Handle,
    parser: Arc<Mutex<StdinAnsiParser>>,
    kind: ForwardKind,
    token: u32,
    deadline: std::time::Duration,
    on_timeout: F,
) where
    F: FnOnce(u32, Vec<u8>) + Send + 'static,
{
    runtime.spawn(async move {
        tokio::time::sleep(deadline).await;
        let payload = parser.lock().unwrap().close_on_timeout(kind, token);
        if let Some((t, bytes)) = payload {
            on_timeout(t, bytes);
        }
    });
}
