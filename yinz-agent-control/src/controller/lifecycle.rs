use super::*;

impl super::AgentController {
    pub(super) fn observe_control_hook(
        &mut self,
        host: &impl AgentHost,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> bool {
        if hook.is_some_and(|hook| hook.archive_only) {
            return false;
        }
        let pane = PaneId::Terminal(event.pane_id);
        let parent = hook.is_some_and(|hook| !hook.is_worker());
        let claude_notification_stop = self.observe_claude_confirmation(pane, event, hook, parent);
        self.observe_agy_confirmation(pane, event, hook, parent);
        let mut clear_revision = None;
        let clear_started = host.pane_info(pane).is_some_and(|info| {
            info.agent.is_some_and(|agent| {
                agent.source == AgentSource::Reported
                    && agent.kind == event.kind
                    && agent.session_id == event.session_id
            })
        });
        if parent
            && let Some(input) = self.agent_clear_inputs.get_mut(&pane)
            && event.kind == input.kind
            && hook
                .is_some_and(|hook| event.session_id.as_deref() == Some(hook.session_id.as_str()))
        {
            if event.hook == "SessionEnd" {
                input.session_ended = true;
            } else if event.hook == "SessionStart" && clear_started {
                clear_revision = Some(input.input_revision);
            }
        }
        if clear_revision.is_some() {
            self.agent_clear_inputs.remove(&pane);
        }
        // Accepted nonpositive registry observations describe presentation, not a
        // new lifecycle hook. Real hooks and positive input evidence retain the
        // recovery fence, including when they carry registry enrichment.
        let passive_permission = passive_claude_permission(event, hook);
        if !passive_permission && let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.last_report = Instant::now();
        }
        let notification = parent
            && hook.is_some_and(|hook| self.agent_task_notification(pane, &event.kind, hook));
        if let Some(hook) = hook.filter(|_| parent && !notification) {
            self.track_agent_turn(pane, event, hook);
        }
        if parent
            && event.hook == "SessionStart"
            && hook.is_some_and(|hook| hook.source.as_deref() != Some("compact"))
            && self
                .agent_dialog_inputs
                .get(&pane)
                .is_some_and(|input| !input.startup)
        {
            self.agent_dialog_inputs.remove(&pane);
        }
        // Reconcile before replying to completion, so both prompt and wait replies see the
        // consumed dialog revision. A newer manual draft is never consumed with it.
        let mut dialog_revision = None;
        if let Some(input) = self.agent_dialog_inputs.get_mut(&pane)
            && event.kind == input.kind
            && hook.is_some_and(|hook| {
                hook.claude_recovered_after.is_none()
                    && event.session_id.as_deref() == Some(hook.session_id.as_str())
                    && !hook.session_id.is_empty()
                    && hook.codex_hooks_ready != Some(false)
                    && input
                        .conversation
                        .as_deref()
                        .is_none_or(|conversation| conversation == hook.session_id)
            })
        {
            let state = hook.and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook));
            if state == Some(AgentState::Waiting) {
                input.fresh_waiting = true;
            } else if (input.startup
                && parent
                && ((event.hook == "SessionStart" && state == Some(AgentState::Idle))
                    || (input.kind == "agy"
                        && matches!(event.hook.as_str(), "UserPromptSubmit" | "PreInvocation")
                        && matches!(state, Some(AgentState::Busy | AgentState::Idle)))))
                || (!input.startup
                    && parent
                    && event.hook != "SessionStart"
                    && matches!(state, Some(AgentState::Busy | AgentState::Idle)))
            {
                dialog_revision = Some(input.input_revision);
            }
        }
        if let Some(revision) = dialog_revision {
            self.agent_dialog_inputs.remove(&pane);
            if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
                lifecycle.input_revision = Some(revision);
            }
        }
        let accepted_input_revision = hook
            .filter(|_| parent && event.hook == "UserPromptSubmit")
            .and_then(|hook| {
                self.agent_controls.iter().find(|pending| {
                    pending.pane == pane
                        && pending.kind == event.kind
                        && pending.prompt.as_ref().is_some_and(|p| {
                            p.matches_submission(hook)
                                && (!p.acknowledged() || p.acknowledges(hook))
                        })
                })
            })
            .map(|pending| pending.input_revision);
        if parent {
            log::debug!(
            "agent lifecycle hook session={:?} pane={pane:?} kind={:?} conversation={:?} hook={:?} state={:?}",
            host.session_name(), event.kind, event.session_id, event.hook,
            hook.and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook)),
        );
            if hook.is_some_and(|hook| {
                hook.codex_hooks_ready == Some(false) || hook.muse_hooks_ready == Some(false)
            }) {
                self.remove_agent_lifecycle(host, pane, "hooks_unverified");
                self.agent_dialog_inputs.remove(&pane);
            } else if event.hook == "SessionEnd" {
                self.remove_agent_lifecycle(host, pane, "session_end");
                self.agent_dialog_inputs.remove(&pane);
                self.agent_worker_waits.remove(&pane);
            } else if providers::muse::provisional_start(event, hook) {
                self.bind_provisional_muse(host, pane, clear_revision.or(dialog_revision));
            } else if let Some(conversation) = event.session_id.as_ref().filter(|id| {
                !id.is_empty()
                    && hook
                        .as_ref()
                        .and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook))
                        .is_some()
            }) {
                // Some(None) means a transferred draft: even a startup report cannot
                // treat the destination's zero input revision as an empty source composer.
                let transferred = self.warped_agent_inputs.remove(&pane).map(|input| {
                    (input.kind == event.kind
                        && (input.conversation.is_empty() || input.conversation == *conversation)
                        && input.consumed == Some(true)
                        && input.input_revision == host.agent_input_revision(pane))
                    .then_some(input.input_revision)
                });
                let previous = self.agent_lifecycle.get(&pane).filter(|previous| {
                    previous.kind == event.kind && previous.conversation == *conversation
                });
                if previous.is_none() {
                    log::debug!(
                    "agent lifecycle created session={:?} pane={pane:?} kind={:?} conversation={conversation:?}",
                    host.session_name(), event.kind,
                );
                }
                // Without tracked CLI dialog input, startup cannot consume a draft. Only a pane
                // with no input can establish consumption; matching evidence is preserved as-is.
                let input_revision = clear_revision
                    .or(dialog_revision)
                    .or(if event.hook == "UserPromptSubmit" && !notification {
                        accepted_input_revision
                    } else {
                        None
                    })
                    .or(if previous.is_none() {
                        transferred.flatten()
                    } else {
                        None
                    })
                    .or(match event.hook.as_str() {
                        "SessionStart" if transferred.is_some() && previous.is_none() => {
                            transferred.flatten()
                        },
                        "SessionStart" => previous
                            .map(|previous| previous.input_revision)
                            .unwrap_or_else(|| (host.agent_input_revision(pane) == 0).then_some(0)),
                        // Claude starts a notification turn by itself; it consumes no input.
                        "UserPromptSubmit" if !notification => {
                            accepted_input_revision.or_else(|| {
                                providers::agy::initial_prompt_revision(
                                    &event.kind,
                                    transferred,
                                    previous,
                                    host,
                                    pane,
                                )
                            })
                        },
                        "PreInvocation" if event.kind == "agy" => {
                            providers::agy::pre_invocation_revision(
                                accepted_input_revision,
                                transferred,
                                previous,
                                host,
                                pane,
                            )
                        },
                        // An uncorrelated acceptance may arrive after a newer draft. Tool progress
                        // and completion preserve consumption, including when it is unknown.
                        _ => previous.and_then(|previous| previous.input_revision),
                    });
                let ended_turn = match event.hook.as_str() {
                    "Interrupt" | "StopFailure" | "Stop" if event.kind == "claude" => {
                        providers::claude::ended_turn(previous, &event.hook, hook)
                    },
                    "Interrupt" | "StopFailure" => hook
                        .and_then(|h| h.turn_id.clone())
                        .filter(|id| !id.is_empty()),
                    "UserPromptSubmit" if !notification => None,
                    "PreInvocation" => None,
                    _ => previous.and_then(|previous| previous.ended_turn.clone()),
                };
                self.agent_lifecycle.insert(
                    pane,
                    Lifecycle {
                        kind: event.kind.clone(),
                        conversation: conversation.clone(),
                        input_revision,
                        last_report: Instant::now(),
                        ended_turn,
                        closeout_turn: if event.hook == "UserPromptSubmit" && !notification {
                            hook.and_then(|hook| {
                                Provider::for_kind(&event.kind)
                                    .closeout_id(hook.turn_id.as_deref(), hook.prompt_id.as_deref())
                            })
                            .map(str::to_owned)
                            .filter(|id| !id.is_empty())
                        } else {
                            previous.and_then(|previous| previous.closeout_turn.clone())
                        },
                        last_message: previous.and_then(|previous| previous.last_message.clone()),
                        prompt_id: if event.hook == "UserPromptSubmit" && !notification {
                            hook.and_then(|hook| hook.prompt_id.clone())
                        } else {
                            previous.and_then(|previous| previous.prompt_id.clone())
                        },
                        previous_prompt_id: providers::agy::previous_prompt_id(
                            previous,
                            event,
                            notification,
                        ),
                        claude: providers::ClaudeLifecycle::after_hook(
                            previous,
                            event,
                            hook,
                            notification,
                            claude_notification_stop,
                        ),
                        agy: previous.map(|l| l.agy).unwrap_or_default().after_hook(
                            &event.kind,
                            &event.hook,
                            hook,
                        ),
                    },
                );
            }
            // An adopted pane has no lifecycle until its first stateful hook. Register a
            // notification after creating that lifecycle so its confirmation can be matched.
            if let Some(hook) = hook.filter(|_| notification || event.hook == "UserPromptSubmit") {
                self.track_agent_turn(pane, event, hook);
            }
        }
        true
    }
}

impl super::AgentController {
    /// Carry a plugin's run binding into the presentation snapshots kept for `agent`, so a
    /// later republish of either snapshot neither restores an old run nor revives an unbound one.
    pub fn rebind_agent_parent_runs(&mut self, pane: PaneId, agent: &AgentInfo) {
        let same =
            |parent: &AgentInfo| parent.kind == agent.kind && parent.session_id == agent.session_id;
        if let Some(parent) = self
            .agent_lifecycle
            .get_mut(&pane)
            .and_then(|l| l.claude.activity.parent.as_mut())
            .filter(|parent| same(parent))
        {
            parent.run_id = agent.run_id.clone();
        }
        if let Some(workers) = self
            .agent_worker_waits
            .get_mut(&pane)
            .filter(|w| same(&w.parent))
        {
            workers.parent.run_id = agent.run_id.clone();
        }
    }
}

impl super::AgentController {
    /// Whether this hook is work from a turn that already ended by interruption or failure,
    /// such as the tool call Codex finishes after an Esc. Hooks without a turn id never are.
    pub(in crate::controller) fn is_after_ended_turn(
        &self,
        pane: PaneId,
        kind: &str,
        hook: &HookEvent,
    ) -> bool {
        !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "PreInvocation" | "SessionStart" | "SessionEnd"
        ) && matches!(
            yinz_types::agent::hook_state_for(kind, hook),
            Some(AgentState::Busy | AgentState::Waiting)
        ) && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind
                && lifecycle.conversation == hook.session_id
                && lifecycle.ended_turn.is_some()
                && lifecycle.ended_turn == hook.turn_id
        })
    }
}

impl super::AgentController {
    /// Track Claude notification turns and completed turn text. Completions must be tracked
    /// before pending operations are answered; notifications need an initialized lifecycle.
    pub(in crate::controller) fn track_agent_turn(
        &mut self,
        pane: PaneId,
        event: &AgentEvent,
        hook: &HookEvent,
    ) {
        let notification = self.agent_task_notification(pane, &event.kind, hook);
        let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|lifecycle| {
            lifecycle.kind == event.kind
                && lifecycle.conversation == hook.session_id
                && event.session_id.as_deref() == Some(hook.session_id.as_str())
        }) else {
            return;
        };
        if event.kind == "claude" {
            providers::claude::track_turn(lifecycle, event, hook, notification);
        } else {
            match event.hook.as_str() {
                "UserPromptSubmit" => lifecycle.last_message = None,
                "Stop" | "StopFailure" => lifecycle.last_message = event.last_message.clone(),
                _ => {},
            }
        }
    }
}

impl super::AgentController {
    pub fn invalidate_agent_lifecycle(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        reason: &str,
    ) {
        if let Some(operation) = self.agent_draft_clears.get_mut(&pane) {
            operation.failure = Some((
                AgentOutcome::TargetChanged,
                "agent identity invalidated".into(),
            ));
        }
        if matches!(
            reason,
            "pane_closed" | "held" | "agent_cleared" | "agent_exited"
        ) {
            self.retain_closed_agent(host, pane, Instant::now());
        } else if reason != "session_ended" {
            self.closed_agent_submissions
                .retain(|(_, proof, _, _)| proof.pane_id != pane);
        }
        self.agent_terminal_claims.remove(&pane);
        self.remove_agent_lifecycle(host, pane, reason);
        self.agent_dialog_inputs.remove(&pane);
        self.agent_clear_inputs.remove(&pane);
        self.agent_worker_waits.remove(&pane);
        self.claude.permission_waits.remove(&pane);
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.agent_controls) {
            // A command may reset/switch the conversation before its submit-write receipt.
            // No further input is allowed; retain only the delivery wait across this edge.
            if pending.pane == pane && pending.command_enter_sent {
                keep.push(pending);
                continue;
            }
            if pending.pane == pane && pending.submission_id.is_some() {
                self.finish_agent_control(
                    host,
                    &pending,
                    if host.pane_was_destroyed(pane) {
                        AgentOutcome::TargetClosed
                    } else {
                        AgentOutcome::TargetChanged
                    },
                    None,
                    None,
                );
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        if let Some(submissions) = self.agent_submissions.get_mut(&pane) {
            submissions.retain(|submission| {
                self.agent_controls.iter().any(|pending| {
                    pending.pane == pane
                        && pending.command_enter_sent
                        && pending.submission_id.as_deref() == Some(submission.id.as_str())
                })
            });
            if submissions.is_empty() {
                self.agent_submissions.remove(&pane);
            }
        }
    }
}

impl super::AgentController {
    pub(in crate::controller) fn remove_agent_lifecycle(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        reason: &str,
    ) {
        self.warped_agent_inputs.remove(&pane);
        self.claude.restored_observers.retire(pane);
        let previous = self.agent_lifecycle.remove(&pane);
        log::debug!(
            "agent lifecycle removed session={:?} pane={pane:?} reason={reason} previous={:?}",
            host.session_name(),
            previous
                .as_ref()
                .map(|record| (&record.kind, &record.conversation)),
        );
    }
}

impl super::AgentController {
    pub(super) fn has_agent_lifecycle(&self, pane: PaneId, kind: &str, conversation: &str) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind && lifecycle.conversation == conversation
        })
    }
}
