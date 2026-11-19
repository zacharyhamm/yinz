use super::super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::screen::agent_control) struct AgyLifecycle {
    pub(in crate::screen::agent_control) confirmation: Option<bool>,
}

impl super::super::AgentController {
    pub(in crate::screen) fn ignore_agy_confirmation(
        &self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        if hook.is_worker() {
            return false;
        }
        let Some(lifecycle) = self.agent_lifecycle.get(&pane) else {
            return hook.agy_turn_complete.is_some();
        };
        if hook.agy_turn_complete.is_some() {
            if lifecycle.kind != "agy"
                || hook.session_id.is_empty()
                || lifecycle.conversation != hook.session_id
            {
                return true;
            }
            if hook.prompt_id.as_deref().is_none_or(str::is_empty)
                || lifecycle.claude.prompt_id != hook.prompt_id
            {
                return true;
            }
            return !(lifecycle.agy.confirmation.is_none()
                || (lifecycle.agy.confirmation == Some(false)
                    && hook.agy_turn_complete == Some(true)));
        }
        if hook.hook_event_name == "UserPromptSubmit" && hook.prompt_id.is_some() {
            return lifecycle.kind == "agy"
                && lifecycle.conversation == hook.session_id
                && (lifecycle.claude.previous_prompt_id == hook.prompt_id
                    || (lifecycle.claude.prompt_id == hook.prompt_id
                        && lifecycle.agy.confirmation.is_some()));
        }
        hook.hook_event_name == "Stop"
            && lifecycle.kind == "agy"
            && lifecycle.conversation == hook.session_id
            && lifecycle.agy.confirmation == Some(true)
    }
}
