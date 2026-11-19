//! Registry presentation has its own ownership, watermark and observation lease.
use super::*;
use yinz_types::agent::claude_permissions::{Overlay, Status, DETAIL, EVENT, LEASE_MS};

pub(in crate::screen) fn owned_notification(hook: &HookEvent) -> bool {
    hook.hook_event_name == "Notification"
        && hook.notification_type.as_deref() == Some("permission_prompt")
        && hook
            .claude_permission
            .as_ref()
            .is_some_and(|sample| sample.status != Status::Unavailable)
}

impl super::AgentController {
    pub(in crate::screen) fn overlay_claude_permission(&self, pane: PaneId, agent: &mut AgentInfo) {
        if agent.kind == "claude"
            && agent.state != AgentState::Exited
            && self.agent_lifecycle.get(&pane).is_some_and(|l| {
                agent.session_id.as_ref() == Some(&l.conversation)
                    && l.claude
                        .activity
                        .permission
                        .as_ref()
                        .is_some_and(Overlay::active)
            })
        {
            if agent.state != AgentState::Waiting || !agent.wait_needs_a_person {
                agent.detail = None;
                agent.waiting_on = Some(DETAIL.into());
            }
            agent.state = AgentState::Waiting;
            agent.wait_needs_a_person = true;
        }
    }
    pub(in crate::screen) fn release_claude_permission(&mut self, pane: PaneId) {
        if let Some(overlay) = self
            .agent_lifecycle
            .get_mut(&pane)
            .and_then(|l| l.claude.activity.permission.as_mut())
        {
            overlay.release();
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn accept_claude_permission(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        hook: &mut HookEvent,
    ) -> bool {
        let synthetic = hook.hook_event_name == EVENT;
        let Some(sample) = hook.claude_permission.as_ref() else {
            return !synthetic;
        };
        let valid = !hook.archive_only
            && sample.source.pid != 0
            && sample.source.started != 0
            && host.agent_process_matches(pane, "claude")
            && host.pane_info(pane).is_some_and(|info| {
                !info.is_held
                    && !info.exited
                    && info.target == yinz_types::command::PaneTarget::Local
                    && info.agent.as_ref().is_some_and(|agent| {
                        agent.kind == "claude"
                            && agent.session_id.as_ref() == Some(&hook.session_id)
                    })
            })
            && self.agent_lifecycle.get(&pane).is_some_and(|l| {
                l.kind == "claude"
                    && l.conversation == hook.session_id
                    && l.claude.activity.obligations.source.as_ref() == Some(&sample.source)
            });
        if !valid {
            hook.claude_permission = None;
            return !synthetic;
        }
        let lifecycle = self
            .agent_lifecycle
            .get_mut(&pane)
            .expect("validated lifecycle");
        // Elapse before renewing: delayed heartbeats cannot revive a lost lease.
        // A fresh guarded read can recover an unchanged provider dialog.
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(lifecycle.claude.observed_at)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if let Some(overlay) = &mut lifecycle.claude.activity.permission {
            overlay.elapse(elapsed);
            overlay.accept(sample);
        } else {
            lifecycle.claude.activity.permission = Some(Overlay {
                sample: sample.clone(),
                remaining_ms: LEASE_MS,
            });
        }
        // Keep the shared tick coherent with obligation observation and handoffs.
        lifecycle.claude.activity.obligations.elapse(elapsed);
        lifecycle.claude.observed_at = now;
        let overlay = lifecycle
            .claude
            .activity
            .permission
            .as_ref()
            .expect("accepted overlay");
        // Native reminders and reordered synthetic reports must tell control/plugin readers
        // the same truth as the accepted watermark, before any state is applied.
        let mut normalized = overlay.sample.clone();
        if !synthetic
            && sample.status != Status::Unavailable
            && normalized.status == Status::Unavailable
        {
            normalized.status = Status::Inactive;
        }
        hook.claude_permission = Some(normalized);
        true
    }

    pub(in crate::screen) fn claude_visible(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> Option<AgentInfo> {
        let lifecycle = self
            .agent_lifecycle
            .get(&pane)
            .filter(|l| l.kind == "claude")?;
        let mut agent = lifecycle
            .claude
            .activity
            .parent
            .clone()
            .or_else(|| self.agent_worker_waits.get(&pane).map(|w| w.parent.clone()))
            .or_else(|| host.pane_info(pane).and_then(|p| p.agent))?;
        // Parent snapshots supply presentation only. A snapshot taken from a hook report has
        // no run id, and one taken earlier can predate a plugin's rebinding: keep the current
        // record's run and checkout metadata, or overlaying them would unbind or rebind the run.
        if let Some(current) = host
            .pane_agent(pane)
            .filter(|current| current.kind == agent.kind && current.session_id == agent.session_id)
        {
            agent.run_id = current.run_id;
            if agent.cwd.is_none() {
                agent.cwd = current.cwd;
            }
        }
        if agent.state != AgentState::Waiting
            && let Some(workers) = self
                .agent_worker_waits
                .get(&pane)
                .filter(|w| !w.waits.is_empty())
        {
            agent.state = AgentState::Waiting;
            agent.wait_needs_a_person = true;
            agent.waiting_on = workers.waits.values().next().cloned().flatten();
            agent.detail = None;
        }
        if agent.state != AgentState::Waiting
            && let Some(waiting) = lifecycle
                .claude
                .notifications
                .values()
                .find_map(|n| n.waiting.as_ref())
        {
            agent.state = waiting.state;
            agent.detail = waiting.detail.clone();
            agent.waiting_on = waiting.waiting_on.clone();
            agent.wait_needs_a_person = waiting.wait_needs_a_person;
        }
        self.aggregate_claude_state(pane, &mut agent);
        Some(agent)
    }
}

impl super::AgentController {
    pub(in crate::screen) fn notification_permission_panes(&self) -> Vec<PaneId> {
        self.agent_lifecycle
            .iter()
            .filter(|(_, lifecycle)| {
                lifecycle
                    .claude
                    .notifications
                    .values()
                    .any(|notification| notification.permission_revision.is_some())
            })
            .map(|(pane, _)| *pane)
            .collect()
    }
    pub(in crate::screen) fn advance_notification_permission(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> Option<AgentInfo> {
        let info = host.pane_info(pane)?;
        let enter_revision = host.agent_enter_revision(pane);
        let lifecycle = self.agent_lifecycle.get_mut(&pane)?;
        let valid = !info.is_held
            && !info.exited
            && info.agent.as_ref().is_some_and(|agent| {
                agent.kind == "claude" && agent.session_id.as_ref() == Some(&lifecycle.conversation)
            });
        let mut changed = false;
        for notification in lifecycle.claude.notifications.values_mut() {
            if !valid {
                notification.permission_revision = None;
            } else if notification
                .permission_revision
                .is_some_and(|revision| enter_revision > revision)
            {
                notification.permission_revision = None;
                notification.waiting = None;
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        notifications::prune_released_notification_waits(lifecycle);
        let parent = lifecycle.claude.activity.parent.clone()?;
        Some(parent)
    }
    pub(in crate::screen) fn take_permission_waits(
        &mut self,
    ) -> std::collections::HashMap<PaneId, PermissionWait> {
        std::mem::take(&mut self.claude_permission_waits)
    }
    pub(in crate::screen) fn advance_permission_wait(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        wait: PermissionWait,
    ) -> Option<AgentInfo> {
        let info = host.pane_info(pane)?;
        let mut agent = info.agent?;
        if let Some(parent) = self
            .agent_lifecycle
            .get(&pane)
            .and_then(|l| l.claude.activity.parent.as_ref())
        {
            agent = parent.clone();
        } else if let Some(workers) = self.agent_worker_waits.get(&pane) {
            agent = workers.parent.clone();
        }
        if info.is_held
            || info.exited
            || agent.kind != "claude"
            || agent.session_id.as_deref() != Some(wait.conversation.as_str())
            || agent.state != AgentState::Waiting
        {
            return None;
        }
        let enter_revision = host.agent_enter_revision(pane);
        if enter_revision <= wait.input_revision {
            self.claude_permission_waits.insert(pane, wait);
            return None;
        }
        // Enter infers an answer, but an anonymous background permission prompt must
        // not reopen a confirmed caller turn. Publishing aggregates the remaining work.
        agent.state = if self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == "claude"
                && lifecycle.conversation == wait.conversation
                && lifecycle.claude.confirmation == Some(true)
        }) {
            AgentState::Idle
        } else {
            AgentState::Busy
        };
        agent.detail = None;
        agent.waiting_on = None;
        agent.wait_needs_a_person = false;
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.claude.activity.parent = Some(agent.clone());
        }
        if let Some(workers) = self.agent_worker_waits.get_mut(&pane) {
            workers.parent = agent.clone();
        }
        Some(agent)
    }
}

impl super::AgentController {
    pub(in crate::screen) fn permission_activity(
        &self,
        pane: PaneId,
        mut visible: AgentInfo,
    ) -> AgentInfo {
        if visible.state != AgentState::Waiting
            && let Some(workers) = self
                .agent_worker_waits
                .get(&pane)
                .filter(|w| !w.waits.is_empty())
        {
            visible.state = AgentState::Waiting;
            visible.wait_needs_a_person = true;
            visible.waiting_on = workers.waits.values().next().cloned().flatten();
        }
        if visible.state != AgentState::Waiting
            && let Some(waiting) = self.agent_lifecycle.get(&pane).and_then(|lifecycle| {
                lifecycle
                    .claude
                    .notifications
                    .values()
                    .find_map(|n| n.waiting.as_ref())
            })
        {
            visible = waiting.clone();
        }
        visible
    }
}
