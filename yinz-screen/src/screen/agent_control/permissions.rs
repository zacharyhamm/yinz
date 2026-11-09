//! Registry presentation has its own ownership, watermark and observation lease.
use super::*;
use yinz_types::agent::claude_permissions::{Overlay, Status, DETAIL, EVENT, LEASE_MS};

impl Screen {
    pub(in crate::screen) fn accept_claude_permission(
        &mut self,
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
            && self.agent_process_matches(pane, "claude")
            && self.get_pane_info(pane).is_some_and(|info| {
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
                    && l.claude.obligations.source.as_ref() == Some(&sample.source)
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
            .saturating_duration_since(lifecycle.notification_tick)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if let Some(overlay) = &mut lifecycle.claude.permission {
            overlay.elapse(elapsed);
            overlay.accept(sample);
        } else {
            lifecycle.claude.permission = Some(Overlay {
                sample: sample.clone(),
                remaining_ms: LEASE_MS,
            });
        }
        // Keep the shared tick coherent with obligation observation and handoffs.
        lifecycle.claude.obligations.elapse(elapsed);
        lifecycle.notification_tick = now;
        let overlay = lifecycle
            .claude
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

    pub(super) fn claude_visible(&self, pane: PaneId) -> Option<AgentInfo> {
        let lifecycle = self
            .agent_lifecycle
            .get(&pane)
            .filter(|l| l.kind == "claude")?;
        let mut agent = lifecycle
            .claude
            .parent
            .clone()
            .or_else(|| self.agent_worker_waits.get(&pane).map(|w| w.parent.clone()))
            .or_else(|| self.get_pane_info(pane).and_then(|p| p.agent))?;
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
                .claude_notifications
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

    pub(super) fn overlay_claude_permission(&self, pane: PaneId, agent: &mut AgentInfo) {
        if agent.kind == "claude"
            && agent.state != AgentState::Exited
            && self.agent_lifecycle.get(&pane).is_some_and(|l| {
                agent.session_id.as_ref() == Some(&l.conversation)
                    && l.claude.permission.as_ref().is_some_and(Overlay::active)
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

    pub(super) fn release_claude_permission(&mut self, pane: PaneId) {
        if let Some(overlay) = self
            .agent_lifecycle
            .get_mut(&pane)
            .and_then(|l| l.claude.permission.as_mut())
        {
            overlay.release();
        }
    }
}

pub(super) fn owned_notification(hook: &HookEvent) -> bool {
    hook.hook_event_name == "Notification"
        && hook.notification_type.as_deref() == Some("permission_prompt")
        && hook
            .claude_permission
            .as_ref()
            .is_some_and(|sample| sample.status != Status::Unavailable)
}
