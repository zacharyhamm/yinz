//! Host-owned identity and the private workflow delivery query shared by script runtimes.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;
use yinz_types::{data::PluginTag, input::layout::RunPluginLocation};

/// Where the plugin runtime learns this server's session name.
///
/// The name changes while plugins run: a rename republishes `YINZ_SESSION_NAME` from
/// `OsApi::rename_pty_host_session`, and a plugin instance created after it, or a warp started
/// after it, must use the new name. So the server passes [`ServerSession::Process`], which asks
/// the environment on every call. Tests pass [`ServerSession::Fixed`] instead of writing the
/// variable, which every test in a threaded runner shares.
#[derive(Clone, Debug)]
pub enum ServerSession {
    /// `YINZ_SESSION_NAME` at the moment of the call; empty when it is unset.
    Process,
    /// A name no rename can change.
    Fixed(SessionName),
}

impl ServerSession {
    pub fn current(&self) -> SessionName {
        match self {
            ServerSession::Process => yinz_runtime::envs::get_session_name().unwrap_or_default(),
            ServerSession::Fixed(name) => name.clone(),
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServerIdentity {
    pub server_binary: String,
    pub server_session: String,
    pub server_incarnation: &'static str,
}

pub(super) fn server_identity(session: &ServerSession) -> Result<ServerIdentity> {
    static INCARNATION: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    Ok(ServerIdentity {
        server_binary: yinz_runtime::own_binary::server_binary()
            .context("cannot resolve server executable")?
            .to_string_lossy()
            .into_owned(),
        server_session: session.current().to_string(),
        server_incarnation: INCARNATION.as_str(),
    })
}

pub(super) fn is_workflow_bridge(location: &RunPluginLocation) -> bool {
    *location == RunPluginLocation::Yinz(PluginTag::new("swamp-bridge"))
}

pub(super) fn workflow_remote_panes(
    location: &RunPluginLocation,
    senders: &yinz_bus::thread_bus::ThreadSenders,
) -> Result<Vec<serde_json::Value>> {
    anyhow::ensure!(
        is_workflow_bridge(location),
        "workflowRemotePanes is restricted to yinz:swamp-bridge"
    );
    let (tx, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_pty(yinz_bus::pty::PtyInstruction::WorkflowRemotePanes(tx))
        .context("request remote workflow pane identities from PTY")?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .context("receive remote workflow pane identities from PTY (five-second timeout)")
}

#[derive(serde::Deserialize)]
pub(super) struct WorkflowSignal {
    terminal_id: yinz_types::internal_ids::TerminalId,
    owner: String,
    process: Option<yinz_types::internal_ids::ProcessId>,
    supervisor: String,
    kill: bool,
}
pub(super) fn workflow_signal(
    location: &RunPluginLocation,
    senders: &yinz_bus::thread_bus::ThreadSenders,
    request: WorkflowSignal,
) -> Result<()> {
    anyhow::ensure!(
        is_workflow_bridge(location),
        "workflowSignal is restricted to yinz:swamp-bridge"
    );
    let (reply, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_pty(yinz_bus::pty::PtyInstruction::WorkflowSignal {
            terminal_id: request.terminal_id,
            owner: request.owner,
            process: request.process,
            supervisor: request.supervisor,
            kill: request.kill,
            reply,
        })
        .context("signal owned workflow pane")?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .context("receive workflow signal acknowledgement")?
        .map_err(|error| anyhow!(error))
}
