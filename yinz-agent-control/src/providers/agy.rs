use yinz_types::agent::HookEvent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgyLifecycle {
    confirmation: Option<bool>,
}

impl AgyLifecycle {
    pub fn from_confirmation(confirmation: Option<bool>) -> Self {
        Self { confirmation }
    }
    pub fn confirmation(self) -> Option<bool> {
        self.confirmation
    }

    pub fn after_hook(self, kind: &str, name: &str, hook: Option<&HookEvent>) -> Self {
        let confirmation = if matches!(name, "UserPromptSubmit" | "PreInvocation") {
            None
        } else if kind == "agy" && matches!(name, "StopFailure" | "Interrupt") {
            Some(true)
        } else {
            hook.and_then(|hook| hook.agy_turn_complete)
                .or(self.confirmation)
        };
        Self { confirmation }
    }
}

pub struct ConfirmationContext<'a> {
    pub kind: &'a str,
    pub conversation: &'a str,
    pub prompt_id: Option<&'a str>,
    pub previous_prompt_id: Option<&'a str>,
    pub state: AgyLifecycle,
}

pub fn ignore_confirmation(hook: &HookEvent, lifecycle: Option<ConfirmationContext<'_>>) -> bool {
    if hook.is_worker() {
        return false;
    }
    let Some(ConfirmationContext {
        kind,
        conversation,
        prompt_id,
        previous_prompt_id,
        state,
    }) = lifecycle
    else {
        return hook.agy_turn_complete.is_some();
    };
    if hook.agy_turn_complete.is_some() {
        if kind != "agy" || hook.session_id.is_empty() || conversation != hook.session_id {
            return true;
        }
        if hook.prompt_id.as_deref().is_none_or(str::is_empty)
            || prompt_id != hook.prompt_id.as_deref()
        {
            return true;
        }
        return !(state.confirmation.is_none()
            || (state.confirmation == Some(false) && hook.agy_turn_complete == Some(true)));
    }
    if hook.hook_event_name == "UserPromptSubmit" && hook.prompt_id.is_some() {
        return kind == "agy"
            && conversation == hook.session_id
            && (previous_prompt_id == hook.prompt_id.as_deref()
                || (prompt_id == hook.prompt_id.as_deref() && state.confirmation.is_some()));
    }
    hook.hook_event_name == "Stop"
        && kind == "agy"
        && conversation == hook.session_id
        && state.confirmation == Some(true)
}
