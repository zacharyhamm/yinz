use super::super::*;

impl AgentController {
    pub(in crate::screen::agent_control) fn pi_pre_submit_dialog(
        &self,
        pane: PaneId,
        agent: &AgentInfo,
    ) -> bool {
        agent.kind == "pi"
            && agent.is_waiting_on_a_person()
            && self.agent_controls.iter().any(|pending| {
                pending.pane == pane
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(PromptObservation::waiting_before_submit)
            })
    }
}
