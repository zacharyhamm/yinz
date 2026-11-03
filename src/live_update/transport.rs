use super::*;
use std::{
    fs::File,
    io::{Seek, SeekFrom},
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_types::session_update;

pub struct Connection {
    sender: IpcSenderWithContext<ClientToServerMsg>,
    receiver: IpcReceiverWithContext<ServerToClientMsg>,
}
impl Connection {
    fn raw(path: &Path) -> Result<Self> {
        let stream =
            yinz_ipc::consts::ipc_connect(path).context("connect update participant server")?;
        let receiver = IpcReceiverWithContext::new(stream);
        let sender = receiver.get_sender().context("clone update connection")?;
        Ok(Self { sender, receiver })
    }
    pub fn connect(path: &Path) -> Result<Self> {
        // ConnStatus closes its router after replying, so negotiate before opening
        // the coordinator connection. Never probe a legacy server using a new variant.
        let mut probe = Self::raw(path)?;
        probe
            .sender
            .send_client_msg(ClientToServerMsg::ConnStatus)
            .context("probe update capability")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match probe.receive(deadline)? {
                ServerToClientMsg::Connected {
                    update_protocol, ..
                } => {
                    anyhow::ensure!(
                        update_protocol == session_update::PROTOCOL_VERSION,
                        "server does not support this session update protocol"
                    );
                    break;
                },
                ServerToClientMsg::Exit { .. } => bail!("server exited during update negotiation"),
                _ => {},
            }
        }
        Self::raw(path)
    }
    fn receive(&mut self, deadline: Instant) -> Result<ServerToClientMsg> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "update server reply timed out; ownership is unknown until checked"
        );
        self.receiver
            .set_recv_timeout(Some(remaining))
            .context("bound update server reply")?;
        match self.receiver.recv_server_msg() {
            IpcRecv::Msg(message, _) => Ok(message),
            IpcRecv::Idle => bail!("update server reply timed out; inspect the operation journal"),
            IpcRecv::Eof => bail!("update server disconnected; inspect the operation journal"),
            IpcRecv::Malformed => bail!("invalid update server reply"),
        }
    }
    pub fn request(
        &mut self,
        operation: &str,
        command: session_update::Command,
    ) -> Result<serde_json::Value> {
        let token = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + Duration::from_secs(180);
        self.sender
            .send_client_msg_until(
                ClientToServerMsg::SessionUpdate(session_update::Request {
                    token: token.clone(),
                    operation: operation.into(),
                    command,
                }),
                deadline,
            )
            .context("send session update request")?;
        loop {
            match self.receive(deadline)? {
                ServerToClientMsg::SessionUpdateReply(reply)
                    if reply.token == token && reply.operation == operation =>
                {
                    return reply.result.map_err(anyhow::Error::msg)
                },
                ServerToClientMsg::Exit { exit_reason } => {
                    bail!("update server exited: {exit_reason}")
                },
                _ => {},
            }
        }
    }
    pub fn initialize(&mut self, assets: yinz_types::input::cli_assets::CliAssets) -> Result<()> {
        self.sender
            .send_client_msg(ClientToServerMsg::FirstClientConnected { cli_assets: assets })
            .context("initialize private replacement server")
    }
    pub fn rename(&mut self, operation: &str, name: &str) -> Result<()> {
        self.request(
            operation,
            session_update::Command::RenameSession { name: name.into() },
        )?;
        let inventory: session_update::Inventory =
            serde_json::from_value(self.request(operation, session_update::Command::Inventory)?)
                .context("check renamed session identity")?;
        anyhow::ensure!(
            inventory.session == name,
            "session endpoint publication did not complete"
        );
        Ok(())
    }
    pub fn kill(&mut self) -> Result<()> {
        self.sender
            .send_client_msg(ClientToServerMsg::KillSession)
            .context("retire empty source server")
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.sender.send_client_msg_until(
            ClientToServerMsg::ClientExited,
            Instant::now() + Duration::from_millis(100),
        );
    }
}

/// Older servers cannot pin their builtin generation across publication. Probe
/// them with the established message before sending any updater-specific request.
pub fn preflight_local_sessions() -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let entries = match std::fs::read_dir(&*yinz_runtime::consts::YINZ_SOCK_DIR) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("inspect sessions sharing this installation"),
    };
    for entry in entries {
        let entry = entry.context("inspect session endpoint")?;
        if !entry
            .file_type()
            .context("inspect session endpoint type")?
            .is_socket()
        {
            continue;
        }
        let stream = match yinz_ipc::consts::ipc_connect(&entry.path()) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                continue
            },
            Err(error) => return Err(error).context("check installation's live sessions"),
        };
        let receiver = IpcReceiverWithContext::new(stream);
        let sender = receiver
            .get_sender()
            .context("open session capability probe")?;
        let mut probe = Connection { sender, receiver };
        let deadline = Instant::now() + Duration::from_secs(10);
        probe
            .sender
            .send_client_msg_until(ClientToServerMsg::ConnStatus, deadline)
            .context("probe builtin generation compatibility")?;
        loop {
            match probe.receive(deadline)? {
                ServerToClientMsg::Connected {
                    update_protocol, ..
                } => {
                    anyhow::ensure!(update_protocol == session_update::PROTOCOL_VERSION,
                        "session {} uses an older server that cannot preserve its builtin generation; restart that session before updating this installation",
                        entry.file_name().to_string_lossy());
                    break;
                },
                ServerToClientMsg::Exit { .. } => break,
                _ => {},
            }
        }
    }
    Ok(())
}

pub fn command_output(command: &mut Command, input: File, timeout: Duration) -> Result<Vec<u8>> {
    let mut output = tempfile::tempfile().context("create update command output")?;
    let mut errors = tempfile::tempfile().context("create update command errors")?;
    let mut child = command
        .stdin(Stdio::from(input))
        .stdout(output.try_clone().context("clone update output")?)
        .stderr(errors.try_clone().context("clone update errors")?)
        .spawn()
        .context("start update host command")?;
    let deadline = Instant::now() + timeout;
    let result = (|| loop {
        anyhow::ensure!(
            output.metadata().context("measure update output")?.len() <= 4 * 1024 * 1024
                && errors
                    .metadata()
                    .context("measure update diagnostics")?
                    .len()
                    <= 1024 * 1024,
            "update host command exceeded output limit"
        );
        if let Some(status) = child.try_wait().context("wait for update host command")? {
            if !status.success() {
                errors
                    .seek(SeekFrom::Start(0))
                    .context("rewind update errors")?;
                let mut text = String::new();
                errors
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut text)
                    .context("read update errors")?;
                anyhow::ensure!(
                    text.len() <= 1024 * 1024,
                    "update host command exceeded diagnostic limit"
                );
                bail!("update host command failed: {}", text.trim());
            }
            output
                .seek(SeekFrom::Start(0))
                .context("rewind update output")?;
            let mut bytes = Vec::new();
            output
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .context("read update output")?;
            anyhow::ensure!(
                bytes.len() <= 4 * 1024 * 1024,
                "update host command exceeded output limit"
            );
            return Ok(bytes);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "update host command timed out; participant journal must be inspected before retrying"
        );
        std::thread::park_timeout(Duration::from_millis(20));
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn remote_participant(
    host: &str,
    helper: &Path,
    invocation: &ParticipantInvocation,
    env: &yinz_types::envs::EnvironmentVariables,
) -> Result<serde_json::Value> {
    let helper = helper.to_str().context("remote helper path is not UTF-8")?;
    let mut command = yinz_client::remote_sessions::ssh_command(
        host,
        &[helper, yinz_types::update_client::PARTICIPANT_ARG],
        env,
    )?;
    let mut input = tempfile::tempfile().context("stage remote participant invocation")?;
    serde_json::to_writer(&mut input, invocation).context("encode remote invocation")?;
    input
        .seek(SeekFrom::Start(0))
        .context("rewind remote invocation")?;
    serde_json::from_slice::<Result<serde_json::Value, String>>(&command_output(
        &mut command,
        input,
        Duration::from_secs(15 * 60),
    )?)
    .context("decode remote participant reply")?
    .map_err(anyhow::Error::msg)
}

pub fn bootstrap_remote(
    host: &str,
    release: &update::Release,
    env: &yinz_types::envs::EnvironmentVariables,
    operation: &str,
    mut record: impl FnMut(&Path) -> Result<()>,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    uuid::Uuid::parse_str(operation).context("invalid remote update operation")?;
    let script=format!("set -eu; uname -s; uname -m; command -v yinz; mktemp -d \"${{TMPDIR:-/tmp}}/yinz-update.{operation}.XXXXXXXXXX\"");
    let mut command = yinz_client::remote_sessions::ssh_command(host, &["sh", "-c", &script], env)?;
    let output = yinz_process::ssh::bootstrap_output(&mut command, Duration::from_secs(30), None)
        .context("inventory SSH update host")?;
    anyhow::ensure!(
        output.status.success(),
        "SSH update inventory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::str::from_utf8(&output.stdout).context("decode SSH update inventory")?;
    let fields: Vec<_> = text.lines().collect();
    anyhow::ensure!(fields.len() == 4, "unexpected SSH update inventory");
    let os = match fields[0] {
        "Linux" => "linux",
        "Darwin" => "macos",
        other => bail!("unsupported remote operating system {other}"),
    };
    let arch = match fields[1] {
        "x86_64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => bail!("unsupported remote architecture {other}"),
    };
    let executable = PathBuf::from(fields[2]);
    let directory = PathBuf::from(fields[3]);
    anyhow::ensure!(
        executable.is_absolute()
            && directory.is_absolute()
            && directory.file_name().is_some_and(|n| n
                .to_string_lossy()
                .starts_with(&format!("yinz-update.{operation}."))),
        "invalid SSH update paths"
    );
    record(&directory)?;
    let client = http_client()?;
    let prepared = update::prepare_artifact(
        release,
        update::target(os, arch)?,
        &mut |url, output| update::download(&client, url, output),
        &mut |_| {},
    )?;
    let mut archive = tempfile::tempfile().context("stage SSH update helper")?;
    {
        let mut builder = tar::Builder::new(&mut archive);
        builder
            .append_dir_all(".", &prepared.source)
            .context("package SSH update helper")?;
        builder.finish().context("finish SSH update helper")?;
    }
    archive
        .seek(SeekFrom::Start(0))
        .context("rewind SSH update helper")?;
    let mut command = yinz_client::remote_sessions::ssh_command(
        host,
        &["tar", "-xf", "-", "-C", fields[3]],
        env,
    )?;
    command_output(&mut command, archive, Duration::from_secs(600))?;
    let helper = directory.join("yinz");
    let mut command = yinz_client::remote_sessions::ssh_command(
        host,
        &[
            helper.to_str().context("remote helper path")?,
            session_update::CAPABILITIES_ARG,
        ],
        env,
    )?;
    let output = yinz_process::ssh::bootstrap_output(&mut command, Duration::from_secs(20), None)?;
    anyhow::ensure!(
        output.status.success(),
        "new remote executable could not run on {host}"
    );
    let caps: Capabilities =
        serde_json::from_slice(&output.stdout).context("decode remote staged capabilities")?;
    anyhow::ensure!(
        update::version(&caps.build_version)? == update::version(&release.tag_name)?,
        "remote staged executable did not report the selected release version"
    );
    anyhow::ensure!(
        caps.client_server == capabilities().client_server
            && caps.ssh_bootstrap == capabilities().ssh_bootstrap
            && caps.update == capabilities().update
            && caps.migration == capabilities().migration,
        "incompatible remote update helper"
    );
    Ok((executable, directory, helper))
}

pub fn remove_remote_helper(
    host: &str,
    directory: &Path,
    operation: &str,
    env: &yinz_types::envs::EnvironmentVariables,
) -> Result<()> {
    uuid::Uuid::parse_str(operation).context("invalid remote cleanup operation")?;
    anyhow::ensure!(
        directory.is_absolute()
            && directory
                .components()
                .all(|part| !matches!(part, std::path::Component::ParentDir))
            && directory.file_name().is_some_and(|name| name
                .to_string_lossy()
                .starts_with(&format!("yinz-update.{operation}."))),
        "refusing cleanup outside the recorded operation helper directory"
    );
    let mut command = yinz_client::remote_sessions::ssh_command(
        host,
        &[
            "rm",
            "-rf",
            "--",
            directory
                .to_str()
                .context("remote helper path is not UTF-8")?,
        ],
        env,
    )?;
    command_output(
        &mut command,
        tempfile::tempfile().context("create cleanup input")?,
        Duration::from_secs(30),
    )?;
    Ok(())
}
