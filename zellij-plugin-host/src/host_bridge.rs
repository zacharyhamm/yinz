//! Host-owned identity and the private workflow delivery query shared by script runtimes.
use zellij_runtime::errors::prelude::*;
use zellij_types::{data::PluginTag, input::layout::RunPluginLocation};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServerIdentity {
    pub server_binary: String,
    pub server_session: String,
    pub server_incarnation: &'static str,
}

pub(super) fn server_identity() -> Result<ServerIdentity> {
    static INCARNATION: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    Ok(ServerIdentity {
        server_binary: zellij_bus::os_input_output_unix::server_binary()
            .context("cannot resolve server executable")?
            .to_string_lossy()
            .into_owned(),
        server_session: zellij_runtime::envs::get_session_name()
            .unwrap_or_default()
            .to_string(),
        server_incarnation: INCARNATION.as_str(),
    })
}

pub(super) fn is_workflow_bridge(location: &RunPluginLocation) -> bool {
    *location == RunPluginLocation::Zellij(PluginTag::new("swamp-bridge"))
}

pub(super) fn workflow_remote_panes(
    location: &RunPluginLocation,
    senders: &zellij_bus::thread_bus::ThreadSenders,
) -> Result<Vec<serde_json::Value>> {
    anyhow::ensure!(
        is_workflow_bridge(location),
        "workflowRemotePanes is restricted to zellij:swamp-bridge"
    );
    let (tx, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_pty(zellij_bus::pty::PtyInstruction::WorkflowRemotePanes(tx))
        .context("request remote workflow pane identities from PTY")?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .context("receive remote workflow pane identities from PTY (five-second timeout)")
}
