use super::super::*;
use crate::providers::agy::{ignore_confirmation, ConfirmationContext};

impl super::super::AgentController {
    pub(in crate::controller) fn ignore_agy_confirmation(
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

pub(in crate::controller) fn initial_prompt_revision(
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

pub(in crate::controller) fn pre_invocation_revision(
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

impl AgentController {
    pub(in crate::controller) fn observe_agy_confirmation(
        &mut self,
        pane: PaneId,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
        parent: bool,
    ) {
        if parent
            && event.kind == "agy"
            && hook.is_some_and(|hook| hook.agy_turn_complete == Some(false))
            && let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane)
        {
            lifecycle.agy = providers::AgyLifecycle::from_confirmation(Some(false));
        }
    }
}
