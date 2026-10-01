//! Bounds the frame bytes the router has read off the server socket but the client
//! loop has not yet written to the terminal.
//!
//! The channel between them holds up to 50 instructions of any size. With frames
//! carrying multi-megabyte graphics uploads, that let a client hide hundreds of
//! megabytes of backlog from the server, which paces its rendering on what is still
//! queued for the socket. With the budget full, the router stops reading, the
//! socket fills, and the server sees the client fall behind.

use std::sync::{Condvar, Mutex};

/// Frame bytes waiting for the terminal before the router stops reading.
pub(crate) const RENDER_BUDGET_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct RenderBudget {
    limit: usize,
    state: Mutex<BudgetState>,
    room: Condvar,
}

#[derive(Default)]
struct BudgetState {
    pending: usize,
    closed: bool,
}

impl RenderBudget {
    pub fn new(limit: usize) -> Self {
        RenderBudget {
            limit,
            state: Mutex::default(),
            room: Condvar::new(),
        }
    }

    /// Wait until `bytes` fit. A frame larger than the whole budget goes through once
    /// nothing else is pending. Returns at once after `close`, so the router can
    /// finish instead of waiting on a loop that has gone.
    pub fn reserve(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        while !state.closed && state.pending > 0 && state.pending + bytes > self.limit {
            state = self.room.wait(state).unwrap();
        }
        state.pending += bytes;
    }

    /// The client loop wrote a frame of `bytes`.
    pub fn release(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.pending = state.pending.saturating_sub(bytes);
        self.room.notify_all();
    }

    /// The client loop stopped and will write nothing more.
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.room.notify_all();
    }
}
