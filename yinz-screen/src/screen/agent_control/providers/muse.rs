use super::super::*;

impl super::super::AgentController {
    /// Whether the pane holds the launcher's verified Muse hook capability, still waiting on
    /// the provider's first turn to bind a conversation. Claimed by the marker alone.
    pub(in crate::screen) fn has_provisional_muse_lifecycle(&self, pane: PaneId) -> bool {
        self.agent_lifecycle
            .get(&pane)
            .is_some_and(|lifecycle| lifecycle.kind == "muse" && lifecycle.conversation.is_empty())
    }
}
