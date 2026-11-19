use super::*;

impl super::AgentController {
    pub(in crate::screen) fn agent_status(
        &self,
        host: &impl AgentHost,
        pane_id: PaneId,
        pending_claim: Option<NotReadyReason>,
    ) -> Option<AgentStatus> {
        let info = host.pane_info(pane_id)?;
        let mut agent = info.agent?;
        self.aggregate_claude_state(pane_id, &mut agent);
        // The Muse bootstrap proves hook capability before the provider speaks.
        // Only its visible composer can establish readiness before the first turn.
        let provisional_muse = agent.kind == "muse"
            && agent.session_id.is_none()
            && self.has_provisional_muse_lifecycle(pane_id);
        let readiness = self.agent_readiness(host, pane_id, &agent, info.is_held, info.exited)?;
        let status_available = readiness.status_available();
        let prompt_supported = readiness.prompt_supported();
        let cwd = agent.cwd.clone().or_else(|| host.terminal_cwd(pane_id));
        let not_ready_reason = if self.agent_draft_clears.contains_key(&pane_id) {
            Some(NotReadyReason::ClearingDraft)
        } else if !prompt_supported {
            Some(NotReadyReason::PromptUnsupported)
        } else if agent.state != AgentState::Idle
            && !(agent.kind == "claude" && agent.state == AgentState::Busy)
        {
            Some(NotReadyReason::NotIdle)
        } else if let Some(reason) = pending_claim.or_else(|| {
            self.agent_controls
                .iter()
                .filter(|pending| pending.pane == pane_id)
                .find_map(Pending::not_ready_reason)
        }) {
            Some(reason)
        } else if self.claude_steering(pane_id, &agent) {
            (!self.claude_steering_composer(host, pane_id))
                .then_some(NotReadyReason::UnconsumedInput)
        } else if (provisional_muse && !providers::muse_composer(host, pane_id, true))
            || !(self.agent_lifecycle.get(&pane_id).is_some_and(|lifecycle| {
                lifecycle.input_revision == Some(host.agent_input_revision(pane_id))
            }) || (provisional_muse
                && self.agent_dialog_inputs.get(&pane_id).is_some_and(|input| {
                    input.startup
                        && input.kind == "muse"
                        && input.conversation.is_none()
                        && input.input_revision == host.agent_input_revision(pane_id)
                        && host.agent_input_at(pane_id).is_some_and(|at| {
                            host.last_output(pane_id).is_some_and(|output| output > at)
                        })
                })))
        {
            Some(NotReadyReason::UnconsumedInput)
        } else {
            None
        };
        let not_ready_reason = not_ready_reason.filter(|reason| {
            *reason != NotReadyReason::UnconsumedInput
                // Being able to clear a Claude draft does not mean it has been
                // consumed. Finishing the last worker must not make that draft ready.
                || providers::draft_composer(host, pane_id, &agent.kind)
                    .is_none_or(|composer| agent.kind == "claude" && !composer.empty)
                || self.claude_steering(pane_id, &agent)
                || self.claude_background_pending(pane_id)
        });
        let lifecycle = self.agent_lifecycle.get(&pane_id).filter(|lifecycle| {
            lifecycle.kind == agent.kind
                && agent.session_id.as_ref() == Some(&lifecycle.conversation)
        });
        Some(AgentStatus {
            pane_id,
            session: host.session_name().to_owned(),
            host_id: yinz_runtime::host_identity::host_id(),
            tab_id: host.tab_for_pane(pane_id),
            is_suppressed: info.is_suppressed,
            program_status: info.program_status.clone(),
            target: info.target,
            cwd,
            branch: self
                .agent_launches
                .get(&pane_id)
                .and_then(|launch| launch.branch.clone()),
            ready: not_ready_reason.is_none(),
            not_ready_reason,
            status_available,
            prompt_supported,
            readiness: Some(readiness),
            input_revision: Some(host.agent_input_revision(pane_id)),
            clear_draft_supported: prompt_supported
                && providers::draft_composer(host, pane_id, &agent.kind).is_some(),
            background: lifecycle.is_some_and(Lifecycle::event_background),
            background_tasks: lifecycle
                .map(|lifecycle| lifecycle.claude.activity.obligations.unresolved_tasks())
                .unwrap_or_default(),
            agent,
        })
    }

    fn agent_readiness(
        &self,
        host: &impl AgentHost,
        pane_id: PaneId,
        agent: &AgentInfo,
        held: bool,
        exited: bool,
    ) -> Option<AgentReadiness> {
        let input = host.pane_input(pane_id)?;
        let lifecycle = self.agent_lifecycle.get(&pane_id);
        let kind = yinz_types::agent::kind_of(&agent.kind);
        let process_matches = kind.is_some() && input.command_kind == kind;
        Some(AgentReadiness {
            kind_supported: supports_prompt(&agent.kind),
            reported: agent.source == AgentSource::Reported,
            lifecycle_kind: lifecycle.map(|lifecycle| lifecycle.kind.clone()),
            lifecycle_session_id: lifecycle.map(|lifecycle| lifecycle.conversation.clone()),
            lifecycle_matches: agent.session_id.as_ref().is_some_and(|conversation| {
                self.has_agent_lifecycle(pane_id, &agent.kind, conversation)
            }),
            provisional_lifecycle: agent.kind == "muse"
                && agent.session_id.is_none()
                && self.has_provisional_muse_lifecycle(pane_id),
            last_report_age_ms: lifecycle.map(|lifecycle| {
                u64::try_from(lifecycle.last_report.elapsed().as_millis()).unwrap_or(u64::MAX)
            }),
            held,
            exited,
            agent_exited: agent.state == AgentState::Exited,
            suspended: input.suspended,
            sync_active: input.sync_active,
            excluded_from_sync: input.excluded_from_sync,
            command_present: input.command_present,
            command_kind: input.command_kind,
            process_matches,
            bracketed_paste: input.bracketed_paste,
        })
    }

    pub(in crate::screen) fn detected_agent_status(
        &self,
        host: &impl AgentHost,
        pane_id: PaneId,
    ) -> Option<AgentStatus> {
        let PaneId::Terminal(id) = pane_id else {
            return None;
        };
        let info = host.pane_info(pane_id)?;
        if info.agent.is_some() || info.is_held || info.exited {
            return None;
        }
        let (program, args) = host.foreground_command(id)?.split_first()?;
        let kind =
            yinz_types::agent::kind_from_program_and_args(std::path::Path::new(program), args)?;
        let cwd = host.terminal_cwd(pane_id);
        let agent = AgentInfo {
            kind: kind.to_owned(),
            state: AgentState::Busy,
            source: AgentSource::Detected,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: cwd.clone(),
            session_id: None,
            run_id: None,
        };
        Some(AgentStatus {
            pane_id,
            session: host.session_name().to_owned(),
            host_id: yinz_runtime::host_identity::host_id(),
            tab_id: host.tab_for_pane(pane_id),
            is_suppressed: info.is_suppressed,
            program_status: info.program_status.clone(),
            target: info.target,
            cwd: cwd.clone(),
            branch: None,
            ready: false,
            not_ready_reason: Some(NotReadyReason::Unmanaged),
            status_available: false,
            prompt_supported: false,
            input_revision: Some(host.agent_input_revision(pane_id)),
            clear_draft_supported: false,
            background: false,
            background_tasks: Vec::new(),
            readiness: self.agent_readiness(host, pane_id, &agent, info.is_held, info.exited),
            agent,
        })
    }

    pub(super) fn claude_steering_composer(&self, host: &impl AgentHost, pane: PaneId) -> bool {
        providers::draft_composer(host, pane, "claude").is_some_and(|c| c.empty)
            && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.input_revision == Some(host.agent_input_revision(pane))
            })
    }
}
