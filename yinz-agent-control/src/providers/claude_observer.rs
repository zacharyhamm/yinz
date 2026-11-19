use crate::host::AgentHost;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    pane_id::PaneId,
};

#[derive(Default)]
pub struct RestoredObservers {
    generations: HashMap<PaneId, Instant>,
}

impl RestoredObservers {
    pub fn retire(&mut self, pane: PaneId) {
        self.generations.remove(&pane);
    }
}

impl RestoredObservers {
    pub fn enroll(&mut self, host: &impl AgentHost, pane: PaneId) {
        let PaneId::Terminal(terminal) = pane else {
            return;
        };
        let Some(info) = host.pane_info(pane) else {
            return;
        };
        let Some(expected) = info
            .agent
            .filter(|agent| agent.kind == "claude" && agent.source == AgentSource::Reported)
        else {
            return;
        };
        let Some(conversation) = expected.session_id.clone().filter(|id| !id.is_empty()) else {
            return;
        };
        if info.host.is_some() || info.is_held || info.exited {
            return;
        }
        let Some((observe, sender)) = host.restored_observer() else {
            return;
        };
        let input_revision = host.agent_input_revision(pane);
        let generation = Instant::now();
        self.generations.insert(pane, generation);
        observe(
            terminal,
            conversation,
            Box::new(move |state| {
                let (reply, response) = std::sync::mpsc::channel();
                if sender
                    .send(
                        yinz_bus::screen::ScreenInstruction::ReconcileRestoredClaude {
                            pane,
                            generation,
                            expected: expected.clone(),
                            input_revision,
                            last_hook: info.last_hook_at_ms,
                            state,
                            reply,
                        },
                    )
                    .is_err()
                {
                    return false;
                }
                response
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap_or(false)
            }),
        )
        .context("observe adopted Claude state")
        .non_fatal();
    }
}

impl RestoredObservers {
    pub fn activity(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        generation: Instant,
        expected: &AgentInfo,
        input_revision: u64,
        last_hook: Option<i64>,
        state: Option<AgentState>,
        orphan: bool,
    ) -> Option<Option<AgentInfo>> {
        if self.generations.get(&pane) != Some(&generation) {
            return None;
        }
        let current = host
            .pane_info(pane)
            .filter(|info| {
                !info.is_held
                    && !info.exited
                    && info.host.is_none()
                    && info.last_hook_at_ms == last_hook
                    && host.agent_input_revision(pane) == input_revision
                    && orphan
            })
            .and_then(|info| info.agent)
            .filter(|agent| {
                agent.kind == expected.kind
                        && agent.session_id == expected.session_id
                        && agent.run_id == expected.run_id
                        && agent.source == expected.source
                        // The OS observer validates the foreground group and Claude's
                        // process instance; invoked_with may still name the shell.
                        && agent.state != AgentState::Exited
            });
        let Some(mut current) = current else {
            self.generations.remove(&pane);
            return None;
        };
        if let Some(state) = state.filter(|state| {
            matches!(
                state,
                AgentState::Idle | AgentState::Busy | AgentState::Waiting
            )
        }) && current.state != state
        {
            current.state = state;
            current.detail = None;
            current.waiting_on =
                (state == AgentState::Waiting).then(|| "approve Claude permission request".into());
            current.wait_needs_a_person = state == AgentState::Waiting;
            return Some(Some(current));
        }
        Some(None)
    }
}
