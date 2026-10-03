//! Bounds the frames and frame bytes the router has read off the server socket but the client
//! loop has not yet written to the terminal.
//!
//! The channel between them holds up to 50 instructions of any size. With frames
//! carrying multi-megabyte graphics uploads, that let a client hide hundreds of
//! megabytes of backlog from the server, which paces its rendering on what is still
//! queued for the socket. With the budget full, the router stops reading, the
//! socket fills, and the server sees the client fall behind. At most two frames
//! are reserved, including the one being written; the router can also hold one
//! decoded frame while waiting for room.

use std::sync::{Condvar, Mutex};
use yinz_runtime::errors::prelude::*;

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
    frames: usize,
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
    pub fn reserve(&self, bytes: usize) -> Result<()> {
        let err_context = "failed to reserve room for a frame";
        let mut state = self.state.lock().to_anyhow().context(err_context)?;
        while !state.closed
            && (state.frames >= 2
                || (state.frames > 0
                    && (state.pending > self.limit
                        || bytes > self.limit.saturating_sub(state.pending))))
        {
            state = self.room.wait(state).to_anyhow().context(err_context)?;
        }
        state.pending += bytes;
        state.frames += 1;
        Ok(())
    }

    /// The client loop wrote a frame of `bytes`.
    pub fn release(&self, bytes: usize) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .to_anyhow()
            .context("failed to release a written frame")?;
        state.pending = state.pending.saturating_sub(bytes);
        state.frames = state.frames.saturating_sub(1);
        self.room.notify_all();
        Ok(())
    }

    /// The client loop stopped and will write nothing more.
    pub fn close(&self) -> Result<()> {
        self.state
            .lock()
            .to_anyhow()
            .context("failed to close the render budget")?
            .closed = true;
        self.room.notify_all();
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit/render_budget_tests.rs"]
mod tests;
