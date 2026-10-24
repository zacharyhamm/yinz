//! Applying reported and detected agent state before broadcasting lifecycle events.

use super::Screen;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::TerminalId;
use std::time::Duration;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::{AgentInfo, AgentSource};

pub(super) fn apply_queued_agent_report(
    screen: &mut Screen,
    report: &yinz_types::agent::report::AgentReport,
    accepted_at: std::time::Instant,
) -> Result<bool> {
    let pane_id = report.pane_id;
    if screen.tab_containing_pane(pane_id).is_none() {
        return Ok(false);
    }
    let hook = report
        .event
        .as_ref()
        .and_then(|event| serde_json::from_str::<yinz_types::agent::HookEvent>(&event.raw).ok());
    // Older installed reporters can still send Idle for an ordinary Claude or Antigravity Stop.
    // Correlate internal confirmations before applying or broadcasting any state.
    if report.kind == "claude"
        && hook
            .as_ref()
            .is_some_and(|hook| screen.ignore_claude_confirmation(pane_id, hook))
    {
        log::debug!(
            "agent report ignored report={:?} reason=claude_confirmation",
            report.report_id
        );
        return Ok(true);
    }
    if report.kind == "agy"
        && hook
            .as_ref()
            .is_some_and(|hook| screen.ignore_agy_confirmation(pane_id, hook))
    {
        log::debug!(
            "agent report ignored report={:?} reason=agy_confirmation",
            report.report_id
        );
        return Ok(true);
    }
    let mut normalized;
    let report = if matches!(report.kind.as_str(), "claude" | "agy")
        && hook
            .as_ref()
            .is_some_and(|hook| hook.hook_event_name == "Stop")
    {
        let hook = hook.as_ref().expect("checked Stop hook");
        normalized = report.clone();
        let complete = match report.kind.as_str() {
            "claude" => hook.claude_turn_complete,
            "agy" => hook.agy_turn_complete,
            _ => None,
        };
        if complete != Some(true) || screen.claude_turn_continues(pane_id, hook) {
            if let yinz_types::agent::report::AgentReportState::Set(agent) = &mut normalized.state {
                agent.state = yinz_types::agent::AgentState::Busy;
            }
            if let Some(event) = &mut normalized.event {
                event.state = Some(yinz_types::agent::AgentState::Busy);
            }
        }
        &normalized
    } else {
        report
    };
    // Bootstrap must not overwrite newer provider evidence or observe/broadcast stale startup.
    if report.kind == "codex"
        && hook.as_ref().is_some_and(|hook| {
            hook.hook_event_name == "SessionStart"
                && hook.codex_hooks_ready == Some(true)
                && screen.has_agent_lifecycle(pane_id, &report.kind, &hook.session_id)
        })
    {
        log::debug!(
            "agent report ignored report={:?} reason=duplicate_codex_bootstrap",
            report.report_id
        );
        return Ok(true);
    }
    // The Muse bootstrap is unpinned - it carries no session - so a pane whose
    // provider has already spoken keeps its record whatever the launcher says.
    if report.kind == "muse"
        && hook.as_ref().is_some_and(|hook| {
            hook.hook_event_name == "SessionStart"
                && hook.muse_hooks_ready.is_some()
                && hook.session_id.is_empty()
        })
        && screen
            .get_pane_info(pane_id)
            .and_then(|info| info.agent)
            .is_some_and(|agent| {
                agent.kind == "muse"
                    && agent.source == AgentSource::Reported
                    && agent.session_id.is_some()
            })
    {
        log::debug!(
            "agent report ignored report={:?} reason=late_muse_bootstrap",
            report.report_id
        );
        return Ok(true);
    }
    // Work finishing after its turn was interrupted must not make the idle agent busy again.
    if hook
        .as_ref()
        .is_some_and(|hook| screen.is_after_ended_turn(pane_id, &report.kind, hook))
    {
        log::debug!(
            "agent report ignored report={:?} reason=ended_turn",
            report.report_id
        );
        return Ok(true);
    }
    screen.refresh_agent_controls();
    let mut trace = yinz_runtime::agent::report::report_diagnostics(report);
    trace.session = screen.session_name.to_string();
    trace.record("screen_queue", accepted_at.elapsed(), "ok");
    let started = std::time::Instant::now();
    trace.record("state", Duration::ZERO, "enter");
    log::debug!(
        "agent report applying report={:?} session={:?} pane={pane_id:?} kind={:?} hook={:?} conversation={:?}",
        report.report_id, screen.session_name, report.kind,
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
    let result = screen.apply_agent_report_state(report, hook.as_ref());
    trace.record(
        "state",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" },
    );
    result?;
    if let Some(event) = &report.event {
        screen.agent_control_event(event, hook.as_ref());
        if trace.enabled {
            let lifecycle = screen
                .get_pane_info(pane_id)
                .and_then(|info| info.agent)
                .is_some_and(|agent| {
                    agent.source == AgentSource::Reported
                        && (agent.session_id.as_deref().is_some_and(|conversation| {
                            screen.has_agent_lifecycle(pane_id, &agent.kind, conversation)
                        }) || (agent.kind == "muse"
                            && agent.session_id.is_none()
                            && screen.has_provisional_muse_lifecycle(pane_id)))
                });
            trace.record(
                "lifecycle",
                Duration::ZERO,
                if lifecycle { "ok" } else { "unavailable" },
            );
        }
        let submission = hook
            .as_ref()
            .and_then(|hook| screen.event_submission(pane_id, hook));
        screen.observe_agent_event(event, hook.as_ref(), submission);
    }
    let started = std::time::Instant::now();
    trace.record("event_enqueue", Duration::ZERO, "enter");
    let result = if let Some(event) = &report.event {
        screen
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
        screen.terminal_foreground_cmds.remove(&terminal_id);
    } else {
        screen.terminal_foreground_cmds.insert(terminal_id, command);
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
    screen.claude_permission_waits.remove(&pane_id);
    // Read before the write: which hooks a change fires is a question about the edge, and the
    // pane only answers whether anything changed.
    let Some(previous) = screen
        .tab_containing_pane(pane_id)
        .map(|tab| tab.pane_agent_info(pane_id))
    else {
        return Ok(false);
    };
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
        agent
    });
    let changed = screen
        .tab_containing_pane_mut(pane_id)
        .map(|tab| tab.set_pane_agent(pane_id, agent.clone()))
        .unwrap_or(false);
    // A Muse pane verified at startup binds its conversation with its first turn:
    // the record moves from the bootstrap's empty session to the provider's, and
    // that expected edge must not end the submission that caused it.
    let provisional_muse_bind = previous
        .as_ref()
        .is_some_and(|previous| previous.kind == "muse" && previous.session_id.is_none())
        && agent
            .as_ref()
            .is_some_and(|agent| agent.kind == "muse" && agent.session_id.is_some());
    if !provisional_muse_bind
        && (previous
            .as_ref()
            .map(|agent| (&agent.kind, &agent.session_id))
            != agent.as_ref().map(|agent| (&agent.kind, &agent.session_id))
            || agent
                .as_ref()
                .is_none_or(|agent| agent.state == yinz_types::agent::AgentState::Exited))
    {
        let reason = if agent.is_none() {
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
        screen.invalidate_agent_lifecycle(pane_id, reason);
    }
    // A report that repeats itself costs nothing; a changed one is news to every reader of the
    // manifest.
    if changed {
        screen.fire_agent_state_hooks(pane_id, previous.as_ref(), agent.as_ref());
        if screen.update_plugin_tab_names() {
            screen.log_and_report_session_state()?;
        } else {
            screen.report_pane_state()?;
        }
        screen.render()?;
    }
    Ok(true)
}
