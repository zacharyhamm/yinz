use super::*;
const HOOK_FLASH_DURATION: Duration = Duration::from_millis(150);

pub(in crate::controller) struct HookFlash {
    kind: String,
    session: Option<String>,
    run: Option<String>,
    deadline: Instant,
    cleared: bool,
    palette_flash: bool,
}

impl HookFlash {
    fn matches(&self, agent: &AgentInfo) -> bool {
        !self.cleared
            && self.kind == agent.kind
            && self.session == agent.session_id
            && self.run == agent.run_id
    }
}

pub enum AgentUpdate {
    Missing,
    Unchanged,
    Changed {
        previous: Option<AgentInfo>,
        current: Option<AgentInfo>,
    },
}
impl super::AgentController {
    pub fn stage_agent(
        &mut self,
        host: &mut impl AgentHost,
        pane_id: PaneId,
        agent: Option<AgentInfo>,
        remember_parent: bool,
        preserve_shutdown: bool,
    ) -> AgentUpdate {
        if remember_parent {
            self.claude.clear_permission_wait(pane_id);
        }
        // Read before the write: which hooks a change fires is a question about the edge, and the
        // pane only answers whether anything changed.
        if host.tab_for_pane(pane_id).is_none() {
            return AgentUpdate::Missing;
        }
        let previous = host.pane_agent(pane_id);
        // The one field of a record the reporter does not know. A hook says everything the agent
        // knows about itself and nothing about whose books it is being kept in, so a report that
        // leaves the run id unset inherits the pane's rather than erasing it. Only clearing the
        // record clears the run id with it. Done before the comparison below, so a report that is
        // identical apart from the id it did not supply still costs no hook and no manifest.
        let agent = agent.map(|mut agent| {
            if agent.run_id.is_none() {
                agent.run_id = previous
                    .as_ref()
                    .and_then(|previous| previous.run_id.clone());
            }
            if remember_parent {
                self.remember_claude_parent(pane_id, &agent);
            }
            self.aggregate_claude_state(pane_id, &mut agent);
            agent
        });
        let changed = host.set_agent(pane_id, agent.clone());
        if self
            .agent_hook_flashes
            .get(&pane_id)
            .is_some_and(|flash| agent.as_ref().is_none_or(|agent| !flash.matches(agent)))
        {
            self.agent_hook_flashes.remove(&pane_id);
        }
        if let Some(flash) = self.agent_hook_flashes.get_mut(&pane_id)
            && agent
                .as_ref()
                .is_none_or(|agent| agent.state != AgentState::Busy)
        {
            // A title flash survives a state transition; the palette's busy-only flash does not.
            flash.palette_flash = false;
        }
        // A Muse pane verified at startup binds its conversation with its first turn:
        // the record moves from the bootstrap's empty session to the provider's, and
        // that expected edge must not end the submission that caused it.
        if !providers::muse::binds_session(previous.as_ref(), agent.as_ref())
            && (previous.is_some() || agent.is_some())
            && (previous
                .as_ref()
                .map(|agent| (&agent.kind, &agent.session_id))
                != agent.as_ref().map(|agent| (&agent.kind, &agent.session_id))
                || agent
                    .as_ref()
                    .is_none_or(|agent| agent.state == yinz_types::agent::AgentState::Exited))
        {
            let reason = if agent.is_none() && preserve_shutdown {
                "session_ended"
            } else if agent.is_none() {
                "record_cleared"
            } else if previous.as_ref().map(|agent| &agent.kind)
                != agent.as_ref().map(|agent| &agent.kind)
            {
                "kind_changed"
            } else if previous
                .as_ref()
                .and_then(|agent| agent.session_id.as_ref())
                != agent.as_ref().and_then(|agent| agent.session_id.as_ref())
            {
                "conversation_changed"
            } else {
                "agent_exited"
            };
            self.invalidate_agent_lifecycle(host, pane_id, reason);
        }
        if changed {
            fn worktree_directory(agent: Option<&AgentInfo>) -> Option<&str> {
                agent
                    .filter(|agent| agent.state != AgentState::Exited)?
                    .cwd
                    .as_deref()
            }
            if worktree_directory(previous.as_ref()) != worktree_directory(agent.as_ref()) {
                self.agent_worktrees.refresh_agent_worktrees(host);
            }
            AgentUpdate::Changed {
                previous,
                current: agent,
            }
        } else {
            AgentUpdate::Unchanged
        }
    }
}

impl super::AgentController {
    pub fn pane_title_hook_is_flashing(
        &self,
        pane: &yinz_types::data::PaneInfo,
        now: Instant,
    ) -> bool {
        !pane.exited
            && self.agent_hook_flashes.get(&pane.id).is_some_and(|flash| {
                now < flash.deadline
                    && match pane.agent.as_ref() {
                        Some(agent) => flash.matches(agent),
                        None => flash.cleared,
                    }
            })
    }
    pub fn agent_hook_is_flashing(&self, pane: &yinz_types::data::PaneInfo, now: Instant) -> bool {
        pane.agent
            .as_ref()
            .is_some_and(|agent| agent.state == AgentState::Busy)
            && self
                .agent_hook_flashes
                .get(&pane.id)
                .is_some_and(|flash| flash.palette_flash)
            && self.pane_title_hook_is_flashing(pane, now)
    }
}

impl super::AgentController {
    pub(in crate::controller) fn flash_agent_hook(
        &mut self,
        host: &impl AgentHost,
        now: Instant,
        pane: PaneId,
        kind: &str,
        hook: &yinz_types::agent::HookEvent,
        cleared_agent: Option<AgentInfo>,
    ) {
        let Some(info) = host.pane_info(pane) else {
            return;
        };
        let cleared = info.agent.is_none();
        let Some(agent) = info.agent.or(cleared_agent).filter(|agent| {
            !info.exited
                && agent.kind == kind
                && agent.session_id.as_deref().unwrap_or_default() == hook.session_id
        }) else {
            return;
        };
        self.agent_hook_flashes.insert(
            pane,
            HookFlash {
                palette_flash: !cleared && agent.state == AgentState::Busy,
                kind: agent.kind,
                session: agent.session_id,
                run: agent.run_id,
                deadline: now + HOOK_FLASH_DURATION,
                cleared,
            },
        );
    }

    pub fn reap_agent_hook_flashes(&mut self, host: &impl AgentHost, now: Instant) {
        let stale: Vec<_> = self
            .agent_hook_flashes
            .keys()
            .copied()
            .filter(|pane| {
                host.pane_info(*pane)
                    .is_none_or(|pane| !self.pane_title_hook_is_flashing(&pane, now))
            })
            .collect();
        for pane in stale {
            self.agent_hook_flashes.remove(&pane);
        }
    }

    pub fn flash_panes(&self) -> impl ExactSizeIterator<Item = PaneId> + '_ {
        self.agent_hook_flashes.keys().copied()
    }
    pub fn clear_hook_flash(&mut self, pane: PaneId) {
        self.agent_hook_flashes.remove(&pane);
    }
    pub(in crate::controller) fn record_hook_activity(&mut self, pane: PaneId, at: Instant) {
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.last_report = at;
        }
    }
    pub fn retire_restored_observer(&mut self, pane: PaneId) {
        self.claude.restored_observers.retire(pane);
    }
}

impl super::AgentController {
    pub(in crate::controller) fn apply_agent_report_state(
        &mut self,
        host: &mut impl AgentHost,
        report: &yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<bool> {
        use yinz_types::agent::report::AgentReportState;
        use yinz_types::agent::{hook_state_for, waiting_detail, waiting_needs_a_person};
        let pane = report.pane_id;
        if report.kind == "claude"
            && let Some(applied) = self.apply_claude_permission_report(host, pane, hook)
        {
            return Ok(applied);
        }
        // Only the matching clear lifecycle may carry its submitted input across the
        // record removal and conversation replacement. All other invalidations discard it.
        let clear_transition = hook.is_some_and(|hook| {
            !hook.is_worker()
                && report.event.as_ref().is_some_and(|event| {
                    event.kind == report.kind
                        && PaneId::Terminal(event.pane_id) == pane
                        && event.hook == hook.hook_event_name
                        && event.session_id.as_deref() == Some(hook.session_id.as_str())
                })
                && host.agent_process_matches(pane, &report.kind)
                && host
                    .pane_info(pane)
                    .is_some_and(|info| !info.is_held && !info.exited)
                && self.agent_clear_inputs.get(&pane).is_some_and(|input| {
                    input.kind == report.kind
                        && Instant::now() < input.deadline
                        && ((hook.hook_event_name == "SessionEnd"
                            && hook.session_id == input.conversation)
                            || (input.session_ended
                                && hook.hook_event_name == "SessionStart"
                                && hook.source.as_deref() == Some("clear")
                                && !hook.session_id.is_empty()
                                && hook.session_id != input.conversation))
                })
        });
        let preserve_shutdown = !clear_transition
            && report.kind == "claude"
            && matches!(report.state, AgentReportState::Clear)
            && hook.is_some_and(|hook| {
                hook.hook_event_name == "SessionEnd"
                    && !hook.is_worker()
                    && !hook.archive_only
                    && self.has_agent_lifecycle(pane, "claude", &hook.session_id)
                    && report.event.as_ref().is_some_and(|event| {
                        event.kind == report.kind
                            && PaneId::Terminal(event.pane_id) == pane
                            && event.hook == hook.hook_event_name
                            && event.session_id.as_deref() == Some(hook.session_id.as_str())
                    })
            })
            && self.closed_agent_proof(host, pane).is_some();
        if preserve_shutdown {
            self.retain_closed_agent(host, pane, Instant::now());
        }
        if !clear_transition
            && hook.is_some_and(|hook| {
                !hook.is_worker()
                    && matches!(hook.hook_event_name.as_str(), "SessionStart" | "SessionEnd")
            })
        {
            self.agent_clear_inputs.remove(&pane);
        }
        let input_revision = host.agent_input_revision(pane);
        let mut notification_report = false;
        let mut workers = self.agent_worker_waits.remove(&pane);
        let agent = if let Some(hook) = hook.filter(|hook| hook.is_worker()) {
            let Some(mut parent) = host.pane_agent(pane) else {
                return Ok(true);
            };
            if let Some(raw) = self
                .agent_lifecycle
                .get(&pane)
                .and_then(|l| l.claude.activity.parent.as_ref())
            {
                parent.state = raw.state;
                parent.detail = raw.detail.clone();
                parent.waiting_on = raw.waiting_on.clone();
                parent.wait_needs_a_person = raw.wait_needs_a_person;
            }
            if let Some(workers) = &workers {
                // Only the presentation is overlaid. Keep current run and checkout metadata.
                parent.state = workers.parent.state;
                parent.detail = workers.parent.detail.clone();
                parent.waiting_on = workers.parent.waiting_on.clone();
                parent.wait_needs_a_person = workers.parent.wait_needs_a_person;
            }
            if parent.kind != report.kind
                || parent.session_id.as_deref() != Some(hook.session_id.as_str())
            {
                if let Some(workers) = workers {
                    self.agent_worker_waits.insert(pane, workers);
                }
                return Ok(true);
            }
            if let Some(id) = &hook.agent_id {
                let claude = self
                    .agent_lifecycle
                    .get_mut(&pane)
                    .filter(|l| report.kind == "claude" && l.conversation == hook.session_id)
                    .map(|l| &mut l.claude.activity);
                let release = if let Some(claude) = claude {
                    providers::claude::worker_wait_released(claude, workers.as_ref(), id, hook)
                } else {
                    hook_state_for(&report.kind, hook).is_some()
                        || matches!(hook.hook_event_name.as_str(), "SubagentStop" | "SessionEnd")
                };
                if hook_state_for(&report.kind, hook) == Some(AgentState::Waiting)
                    && waiting_needs_a_person(hook)
                {
                    workers
                        .get_or_insert_with(|| WorkerWaits {
                            parent: parent.clone(),
                            waits: Default::default(),
                        })
                        .waits
                        .insert(id.clone(), waiting_detail(hook));
                } else if release && let Some(workers) = &mut workers {
                    workers.waits.remove(id);
                }
            }
            Some(parent)
        } else {
            match &report.state {
                AgentReportState::Unchanged => {
                    if let Some(workers) = workers {
                        self.agent_worker_waits.insert(pane, workers);
                    }
                    return Ok(true);
                },
                AgentReportState::Clear => None,
                AgentReportState::Set(agent) => self.claude_report_parent(
                    pane,
                    &report.kind,
                    hook,
                    agent,
                    input_revision,
                    &mut notification_report,
                ),
            }
        };
        if agent.is_none() {
            workers = None;
        }
        let permission_wait = self.claude.report_permission_wait(
            host,
            pane,
            &report.kind,
            hook,
            agent.as_ref(),
            notification_report,
        );
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            notifications::prune_released_notification_waits(lifecycle);
        }
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|l| {
            report.kind == "claude"
                && l.kind == "claude"
                && agent
                    .as_ref()
                    .is_some_and(|a| a.session_id.as_ref() == Some(&l.conversation))
        }) {
            lifecycle.claude.activity.parent = agent.clone();
        }
        let mut visible = agent.clone();
        if let (Some(workers), Some(parent)) = (&mut workers, agent) {
            if workers.parent.kind == parent.kind
                && workers.parent.session_id == parent.session_id
                && parent.state != AgentState::Exited
                && !workers.waits.is_empty()
            {
                workers.parent = parent.clone();
                let mut waiting = parent;
                waiting.state = AgentState::Waiting;
                waiting.wait_needs_a_person = true;
                waiting.waiting_on = workers.waits.values().next().cloned().flatten();
                waiting.detail = None;
                visible = Some(waiting);
            } else {
                workers.waits.clear();
            }
        }
        // The first verified startup changes the record's conversation and invalidates its
        // lifecycle. Carry only its CLI dialog input through that initial transition.
        let startup_input = hook
            .filter(|hook| {
                !hook.is_worker()
                    && hook.codex_hooks_ready != Some(false)
                    && self.agent_dialog_inputs.get(&pane).is_some_and(|input| {
                        input.startup
                            && input.kind == report.kind
                            && input
                                .conversation
                                .as_deref()
                                .is_none_or(|conversation| conversation == hook.session_id)
                    })
                    && ((hook_state_for(&report.kind, hook) == Some(AgentState::Idle)
                        && hook.hook_event_name == "SessionStart")
                        || (report.kind == "agy"
                            && matches!(
                                hook.hook_event_name.as_str(),
                                "UserPromptSubmit" | "PreInvocation"
                            )))
            })
            .and_then(|_| self.agent_dialog_inputs.remove(&pane));
        let clear_input = clear_transition
            .then(|| self.agent_clear_inputs.remove(&pane))
            .flatten();
        if !preserve_shutdown {
            self.claude.clear_permission_wait(pane);
        }
        let result = !matches!(
            self.stage_agent(
                host,
                pane,
                if preserve_shutdown { None } else { visible },
                false,
                preserve_shutdown
            ),
            reports::AgentUpdate::Missing
        );
        if result {
            self.claude.restore_permission_wait(pane, permission_wait);
        }
        if let Some(input) = startup_input {
            self.agent_dialog_inputs.insert(pane, input);
        }
        if let Some(input) = clear_input {
            self.agent_clear_inputs.insert(pane, input);
        }
        if let Some(workers) = workers.filter(|workers| !workers.waits.is_empty()) {
            self.agent_worker_waits.insert(pane, workers);
        }
        Ok(result)
    }
}

pub(in crate::controller) enum ReportDecision {
    Apply,
    Ignore,
    Archive(AgentEvent),
}
