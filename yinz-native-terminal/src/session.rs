use crate::emulation::terminal::TerminalState;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_cli::args::{CliArgs, TerminalArgs};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::LayoutInfo;
use yinz_types::envs::EnvironmentVariables;
use yinz_types::input::actions::Action;
use yinz_types::internal_ids::SessionName;
use yinz_types::pane_size::Size;

pub type Sender = Arc<Mutex<IpcSenderWithContext<ClientToServerMsg>>>;
pub struct Connection {
    pub sender: Sender,
    pub receiver: IpcReceiverWithContext<ServerToClientMsg>,
    pub name: String,
}
pub fn send(sender: &Sender, message: ClientToServerMsg) -> Result<()> {
    sender
        .lock()
        .map_err(|_| anyhow!("native IPC sender lock poisoned"))?
        .send_client_msg(message)
        .context("send native client message")
}
pub fn action(sender: &Sender, action: Action) -> Result<()> {
    send(
        sender,
        ClientToServerMsg::Action {
            action,
            terminal_id: None,
            client_id: None,
            is_cli_client: false,
        },
    )
}
pub fn connect(args: &TerminalArgs, opts: &CliArgs, size: Size) -> Result<Connection> {
    connect_to(args, opts, size, None)
}
pub fn connect_to(
    args: &TerminalArgs,
    original_opts: &CliArgs,
    size: Size,
    target: Option<&yinz_types::data::SessionConnection>,
) -> Result<Connection> {
    let sessions = yinz_session::sessions::get_sessions()
        .map_err(|error| anyhow!("list local sessions: {error:?}"))?;
    let PreparedConnection {
        name,
        new,
        env,
        opening,
    } = prepare_connection(args, original_opts, size, target, &sessions)?;
    let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
    if new {
        std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR)
            .context("create session socket directory")?;
        yinz_client::spawn_server_with_termios(
            &path,
            original_opts.debug,
            &name.clone().into(),
            &env,
            Some(&None),
        )
        .context("spawn native session server")?;
    }
    let stream =
        yinz_ipc::consts::ipc_connect(&path).context("connect native client to session")?;
    let mut sender = IpcSenderWithContext::new(stream);
    let receiver = sender
        .get_receiver()
        .context("clone native session receiver")?;
    sender
        .send_client_msg(opening)
        .context("send native session handshake")?;
    let sender = Arc::new(Mutex::new(sender));
    send(
        &sender,
        ClientToServerMsg::StructuredRenderSupport { supported: true },
    )?;
    Ok(Connection {
        sender,
        receiver,
        name,
    })
}

struct PreparedConnection {
    name: String,
    new: bool,
    env: EnvironmentVariables,
    opening: ClientToServerMsg,
}

// Resolve and validate the handshake before spawning a server for a new or resurrected session.
fn prepare_connection(
    args: &TerminalArgs,
    original_opts: &CliArgs,
    size: Size,
    target: Option<&yinz_types::data::SessionConnection>,
    sessions: &[(SessionName, Duration)],
) -> Result<PreparedConnection> {
    let mut opts = original_opts.clone();
    if let Some(target) = target {
        opts.layout = None;
        opts.layout_string = None;
        if let Some(layout) = &target.layout {
            match layout {
                yinz_types::data::LayoutInfo::File(path, _)
                | yinz_types::data::LayoutInfo::BuiltIn(path) => opts.layout = Some(path.into()),
                yinz_types::data::LayoutInfo::Stringified(text) => {
                    opts.layout_string = Some(text.clone())
                },
            }
        }
    }
    let opts = &opts;
    let name = select_session_name(args, opts, sessions, target)?;
    let new = !sessions.iter().any(|(session, _)| session.as_str() == name);
    let named = target.map_or(
        args.session_name.is_some() || opts.session.is_some(),
        |target| target.name.is_some(),
    );
    let resurrection = if new && named {
        let session_name = SessionName::new(&name);
        yinz_session::sessions::resurrection_layout(&session_name)
            .map_err(anyhow::Error::msg)
            .context("load native session checkpoint")?
            .map(|_| {
                LayoutInfo::File(
                    yinz_runtime::consts::session_layout_cache_file_name(&session_name)
                        .display()
                        .to_string(),
                    Default::default(),
                )
            })
    } else {
        None
    };
    anyhow::ensure!(
        !new || resurrection.is_some()
            || target.is_some()
            || args.create
            || args.session_name.is_none(),
        "session {name:?} does not exist; use --create"
    );
    let (config, layout, options, _, _) = yinz_cli::setup::Setup::from_cli_args(opts)
        .map_err(|error| anyhow!("{error}"))
        .context("load native client configuration")?;
    let cwd = args
        .working_directory
        .as_ref()
        .map(|path| std::fs::canonicalize(path).context("resolve native working directory"))
        .transpose()?;
    let mut opening = yinz_client::native_first_message(
        opts,
        &options,
        &config.env,
        resurrection.or(layout),
        size,
        new,
        target.and_then(|target| target.cwd.clone()).or(cwd),
    );
    if let (
        Some(target),
        ClientToServerMsg::AttachClient {
            tab_number_to_focus,
            pane_to_focus,
            ..
        },
    ) = (target, &mut opening)
    {
        *tab_number_to_focus = target.tab_position;
        *pane_to_focus = target.pane_id;
    }
    Ok(PreparedConnection {
        name,
        new,
        env: config.env,
        opening,
    })
}

fn select_session_name(
    args: &TerminalArgs,
    opts: &CliArgs,
    sessions: &[(yinz_types::internal_ids::SessionName, Duration)],
    target: Option<&yinz_types::data::SessionConnection>,
) -> Result<String> {
    let explicit = match target {
        Some(target) => target.name.as_ref().map(|name| name.as_str()),
        None => args.session_name.as_deref().or(opts.session.as_deref()),
    };
    if let Some(name) = explicit {
        yinz_session::sessions::validate_session_name(name).map_err(|error| anyhow!("{error}"))?;
        return Ok(name.to_owned());
    }
    // An unnamed reconnect requests a new session, regardless of startup arguments
    // or how many sessions are already running.
    if target.is_none() {
        match sessions {
            [(name, _)] => return Ok(name.to_string()),
            [] => {},
            _ => anyhow::bail!(
                "several local sessions exist; select one with `yinz terminal <session>`"
            ),
        }
    }
    loop {
        let name = yinz_session::sessions::generate_random_name();
        if !sessions.iter().any(|(session, _)| session.as_str() == name) {
            return Ok(name);
        }
    }
}

pub struct Recorder(Option<File>);
impl Recorder {
    pub fn new(path: Option<&Path>) -> Result<Self> {
        Ok(Self(
            path.map(File::create)
                .transpose()
                .context("create native frame recording")?,
        ))
    }
    pub fn frame(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(file) = &mut self.0 {
            let len = u32::try_from(bytes.len()).context("native frame too large to record")?;
            file.write_all(&len.to_le_bytes())
                .context("write native frame length")?;
            file.write_all(bytes).context("write native frame")?;
        }
        Ok(())
    }
}

pub fn headless(mut connection: Connection, args: &TerminalArgs) -> Result<()> {
    let mut state = TerminalState::new(args.rows as usize, args.cols as usize);
    let mut recorder = Recorder::new(args.record.as_deref())?;
    let started = Instant::now();
    let finish = args
        .duration_secs
        .map(|seconds| started + Duration::from_secs(seconds));
    let mut seen_frame = false;
    loop {
        let deadline = if seen_frame {
            finish.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600))
        } else {
            (started + Duration::from_secs(10))
                .min(finish.unwrap_or(started + Duration::from_secs(10)))
        };
        if Instant::now() >= deadline {
            if !seen_frame && !finish.is_some_and(|finish| Instant::now() >= finish) {
                anyhow::bail!("server did not send structured rendering; restart the session with this Yinz build");
            }
            break;
        }
        let received = connection.receiver.recv_server_msg_until(deadline);
        if let Some((message, _)) = received.msg() {
            match message {
                ServerToClientMsg::RenderFrame { frame } => {
                    let applied = state
                        .apply_frame(&frame)
                        .context("apply headless native frame")?;
                    seen_frame = true;
                    recorder.frame(&frame)?;
                    send(
                        &connection.sender,
                        ClientToServerMsg::RenderFrameAck { seq: applied.seq },
                    )?;
                },
                ServerToClientMsg::Exit {
                    exit_reason: yinz_ipc::ipc::ExitReason::Error(message),
                } => anyhow::bail!("{message}"),
                ServerToClientMsg::Exit { .. } => break,
                ServerToClientMsg::LogError { lines } => anyhow::bail!("{}", lines.join("\n")),
                _ => {},
            }
        } else if finish.is_some_and(|finish| Instant::now() >= finish) {
            break;
        } else {
            anyhow::bail!("native session stopped responding before a structured frame arrived");
        }
    }
    send(&connection.sender, ClientToServerMsg::ClientExited).non_fatal();
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub rows: usize,
    pub cols: usize,
    pub cell_width: usize,
    pub cell_height: usize,
}

#[cfg(test)]
#[path = "unit/session_tests.rs"]
mod tests;
