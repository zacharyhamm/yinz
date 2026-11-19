//! Applying reported and detected agent state before broadcasting lifecycle events.

use super::{agent_control::reports::AgentUpdate, Screen};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::TerminalId;
use std::time::{Duration, Instant};
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
    let mut prepared = report.clone();
    let report = &mut prepared;
    let pane_id = report.pane_id;
    if screen.tab_containing_pane(pane_id).is_none() {
        return Ok(false);
    }
    let mut hook = report
        .event
        .as_ref()
        .and_then(|event| serde_json::from_str::<yinz_types::agent::HookEvent>(&event.raw).ok());
    if let Some(hook) = hook
        .as_mut()
        .filter(|h| super::agent_control::providers::claude::matches_report(report, h))
    {
        if !screen
            .agents
            .admit_claude_report(&screen.core, report, hook)?
        {
            return Ok(true);
        }
        screen.retain_claude_evidence(pane_id, hook);
    }
    use super::agent_control::reports::ReportDecision;
    match screen
        .agents
        .prepare_report(&screen.core, report, hook.as_ref())?
    {
        ReportDecision::Apply => {},
        ReportDecision::Ignore => return Ok(true),
        ReportDecision::Archive(event) => {
            screen
                .core
                .bus
                .senders
                .send_to_plugin(PluginInstruction::Update(vec![(
                    None,
                    None,
                    yinz_types::data::Event::AgentEvent(event),
                )]))?;
            return Ok(true);
        },
    }
    screen.refresh_agent_controls();
    let trace = yinz_runtime::agent::report::report_diagnostics(report, &screen.core.session_name);
    trace.record("screen_queue", accepted_at.elapsed(), "ok");
    let started = std::time::Instant::now();
    trace.record("state", Duration::ZERO, "enter");
    log::debug!(
        "agent report applying report={:?} session={:?} pane={pane_id:?} kind={:?} hook={:?} conversation={:?}",
        report.report_id, screen.core.session_name, report.kind,
        hook.as_ref().map(|hook| &hook.hook_event_name),
        hook.as_ref().map(|hook| &hook.session_id),
    );
    if let Some(hook) = hook
        .as_ref()
        .filter(|hook| hook.hook_event_name == "PermissionRequest")
    {
        log::debug!(
            "agent permission report={:?} mode={:?} reviewer={:?} mapped_state={:?}",
            report.report_id,
            hook.permission_mode,
            hook.approvals_reviewer,
            yinz_types::agent::hook_state_for(&report.kind, hook),
        );
    }
    let cleared_agent = matches!(
        report.state,
        yinz_types::agent::report::AgentReportState::Clear
    )
    .then(|| screen.get_pane_info(pane_id).and_then(|pane| pane.agent))
    .flatten();
    let previous = screen.get_pane_info(pane_id).and_then(|pane| pane.agent);
    if report.kind == "claude"
        && let Some(hook) = &hook
    {
        screen.agents.track_claude_background(pane_id, hook);
    }
    let result = screen.apply_agent_report_state(report, hook.as_ref());
    trace.record(
        "state",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" },
    );
    result?;
    if let Some(event) = &report.event {
        let completed = screen.agent_control_event(event, hook.as_ref());
        if trace.enabled {
            let lifecycle = screen
                .get_pane_info(pane_id)
                .and_then(|info| info.agent)
                .is_some_and(|agent| {
                    agent.source == AgentSource::Reported
                        && (agent.session_id.as_deref().is_some_and(|conversation| {
                            screen
                                .agents
                                .has_agent_lifecycle(pane_id, &agent.kind, conversation)
                        }) || (agent.kind == "muse"
                            && agent.session_id.is_none()
                            && screen.agents.has_provisional_muse_lifecycle(pane_id)))
                });
            trace.record(
                "lifecycle",
                Duration::ZERO,
                if lifecycle { "ok" } else { "unavailable" },
            );
        }
        let submission = hook
            .as_ref()
            .and_then(|hook| screen.agents.event_submission(pane_id, hook));
        // Correlated provider boundaries can retire a stale presentation report. This
        // never turns an OSC report into completion evidence or releases input ownership.
        let retire_status = hook.as_ref().is_some_and(|hook| {
            !hook.is_worker()
                && !hook.archive_only
                && ((hook.hook_event_name == "UserPromptSubmit" && submission.is_some())
                    || (hook.hook_event_name == "Stop"
                        && completed
                        && (hook.claude_turn_complete == Some(true)
                            || hook.agy_turn_complete == Some(true)
                            || (report.kind == "codex" && hook.turn_id.is_some()))))
        });
        screen.observe_agent_event(event, hook.as_ref(), submission);
        if retire_status {
            screen.with_pane(pane_id, |pane| pane.retire_program_status());
        }
    }
    let current = screen.get_pane_info(pane_id).and_then(|pane| pane.agent);
    if previous != current {
        screen.fire_agent_state_hooks(pane_id, previous.as_ref(), current.as_ref());
        if screen.update_plugin_tab_names() {
            screen.log_and_report_session_state()?;
        }
    }
    if let Some(hook) = hook.as_ref().filter(|hook| !hook.archive_only) {
        let now = screen.frame_now();
        screen.agents.flash_agent_hook(
            &screen.core,
            now,
            pane_id,
            &report.kind,
            hook,
            cleared_agent,
        );
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        screen.with_pane(pane_id, |pane| pane.set_last_hook_at_ms(at));
    }
    if previous != current || hook.as_ref().is_some_and(|hook| !hook.archive_only) {
        // Publish even identical or stateless hooks, before the corresponding AgentEvent.
        screen.report_pane_state()?;
        screen.render()?;
    }
    let started = std::time::Instant::now();
    trace.record("event_enqueue", Duration::ZERO, "enter");
    let result = if let Some(event) = &report.event {
        screen
            .core
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                yinz_types::data::Event::AgentEvent(event.clone()),
            )]))
    } else {
        Ok(())
    };
    trace.record(
        "event_enqueue",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" },
    );
    result?;
    Ok(true)
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
