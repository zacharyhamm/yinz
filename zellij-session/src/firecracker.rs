//! Close a VM lease's panes through its owning server before the provider releases it.
use crate::sessions::{session_status, validate_session_name};
use std::time::{Duration, Instant};
use zellij_ipc::probe::SessionProbe;
use zellij_runtime::errors::prelude::*;
use zellij_types::{input::actions::Action, internal_ids::SessionName};

pub fn close_panes(session: &str, name: &str, timeout: Duration) -> Result<()> {
    validate_session_name(session).map_err(|error| anyhow!(error))?;
    zellij_types::command::validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let deadline = Instant::now() + timeout;
    let mut requested = std::collections::BTreeSet::new();
    let result = (|| loop {
        if Instant::now() >= deadline {
            bail!("pane closure timed out; VM lease was not released");
        }
        match session_status(&SessionName::from(session))
            .map_err(|error| anyhow!("session probe failed: {error:?}"))?
        {
            SessionProbe::Absent => return Ok(()),
            SessionProbe::Unresponsive => {
                bail!("owning session is unresponsive; VM lease was not released")
            },
            SessionProbe::Alive => {},
        }
        let panes = live_vm_panes(session, name, deadline)?;
        if panes.is_empty() {
            return Ok(());
        }
        for pane_id in panes {
            if requested.insert(pane_id) {
                vm_session_action(
                    session,
                    Action::CloseTerminalPane { pane_id },
                    false,
                    deadline,
                )?;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    })();
    // Closing the last pane may shut down its server before the acknowledgement arrives.
    if result.is_err()
        && matches!(
            session_status(&SessionName::from(session)),
            Ok(SessionProbe::Absent)
        )
    {
        return Ok(());
    }
    result.context("could not confirm pane closure; VM lease was not released")
}

/// Read a live manifest over IPC; cached session metadata is not a closure acknowledgement.
fn vm_session_action(
    session: &str,
    action: Action,
    listing: bool,
    deadline: Instant,
) -> zellij_runtime::errors::prelude::Result<String> {
    use zellij_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    if Instant::now() >= deadline {
        bail!("pane closure timed out; VM lease was not released");
    }
    let socket =
        zellij_ipc::consts::ipc_connect(&zellij_runtime::consts::ZELLIJ_SOCK_DIR.join(session))
            .context("could not contact the VM's owning session")?;
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>();
    receiver.set_recv_timeout(Some(
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
            .min(Duration::from_secs(2)),
    ))?;
    sender.send_client_msg(ClientToServerMsg::Action {
        action,
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    })?;
    let result = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::Log { lines }, _) => break Ok(lines.join("\n")),
            IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                break Err(anyhow!(lines.join("\n")))
            },
            IpcRecv::Msg(ServerToClientMsg::UnblockInputThread, _) if !listing => {
                break Ok(String::new())
            },
            IpcRecv::Eof => {
                break Err(anyhow!(
                    "owning session disconnected before acknowledging the request"
                ))
            },
            _ if std::time::Instant::now() >= deadline => {
                break Err(anyhow!("owning session did not answer before the deadline"))
            },
            _ => {},
        }
    };
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .non_fatal();
    result
}

/// A fresh manifest from a named session, including pane targets omitted by cached metadata.
pub fn list_panes(
    session: &str,
    timeout: Duration,
) -> Result<Vec<zellij_types::data::PaneListEntry>> {
    validate_session_name(session).map_err(|error| anyhow!(error))?;
    list_panes_until(session, Instant::now() + timeout)
}

fn list_panes_until(
    session: &str,
    deadline: Instant,
) -> Result<Vec<zellij_types::data::PaneListEntry>> {
    let json = vm_session_action(
        session,
        Action::ListPanes {
            show_tab: false,
            show_command: false,
            show_state: false,
            show_geometry: false,
            show_all: true,
            output_json: true,
        },
        true,
        deadline,
    )?;
    let panes: Vec<zellij_types::data::PaneListEntry> = serde_json::from_str(&json)?;
    Ok(panes)
}

fn live_vm_panes(
    session: &str,
    name: &str,
    deadline: Instant,
) -> zellij_runtime::errors::prelude::Result<Vec<zellij_types::internal_ids::TerminalId>> {
    let panes = list_panes_until(session, deadline)?;
    Ok(panes
        .into_iter()
        .filter_map(|entry| {
            let pane = entry.pane_info;
            match pane.id {
                zellij_types::data::PaneId::Terminal(id)
                    if pane.target.firecracker_vm() == Some(name) =>
                {
                    Some(id)
                },
                _ => None,
            }
        })
        .collect())
}
