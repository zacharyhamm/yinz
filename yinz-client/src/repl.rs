//! A persistent CLI pipe connection to one private session REPL instance.
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    input::actions::Action,
    internal_ids::{SessionName, TerminalId},
};

const TIMEOUT: Duration = Duration::from_secs(35);

pub struct Client {
    sender: IpcSenderWithContext<ClientToServerMsg>,
    receiver: IpcReceiverWithContext<ServerToClientMsg>,
    configuration: BTreeMap<String, String>,
    caller: Option<TerminalId>,
    pipe_id: String,
    started: bool,
    failed: bool,
}

impl Client {
    pub fn connect(session: &SessionName, caller: Option<TerminalId>) -> Result<Self> {
        Self::connect_at(&yinz_runtime::consts::YINZ_SOCK_DIR.join(session), caller)
    }

    fn connect_at(path: &Path, caller: Option<TerminalId>) -> Result<Self> {
        let socket =
            crate::os_input_output::connect_with_deadline(path, TIMEOUT, Duration::from_millis(50))
                .context("connect REPL to selected session")?;
        let (sender, receiver) =
            crate::os_input_output_unix::setup_ipc(socket, path).context("set up REPL IPC")?;
        Ok(Self {
            sender,
            receiver,
            caller,
            pipe_id: uuid::Uuid::new_v4().to_string(),
            started: false,
            failed: false,
            configuration: BTreeMap::from([
                ("shared".into(), "true".into()),
                ("_yinz_id".into(), uuid::Uuid::new_v4().to_string()),
            ]),
        })
    }

    pub fn eval(&mut self, source: &str) -> Result<serde_json::Value> {
        anyhow::ensure!(
            !self.failed,
            "cannot continue after a transport failure; exit and reconnect the REPL"
        );
        self.started = true;
        let result = self.request("__yinz_repl_eval", Some(source.into()), TIMEOUT);
        self.failed = result.is_err();
        result
    }

    pub fn close(&mut self) -> Result<()> {
        if self.started {
            self.started = false;
            if self.failed {
                let message = self.message("__yinz_repl_close", None, self.pipe_id.clone());
                return self
                    .sender
                    .send_client_msg(message)
                    .context("close failed REPL transport");
            }
            self.request("__yinz_repl_close", None, Duration::from_secs(5))?;
        }
        Ok(())
    }

    fn message(&self, name: &str, payload: Option<String>, pipe_id: String) -> ClientToServerMsg {
        ClientToServerMsg::Action {
            action: Action::CliPipe {
                focus_plugin: false,
                pipe_id,
                name: Some(name.into()),
                payload,
                plugin: Some("yinz:repl".into()),
                args: None,
                configuration: Some(self.configuration.clone()),
                launch_new: false,
                skip_cache: false,
                floating: None,
                in_place: None,
                cwd: None,
                pane_title: Some("Scheme REPL".into()),
            },
            terminal_id: self.caller,
            client_id: None,
            is_cli_client: true,
        }
    }

    fn request(
        &mut self,
        name: &str,
        payload: Option<String>,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        let pipe_id = self.pipe_id.clone();
        self.sender
            .send_client_msg(self.message(name, payload, pipe_id.clone()))
            .context("send REPL expression; it was not retried")?;
        let deadline = Instant::now() + timeout;
        let mut reply = None;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "session REPL timed out; the expression was not retried"
            );
            self.receiver
                .set_recv_timeout(Some(remaining))
                .context("bound REPL reply wait")?;
            match self.receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::CliPipeOutput { pipe_name, output }, _)
                    if pipe_name == pipe_id =>
                {
                    anyhow::ensure!(reply.is_none(), "session REPL sent more than one reply");
                    reply =
                        Some(serde_json::from_str(&output).context("decode session REPL reply")?);
                },
                IpcRecv::Msg(ServerToClientMsg::UnblockCliPipeInput { pipe_name }, _)
                    if pipe_name == pipe_id =>
                {
                    return reply.context("session REPL ended without a reply")
                },
                IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                    bail!("{}", lines.join("\n"))
                },
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                    bail!("session exited: {exit_reason:?}")
                },
                IpcRecv::Idle => bail!("session REPL timed out; the expression was not retried"),
                IpcRecv::Eof => bail!("session disconnected; the expression was not retried"),
                IpcRecv::Malformed => bail!("invalid IPC reply from session"),
                IpcRecv::Msg(..) => {},
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.started {
            let message = self.message("__yinz_repl_close", None, self.pipe_id.clone());
            self.sender
                .send_client_msg(message)
                .context("close abandoned REPL")
                .non_fatal();
        }
        self.sender
            .send_client_msg(ClientToServerMsg::ClientExited)
            .context("disconnect REPL client")
            .non_fatal();
    }
}

#[cfg(test)]
#[path = "unit/repl.rs"]
mod tests;
