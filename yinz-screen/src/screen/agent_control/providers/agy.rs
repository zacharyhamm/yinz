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

pub(in crate::screen::agent_control) fn initial_prompt_revision(
    kind: &str,
    transferred: Option<Option<u64>>,
    previous: Option<&Lifecycle>,
    host: &impl AgentHost,
    pane: PaneId,
) -> Option<u64> {
    (kind == "agy"
        && transferred.is_none()
        && previous.is_none()
        && host.agent_input_revision(pane) == 0)
        .then_some(0)
}

pub(in crate::screen::agent_control) fn pre_invocation_revision(
    accepted: Option<u64>,
    transferred: Option<Option<u64>>,
    previous: Option<&Lifecycle>,
    host: &impl AgentHost,
    pane: PaneId,
) -> Option<u64> {
    accepted.or_else(|| {
        previous.map(|l| l.input_revision).unwrap_or_else(|| {
            transferred.unwrap_or_else(|| (host.agent_input_revision(pane) == 0).then_some(0))
        })
    })
}
