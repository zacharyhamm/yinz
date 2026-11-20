use crate::emulation::terminal::TerminalState;
use anyhow::ensure;
use interprocess::local_socket::Stream;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_cli::args::{CliArgs, TerminalArgs};
use yinz_client::os_input_output::{
    connect_with_deadline, CONNECT_TO_SERVER_RETRY_INTERVAL, CONNECT_TO_SERVER_TIMEOUT,
};
use yinz_config::config::ConfigError;
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::native_media::{self, MediaRoot, Probe};
use yinz_types::data::LayoutInfo;
use yinz_types::envs::EnvironmentVariables;
use yinz_types::input::actions::Action;
use yinz_types::internal_ids::SessionName;
use yinz_types::pane_size::Size;
use yinz_types::render_negotiation as caps;

pub type Sender = Arc<Mutex<IpcSenderWithContext<ClientToServerMsg>>>;
pub struct Connection {
    pub sender: Sender,
    pub receiver: IpcReceiverWithContext<ServerToClientMsg>,
    pub name: String,
    pub endpoint: Endpoint,
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
    connect_to(args, opts, size, None, None)
}
pub fn connect_to(
    args: &TerminalArgs,
    original_opts: &CliArgs,
    size: Size,
    target: Option<&yinz_types::data::SessionConnection>,
    deadline: Option<Instant>,
) -> Result<Connection> {
    let sessions = yinz_session::sessions::get_sessions()
        .map_err(|error| anyhow!("list local sessions: {error:?}"))?;
    let PreparedConnection {
        name,
        new,
        env,
        opening,
    } = prepare_connection(args, original_opts, size, target, &sessions)?;
    anyhow::ensure!(
        deadline.is_none_or(|deadline| Instant::now() < deadline),
        "timed out preparing session {name:?}"
    );
    let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
    let stream = if new {
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
        // The double-fork launcher can exit before the daemon binds its socket.
        connect_with_deadline(
            &path,
            deadline.map_or(CONNECT_TO_SERVER_TIMEOUT, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            }),
            CONNECT_TO_SERVER_RETRY_INTERVAL,
        )
        .context("connect native client to session")?
    } else {
        yinz_ipc::consts::ipc_connect(&path).context("connect native client to session")?
    };
    open(stream, name, Endpoint::Local, opening, deadline)
}

#[derive(Default, Clone, Debug)]
pub enum Endpoint {
    #[default]
    Local,
    Forwarded(Option<String>),
}

impl Endpoint {
    pub fn ansi_attach(&self, name: &str) -> String {
        let name = shell_argument(name);
        match self {
            Self::Local => format!("`yinz attach -- {name}`"),
            Self::Forwarded(Some(host)) => format!("`yinz attach --remote={} -- {name}`", shell_argument(host)),
            Self::Forwarded(None) => format!("`yinz attach --remote HOST -- {name}`, replacing HOST with the host used to establish the forward"),
        }
    }

    fn guidance(&self, name: &str) -> String {
        let server = match self {
            Self::Local => "server",
            Self::Forwarded(_) => "remote server",
        };
        format!("update the {server} or use {}", self.ansi_attach(name))
    }
}

fn shell_argument(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:@".contains(&b))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(crate) fn forwarded(
    stream: Stream,
    host: Option<String>,
    name: String,
    args: &TerminalArgs,
    focus: &yinz_types::data::SessionConnection,
) -> Result<Connection> {
    let opening = ClientToServerMsg::AttachClient {
        cli_assets: yinz_types::input::cli_assets::CliAssets {
            terminal_window_size: args.opening_size(),
            host_terminal_env: yinz_runtime::host_terminal_env::host_terminal_env_with_overrides(
                &Default::default(),
            ),
            timezone: yinz_runtime::client_time::capture_timezone(),
            retains_source_on_switch: !args.headless,
            ..Default::default()
        },
        pane_to_focus: focus.pane_id,
        tab_number_to_focus: focus.tab_position,
    };
    open(stream, name, Endpoint::Forwarded(host), opening, None)
}

fn open(
    stream: Stream,
    name: String,
    endpoint: Endpoint,
    opening: ClientToServerMsg,
    deadline: Option<Instant>,
) -> Result<Connection> {
    let mut sender = IpcSenderWithContext::new(stream);
    let receiver = sender
        .get_receiver()
        .context("clone native session receiver")?;
    sender
        .send_client_msg_until(
            opening,
            deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(10)),
        )
        .context("open native attachment")?;
    sender
        .set_send_timeout(None)
        .context("restore native IPC timeout")?;
    Ok(Connection {
        sender: Arc::new(Mutex::new(sender)),
        receiver,
        name,
        endpoint,
    })
}

#[derive(Default)]
pub(crate) enum Handshake {
    #[default]
    Opening,
    Offered(caps::Offer, Option<Probe>),
    Selected(caps::Agreement, Option<Arc<MediaRoot>>),
}

impl Handshake {
    pub fn timeout(&self, endpoint: &Endpoint, name: &str) -> anyhow::Error {
        match self {
            Self::Opening => anyhow!(
                "session {name:?} did not advertise native negotiation for safe image transport; {}",
                endpoint.guidance(name)
            ),
            Self::Offered(..) => anyhow!("session {name:?} did not answer its native render offer"),
            Self::Selected(..) => anyhow!("session {name:?} did not send its first native repaint"),
        }
    }

    pub fn control(
        &mut self,
        message: &ServerToClientMsg,
        endpoint: &Endpoint,
        name: &str,
        sender: &Sender,
        deadline: Instant,
    ) -> Result<bool> {
        match message {
            ServerToClientMsg::Connected {
                native_render_revision,
                ..
            } => {
                ensure!(
                    matches!(self, Self::Opening),
                    "duplicate Connected from {name:?}"
                );
                let Some(revision) = native_render_revision else {
                    return Err(self.timeout(endpoint, name));
                };
                ensure!(*revision == caps::REVISION, "session {name:?} advertised native negotiation revision {revision}, supported {}; {}", caps::REVISION, endpoint.guidance(name));
                let probe = matches!(endpoint, Endpoint::Local)
                    .then(|| Probe::create(&native_media::directory()).ok())
                    .flatten();
                let offer = caps::Offer::reader(probe.as_ref().map(|p| p.offer.clone()));
                let mut tx = sender
                    .lock()
                    .map_err(|_| anyhow!("native IPC sender lock poisoned"))?;
                tx.send_client_msg_until(
                    ClientToServerMsg::NativeRenderRequest(caps::Request::Offer(offer.clone())),
                    deadline,
                )
                .context("offer native rendering")?;
                tx.set_send_timeout(None)
                    .context("restore native IPC timeout")?;
                *self = Self::Offered(offer, probe);
            },
            ServerToClientMsg::NativeRenderResult(caps::Decision::Withdrawn(high))
                if matches!(self, Self::Selected(..)) =>
            {
                bail!("session {name:?} withdrew native rendering after frame {high}; reattach for a new native repaint")
            },
            ServerToClientMsg::NativeRenderResult(decision) => {
                let Self::Offered(offer, probe) = self else {
                    bail!("unexpected native render result from {name:?}");
                };
                match decision {
                    caps::Decision::Selected(agreement) => {
                        ensure!(offer.accepts(agreement, probe.as_ref().map(|p| p.expected.as_slice())), "session {name:?} selected unoffered native capabilities or an invalid media proof");
                        let root = probe
                            .take()
                            .filter(|_| agreement.media & caps::SHARED != 0)
                            .map(|p| Arc::new(p.root));
                        *self = Self::Selected(agreement.clone(), root);
                    },
                    caps::Decision::Rejected(reason) => {
                        bail!("session {name:?} rejected native rendering: {reason}")
                    },
                    caps::Decision::Withdrawn(_) => {
                        bail!("session {name:?} withdrew native rendering")
                    },
                }
            },
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn begin_frames(&self, state: &mut TerminalState) -> Result<()> {
        let Self::Selected(agreement, root) = self else {
            bail!("native frame arrived before selection");
        };
        state.negotiate(agreement.clone(), root.clone());
        Ok(())
    }
}

pub(crate) struct PreparedConnection {
    name: String,
    new: bool,
    env: EnvironmentVariables,
    pub(crate) opening: ClientToServerMsg,
}

// Resolve and validate the handshake before spawning a server for a new or resurrected session.
pub(crate) fn prepare_connection(
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
    let (config, layout, options, _, _) = yinz_cli::setup::Setup::native_connection_config(opts)
        .map_err(|error| {
            let detail = error.diagnostic_message();
            if matches!(error, ConfigError::IoPath(..)) {
                anyhow!("{detail}. Restore a readable regular file at the reported path and retry.")
            } else {
                anyhow!("{detail}")
            }
        })
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
        layout,
        resurrection,
        size,
        new,
        !args.headless,
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

pub enum Recorder {
    Disabled,
    Pending(PathBuf),
    Writing(File),
}
impl Recorder {
    pub fn new(path: Option<&Path>) -> Self {
        path.map_or(Self::Disabled, |path| Self::Pending(path.to_owned()))
    }

    pub fn check_destination(path: &Path) -> Result<()> {
        use nix::unistd::{access, AccessFlags};
        use std::os::unix::fs::FileTypeExt;

        // Do not create or truncate a recording until a repaint has been applied.
        // Follow a dangling link too: File::create would create its target.
        let mut target = path.to_owned();
        loop {
            match std::fs::metadata(&target) {
                Ok(metadata) => {
                    ensure!(!metadata.is_dir(), "recording destination is a directory");
                    ensure!(
                        !metadata.file_type().is_socket(),
                        "recording destination is a socket"
                    );
                    return access(&target, AccessFlags::W_OK).context("check write access");
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("inspect recording destination"),
            }
            let leaf = target
                .as_os_str()
                .as_encoded_bytes()
                .rsplit(|&b| b == b'/')
                .next();
            ensure!(
                !matches!(leaf, None | Some(b"" | b"." | b"..")),
                "expected a recording file name"
            );
            match target.read_link() {
                Ok(link) => {
                    target.set_file_name(link);
                    continue;
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("resolve recording destination"),
            }
            let parent = target
                .parent()
                .context("locate recording directory")?
                .join(".");
            return access(&parent, AccessFlags::W_OK | AccessFlags::X_OK)
                .with_context(|| format!("check recording directory {}", parent.display()));
        }
    }

    pub fn finish(&self, reason: &str) -> Result<()> {
        if let Self::Pending(path) = self {
            bail!(
                "{reason} before the first repaint. No frames were written to {}",
                path.display()
            );
        }
        Ok(())
    }

    pub fn frame(&mut self, bytes: &[u8]) -> Result<()> {
        if let Self::Pending(path) = self {
            let file = File::create(path.as_path())
                .with_context(|| format!("create native frame recording {}", path.display()))?;
            *self = Self::Writing(file);
        }
        if let Self::Writing(file) = self {
            let len = u32::try_from(bytes.len()).context("native frame too large to record")?;
            file.write_all(&len.to_le_bytes())
                .context("write native frame length")?;
            file.write_all(bytes).context("write native frame")?;
        }
        Ok(())
    }
}

pub(crate) fn exit_error(message: String) -> anyhow::Error {
    if message.starts_with("Initial layout failed at ") {
        anyhow!("{message}\nRetry with --rows or --cols to give the initial layout more room.")
    } else {
        anyhow!("{message}")
    }
}

pub fn headless(mut connection: Connection, args: &TerminalArgs) -> Result<()> {
    let size = args.opening_size();
    let mut state = TerminalState::new(size.rows, size.cols);
    let mut recorder = Recorder::new(args.record.as_deref());
    let started = Instant::now();
    let finish = args
        .duration_secs
        .map(|seconds| started + Duration::from_secs(seconds));
    let mut last_frame = None;
    let mut handshake = Handshake::default();
    loop {
        let deadline = if last_frame.is_some() {
            finish.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600))
        } else {
            (started + Duration::from_secs(10))
                .min(finish.unwrap_or(started + Duration::from_secs(10)))
        };
        if Instant::now() >= deadline {
            if last_frame.is_none() {
                return Err(handshake.timeout(&connection.endpoint, &connection.name));
            }
            break;
        }
        use yinz_ipc::ipc::IpcRecv;
        let message = match connection.receiver.recv_server_msg_until(deadline) {
            IpcRecv::Msg(message, _) => message,
            IpcRecv::Idle => continue,
            IpcRecv::Malformed => bail!(
                "invalid IPC message from native session {:?}",
                connection.name
            ),
            IpcRecv::Eof => bail!(
                "native session {:?} closed the IPC stream before Exit",
                connection.name
            ),
        };
        {
            if handshake.control(
                &message,
                &connection.endpoint,
                &connection.name,
                &connection.sender,
                deadline,
            )? {
                continue;
            }
            match message {
                ServerToClientMsg::RenderFrame { frame } => {
                    if last_frame.is_none() {
                        handshake.begin_frames(&mut state)?;
                    }
                    let applied = state
                        .apply_frame(&frame)
                        .context("apply headless native frame")?;
                    recorder.frame(&frame)?;
                    last_frame = Some(applied.seq);
                    send(
                        &connection.sender,
                        ClientToServerMsg::RenderFrameAck { seq: applied.seq },
                    )?;
                },
                ServerToClientMsg::Exit {
                    exit_reason: yinz_ipc::ipc::ExitReason::Error(message),
                } => return Err(exit_error(message)),
                ServerToClientMsg::SwitchSession { connect_to_session } => {
                    let destination = connect_to_session.name.as_ref().map_or_else(
                        || "a new session".to_owned(),
                        |name| format!("{:?}", name.as_str()),
                    );
                    let boundary = last_frame.map_or_else(
                        || "before the first repaint".to_owned(),
                        |seq| format!("after frame {seq}"),
                    );
                    bail!("session {:?} requested a switch to {destination}; headless capture ended {boundary} on the source", connection.name);
                },
                ServerToClientMsg::Exit { .. } if last_frame.is_none() => {
                    bail!("session exited before native negotiation and repaint")
                },
                ServerToClientMsg::Exit { .. } => break,
                ServerToClientMsg::ConfigFileUpdated if args.record.is_some() => {
                    log::info!("native recording received ConfigFileUpdated");
                },
                ServerToClientMsg::LogError { lines } => {
                    for line in lines {
                        log::error!("{line}");
                    }
                },
                _ => {},
            }
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
