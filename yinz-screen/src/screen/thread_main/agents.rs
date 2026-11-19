use super::*;
use std::time::Instant;
use yinz_bus::route::NotificationEnd;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::agent::AgentInfo;
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::TerminalId;

pub(super) fn commit_agent_launch(
    screen: &mut Screen,
    terminal_id: TerminalId,
    reply: std::sync::mpsc::Sender<bool>,
) -> Result<()> {
    let accepted = screen.validate_launch_slot(terminal_id).is_ok()
        && screen
            .core
            .bus
            .senders
            .launches
            .gate(terminal_id)
            .is_some_and(|gate| gate.handoff());
    let _ = reply.send(accepted);
    Ok(())
}

pub(super) fn release_agent_launch(
    screen: &mut Screen,
    terminal_id: TerminalId,
    reply: std::sync::mpsc::Sender<bool>,
) -> Result<()> {
    let gate = screen.core.bus.senders.launches.gate(terminal_id);
    let owned = gate.is_some_and(|gate| !gate.snapshot().finished);
    if owned {
        screen.close_pane_explicitly(PaneId::Terminal(terminal_id), None);
    }
    let released = owned
        && screen
            .tab_id_containing_pane(PaneId::Terminal(terminal_id))
            .is_none();
    if released && let Some(gate) = screen.core.bus.senders.launches.gate(terminal_id) {
        gate.layout_released();
    }
    let _ = reply.send(released);
    Ok(())
}

pub(super) fn update_detected_agent_theme(
    event_loop: &mut EventLoopState,
    screen: &mut Screen,
    terminal_id: TerminalId,
    state: Option<yinz_types::agent::theme::DetectedAgentTheme>,
) -> Result<()> {
    if crate::screen::agent_reports::apply_detected_agent_theme(screen, terminal_id, state.clone())?
    {
        screen.render()?;
    } else {
        event_loop
            .pending
            .waiting_for_pane
            .entry(PaneId::Terminal(terminal_id))
            .or_default()
            .push(ScreenInstruction::UpdateDetectedAgentTheme(
                terminal_id,
                state,
            ));
    }
    Ok(())
}

pub(super) fn restart_agent_pane(
    screen: &mut Screen,
    pane_id: PaneId,
    prompt: Option<String>,
    session: yinz_types::agent::control::AgentRestartSession,
    originating_plugin: yinz_types::data::OriginatingPlugin,
    response: std::sync::mpsc::Sender<Result<yinz_types::agent::control::RestartedAgent, String>>,
) -> Result<()> {
    let result = screen.restart_agent_pane(
        pane_id,
        prompt.as_deref(),
        session,
        Some(originating_plugin),
    );
    let _ = response.send(result);
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn resolve_agent_tab(
    screen: &mut Screen,
    placement: yinz_types::agent::control::AgentPlacement,
    invoked_with: Option<Run>,
    client_id: ClientId,
    originating_pane: Option<PaneId>,
    launch: std::sync::Arc<yinz_bus::agent_launch::LaunchGate>,
    reply: std::sync::mpsc::Sender<Result<yinz_bus::route::AgentTabPlacement, String>>,
) -> Result<()> {
    let result = screen.resolve_launch_tab(
        placement,
        invoked_with,
        client_id,
        originating_pane,
        &launch,
    );
    // If the receiver disappeared, retain the tab: the outcome is unknown.
    let _ = reply.send(result);
    Ok(())
}

pub(super) fn agent_report(
    state: &mut EventLoopState,
    screen: &mut Screen,
    report: yinz_types::agent::report::AgentReport,
    accepted_at: std::time::Instant,
) -> Result<()> {
    if !apply_queued_agent_report(screen, &report, accepted_at)? {
        state
            .pending
            .waiting_for_pane
            .entry(report.pane_id)
            .or_default()
            .push(ScreenInstruction::AgentReport(report, accepted_at));
    }
    Ok(())
}

pub(super) fn restore_warped_agent_input(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_id: PaneId,
    input: yinz_types::data::WarpedAgentInput,
) -> Result<()> {
    if screen.get_pane_info(pane_id).is_none() {
        state
            .pending
            .waiting_for_pane
            .entry(pane_id)
            .or_default()
            .push(ScreenInstruction::RestoreWarpedAgentInput(pane_id, input));
    } else {
        screen.restore_warped_agent_input(pane_id, input);
    }
    Ok(())
}

pub(super) fn set_pane_agent(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_id: PaneId,
    agent: Option<AgentInfo>,
    _completion: Option<NotificationEnd>,
) -> Result<()> {
    if !apply_pane_agent_report(screen, pane_id, agent.clone())? {
        let pending_events = state.pending.waiting_for_pane.entry(pane_id).or_default();
        pending_events.retain(|event| !matches!(event, ScreenInstruction::SetPaneAgent(..)));
        pending_events.push(ScreenInstruction::SetPaneAgent(pane_id, agent, _completion));
        return Ok(());
    }
    Ok(())
}

pub(super) fn reconcile_restored_claude(
    screen: &mut Screen,
    pane: PaneId,
    generation: Instant,
    expected: AgentInfo,
    input_revision: u64,
    last_hook: Option<i64>,
    state: Option<yinz_types::agent::AgentState>,
    reply: std::sync::mpsc::Sender<bool>,
) -> Result<()> {
    let keep = screen.reconcile_restored_claude(
        pane,
        generation,
        &expected,
        input_revision,
        last_hook,
        state,
    )?;
    let _ = reply.send(keep);
    Ok(())
}

pub(super) fn set_pane_agent_run(
    screen: &mut Screen,
    pane_id: PaneId,
    run_id: Option<String>,
) -> Result<()> {
    // Unlike a record, this waits for nothing: a run id names a record, and a pane
    // with no record has nothing for it to name. Dropping it is the answer, not
    // queueing it — the plugin that sent this re-sends on its next pass anyway.
    let updated = screen
        .tab_containing_pane(pane_id)
        .and_then(|tab| tab.pane_agent_info(pane_id))
        .filter(|agent| agent.run_id != run_id)
        .map(|mut agent| {
            agent.run_id = run_id;
            agent
        });
    if let Some(agent) = updated {
        screen.agents.rebind_agent_parent_runs(pane_id, &agent);
        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
            tab.set_pane_agent(pane_id, Some(agent));
        }
        screen.report_pane_state()?;
    }
    Ok(())
}
