//! Applying reported and detected agent state before broadcasting lifecycle events.

use super::Screen;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::TerminalId;
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

const HOOK_FLASH_DURATION: Duration = Duration::from_millis(150);

pub(super) struct HookFlash {
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

    pub(super) fn agent_hook_is_flashing(
        &self,
        pane: &yinz_types::data::PaneInfo,
        now: Instant,
    ) -> bool {
        pane.agent
            .as_ref()
            .is_some_and(|agent| agent.state == AgentState::Busy)
            && self
                .agent_hook_flashes
                .get(&pane.id)
                .is_some_and(|flash| flash.palette_flash)
            && self.pane_title_hook_is_flashing(pane, now)
    }

    pub(super) fn pane_title_hook_is_flashing(
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

    pub(super) fn pane_title_hook_flashes_are_visible(&self, now: Instant) -> bool {
        self.pane_animations
            && self.agent_hook_flashes.keys().any(|id| {
                self.get_pane_info(*id)
                    .is_some_and(|pane| self.pane_title_hook_is_flashing(&pane, now))
                    && self
                        .active_tab_ids
                        .values()
                        .copied()
                        .chain(self.watched_tab_id())
                        .any(|tab_id| {
                            self.tabs.get(&tab_id).is_some_and(|tab| {
                                tab.agent_title_is_visible(
                                    *id,
                                    self.followed_client_id
                                        .filter(|_| self.watched_tab_id() == Some(tab_id)),
                                )
                            })
                        })
            })
    }

    pub(super) fn reap_agent_hook_flashes(&mut self, now: Instant) {
        let stale: Vec<_> = self
            .agent_hook_flashes
            .keys()
            .copied()
            .filter(|pane| {
                self.get_pane_info(*pane)
                    .is_none_or(|pane| !self.pane_title_hook_is_flashing(&pane, now))
            })
            .collect();
        for pane in stale {
            self.agent_hook_flashes.remove(&pane);
        }
    }

    fn flash_agent_hook(
        &mut self,
        pane: PaneId,
        kind: &str,
        hook: &yinz_types::agent::HookEvent,
        cleared_agent: Option<AgentInfo>,
    ) {
        let Some(info) = self.get_pane_info(pane) else {
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
                deadline: self.frame_now() + HOOK_FLASH_DURATION,
                cleared,
            },
        );
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
    if report.kind == "claude"
        && let Some(hook) = hook.as_mut()
        && report.event.as_ref().is_some_and(|event| {
            event.kind == "claude"
                && PaneId::Terminal(event.pane_id) == pane_id
                && event.hook == hook.hook_event_name
                && event.session_id.as_deref() == Some(hook.session_id.as_str())
        })
    {
        if !screen.accept_claude_source(pane_id, hook) {
            return Ok(true);
        }
        if !screen.accept_claude_permission(pane_id, hook) {
            return Ok(true);
        }
        if let Some(event) = &mut report.event {
            let mut raw: serde_json::Value =
                serde_json::from_str(&event.raw).context("parse permission report")?;
            if hook.claude_permission.is_some() || raw.get("_yinz_claude_permission").is_some() {
                raw["_yinz_claude_permission"] = serde_json::to_value(&hook.claude_permission)
                    .context("normalize permission sample")?;
                *event = yinz_types::agent::AgentEvent::from_hook(
                    "claude",
                    event.pane_id,
                    hook,
                    serde_json::to_string(&raw)
                        .context("serialize normalized permission report")?,
                );
            }
        }
        if hook.hook_event_name == yinz_types::agent::claude_permissions::EVENT
            || (hook.hook_event_name == "Notification"
                && hook.notification_type.as_deref() == Some("permission_prompt")
                && hook.claude_permission.as_ref().is_some_and(|s| {
                    s.status != yinz_types::agent::claude_permissions::Status::Unavailable
                }))
        {
            report.state = yinz_types::agent::report::AgentReportState::Unchanged;
        }
        screen.correlate_claude_terminal_hook(pane_id, hook);
        screen.retain_claude_evidence(pane_id, hook);
    }
    let report = &*report;
    if report.kind == "claude"
        && let Some(hook) = hook
            .as_ref()
            .filter(|hook| hook.hook_event_name == "SubagentStop")
    {
        // Worker Stops are useful archive evidence but cannot identify which execution
        // ended; Claude's session registry decides background activity. Never pass them
        // through activity or worker-wait reconciliation.
        if screen.has_agent_lifecycle(pane_id, "claude", &hook.session_id)
            && hook.agent_id.as_deref().is_some_and(|id| !id.is_empty())
            && report.event.as_ref().is_some_and(|event| {
                event.kind == "claude" && event.session_id.as_deref() == Some(&hook.session_id)
            })
        {
            if !hook.archive_only
                && let Some(lifecycle) = screen.agent_lifecycle.get_mut(&pane_id)
            {
                // Reporting after Esc invalidates the recovery quiet-period proof,
                // even though this report cannot change working/waiting state.
                lifecycle.last_report = Instant::now();
            }
            if let Some(event) = &report.event {
                let mut archived = hook.clone();
                archived.archive_only = true;
                let mut event = event.clone();
                event.state = None;
                event.raw = archived.broadcast_json(
                    serde_json::to_string(&archived)
                        .context("serialize archive-only Claude worker Stop")?,
                );
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        None,
                        None,
                        yinz_types::data::Event::AgentEvent(event),
                    )]))?;
            }
        }
        return Ok(true);
    }
    if report.kind == "claude"
        && let Some(hook) = hook.as_ref().filter(|hook| {
            !hook.is_worker()
                && !hook.archive_only
                && hook.hook_event_name == "UserPromptSubmit"
                && hook.claude_recovered_after.is_none()
                && screen.ignore_claude_confirmation(pane_id, hook)
        })
        && let Some(event) = &report.event
    {
        screen.correlate_recovered_claude_submit(event, hook);
        return Ok(true);
    }
    // Older installed reporters can still send Idle for an ordinary Claude or Antigravity Stop.
    // Correlate internal confirmations before applying or broadcasting any state.
    if report.kind == "claude"
        && hook.as_ref().is_some_and(|hook| {
            screen.ignore_claude_recovery(pane_id, hook)
                || screen.ignore_claude_confirmation(pane_id, hook)
                || screen.ignore_claude_worker(pane_id, hook)
        })
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
    let recovered_wait = hook
        .as_ref()
        .filter(|hook| {
            report.kind == "claude"
                && hook.claude_recovered_after.is_some()
                && hook.hook_event_name == "UserPromptSubmit"
        })
        .and_then(|_| screen.get_pane_info(pane_id).and_then(|info| info.agent))
        .filter(|agent| agent.state == AgentState::Waiting);
    let report = if let Some(waiting) = recovered_wait {
        normalized = report.clone();
        normalized.state = yinz_types::agent::report::AgentReportState::Set(waiting);
        if let Some(event) = &mut normalized.event {
            event.state = Some(AgentState::Waiting);
        }
        &normalized
    } else if report.kind == "claude"
        && hook
            .as_ref()
            .is_some_and(|hook| screen.preserve_claude_start_state(pane_id, hook))
    {
        // Resuming the current process proves no foreground turn or native wait
        // ended. A replacement has already retired its old parent before this point.
        normalized = report.clone();
        normalized.state = yinz_types::agent::report::AgentReportState::Unchanged;
        if let Some(event) = &mut normalized.event {
            event.state = None;
        }
        &normalized
    } else if matches!(report.kind.as_str(), "claude" | "agy")
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
        if complete != Some(true) && !(report.kind == "claude" && hook.is_claude_interruption()) {
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
    let trace = yinz_runtime::agent::report::report_diagnostics(report, &screen.session_name);
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
        screen.track_claude_background(pane_id, hook);
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
        // Correlated provider boundaries can retire a stale presentation report. This
        // never turns an OSC report into completion evidence or releases input ownership.
        let retire_status = hook.as_ref().is_some_and(|hook| {
            !hook.is_worker()
                && !hook.archive_only
                && submission.is_some()
                && (hook.hook_event_name == "UserPromptSubmit"
                    || (hook.hook_event_name == "Stop"
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
        screen.flash_agent_hook(pane_id, &report.kind, hook, cleared_agent);
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
    set_pane_agent_report(screen, pane_id, agent, true, true, false)
}

pub(super) fn stage_pane_agent_report(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: Option<AgentInfo>,
) -> Result<bool> {
    screen.claude_permission_waits.remove(&pane_id);
    set_pane_agent_report(screen, pane_id, agent, false, false, false)
}

/// Only the validated SessionEnd path may preserve a proof frozen before record removal.
pub(super) fn stage_pane_agent_shutdown(screen: &mut Screen, pane_id: PaneId) -> Result<bool> {
    set_pane_agent_report(screen, pane_id, None, false, false, true)
}

/// Stage aggregated activity without replacing the parent or its native input ownership.
pub(super) fn stage_pane_agent_activity(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: AgentInfo,
) -> Result<bool> {
    set_pane_agent_report(screen, pane_id, Some(agent), false, false, false)
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
    if remember_parent {
        screen.claude_permission_waits.remove(&pane_id);
    }
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
        if remember_parent {
            screen.remember_claude_parent(pane_id, &agent);
        }
        screen.aggregate_claude_state(pane_id, &mut agent);
        agent
    });
    let changed = screen
        .tab_containing_pane_mut(pane_id)
        .map(|tab| tab.set_pane_agent(pane_id, agent.clone()))
        .unwrap_or(false);
    if screen
        .agent_hook_flashes
        .get(&pane_id)
        .is_some_and(|flash| agent.as_ref().is_none_or(|agent| !flash.matches(agent)))
    {
        screen.agent_hook_flashes.remove(&pane_id);
    }
    if let Some(flash) = screen.agent_hook_flashes.get_mut(&pane_id)
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
    let provisional_muse_bind = previous
        .as_ref()
        .is_some_and(|previous| previous.kind == "muse" && previous.session_id.is_none())
        && agent
            .as_ref()
            .is_some_and(|agent| agent.kind == "muse" && agent.session_id.is_some());
    if !provisional_muse_bind
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
        screen.invalidate_agent_lifecycle(pane_id, reason);
    }
    // A report that repeats itself costs nothing; a changed one is news to every reader of the
    // manifest.
    if changed && publish {
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
        screen.bus.senders.send_to_pty_writer(
            yinz_bus::pty_writer::PtyWriteInstruction::RetainAgentTheme(terminal_id, event),
        )?;
    }
    Ok(true)
}
