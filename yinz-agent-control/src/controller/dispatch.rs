//! Synchronous coordination at publication and layout boundaries. Borrow the
//! controller and pane host together only until a cross-domain effect is due.
use super::*;

pub trait AgentSession {
    fn agents_and_host(&mut self) -> (&mut AgentController, &mut impl AgentHost);
    fn complete_close(&mut self, close: AgentClose);
    fn publish_session(&mut self);
    fn publish_if_tab_names_changed(&mut self) -> Result<()>;
    fn publish_panes(&mut self) -> Result<()>;
    fn publish_activity(&mut self, pane: PaneId, agent: AgentInfo) -> Result<()>;
    fn publish_report(&mut self, pane: PaneId, agent: AgentInfo) -> Result<()>;
    fn enqueue_event(&mut self, event: AgentEvent) -> Result<()>;
    fn frame_now(&self) -> Instant;
    fn retire_program_status(&mut self, pane: PaneId);
    fn set_last_hook(&mut self, pane: PaneId, at: i64);
}

pub fn request(
    session: &mut impl AgentSession,
    request: AgentRequest,
    client: Reply,
    now: Instant,
) {
    let (agents, host) = session.agents_and_host();
    match agents.handle(host, request, client, now) {
        None => {},
        Some(Completion::Close(close)) => session.complete_close(close),
        Some(Completion::Refresh) => tick(session, now),
        Some(Completion::Recover {
            pane,
            agent,
            revision,
            client,
        }) => {
            recover(session, pane, &agent, revision);
            reply_ok(session, pane, &client, None);
        },
        Some(Completion::Visibility { pane, client }) => {
            session.publish_session();
            reply_ok(session, pane, &client, None);
        },
        Some(Completion::Restarted { client, response }) => {
            agents.refresh_agent_worktrees(host);
            session.publish_session();
            let (agents, host) = session.agents_and_host();
            agents.agent_reply(host, &client, *response);
        },
    }
}

fn reply_ok(
    session: &mut impl AgentSession,
    pane: PaneId,
    client: &Reply,
    message: Option<String>,
) {
    let (agents, host) = session.agents_and_host();
    agents.agent_reply(
        host,
        client,
        agents.agent_result(host, pane, AgentOutcome::Ok, message),
    );
}

fn recover(session: &mut impl AgentSession, pane: PaneId, agent: &AgentInfo, revision: u64) {
    let reset = agent.state != AgentState::Idle;
    session
        .agents_and_host()
        .0
        .begin_recovery(pane, revision, reset);
    if reset {
        let mut idle = agent.clone();
        idle.state = AgentState::Idle;
        idle.detail = None;
        idle.waiting_on = None;
        idle.wait_needs_a_person = false;
        session
            .publish_report(pane, idle)
            .context("reset an interrupted agent to idle")
            .non_fatal();
        broadcast_interrupt(session, pane, agent)
            .context("announce an interrupted turn to plugins")
            .non_fatal();
    }
    session.agents_and_host().0.finish_recovery(pane);
}

fn broadcast_interrupt(
    session: &mut impl AgentSession,
    pane: PaneId,
    agent: &AgentInfo,
) -> Result<()> {
    let PaneId::Terminal(terminal) = pane else {
        return Ok(());
    };
    let hook = HookEvent {
        hook_event_name: "Interrupt".into(),
        session_id: agent.session_id.clone().unwrap_or_default(),
        cwd: agent.cwd.clone(),
        ..Default::default()
    };
    let json = serde_json::to_string(&hook).context("serialize an interrupt event")?;
    session.enqueue_event(AgentEvent::from_hook(&agent.kind, terminal, &hook, json))
}

fn publish_observation(session: &mut impl AgentSession, pane: PaneId) {
    let (agents, host) = session.agents_and_host();
    let previous = host.pane_info(pane).and_then(|p| p.agent);
    agents.reconcile_claude_parent(host, pane);
    let current = host.pane_info(pane).and_then(|p| p.agent);
    if previous != current
        && let (Some(previous), Some(current)) = (previous, current)
    {
        host.set_agent(pane, Some(previous));
        session
            .publish_activity(pane, current)
            .context("publish Claude notification observation activity")
            .non_fatal();
    }
}

fn retain_evidence(session: &mut impl AgentSession, pane: PaneId, hook: &HookEvent) {
    if session
        .agents_and_host()
        .0
        .retain_claude_evidence(pane, hook)
    {
        publish_observation(session, pane);
    }
}

pub fn observe_hook(
    session: &mut impl AgentSession,
    event: &AgentEvent,
    hook: Option<&HookEvent>,
) -> bool {
    let (agents, host) = session.agents_and_host();
    if !agents.observe_control_hook(host, event, hook) {
        return false;
    }
    let pane = PaneId::Terminal(event.pane_id);
    if let Some(hook) = hook.filter(|_| event.kind == "claude") {
        retain_evidence(session, pane, hook);
        let (agents, host) = session.agents_and_host();
        if !hook.archive_only && agents.has_agent_lifecycle(pane, "claude", &hook.session_id) {
            agents.reconcile_claude_parent(host, pane);
        }
    }
    let (agents, host) = session.agents_and_host();
    agents.correlate_agent_submissions(host, event, hook)
}

pub fn tick(session: &mut impl AgentSession, now: Instant) {
    for pane in session.agents_and_host().0.expire_claude_observations(now) {
        publish_observation(session, pane);
    }
    for pane in session.agents_and_host().0.notification_permission_panes() {
        let (agents, host) = session.agents_and_host();
        if let Some(parent) = agents.advance_notification_permission(host, pane) {
            let visible = agents.permission_activity(pane, parent);
            session
                .publish_activity(pane, visible)
                .context("clear Claude permission wait after Enter")
                .non_fatal();
        }
    }
    for (pane, wait) in session.agents_and_host().0.take_permission_waits() {
        let (agents, host) = session.agents_and_host();
        if let Some(agent) = agents.advance_permission_wait(host, pane, wait) {
            let visible = agents.permission_activity(pane, agent);
            session
                .publish_activity(pane, visible)
                .context("clear Claude permission wait after Enter")
                .non_fatal();
        }
    }
    for pane in session.agents_and_host().0.clearing_panes() {
        let (agents, host) = session.agents_and_host();
        if let Some((agent, revision, client)) = agents.advance_draft_clear(host, pane, now) {
            recover(session, pane, &agent, revision);
            if let Some(client) = client {
                reply_ok(
                    session,
                    pane,
                    &client,
                    Some("Composer verified empty; task completion remains unverified.".into()),
                );
            }
        }
    }
    let (agents, host) = session.agents_and_host();
    agents.advance_pending_controls(host, now);
}

pub fn apply_queued_agent_report(
    session: &mut impl AgentSession,
    report: &yinz_types::agent::report::AgentReport,
    accepted_at: std::time::Instant,
) -> Result<bool> {
    let mut prepared = report.clone();
    let report = &mut prepared;
    let pane_id = report.pane_id;
    if session.agents_and_host().1.tab_for_pane(pane_id).is_none() {
        return Ok(false);
    }
    let mut hook = report
        .event
        .as_ref()
        .and_then(|event| serde_json::from_str::<yinz_types::agent::HookEvent>(&event.raw).ok());
    if let Some(hook) = hook
        .as_mut()
        .filter(|h| providers::claude::matches_report(report, h))
    {
        let (agents, host) = session.agents_and_host();
        if !agents.admit_claude_report(host, report, hook)? {
            return Ok(true);
        }
        retain_evidence(session, pane_id, hook);
    }
    let (agents, host) = session.agents_and_host();
    match agents.prepare_report(host, report, hook.as_ref())? {
        ReportDecision::Apply => {},
        ReportDecision::Ignore => return Ok(true),
        ReportDecision::Archive(event) => {
            session.enqueue_event(event)?;
            return Ok(true);
        },
    }
    tick(session, Instant::now());
    let (agents, host) = session.agents_and_host();
    let trace =
        yinz_runtime::agent::report::report_diagnostics(report, &host.session_name().into());
    trace.record("screen_queue", accepted_at.elapsed(), "ok");
    let started = std::time::Instant::now();
    trace.record("state", Duration::ZERO, "enter");
    log::debug!(
        "agent report applying report={:?} session={:?} pane={pane_id:?} kind={:?} hook={:?} conversation={:?}",
        report.report_id, host.session_name(), report.kind,
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
    .then(|| host.pane_info(pane_id).and_then(|pane| pane.agent))
    .flatten();
    let previous = host.pane_info(pane_id).and_then(|pane| pane.agent);
    if report.kind == "claude"
        && let Some(hook) = &hook
    {
        agents.track_claude_background(pane_id, hook);
    }
    let result = agents.apply_agent_report_state(host, report, hook.as_ref());
    trace.record(
        "state",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" },
    );
    result?;
    if let Some(event) = &report.event {
        let completed = observe_hook(session, event, hook.as_ref());
        let (agents, host) = session.agents_and_host();
        if trace.enabled {
            let lifecycle = host
                .pane_info(pane_id)
                .and_then(|info| info.agent)
                .is_some_and(|agent| {
                    agent.source == AgentSource::Reported
                        && (agent.session_id.as_deref().is_some_and(|conversation| {
                            agents.has_agent_lifecycle(pane_id, &agent.kind, conversation)
                        }) || (agent.kind == "muse"
                            && agent.session_id.is_none()
                            && agents.has_provisional_muse_lifecycle(pane_id)))
                });
            trace.record(
                "lifecycle",
                Duration::ZERO,
                if lifecycle { "ok" } else { "unavailable" },
            );
        }
        let submission = hook
            .as_ref()
            .and_then(|hook| agents.event_submission(pane_id, hook));
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
        agents.observe_agent_event(host, event, hook.as_ref(), submission);
        if retire_status {
            session.retire_program_status(pane_id);
        }
    }
    let (_, host) = session.agents_and_host();
    let current = host.pane_info(pane_id).and_then(|pane| pane.agent);
    if previous != current {
        host.state_hooks(pane_id, previous.as_ref(), current.as_ref());
        session.publish_if_tab_names_changed()?;
    }
    if let Some(hook) = hook.as_ref().filter(|hook| !hook.archive_only) {
        let now = session.frame_now();
        let (agents, host) = session.agents_and_host();
        agents.flash_agent_hook(host, now, pane_id, &report.kind, hook, cleared_agent);
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        session.set_last_hook(pane_id, at);
    }
    if previous != current || hook.as_ref().is_some_and(|hook| !hook.archive_only) {
        // Publish even identical or stateless hooks, before the corresponding AgentEvent.
        session.publish_panes()?;
    }
    let started = std::time::Instant::now();
    trace.record("event_enqueue", Duration::ZERO, "enter");
    let result = if let Some(event) = &report.event {
        session.enqueue_event(event.clone())
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
