//! The short-lived local reporter deliberately bypasses configuration and CLI action dispatch.
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use yinz_runtime::errors::prelude::*;
use {
    yinz_cli::args::CliAction, yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcRecv,
    yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
    yinz_runtime::agent::report::reporting_watchdog, yinz_runtime::agent::report::ReportTrace,
    yinz_runtime::agent::report::REPORT_DEADLINE, yinz_types::agent::report::AgentReport,
    yinz_types::agent::report::AgentReportState, yinz_types::data::PaneId,
    yinz_types::input::actions::Action,
};

pub fn run(mut action: CliAction, session: Option<String>, started: Instant) -> Result<()> {
    let trace = Arc::new(Mutex::new(ReportTrace::new(started)));
    reporting_watchdog(trace.clone())?;
    let result = (|| {
        let explicit_target = session.is_some();
        let explicit_pane = match &action {
            CliAction::AgentReport { pane_id, .. } => pane_id.clone(),
            _ => None,
        };
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .enter("pane_owner");
        let (session, caller) = yinz_process::pane_context::target(session.map(Into::into))?;
        crate::commands::fill_caller_pane(&mut action, caller);
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
                t.session = session.to_string();
            }
            t.enter("stdin_parse");
        }
        // Reuse the same parser, state mapping, and bounded event construction as manual
        // actions. These values are packaged, never dispatched as generic actions.
        let actions =
            yinz_cli::actions::actions_from_cli(action, Box::new(std::path::PathBuf::new), None)
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
        let session = match session {
            Some(session) => session,
            None => {
                let sessions = yinz_session::sessions::get_sessions()
                    .map_err(|_| anyhow!("cannot discover sessions"))?;
                if sessions.len() != 1 {
                    return Err(anyhow!("specify a session for the report"));
                }
                sessions[0].0.clone()
            },
        };
        yinz_session::sessions::validate_session_name(session.as_str()).map_err(|e| anyhow!(e))?;
        yinz_runtime::agent::report::validate_report(&report)?;
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .session = session.to_string();
        trace
            .lock()
            .map_err(|_| anyhow!("report trace poisoned"))?
            .enter("connect");
        let socket =
            yinz_ipc::consts::ipc_connect(&yinz_runtime::consts::YINZ_SOCK_DIR.join(&session))
                .context("connect report session")?;
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let mut receiver = sender
            .get_receiver::<ServerToClientMsg>()
            .context("set up report acknowledgement")?;
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
        let kind = report.kind.clone();
        let observation = report
            .event
            .as_ref()
            .filter(|event| matches!(event.kind.as_str(), "claude" | "agy"))
            .and_then(|event| {
                serde_json::from_str::<yinz_types::agent::HookEvent>(&event.raw).ok()
            });
        let observer_route = if explicit_target {
            yinz_process::claude_turn::Route::Local {
                session: session.to_string(),
                pane: pane_id.to_string(),
            }
        } else if let Ok(reference) = std::env::var(yinz_process::pty_host::PANE_REF_ENV_KEY) {
            yinz_process::claude_turn::Route::Pane {
                reference,
                pane: explicit_pane,
            }
        } else {
            yinz_process::claude_turn::Route::Local {
                session: session.to_string(),
                pane: pane_id.to_string(),
            }
        };
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
            ) if accepted == report_id => {
                if let Some(hook) = observation {
                    yinz_process::claude_turn::start(&kind, &hook, observer_route)
                        .with_context(|| format!("observe {kind} turn completion"))?;
                }
                return Ok(());
            },
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
    if let Ok(mut t) = trace.lock()
        && t.claim_finish()
    {
        t.finish(result.is_ok());
    }
    result
}

fn report_parts(
    actions: Vec<Action>,
) -> Result<(
    PaneId,
    AgentReportState,
    Option<yinz_types::agent::AgentEvent>,
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
                pane = Some(PaneId::Terminal(e.pane_id));
                event = Some(e);
            },
            _ => return Err(anyhow!("unexpected agent report action")),
        }
    }
    Ok((pane.context("agent report has no target")?, state, event))
}
