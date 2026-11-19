//! Applying reported and detected agent state before broadcasting lifecycle events.

use super::{agent_control::AgentUpdate, Screen};
use crate::panes::PaneId;
use crate::TerminalId;
use std::time::Instant;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

impl Screen {
    pub(super) fn set_pane_agent_activity(
        &mut self,
        pane: PaneId,
        expected: AgentInfo,
        state: AgentState,
    ) -> Result<()> {
        // Sampling runs in a plugin. A report, restart, or exit may have arrived since
        // it read the manifest; inferred activity must never overwrite that evidence.
        if expected.source == AgentSource::Reported
            || !matches!(state, AgentState::Busy | AgentState::Idle)
            || !self.get_pane_info(pane).is_some_and(|info| {
                !info.is_held && !info.exited && info.agent.as_ref() == Some(&expected)
            })
            || expected.state == AgentState::Exited
            || expected.state == state
        {
            return Ok(());
        }
        let mut agent = expected;
        agent.state = state;
        agent.detail = None;
        agent.waiting_on = None;
        agent.wait_needs_a_person = false;
        publish_pane_agent_activity(self, pane, agent)
            .context("publishing inferred agent activity")?;
        Ok(())
    }

    pub(super) fn pane_title_hook_flashes_are_visible(&self, now: Instant) -> bool {
        self.core.pane_animations
            && self.agents.flash_panes().any(|id| {
                self.get_pane_info(id)
                    .is_some_and(|pane| self.agents.pane_title_hook_is_flashing(&pane, now))
                    && self
                        .core
                        .active_tab_ids
                        .values()
                        .copied()
                        .chain(self.watched_tab_id())
                        .any(|tab_id| {
                            self.core.tabs.get(&tab_id).is_some_and(|tab| {
                                tab.agent_title_is_visible(
                                    id,
                                    self.core
                                        .followed_client_id
                                        .filter(|_| self.watched_tab_id() == Some(tab_id)),
                                )
                            })
                        })
            })
    }

    pub(super) fn reap_agent_hook_flashes(&mut self, now: Instant) {
        self.agents.reap_agent_hook_flashes(&self.core, now);
    }
}

pub(super) fn apply_queued_agent_report(
    screen: &mut Screen,
    report: &yinz_types::agent::report::AgentReport,
    accepted_at: std::time::Instant,
) -> Result<bool> {
    yinz_agent_control::controller::dispatch::apply_queued_agent_report(screen, report, accepted_at)
}

pub(super) fn apply_pane_foreground_command(
    screen: &mut Screen,
    terminal_id: TerminalId,
    command: Vec<String>,
    program: Option<String>,
) -> bool {
    let detected = command.split_first().is_some_and(|(program, args)| {
        yinz_types::agent::kind_from_program_and_args(std::path::Path::new(program), args).is_some()
    });
    if command.is_empty() {
        screen.core.terminal_foreground_cmds.remove(&terminal_id);
    } else {
        screen
            .core
            .terminal_foreground_cmds
            .insert(terminal_id, command);
    }
    screen
        .with_pane(PaneId::Terminal(terminal_id), |pane| {
            pane.set_detected_agent(detected);
            pane.set_running_program(program)
        })
        .is_some()
}

pub(crate) fn apply_pane_agent_report(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: Option<AgentInfo>,
) -> Result<bool> {
    let accepted = set_pane_agent_report(screen, pane_id, agent, true, true, false)?;
    if accepted {
        // An authoritative report supersedes restored observation even when the
        // record is identical. Observer activity uses its own publication path.
        screen.agents.retire_restored_observer(pane_id);
    }
    Ok(accepted)
}

/// Publish aggregated activity without replacing the underlying parent state or its wait.
pub(super) fn publish_pane_agent_activity(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: AgentInfo,
) -> Result<bool> {
    set_pane_agent_report(screen, pane_id, Some(agent), true, false, false)
}

fn set_pane_agent_report(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: Option<AgentInfo>,
    publish: bool,
    remember_parent: bool,
    preserve_shutdown: bool,
) -> Result<bool> {
    let update = screen.agents.stage_agent(
        &mut screen.core,
        pane_id,
        agent,
        remember_parent,
        preserve_shutdown,
    );
    if let AgentUpdate::Changed { previous, current } = &update
        && publish
    {
        screen.fire_agent_state_hooks(pane_id, previous.as_ref(), current.as_ref());
        if screen.update_plugin_tab_names() {
            screen.log_and_report_session_state()?;
        } else {
            screen.report_pane_state()?;
        }
        screen.render()?;
    }
    Ok(!matches!(update, AgentUpdate::Missing))
}

/// Apply discovery state through the same grid/retention path as managed palettes.
pub(super) fn apply_detected_agent_theme(
    screen: &mut Screen,
    terminal_id: TerminalId,
    state: Option<yinz_types::agent::theme::DetectedAgentTheme>,
) -> Result<bool> {
    let updates = screen.with_pane(PaneId::Terminal(terminal_id), |pane| {
        let Some(terminal) = pane.terminal_io_mut() else {
            return vec![];
        };
        terminal.update_detected_agent_theme(state);
        terminal.drain_agent_theme_updates()
    });
    let Some(updates) = updates else {
        return Ok(false);
    };
    for event in updates {
        screen.core.bus.senders.send_to_pty_writer(
            yinz_bus::pty_writer::PtyWriteInstruction::RetainAgentTheme(terminal_id, event),
        )?;
    }
    Ok(true)
}
