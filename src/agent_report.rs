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

/// Bounds the acknowledgement through SSH, where the link sets the pace and SSH keepalives bound
/// a dead connection. A hook on this machine has [`REPORT_DEADLINE`] instead.
const REMOTE_REPORT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Pause between attempts while a live update moves the session name to its replacement.
const UPDATE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);
/// Retries stop this long before the deadline, so a failed report is still recorded.
const UPDATE_RETRY_MARGIN: std::time::Duration = std::time::Duration::from_millis(100);

pub fn run(
    mut action: CliAction,
    session: Option<String>,
    data_dir: Option<&std::path::Path>,
    started: Instant,
) -> Result<()> {
    let trace = Arc::new(Mutex::new(ReportTrace::new(started)));
    let remote = crate::remote_commands::selected().is_some();
    // The watchdog keeps a hook from stalling its agent. Its `_exit` also skips the atexit handler
    // that tears down an SSH forward, and no local agent is waiting on a remote report.
    if !remote {
        reporting_watchdog(trace.clone())?;
    }
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
        let (session, caller) = crate::remote_commands::pane_target(session.map(Into::into))?;
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
        let actions = yinz_cli::actions::actions_from_cli(
            action,
            yinz_cli::actions::PathResolution::Session,
            None,
        )
        .map_err(|_| anyhow!("invalid agent report input"))?;
        let (pane_id, state, mut event) = report_parts(actions)?;
        if let Some(event) = event.as_mut().filter(|e| e.kind == "claude") {
            let mut hook: yinz_types::agent::HookEvent =
                serde_json::from_str(&event.raw).context("parse local Claude evidence")?;
            yinz_process::claude_turn::prepare_hook(&mut hook, !remote);
            let mut raw: serde_json::Value =
                serde_json::from_str(&event.raw).context("parse native Claude payload")?;
            raw["_yinz_claude_notifications"] = serde_json::to_value(&hook.claude_notifications)
                .context("serialize local Claude identities")?;
            raw["_yinz_claude_permission"] = serde_json::to_value(&hook.claude_permission)
                .context("serialize local permission evidence")?;
            event.raw = hook.broadcast_json(
                serde_json::to_string(&raw).context("serialize local Claude evidence")?,
            );
        }
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
        let target = crate::remote_commands::target(&session).socket;
        let deadline = started
            + if remote {
                REMOTE_REPORT_DEADLINE
            } else {
                REPORT_DEADLINE
            };
        // A live update moves the session name between servers. Retry within this hook's own
        // deadline, so the report reaches whichever server owns the name next.
        let may_retry = || Instant::now() + UPDATE_RETRY_PAUSE + UPDATE_RETRY_MARGIN < deadline;
        let mut connect_retried = false;
        // A sealed source proves that an update is moving the name, even for a pane spawned
        // before servers exported their data directory, whose hooks have no journal to consult.
        let mut sealed_source = false;
        loop {
            trace
                .lock()
                .map_err(|_| anyhow!("report trace poisoned"))?
                .enter("connect");
            let socket = match yinz_ipc::consts::ipc_connect(&target) {
                Ok(socket) => socket,
                Err(error) => {
                    if !remote
                        && may_retry()
                        && (sealed_source || session_name_moving(data_dir, &target))
                    {
                        if !std::mem::replace(&mut connect_retried, true) {
                            let t = trace.lock().map_err(|_| anyhow!("report trace poisoned"))?;
                            t.record("connect", std::time::Duration::ZERO, "retry");
                        }
                        std::thread::sleep(UPDATE_RETRY_PAUSE);
                        continue;
                    }
                    return Err(error).context("connect report session");
                },
            };
            let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
            let mut receiver = sender
                .get_receiver::<ServerToClientMsg>()
                .context("set up report acknowledgement")?;
            receiver
                .set_recv_timeout(Some(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .max(std::time::Duration::from_millis(1)),
                ))
                .context("bound report acknowledgement")?;
            trace
                .lock()
                .map_err(|_| anyhow!("report trace poisoned"))?
                .enter("write");
            sender
                .send_client_msg(ClientToServerMsg::AgentReport(report.clone()))
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
                    if let Some(hook) =
                        observation.filter(|_| crate::remote_commands::selected().is_none())
                    {
                        yinz_process::claude_turn::start(&kind, &hook, observer_route)
                            .with_context(|| format!("observe {kind} turn completion"))?;
                    }
                    return Ok(());
                },
                IpcRecv::Msg(ServerToClientMsg::AgentReportRetry { report_id: refused }, _)
                    if refused == report_id && may_retry() =>
                {
                    // A released source handed its reports to the replacement.
                    sealed_source = true;
                    trace
                        .lock()
                        .map_err(|_| anyhow!("report trace poisoned"))?
                        .record(
                            "acknowledgement",
                            acknowledgement_started.elapsed(),
                            "retry",
                        );
                    std::thread::sleep(UPDATE_RETRY_PAUSE);
                    continue;
                },
                IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _) => "mismatch",
                IpcRecv::Msg(ServerToClientMsg::AgentReportRetry { .. }, _) => "retry_expired",
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
            return Err(anyhow!("session did not acknowledge this report"));
        }
    })();
    if let Ok(mut t) = trace.lock()
        && t.claim_finish()
    {
        t.finish(result.is_ok());
    }
    result
}

/// Whether a live update coordinator is moving this session's name between servers, so a
/// missing endpoint is about to reappear. A dead session still fails at once.
fn session_name_moving(data_dir: Option<&std::path::Path>, socket: &std::path::Path) -> bool {
    let data_dir = journal_data_dir(
        data_dir,
        std::env::var_os(yinz_runtime::envs::SESSION_DATA_DIR_ENV_KEY),
    );
    yinz_runtime::session_update::read_active(&data_dir, socket)
        .context("read live update journal for report retry")
        .to_log()
        .ok()
        .flatten()
        .is_some_and(|journal| matches!(journal["phase"].as_str(), Some("Transfer" | "Recovering")))
}

/// The data directory whose journal says whether this session's name is moving. Hooks are
/// installed without `--data-dir`, so the pane's server names its own; anything else falls
/// back to the default.
fn journal_data_dir(
    explicit: Option<&std::path::Path>,
    session: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    explicit
        .map(Into::into)
        .or_else(|| {
            session
                .map(std::path::PathBuf::from)
                .filter(|dir| dir.is_absolute())
        })
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir)
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

#[cfg(test)]
#[path = "unit/agent_report.rs"]
mod tests;
