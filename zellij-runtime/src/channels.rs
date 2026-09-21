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
use zellij_types::diagnostics::ErrorContext;

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
        for (sender, receiver) in [unbounded(), bounded(2)] {
            let shutdown = Shutdown::default();
            let sender = shutdown.sender(sender);
            sender.send("finish queued work").unwrap();
            shutdown.begin();
            sender.clone().send("exit").unwrap();
            assert_eq!(receiver.try_recv().unwrap().0, "finish queued work");
            assert_eq!(receiver.try_recv().unwrap().0, "exit");
        }
    }

    #[test]
    fn shutdown_tolerates_disconnects_across_channels_and_clones() {
        let shutdown = Shutdown::default();
        let (sender, receiver) = unbounded();
        let sender = shutdown.sender(sender);
        let cloned_sender = sender.clone();
        let (bounded_sender, bounded_receiver) = bounded(1);
        let bounded_sender = shutdown.sender(bounded_sender);
        drop(receiver);
        drop(bounded_receiver);

        assert!(sender.send("unexpected disconnect").is_err());
        assert!(bounded_sender.send("unexpected disconnect").is_err());
        shutdown.begin();
        sender.send("original").unwrap();
        cloned_sender.send("clone").unwrap();
        bounded_sender.send("bounded").unwrap();
        bounded_sender.clone().send("bounded clone").unwrap();
    }

    #[test]
    fn required_sends_preserve_delivery_errors_after_shutdown() {
        for (sender, receiver) in [unbounded(), bounded(2)] {
            let shutdown = Shutdown::default();
            let sender = shutdown.sender(sender);
            let cloned_sender = sender.clone();
            sender.send_required("before shutdown").unwrap();
            shutdown.begin();
            cloned_sender.send_required("after shutdown").unwrap();
            assert_eq!(receiver.try_recv().unwrap().0, "before shutdown");
            assert_eq!(receiver.try_recv().unwrap().0, "after shutdown");
            drop(receiver);

            let (undelivered, context) = sender.send_required("original").unwrap_err().0;
            assert_eq!(undelivered, "original");
            assert_eq!(context.calls(), get_current_ctx().calls());
            let (undelivered, context) = cloned_sender.send_required("clone").unwrap_err().0;
            assert_eq!(undelivered, "clone");
            assert_eq!(context.calls(), get_current_ctx().calls());
        }
    }

    #[test]
    fn independent_sessions_keep_their_send_errors() {
        let shutdown = Shutdown::default();
        let other_shutdown = Shutdown::default();
        let (sender, receiver) = unbounded();
        let sender = shutdown.sender(sender);
        let (other_sender, other_receiver) = unbounded();
        let other_sender = other_shutdown.sender(other_sender);
        drop(receiver);
        drop(other_receiver);

        shutdown.begin();
        sender.send("shutting down").unwrap();
        assert!(other_sender.send("still running").is_err());
    }

    #[test]
    fn shutdown_releases_a_bounded_send_when_its_receiver_exits() {
        use std::{thread, time::Duration};

        let shutdown = Shutdown::default();
        let (sender, receiver) = bounded(0);
        let sender = shutdown.sender(sender);
        let (finished, completion) = unbounded();
        let worker = thread::spawn(move || {
            finished.send(sender.send("pending update")).unwrap();
        });

        // A rendezvous receive is ready only once the sender is waiting. Observe it
        // without taking the message, then disconnect to interrupt that pending send.
        let mut selector = Select::new();
        selector.recv(&receiver);
        selector.ready_timeout(Duration::from_secs(5)).unwrap();
        drop(selector);
        assert!(completion.try_recv().is_err());
        shutdown.begin();
        drop(receiver);
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
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
