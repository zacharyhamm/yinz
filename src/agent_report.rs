//! The short-lived local reporter deliberately bypasses configuration and CLI action dispatch.
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use zellij_runtime::errors::prelude::*;
use zellij_utils::{
    agent::report::{
        reporting_watchdog, AgentReport, AgentReportState, ReportTrace, REPORT_DEADLINE,
    },
    cli::CliAction,
    data::PaneId,
    input::actions::Action,
    ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg},
};

pub fn run(mut action: CliAction, mut session: Option<String>, started: Instant) -> Result<()> {
    let trace = Arc::new(Mutex::new(ReportTrace::new(started)));
    reporting_watchdog(trace.clone())?;
    let result = (|| {
        {
            let mut t = trace.lock().map_err(|_| anyhow!("report trace poisoned"))?;
            if let CliAction::AgentReport { kind, pane_id, .. } = &action {
                t.kind = kind.clone().unwrap_or_else(|| "claude".into());
                t.pane = pane_id
                    .clone()
                    .or_else(|| std::env::var("ZELLIJ_PANE_ID").ok())
                    .unwrap_or_default();
            }
            t.session = session
                .clone()
                .or_else(|| zellij_runtime::envs::get_session_name().ok())
                .unwrap_or_default();
            t.enter("pane_owner");
        }
        crate::commands::retarget_implicit_agent_report(
            &mut action,
            &mut session,
            std::env::var(zellij_server::pty_host::PANE_REF_ENV_KEY).ok(),
            |reference| {
                zellij_server::pty_host::client::query(
                    &zellij_server::pty_host::protocol::socket_path(reference),
                )
                .map_err(|_| "could not resolve pane owner".into())
            },
        )
        .map_err(|e| anyhow!(e))?;
        {
            let mut t = trace.lock().map_err(|_| anyhow!("report trace poisoned"))?;
            if let CliAction::AgentReport {
                pane_id: Some(pane_id),
                ..
            } = &action
            {
                t.pane = pane_id.clone();
            }
            if let Some(session) = &session {
                t.session = session.clone();
            }
            t.enter("stdin_parse");
        }
        // Reuse the same parser, state mapping, and bounded event construction as manual
        // actions. These values are packaged, never dispatched as generic actions.
        let actions = Action::actions_from_cli(action, Box::new(std::path::PathBuf::new), None)
            .map_err(|_| anyhow!("invalid agent report input"))?;
        let (pane_id, state, event) = report_parts(actions)?;
        let report = {
            let mut t = trace.lock().map_err(|_| anyhow!("report trace poisoned"))?;
            t.pane = format!("{pane_id:?}");
            if let AgentReportState::Set(agent) = &state {
                t.kind = agent.kind.clone();
            }
            if let Some(event) = &event {
                t.kind = event.kind.clone();
                t.event = event.hook.clone();
                t.payload_bytes = event.raw.len();
            }
            t.enter("session_selection");
            AgentReport {
                report_id: t.report_id.clone(),
                pane_id,
                state,
                event,
                trace: t.enabled,
                process_id: t.process_id,
                kind: t.kind.clone(),
            }
        };
        let session = session.or_else(|| zellij_runtime::envs::get_session_name().ok());
        let session = match session {
            Some(session) => session,
            None => {
                let sessions = zellij_utils::sessions::get_sessions()
                    .map_err(|_| anyhow!("cannot discover sessions"))?;
                if sessions.len() != 1 {
                    return Err(anyhow!("specify a session for the report"));
                }
                sessions[0].0.clone()
            },
        };
        zellij_utils::sessions::validate_session_name(&session).map_err(|e| anyhow!(e))?;
        report.validate()?;
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .session = session.clone();
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .enter("connect");
        let socket =
            zellij_utils::consts::ipc_connect(&zellij_utils::consts::ZELLIJ_SOCK_DIR.join(session))
                .context("connect report session")?;
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let mut receiver = sender.get_receiver::<ServerToClientMsg>();
        receiver
            .set_recv_timeout(Some(
                REPORT_DEADLINE
                    .saturating_sub(started.elapsed())
                    .max(std::time::Duration::from_millis(1)),
            ))
            .context("bound report acknowledgement")?;
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .enter("write");
        let report_id = report.report_id.clone();
        sender
            .send_client_msg(ClientToServerMsg::AgentReport(report))
            .context("write report")?;
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .enter("acknowledgement");
        let acknowledgement_started = Instant::now();
        let outcome = match receiver.recv_server_msg() {
            IpcRecv::Msg(
                ServerToClientMsg::AgentReportAccepted {
                    report_id: accepted,
                },
                _,
            ) if accepted == report_id => return Ok(()),
            IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _) => "mismatch",
            IpcRecv::Msg(ServerToClientMsg::LogError { .. }, _) => "rejected",
            IpcRecv::Msg(_, _) => "unexpected_reply",
            IpcRecv::Eof => "eof",
            IpcRecv::Malformed => "malformed",
            IpcRecv::Idle => "timeout",
        };
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .record(
                "acknowledgement",
                acknowledgement_started.elapsed(),
                outcome,
            );
        Err(anyhow!("session did not acknowledge this report"))
    })();
    if let Ok(t) = trace.lock() {
        t.finish(result.is_ok());
    }
    result
}

fn report_parts(
    actions: Vec<Action>,
) -> Result<(
    PaneId,
    AgentReportState,
    Option<zellij_utils::agent::AgentEvent>,
)> {
    let mut pane = None;
    let mut state = AgentReportState::Unchanged;
    let mut event = None;
    for action in actions {
        match action {
            Action::SetPaneAgent { pane_id, agent } => {
                pane = Some(pane_id);
                state = agent
                    .map(AgentReportState::Set)
                    .unwrap_or(AgentReportState::Clear);
            },
            Action::AgentEvent(e) => {
                pane = Some(PaneId::Terminal(
                    zellij_types::internal_ids::TerminalId::new(e.pane_id),
                ));
                event = Some(e);
            },
            _ => return Err(anyhow!("unexpected agent report action")),
        }
    }
    Ok((pane.context("agent report has no target")?, state, event))
}
