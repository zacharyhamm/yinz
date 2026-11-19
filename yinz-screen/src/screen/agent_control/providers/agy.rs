use super::super::*;
use yinz_agent_control::providers::agy::{ignore_confirmation, ConfirmationContext};

impl super::super::AgentController {
    pub(in crate::screen) fn ignore_agy_confirmation(
        &self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        ignore_confirmation(
            hook,
            self.agent_lifecycle
                .get(&pane)
                .map(|l| ConfirmationContext {
                    kind: &l.kind,
                    conversation: &l.conversation,
                    prompt_id: l.prompt_id.as_deref(),
                    previous_prompt_id: l.previous_prompt_id.as_deref(),
                    state: l.agy,
                }),
        )
    }
}
