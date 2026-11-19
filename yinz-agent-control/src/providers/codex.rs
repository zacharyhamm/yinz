use crate::host::AgentHost;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TerminalId;
use yinz_types::{agent::AgentState, pane_id::PaneId};
pub fn plan_prompt(
    host: &impl AgentHost,
    title: &str,
    body: &str,
    terminal_id: TerminalId,
) -> Result<Option<yinz_types::agent::report::AgentReport>> {
    use yinz_types::agent::report::{AgentReport, AgentReportState};
    use yinz_types::agent::{hook_outcome, AgentEvent, AgentSource, HookEvent, HookOutcome};

    // Codex supplies no structured plan-prompt event. This exact TUI notification text
    // must follow upstream changes; disabled Codex notifications produce no wait here.
    if title != "Plan mode prompt: Implement this plan?"
        && body != "Plan mode prompt: Implement this plan?"
    {
        return Ok(None);
    }
    let pane_id = PaneId::Terminal(terminal_id);
    let Some(agent) = host
        .pane_info(pane_id)
        .and_then(|info| info.agent)
        .filter(|agent| {
            agent.kind == "codex"
                && agent.source == AgentSource::Reported
                && agent.state == AgentState::Idle
        })
    else {
        return Ok(None);
    };
    let Some(session_id) = agent.session_id.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    // ponytail: local dismissal has no signal; the next native hook clears this wait.
    let hook = HookEvent {
        hook_event_name: "Elicitation".into(),
        session_id,
        cwd: agent.cwd,
        message: Some("approve the plan".into()),
        ..Default::default()
    };
    let HookOutcome::Reported(agent) = hook_outcome("codex", &hook) else {
        return Ok(None);
    };
    let raw = serde_json::to_string(&hook).context("serialize a Codex plan prompt")?;
    Ok(Some(AgentReport {
        report_id: uuid::Uuid::new_v4().to_string(),
        pane_id,
        state: AgentReportState::Set(agent),
        event: Some(AgentEvent::from_hook("codex", terminal_id, &hook, raw)),
        trace: false,
        process_id: yinz_types::internal_ids::ProcessId::new(std::process::id()),
        kind: "codex".into(),
    }))
}

pub fn duplicate_bootstrap(
    hook: Option<&yinz_types::agent::HookEvent>,
    has_session: impl FnOnce(&str) -> bool,
) -> bool {
    hook.is_some_and(|hook| {
        hook.hook_event_name == "SessionStart"
            && hook.codex_hooks_ready == Some(true)
            && has_session(&hook.session_id)
    })
}
