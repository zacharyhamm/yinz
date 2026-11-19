//! The agent host contract and provider observations shared with the screen thread.
pub mod close_completed;
pub mod host;
pub mod providers;
pub mod worktrees;

pub type RestoredStatePublisher = Box<dyn Fn(Option<yinz_types::agent::AgentState>) -> bool + Send>;
pub type RestoredAgentObserver = std::sync::Arc<
    dyn Fn(
            yinz_types::internal_ids::TerminalId,
            String,
            RestoredStatePublisher,
        ) -> yinz_runtime::errors::prelude::Result<()>
        + Send
        + Sync,
>;
