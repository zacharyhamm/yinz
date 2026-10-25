//! Definitions and helpers for sending and receiving messages between threads.

use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::errors::get_current_ctx;
pub use crossbeam::channel::{
    bounded, unbounded, Receiver, RecvError, RecvTimeoutError, Select, SendError, Sender,
    TrySendError,
};
use yinz_types::diagnostics::ErrorContext;

/// An [MPSC](mpsc) asynchronous channel with added error context.
pub type ChannelWithContext<T> = (Sender<(T, ErrorContext)>, Receiver<(T, ErrorContext)>);

/// Shared by a session's channels so late sends can tolerate an exited worker.
#[derive(Clone, Default)]
pub struct Shutdown(Arc<AtomicBool>);

impl Shutdown {
    pub fn begin(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn sender<T>(&self, sender: Sender<(T, ErrorContext)>) -> SenderWithContext<T> {
        SenderWithContext {
            sender,
            shutdown: self.clone(),
        }
    }
}

/// Sends messages on an [MPSC](std::sync::mpsc) channel, along with an [`ErrorContext`],
/// blocking when the underlying channel is full.
#[derive(Clone)]
pub struct SenderWithContext<T> {
    sender: Sender<(T, ErrorContext)>,
    shutdown: Shutdown,
}

impl<T: Clone> SenderWithContext<T> {
    pub fn new(sender: Sender<(T, ErrorContext)>) -> Self {
        Shutdown::default().sender(sender)
    }

    /// Sends an event, along with the current [`ErrorContext`], on this
    /// [`SenderWithContext`]'s channel, tolerating disconnects during shutdown.
    pub fn send(&self, event: T) -> Result<(), SendError<(T, ErrorContext)>> {
        match self.send_required(event) {
            // Check after sending: a bounded send may have been waiting when teardown began.
            Err(_) if self.shutdown.0.load(Ordering::Relaxed) => Ok(()),
            result => result,
        }
    }

    /// Sends an event with the current [`ErrorContext`], returning an undelivered
    /// event even during shutdown so the caller can clean up resources it owns.
    pub fn send_required(&self, event: T) -> Result<(), SendError<(T, ErrorContext)>> {
        self.sender.send((event, get_current_ctx()))
    }
}

#[cfg(test)]
#[path = "unit/channels.rs"]
mod tests;

thread_local!(
    /// A key to some thread local storage (TLS) that holds a representation of the thread's call
    /// stack in the form of an [`ErrorContext`].
    pub static OPENCALLS: RefCell<ErrorContext> = RefCell::default()
);

tokio::task_local! {
    /// A key to some task local storage that holds a representation of the task's call
    /// stack in the form of an [`ErrorContext`].
    pub static ASYNCOPENCALLS: RefCell<ErrorContext>;
}
