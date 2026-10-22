//! Correlated requests to session services, with no stdin loop or mutation retries.
use std::time::{Duration, Instant};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::{input::actions::Action, internal_ids::SessionName};

pub fn request(
    session: &SessionName,
    action: Action,
    caller: Option<yinz_types::internal_ids::TerminalId>,
) -> Result<serde_json::Value> {
    pipe_request(session, action, caller, decode_reply)
}

pub fn raw_request(
    session: &SessionName,
    action: Action,
    caller: Option<yinz_types::internal_ids::TerminalId>,
) -> Result<serde_json::Value> {
    pipe_request(session, action, caller, |output| {
        let value: serde_json::Value =
            serde_json::from_str(output).context("invalid service reply")?;
        if let Some(error) = value.get("error") {
            return Err(anyhow!("{error}"));
        }
        Ok(value)
    })
}

fn pipe_request(
    session: &SessionName,
    action: Action,
    caller: Option<yinz_types::internal_ids::TerminalId>,
    decode: impl Fn(&str) -> Result<serde_json::Value>,
) -> Result<serde_json::Value> {
    let timeout = Duration::from_secs(30);
    let Action::CliPipe { ref pipe_id, .. } = action else {
        return Err(anyhow!("expected a service pipe request"));
    };
    let pipe_id = pipe_id.clone();
    exchange(
        session,
        timeout,
        ClientToServerMsg::Action {
            action,
            terminal_id: caller,
            client_id: None,
            is_cli_client: true,
        },
        |message| match message {
            ServerToClientMsg::CliPipeOutput { pipe_name, output } if pipe_name == pipe_id => {
                Some(decode(&output))
            },
            _ => None,
        },
    )
}

/// A server rejection, distinct from an unavailable connection or a timed-out reply.
#[derive(Debug)]
pub struct PluginRequestRejected(pub String);

impl std::fmt::Display for PluginRequestRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for PluginRequestRejected {}

pub fn plugin_request(
    session: &SessionName,
    request: yinz_types::cli::PluginRequest,
) -> Result<yinz_types::cli::PluginResponse> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let value = exchange(
        session,
        Duration::from_secs(30),
        ClientToServerMsg::PluginRequest {
            request_id: request_id.clone(),
            request,
        },
        |message| match message {
            ServerToClientMsg::PluginResponse {
                request_id: id,
                result,
            } if id == request_id => Some(
                result
                    .map_err(|error| anyhow::Error::new(PluginRequestRejected(error)))
                    .and_then(|response| {
                        serde_json::to_value(response).context("encode plugin response")
                    }),
            ),
            _ => None,
        },
    )?;
    serde_json::from_value(value).context("decode plugin response")
}

pub fn data_request(
    session: &SessionName,
    request: yinz_types::cli::DataRequest,
) -> Result<serde_json::Value> {
    let request_id = uuid::Uuid::new_v4().to_string();
    exchange(
        session,
        Duration::from_secs(30),
        ClientToServerMsg::DataRequest {
            request_id: request_id.clone(),
            request,
        },
        |message| match message {
            ServerToClientMsg::DataResponse {
                request_id: id,
                result,
            } if id == request_id => Some(
                result
                    .map_err(anyhow::Error::msg)
                    .and_then(|json| serde_json::from_str(&json).context("invalid data reply")),
            ),
            _ => None,
        },
    )
}

pub(crate) fn exchange(
    session: &SessionName,
    timeout: Duration,
    message: ClientToServerMsg,
    decode: impl FnMut(ServerToClientMsg) -> Option<Result<serde_json::Value>>,
) -> Result<serde_json::Value> {
    exchange_at(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(session),
        session,
        timeout,
        message,
        decode,
    )
}

fn exchange_at(
    path: &std::path::Path,
    session: &SessionName,
    timeout: Duration,
    message: ClientToServerMsg,
    mut decode: impl FnMut(ServerToClientMsg) -> Option<Result<serde_json::Value>>,
) -> Result<serde_json::Value> {
    let socket =
        crate::os_input_output::connect_with_deadline(path, timeout, Duration::from_millis(50))
            .context("cannot connect to selected session")?;
    let (mut sender, mut receiver) =
        crate::os_input_output_unix::setup_ipc(socket, path).context("cannot set up CLI IPC")?;
    let deadline = Instant::now() + timeout;
    sender
        .send_client_msg(message)
        .context("cannot send request; outcome may be unknown, read before retrying a mutation")?;
    let result = (|| loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(missing_reply(session));
        }
        receiver
            .set_recv_timeout(Some(remaining))
            .context("cannot bound service reply wait")?;
        match receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => return Err(anyhow!(lines.join("\n"))),
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => return Err(anyhow!("session exited before the service replied: {exit_reason:?}")),
                IpcRecv::Idle => return Err(missing_reply(session)),
                IpcRecv::Eof => return Err(anyhow!("session disconnected before the service replied; check the stored data before repeating a mutation")),
                IpcRecv::Malformed => return Err(anyhow!("invalid IPC reply from session")),
                IpcRecv::Msg(message, _) => if let Some(result) = decode(message) { return result; },

            }
    })();
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .context("disconnecting CLI client")
        .non_fatal();
    result
}

fn missing_reply(session: &SessionName) -> anyhow::Error {
    anyhow!("no reply from session {session} before the deadline; check required background plugins. The request was not retried; read the stored data before repeating a mutation")
}

fn decode_reply(output: &str) -> Result<serde_json::Value> {
    let reply: serde_json::Value = serde_json::from_str(output).context("invalid service reply")?;
    match reply.get("ok").and_then(serde_json::Value::as_bool) {
        Some(true) => reply
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("service reply has no data")),
        Some(false) => Err(anyhow!(
            "{}",
            reply
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("service request failed")
        )),
        None => Err(anyhow!(
            "service reply has no success status; update the session's background plugin"
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

#[cfg(test)]
#[path = "unit/cli_transport.rs"]
mod transport_tests;
