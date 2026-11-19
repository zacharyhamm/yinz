use super::super::{reports::ReportDecision, *};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::screen::agent_control) struct ClaudeLifecycle {
    pub(in crate::screen::agent_control) activity: ClaudeActivity,
    pub(in crate::screen::agent_control) observed_at: Instant,
    pub(in crate::screen::agent_control) confirmation: Option<bool>,
    /// Prompt whose confirmed turn left background work running.
    pub(in crate::screen::agent_control) background: Option<String>,
    /// Parallel notification turns keyed by prompt ID.
    pub(in crate::screen::agent_control) notifications: BTreeMap<String, ClaudeNotification>,
}

impl super::super::AgentController {
    pub(in crate::screen) fn ignore_claude_confirmation(
        &self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        if hook.is_worker() {
            return false;
        }
        if hook.claude_turn_complete.is_some() {
            return !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && hook.prompt_id.as_ref().is_some_and(|prompt| {
                        lifecycle
                            .claude
                            .activity
                            .turns
                            .get(prompt)
                            .is_some_and(|turn| {
                                turn.confirmation.is_none()
                                    || (turn.confirmation == Some(false)
                                        && (hook.claude_turn_complete == Some(true)
                                            || hook.is_claude_interruption()))
                            })
                    })
            });
        }
        if hook.hook_event_name == "UserPromptSubmit" {
            return self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && hook
                        .prompt_id
                        .as_ref()
                        .is_some_and(|prompt| lifecycle.claude.activity.turns.contains_key(prompt))
            });
        }
        hook.hook_event_name == "Stop"
            && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && lifecycle.claude.confirmation == Some(true)
            })
    }
}

impl super::super::AgentController {
    /// Recovery may extend only the current conversation's accepted prompt chain.
    /// Check before state, input consumption, or event publication can be changed.
    pub(in crate::screen) fn ignore_claude_recovery(&self, pane: PaneId, hook: &HookEvent) -> bool {
        let Some(predecessor) = hook.claude_recovered_after.as_deref() else {
            return false;
        };
        if hook.hook_event_name != "UserPromptSubmit" {
            return false; // Confirmations use the ordinary per-prompt guard.
        }
        let reason = match self.agent_lifecycle.get(&pane) {
            _ if hook.is_worker()
                || hook.session_id.trim().is_empty()
                || predecessor.trim().is_empty()
                || hook
                    .prompt_id
                    .as_deref()
                    .is_none_or(|id| id.trim().is_empty()) =>
            {
                "invalid_identity"
            },
            Some(lifecycle)
                if lifecycle.kind == "claude" && lifecycle.conversation == hook.session_id =>
            {
                if hook
                    .prompt_id
                    .as_ref()
                    .is_some_and(|id| lifecycle.claude.activity.turns.contains_key(id))
                {
                    "duplicate"
                } else if lifecycle.prompt_id.as_deref() != Some(predecessor) {
                    "predecessor_mismatch"
                } else {
                    "accepted"
                }
            },
            _ => "conversation_mismatch",
        };
        let bounded = |id: &str| id.chars().take(128).collect::<String>();
        log::debug!(
            "Claude prompt recovery pane={pane:?} conversation={:?} prompt={:?} predecessor={:?} reason={reason}",
            bounded(&hook.session_id), bounded(hook.prompt_id.as_deref().unwrap_or_default()), bounded(predecessor),
        );
        reason != "accepted"
    }
}

impl super::super::AgentController {
    /// Native terminal hooks omit prompt IDs. Resolve them once before either submission or
    /// lifecycle-event observers see the hook; older overlapping turns must stay open.
    pub(in crate::screen) fn correlate_claude_terminal_hook(
        &self,
        pane: PaneId,
        hook: &mut HookEvent,
    ) {
        if matches!(hook.hook_event_name.as_str(), "StopFailure" | "Interrupt")
            && !hook.is_worker()
            && hook.prompt_id.as_deref().is_none_or(str::is_empty)
            && let Some(lifecycle) = self
                .agent_lifecycle
                .get(&pane)
                .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
        {
            hook.prompt_id = lifecycle.prompt_id.clone().filter(|id| !id.is_empty());
        }
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn ignore_claude_worker(&self, pane: PaneId, hook: &HookEvent) -> bool {
        hook.is_worker()
            && !self.agent_lifecycle.get(&pane).is_some_and(|l| {
                l.kind == "claude"
                    && l.conversation == hook.session_id
                    && !hook.archive_only
                    && hook.agent_id.as_ref().is_some_and(|id| {
                        !id.is_empty()
                            // A release can arrive after execution completion. Tool identity,
                            // not the worker's stopped flag, determines which wait it releases.
                            && !(yinz_types::agent::hook_state_for("claude", hook) == Some(AgentState::Waiting)
                                && hook.tool_use_id.as_ref().is_some_and(|tool| {
                                    l.claude.activity.released_worker_tools.get(id).is_some_and(|tools| tools.contains(tool))
                                }))
                    })
            })
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn remember_claude_parent(&mut self, pane: PaneId, agent: &AgentInfo) {
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|l| {
            agent.kind == "claude"
                && l.kind == "claude"
                && agent.session_id.as_ref() == Some(&l.conversation)
        }) {
            lifecycle.claude.activity.parent = Some(agent.clone());
        }
    }
}

impl super::super::AgentController {
    /// Claude background work an idle wait must still see finish.
    pub(in crate::screen) fn claude_background_pending(&self, pane: PaneId) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == "claude"
                && (lifecycle.claude.background.is_some()
                    || lifecycle.claude.activity.obligations.active()
                    || !lifecycle.claude.notifications.is_empty())
        })
    }
}

impl super::super::AgentController {
    /// Record Claude's registry background status before a confirmation's state is applied,
    /// so the confirmed parent's Idle is aggregated with it. A turn confirmation replaces the
    /// status of every older turn; only that turn's own background watch can report it idle.
    pub(in crate::screen) fn track_claude_background(&mut self, pane: PaneId, hook: &HookEvent) {
        let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|l| {
            l.kind == "claude"
                && l.conversation == hook.session_id
                && !hook.is_worker()
                && !hook.archive_only
        }) else {
            return;
        };
        let Some(prompt) = hook.prompt_id.as_ref().filter(|id| !id.is_empty()) else {
            return;
        };
        if hook.hook_event_name == yinz_types::agent::CLAUDE_BACKGROUND_IDLE {
            if lifecycle.claude.background.as_ref() == Some(prompt) {
                lifecycle.claude.background = None;
            }
        } else if hook.hook_event_name == "Stop"
            && (hook.claude_turn_complete == Some(true)
                || (hook.is_claude_interruption() && hook.claude_background.is_some()))
            && lifecycle
                .claude
                .activity
                .turns
                .get(prompt)
                .is_some_and(|turn| turn.sequence >= lifecycle.claude.activity.message_sequence)
        {
            lifecycle.claude.background =
                (hook.claude_background == Some(true)).then(|| prompt.clone());
        }
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn advance_claude_observers(&mut self, pane: PaneId, from: u64, to: u64) {
        for pending in &mut self.agent_controls {
            if pending.pane == pane
                && pending.kind == "claude"
                && pending.input_revision == from
                && pending
                    .prompt
                    .as_ref()
                    .is_some_and(PromptObservation::acknowledged)
            {
                pending.input_revision = to;
            }
        }
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn claude_steering(&self, pane: PaneId, agent: &AgentInfo) -> bool {
        agent.kind == "claude"
            && (agent.state == AgentState::Busy
                || self.agent_controls.iter().any(|pending| {
                    pending.pane == pane
                        && pending
                            .prompt
                            .as_ref()
                            .is_some_and(PromptObservation::acknowledged)
                }))
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn aggregate_claude_state(&self, pane: PaneId, agent: &mut AgentInfo) {
        if agent.kind == "claude"
            && agent.state == AgentState::Idle
            && agent
                .session_id
                .as_deref()
                .is_some_and(|conversation| self.has_agent_lifecycle(pane, "claude", conversation))
            && self.claude_background_pending(pane)
        {
            agent.state = AgentState::Busy;
        }
        self.overlay_claude_permission(pane, agent);
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn reconcile_claude_parent(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
    ) {
        if self
            .agent_lifecycle
            .get(&pane)
            .is_some_and(|l| l.claude.activity.permission.is_some())
        {
            if let Some(visible) = self.claude_visible(host, pane) {
                host.set_agent(pane, Some(visible));
            }
            return;
        }
        let Some(mut visible) = host.pane_info(pane).and_then(|p| p.agent) else {
            return;
        };
        let Some(lifecycle) = self
            .agent_lifecycle
            .get_mut(&pane)
            .filter(|l| l.kind == "claude" && visible.session_id.as_ref() == Some(&l.conversation))
        else {
            return;
        };
        let parent = lifecycle.claude.activity.parent.get_or_insert_with(|| {
            let mut parent = self
                .agent_worker_waits
                .get(&pane)
                .map_or_else(|| visible.clone(), |w| w.parent.clone());
            if lifecycle.prompt_id.is_none() && !lifecycle.claude.notifications.is_empty() {
                parent.state = AgentState::Idle;
                parent.detail = None;
            }
            parent
        });
        if visible.state != AgentState::Waiting {
            visible.state = parent.state;
            visible.detail = parent.detail.clone();
            visible.waiting_on = parent.waiting_on.clone();
            visible.wait_needs_a_person = parent.wait_needs_a_person;
            if visible.state == AgentState::Idle
                && (lifecycle.claude.background.is_some()
                    || lifecycle.claude.activity.obligations.active()
                    || !lifecycle.claude.notifications.is_empty())
            {
                visible.state = AgentState::Busy;
            }
        }
        if visible.state != AgentState::Waiting
            && let Some(waiting) = lifecycle
                .claude
                .notifications
                .values()
                .find_map(|n| n.waiting.as_ref())
        {
            visible.state = waiting.state;
            visible.detail = waiting.detail.clone();
            visible.waiting_on = waiting.waiting_on.clone();
            visible.wait_needs_a_person = waiting.wait_needs_a_person;
        }
        host.set_agent(pane, Some(visible));
    }
}

impl super::super::AgentController {
    pub(in crate::screen) fn agent_task_notification(
        &self,
        pane: PaneId,
        kind: &str,
        hook: &HookEvent,
    ) -> bool {
        is_claude_task_notification(kind, hook)
            && !self.agent_controls.iter().any(|pending| {
                pending.pane == pane
                    && pending.kind == kind
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.matches_submission(hook))
            })
            && !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == kind
                    && lifecycle.conversation == hook.session_id
                    && hook.prompt_id.is_some()
                    && lifecycle.prompt_id == hook.prompt_id
            })
    }
}

#[derive(Default)]
pub(in crate::screen) struct ClaudeState {
    pub(in crate::screen) permission_waits: HashMap<PaneId, PermissionWait>,
    pub(in crate::screen::agent_control) restored_observers:
        yinz_agent_control::providers::claude_observer::RestoredObservers,
}

impl AgentController {
    pub(in crate::screen) fn admit_claude_report(
        &mut self,
        host: &impl AgentHost,
        report: &mut yinz_types::agent::report::AgentReport,
        hook: &mut HookEvent,
    ) -> Result<bool> {
        let pane_id = report.pane_id;
        if !self.accept_claude_source(host, pane_id, hook) {
            return Ok(false);
        }
        if !self.accept_claude_permission(host, pane_id, hook) {
            return Ok(false);
        }
        if let Some(event) = &mut report.event {
            let mut raw: serde_json::Value =
                serde_json::from_str(&event.raw).context("parse permission report")?;
            if hook.claude_permission.is_some() || raw.get("_yinz_claude_permission").is_some() {
                raw["_yinz_claude_permission"] = serde_json::to_value(&hook.claude_permission)
                    .context("normalize permission sample")?;
                *event = yinz_types::agent::AgentEvent::from_hook(
                    "claude",
                    event.pane_id,
                    hook,
                    serde_json::to_string(&raw)
                        .context("serialize normalized permission report")?,
                );
            }
        }
        if hook.hook_event_name == yinz_types::agent::claude_permissions::EVENT
            || (hook.hook_event_name == "Notification"
                && hook.notification_type.as_deref() == Some("permission_prompt")
                && hook.claude_permission.as_ref().is_some_and(|s| {
                    s.status != yinz_types::agent::claude_permissions::Status::Unavailable
                }))
        {
            report.state = yinz_types::agent::report::AgentReportState::Unchanged;
        }
        self.correlate_claude_terminal_hook(pane_id, hook);
        Ok(true)
    }
}

pub(in crate::screen) fn matches_report(
    report: &yinz_types::agent::report::AgentReport,
    hook: &HookEvent,
) -> bool {
    let pane_id = report.pane_id;
    report.kind == "claude"
        && report.event.as_ref().is_some_and(|event| {
            event.kind == "claude"
                && PaneId::Terminal(event.pane_id) == pane_id
                && event.hook == hook.hook_event_name
                && event.session_id.as_deref() == Some(hook.session_id.as_str())
        })
}

impl AgentController {
    pub(in crate::screen) fn filter_claude_report(
        &mut self,
        host: &impl AgentHost,
        report: &yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<ReportDecision> {
        let pane_id = report.pane_id;
        if let Some(hook) = hook.filter(|hook| hook.hook_event_name == "SubagentStop") {
            // Worker Stops are useful archive evidence but cannot identify which execution
            // ended; Claude's session registry decides background activity. Never pass them
            // through activity or worker-wait reconciliation.
            if self.has_agent_lifecycle(pane_id, "claude", &hook.session_id)
                && hook.agent_id.as_deref().is_some_and(|id| !id.is_empty())
                && report.event.as_ref().is_some_and(|event| {
                    event.kind == "claude" && event.session_id.as_deref() == Some(&hook.session_id)
                })
            {
                if !hook.archive_only {
                    self.record_hook_activity(pane_id, Instant::now());
                }
                if let Some(event) = &report.event {
                    let mut archived = hook.clone();
                    archived.archive_only = true;
                    let mut event = event.clone();
                    event.state = None;
                    event.raw = archived.broadcast_json(
                        serde_json::to_string(&archived)
                            .context("serialize archive-only Claude worker Stop")?,
                    );
                    return Ok(ReportDecision::Archive(event));
                }
            }
            return Ok(ReportDecision::Ignore);
        }
        if let Some(hook) = hook.filter(|hook| {
            !hook.is_worker()
                && !hook.archive_only
                && hook.hook_event_name == "UserPromptSubmit"
                && hook.claude_recovered_after.is_none()
                && self.ignore_claude_confirmation(pane_id, hook)
        }) && let Some(event) = &report.event
        {
            self.correlate_recovered_claude_submit(host, event, hook);
            return Ok(ReportDecision::Ignore);
        }
        // Older installed reporters can still send Idle for an ordinary Claude or Antigravity Stop.
        // Correlate internal confirmations before applying or broadcasting any state.
        if hook.is_some_and(|hook| {
            self.ignore_claude_recovery(pane_id, hook)
                || self.ignore_claude_confirmation(pane_id, hook)
                || self.ignore_claude_worker(pane_id, hook)
        }) {
            log::debug!(
                "agent report ignored report={:?} reason=claude_confirmation",
                report.report_id
            );
            return Ok(ReportDecision::Ignore);
        }
        Ok(ReportDecision::Apply)
    }
}

impl AgentController {
    pub(in crate::screen) fn normalize_claude_state(
        &self,
        host: &impl AgentHost,
        report: &mut yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> bool {
        let pane_id = report.pane_id;
        let recovered_wait = hook
            .as_ref()
            .filter(|hook| {
                hook.claude_recovered_after.is_some() && hook.hook_event_name == "UserPromptSubmit"
            })
            .and_then(|_| host.pane_info(pane_id).and_then(|info| info.agent))
            .filter(|agent| agent.state == AgentState::Waiting);
        if let Some(waiting) = recovered_wait {
            report.state = yinz_types::agent::report::AgentReportState::Set(waiting);
            if let Some(event) = &mut report.event {
                event.state = Some(AgentState::Waiting);
            }
            return true;
        } else if hook
            .as_ref()
            .is_some_and(|hook| self.preserve_claude_start_state(pane_id, hook))
        {
            // Resuming the current process proves no foreground turn or native wait
            // ended. A replacement has already retired its old parent before this point.
            report.state = yinz_types::agent::report::AgentReportState::Unchanged;
            if let Some(event) = &mut report.event {
                event.state = None;
            }
            return true;
        }
        false
    }
}
