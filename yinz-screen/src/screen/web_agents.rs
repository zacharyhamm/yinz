//! Live-agent snapshots and focus for the mobile web interface.
//!
//! Built from the same `LiveAgent` list and helpers the command palette uses, so the web view
//! and the palette agree on order, presentation and focus rules.

use super::Screen;
use yinz_bus::web::{WebAgentRef, WebLiveAgent};
use yinz_runtime::errors::prelude::*;

impl Screen {
    /// Answer every live agent the palette would list.
    pub(super) fn web_live_agents(&mut self, reply: crossbeam::channel::Sender<Vec<WebLiveAgent>>) {
        reply
            .send(Vec::new())
            .map_err(|_| anyhow!("web reply receiver dropped"))
            .non_fatal();
    }

    /// Focus `agent` for the first connected client, as the palette would.
    pub(super) fn web_focus_agent(
        &mut self,
        agent: WebAgentRef,
        reply: crossbeam::channel::Sender<Result<(), String>>,
    ) {
        reply
            .send(Err(format!(
                "Agent {} cannot be focused from the web in this build",
                agent.id
            )))
            .map_err(|_| anyhow!("web reply receiver dropped"))
            .non_fatal();
    }
}
