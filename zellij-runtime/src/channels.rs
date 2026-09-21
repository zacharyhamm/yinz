//! Definitions and helpers for sending and receiving messages between threads.

use std::cell::RefCell;

use crate::errors::get_current_ctx;
pub use crossbeam::channel::{
    bounded, unbounded, Receiver, RecvError, RecvTimeoutError, Select, SendError, Sender,
    TrySendError,
};
use zellij_types::diagnostics::ErrorContext;

/// An [MPSC](mpsc) asynchronous channel with added error context.
pub type ChannelWithContext<T> = (Sender<(T, ErrorContext)>, Receiver<(T, ErrorContext)>);

/// Sends messages on an [MPSC](std::sync::mpsc) channel, along with an [`ErrorContext`],
/// synchronously or asynchronously depending on the underlying [`SenderType`].
#[derive(Clone)]
pub struct SenderWithContext<T> {
    sender: Sender<(T, ErrorContext)>,
}

impl<T: Clone> SenderWithContext<T> {
    pub fn new(sender: Sender<(T, ErrorContext)>) -> Self {
        Self { sender }
    }

    /// Sends an event, along with the current [`ErrorContext`], on this
    /// [`SenderWithContext`]'s channel.
    pub fn send(&self, event: T) -> Result<(), SendError<(T, ErrorContext)>> {
        let err_ctx = get_current_ctx();
        self.sender.send((event, err_ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_sends_return_the_undelivered_message() {
        let (sender, receiver) = unbounded();
        let sender = SenderWithContext::new(sender);
        let cloned_sender = sender.clone();
        drop(receiver);

        let (undelivered, _) = sender.send("original").unwrap_err().0;
        assert_eq!(undelivered, "original");
        let (undelivered, _) = cloned_sender.send("clone").unwrap_err().0;
        assert_eq!(undelivered, "clone");
    }

    #[test]
    fn connected_sends_deliver_in_order() {
        let (sender, receiver) = unbounded();
        let sender = SenderWithContext::new(sender);
        sender.send("finish queued work").unwrap();
        sender.send("exit").unwrap();
        assert_eq!(receiver.try_recv().unwrap().0, "finish queued work");
        assert_eq!(receiver.try_recv().unwrap().0, "exit");
    }
}

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
