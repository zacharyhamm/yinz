use super::super::*;

impl super::super::AgentController {
    /// Whether the pane holds the launcher's verified Muse hook capability, still waiting on
    /// the provider's first turn to bind a conversation. Claimed by the marker alone.
    pub(in crate::controller) fn has_provisional_muse_lifecycle(&self, pane: PaneId) -> bool {
        self.agent_lifecycle
            .get(&pane)
            .is_some_and(|lifecycle| lifecycle.kind == "muse" && lifecycle.conversation.is_empty())
    }
}

pub(in crate::controller) fn provisional_start(
    event: &AgentEvent,
    hook: Option<&HookEvent>,
) -> bool {
    event.kind == "muse"
        && event.session_id.is_none()
        && hook.is_some_and(|hook| {
            hook.hook_event_name == "SessionStart"
                && hook.source.as_deref() != Some("compact")
                && hook.session_id.is_empty()
                && hook.muse_hooks_ready == Some(true)
                && yinz_types::agent::hook_state_for(&event.kind, hook).is_some()
        })
}
impl AgentController {
    pub(in crate::controller) fn bind_provisional_muse(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        revision: Option<u64>,
    ) {
        // The launcher's startup announcement: verified hook wiring, but
        // unpinned, because Muse speaks no session id until its first turn.
        // Claimed by the marker alone - a provider hook never carries one.
        let transferred = self.warped_agent_inputs.remove(&pane).map(|input| {
            (input.kind == "muse"
                && input.conversation.is_empty()
                && input.consumed == Some(true)
                && input.input_revision == host.agent_input_revision(pane))
            .then_some(input.input_revision)
        });
        let previous = self
            .agent_lifecycle
            .get(&pane)
            .filter(|previous| previous.kind == "muse" && previous.conversation.is_empty());
        if previous.is_none() {
            log::debug!("agent lifecycle created pane={pane:?} kind=muse provisional=true");
        }
        let input_revision = revision.or(previous
            .map(|previous| previous.input_revision)
            .unwrap_or_else(|| {
                transferred.unwrap_or_else(|| (host.agent_input_revision(pane) == 0).then_some(0))
            }));
        self.agent_lifecycle.insert(
            pane,
            Lifecycle {
                kind: "muse".to_owned(),
                conversation: String::new(),
                input_revision,
                last_report: Instant::now(),
                ended_turn: previous.and_then(|previous| previous.ended_turn.clone()),
                closeout_turn: None,
                last_message: None,
                prompt_id: None,
                previous_prompt_id: None,
                claude: providers::ClaudeLifecycle {
                    activity: ClaudeActivity::default(),
                    observed_at: previous.map_or_else(Instant::now, |l| l.claude.observed_at),
                    confirmation: None,
                    background: None,
                    notifications: BTreeMap::new(),
                },
                agy: providers::AgyLifecycle::default(),
            },
        );
    }
}

pub(super) fn late_bootstrap(
    host: &impl AgentHost,
    pane_id: PaneId,
    hook: Option<&HookEvent>,
) -> bool {
    hook.is_some_and(|hook| {
        hook.hook_event_name == "SessionStart"
            && hook.muse_hooks_ready.is_some()
            && hook.session_id.is_empty()
    }) && host
        .pane_info(pane_id)
        .and_then(|info| info.agent)
        .is_some_and(|agent| {
            agent.kind == "muse"
                && agent.source == yinz_types::agent::AgentSource::Reported
                && agent.session_id.is_some()
        })
}

pub(in crate::controller) fn binds_session(
    previous: Option<&AgentInfo>,
    agent: Option<&AgentInfo>,
) -> bool {
    previous.is_some_and(|previous| previous.kind == "muse" && previous.session_id.is_none())
        && agent.is_some_and(|agent| agent.kind == "muse" && agent.session_id.is_some())
}
