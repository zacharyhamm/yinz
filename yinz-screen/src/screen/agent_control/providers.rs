pub(super) mod agy;
pub(super) use yinz_agent_control::providers::agy::AgyLifecycle;
pub(in crate::screen) mod claude;
pub(super) use claude::{ClaudeLifecycle, ClaudeState};
pub(in crate::screen) use yinz_agent_control::providers::codex;
pub(super) mod muse;
mod pi;
pub(super) use yinz_agent_control::providers::{
    draft_composer, encode_prompt, is_command, muse_composer, Provider,
};

use super::{reports::ReportDecision, AgentController, AgentHost, AgentState, HookEvent, Result};
use yinz_types::agent::report::{AgentReport, AgentReportState};
impl AgentController {
    pub(in crate::screen) fn prepare_report(
        &mut self,
        host: &impl AgentHost,
        report: &mut AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<ReportDecision> {
        let pane_id = report.pane_id;
        if report.kind == "claude" {
            let decision = self.filter_claude_report(host, report, hook)?;
            if !matches!(decision, ReportDecision::Apply) {
                return Ok(decision);
            }
        }
        if report.kind == "agy"
            && hook.is_some_and(|hook| self.ignore_agy_confirmation(pane_id, hook))
        {
            log::debug!(
                "agent report ignored report={:?} reason=agy_confirmation",
                report.report_id
            );
            return Ok(ReportDecision::Ignore);
        }
        let preserved = report.kind == "claude" && self.normalize_claude_state(host, report, hook);
        if !preserved
            && hook.is_some_and(|hook| Provider::for_kind(&report.kind).incomplete_stop(hook))
        {
            if let AgentReportState::Set(agent) = &mut report.state {
                agent.state = AgentState::Busy;
            }
            if let Some(event) = &mut report.event {
                event.state = Some(AgentState::Busy);
            }
        }
        // Bootstrap must not overwrite newer provider evidence or observe/broadcast stale startup.
        if report.kind == "codex"
            && hook.is_some_and(|hook| {
                hook.hook_event_name == "SessionStart"
                    && hook.codex_hooks_ready == Some(true)
                    && self.has_agent_lifecycle(pane_id, &report.kind, &hook.session_id)
            })
        {
            log::debug!(
                "agent report ignored report={:?} reason=duplicate_codex_bootstrap",
                report.report_id
            );
            return Ok(ReportDecision::Ignore);
        }
        // The Muse bootstrap is unpinned - it carries no session - so a pane whose
        // provider has already spoken keeps its record whatever the launcher says.
        if report.kind == "muse"
            && hook.is_some_and(|hook| {
                hook.hook_event_name == "SessionStart"
                    && hook.muse_hooks_ready.is_some()
                    && hook.session_id.is_empty()
            })
            && host
                .pane_info(pane_id)
                .and_then(|info| info.agent)
                .is_some_and(|agent| {
                    agent.kind == "muse"
                        && agent.source == yinz_types::agent::AgentSource::Reported
                        && agent.session_id.is_some()
                })
        {
            log::debug!(
                "agent report ignored report={:?} reason=late_muse_bootstrap",
                report.report_id
            );
            return Ok(ReportDecision::Ignore);
        }
        // Work finishing after its turn was interrupted must not make the idle agent busy again.
        if hook.is_some_and(|hook| self.is_after_ended_turn(pane_id, &report.kind, hook)) {
            log::debug!(
                "agent report ignored report={:?} reason=ended_turn",
                report.report_id
            );
            return Ok(ReportDecision::Ignore);
        }
        Ok(ReportDecision::Apply)
    }
}
