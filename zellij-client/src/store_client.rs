//! One request to the running hocus-store, with no stdin loop or mutation retries.
use std::time::{Duration, Instant};
use zellij_ipc::ipc::{ClientToServerMsg, IpcRecv, ServerToClientMsg};
use zellij_runtime::errors::prelude::*;
use zellij_types::{input::actions::Action, internal_ids::SessionName};

pub fn request(session: &SessionName, action: Action) -> Result<serde_json::Value> {
    let timeout = Duration::from_secs(30);
    let Action::CliPipe { ref pipe_id, .. } = action else {
        return Err(anyhow!("expected a store pipe request"));
    };
    let pipe_id = pipe_id.clone();
    let path = zellij_runtime::consts::ZELLIJ_SOCK_DIR.join(session);
    let socket =
        crate::os_input_output::connect_with_deadline(&path, timeout, Duration::from_millis(50))
            .context("cannot connect to the notes and todos session")?;
    let (mut sender, mut receiver) = crate::os_input_output_unix::setup_ipc(socket, &path);
    let deadline = Instant::now() + timeout;
    sender
        .send_client_msg(ClientToServerMsg::Action {
            action,
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })
        .context("cannot send store request")?;
    let result = (|| {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(missing_reply(session));
            }
            receiver
                .set_recv_timeout(Some(remaining))
                .context("cannot bound store reply wait")?;
            match receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::CliPipeOutput { pipe_name, output }, _) if pipe_name == pipe_id => {
                    return decode_reply(&output);
                },
                IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => return Err(anyhow!(lines.join("\n"))),
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => return Err(anyhow!("session exited before the store replied: {exit_reason:?}")),
                IpcRecv::Idle => return Err(missing_reply(session)),
                IpcRecv::Eof => return Err(anyhow!("session disconnected before the store replied; check the stored data before repeating a mutation")),
                IpcRecv::Malformed => return Err(anyhow!("invalid IPC reply from session")),
                // Only structured output proves that the store handled the request.
                _ => {},
            }
        }
    })();
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .context("disconnecting store client")
        .non_fatal();
    result
}

fn missing_reply(session: &SessionName) -> anyhow::Error {
    anyhow!("no reply from hocus-store in session {session} within 30 seconds; check that hocus-store is loaded. The request was not retried; read the stored data before repeating a mutation")
}

fn decode_reply(output: &str) -> Result<serde_json::Value> {
    let reply: serde_json::Value = serde_json::from_str(output)
        .context("invalid store reply (the session may need an updated hocus-store)")?;
    match reply.get("ok").and_then(serde_json::Value::as_bool) {
        Some(true) => reply
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("store reply has no data")),
        Some(false) => Err(anyhow!(
            "{}",
            reply
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("store request failed")
        )),
        None => Err(anyhow!(
            "store reply has no success status; update the session's hocus-store"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_structured_success_is_success() {
        assert_eq!(
            decode_reply(r#"{"ok":true,"data":{"id":7}}"#).unwrap()["id"],
            7
        );
        assert_eq!(
            decode_reply(r#"{"ok":true,"data":"remembered p"}"#).unwrap(),
            "remembered p"
        );
        assert!(decode_reply(r#"{"ok":false,"error":"conflict"}"#)
            .unwrap_err()
            .to_string()
            .contains("conflict"));
        for bad in ["raw error", r#"{"id":7}"#, r#"{"ok":true}"#] {
            assert!(decode_reply(bad).is_err());
        }
    }
}
