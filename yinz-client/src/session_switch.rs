//! Preparation owns a second connection; only Commit releases the source.
use crate::{os_input_output::ClientOsApi, remote_sessions, session_discovery::Discovery};
use anyhow::ensure;
use interprocess::local_socket::prelude::*;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::{
    consts::{YINZ_CACHE_DIR, YINZ_SOCK_DIR},
    errors::prelude::*,
    termios::WireTermios,
};
use yinz_types::envs::EnvironmentVariables;
use yinz_types::{diagnostics::ErrorContext, input::cli_assets::CliAssets, session_destination::*};

const STARTUP_OUTPUT_BUDGET: usize = 2 * 1024 * 1024;

fn output_size(message: &ServerToClientMsg) -> Result<usize> {
    match message {
        ServerToClientMsg::Render { content } => Ok(content.len()),
        _ => serde_json::to_vec(message)
            .map(|bytes| bytes.len())
            .context("measure destination output"),
    }
}

#[derive(Default)]
pub(crate) struct LocalConfig {
    pub config: yinz_config::config::Config,
    pub options: yinz_types::input::options::Options,
}

impl LocalConfig {
    pub fn load(args: &yinz_cli::args::CliArgs) -> Result<Self> {
        let (config, options) = yinz_cli::setup::Setup::config_without_layout(args)
            .map_err(|error| anyhow!("{error}"))
            .context("load local configuration without a session layout")?;
        Ok(Self { config, options })
    }

    pub fn reload(&mut self, args: &yinz_cli::args::CliArgs) {
        match Self::load(args) {
            Ok(config) => *self = config,
            Err(error) => log::error!("Failed to reload local config: {error:#}"),
        }
    }
}

pub struct PreparedConnection {
    pub(crate) sender: Option<IpcSenderWithContext<ClientToServerMsg>>,
    pub(crate) receiver: Option<IpcReceiverWithContext<ServerToClientMsg>>,
    pub(crate) buffered: VecDeque<(ServerToClientMsg, ErrorContext)>,
    pub(crate) forward: Option<remote_sessions::ClientForward>,
    pub(crate) machine: Destination,
    pub(crate) name: String,
}

impl Drop for PreparedConnection {
    fn drop(&mut self) {
        if let Some(sender) = &mut self.sender {
            sender
                .send_client_msg_until(
                    ClientToServerMsg::ClientExited,
                    Instant::now() + Duration::from_millis(100),
                )
                .context("detach unused destination")
                .non_fatal();
        }
    }
}

struct Preparation {
    id: String,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<PreparedConnection>>,
    result: Option<Result<PreparedConnection, String>>,
}

impl Preparation {
    fn update(&mut self) {
        if self.result.is_none() {
            self.result = match self.receiver.try_recv() {
                Ok(result) => Some(result.map_err(|error| format!("{error:#}"))),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Session preparation worker failed".into()))
                },
            };
        }
    }
}
impl Drop for Preparation {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        // Dropping the receiver releases late connections without waiting for the worker.
    }
}

pub(crate) struct Sessions<'a> {
    machine: Destination,
    name: String,
    cli_args: &'a yinz_cli::args::CliArgs,
    local_config: &'a mut LocalConfig,
    inventory: Option<(String, Discovery)>,
    preparation: Option<Preparation>,
    workers: Vec<JoinHandle<()>>,
}
impl<'a> Sessions<'a> {
    pub fn new(
        host: Option<String>,
        name: String,
        cli_args: &'a yinz_cli::args::CliArgs,
        local_config: &'a mut LocalConfig,
    ) -> Self {
        Self {
            machine: host.map(Destination::Remote).unwrap_or(Destination::Local),
            name,
            cli_args,
            local_config,
            inventory: None,
            preparation: None,
            workers: Vec::new(),
        }
    }

    pub fn cancel(&mut self) {
        self.inventory = None;
        self.preparation = None;
    }

    pub fn shutdown(&mut self) {
        self.cancel();
        for worker in self.workers.drain(..) {
            join_worker(worker);
        }
    }

    fn reap_finished_workers(&mut self) {
        for worker in self.workers.extract_if(.., |worker| worker.is_finished()) {
            join_worker(worker);
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn rename(&mut self, name: String) {
        self.name = name;
    }

    fn hosts(&self) -> Vec<String> {
        let mut hosts = yinz_runtime::remote_hosts::load(&YINZ_CACHE_DIR).unwrap_or_else(|error| {
            log::warn!("read remembered SSH hosts: {error:#}");
            Vec::new()
        });
        if let Destination::Remote(host) = &self.machine {
            hosts.push(host.clone());
        }
        hosts.retain(|host| yinz_process::ssh::validate_ssh_host(host).is_ok());
        hosts.sort();
        hosts.dedup();
        hosts
    }

    fn inventory(&mut self, id: String) -> Result<ClientReply> {
        if self
            .inventory
            .as_ref()
            .is_none_or(|(current, _)| *current != id)
        {
            self.inventory = None;
            let mut hosts = self.hosts();
            hosts.insert(0, String::new());
            let env = self.local_config.config.env.clone();
            self.inventory = Some((
                id,
                Discovery::start_with(hosts, move |host, cancelled| {
                    if !host.is_empty() {
                        return remote_sessions::discover(host, cancelled, &env);
                    }
                    yinz_cli::sessions::collect_sessions(false)
                        .map_err(std::io::Error::from)
                        .context("scan client-local sessions")
                        .map(|rows| {
                            rows.into_iter()
                                .filter(|(_, _, status)| {
                                    *status == yinz_cli::sessions::SessionListing::Running
                                })
                                .map(|(name, age, listing)| yinz_cli::session_picker::PickerRow {
                                    label: format!("Local / {name}"),
                                    destination: Some(
                                        yinz_cli::session_picker::Destination::Local(name),
                                    ),
                                    age,
                                    listing,
                                    current: false,
                                    status: None,
                                })
                                .collect()
                        })
                }),
            ));
        }
        let (_, discovery) = self.inventory.as_mut().expect("inventory initialized");
        discovery.update();
        let entries = discovery
            .rows(&[])
            .into_iter()
            .filter_map(|row| {
                use yinz_cli::session_picker::Destination as Pick;
                let (destination, name) = match row.destination {
                    Some(Pick::Local(name)) => (Destination::Local, Some(name.into_string())),
                    Some(Pick::Remote { host, name }) => {
                        (Destination::Remote(host), Some(name.into_string()))
                    },
                    None => (
                        if row.label.is_empty() {
                            Destination::Local
                        } else {
                            Destination::Remote(row.label)
                        },
                        None,
                    ),
                };
                if name.is_some() && row.listing != yinz_cli::sessions::SessionListing::Running {
                    return None;
                }
                Some(Entry {
                    destination,
                    name,
                    status: row.status.unwrap_or_else(|| "Running".into()),
                    current: false,
                    saved: false,
                })
            })
            .collect();
        Ok(ClientReply::Inventory(Inventory {
            current_machine: self.machine.clone(),
            entries,
            current_server_only: false,
        }))
    }

    pub fn handle(
        &mut self,
        request: ClientRequest,
        os: &dyn ClientOsApi,
    ) -> (Result<ClientReply>, Option<PreparedConnection>) {
        self.reap_finished_workers();
        let result = (|| match request {
            ClientRequest::Inventory(id) => self.inventory(id),
            ClientRequest::CancelAll => {
                self.cancel();
                Ok(ClientReply::Complete)
            },
            ClientRequest::Switch(SwitchRequest::Cancel(id)) => {
                if self
                    .preparation
                    .as_ref()
                    .is_some_and(|pending| pending.id == id)
                {
                    self.preparation = None;
                }
                if self
                    .inventory
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id)
                {
                    self.inventory = None;
                }
                Ok(ClientReply::Complete)
            },
            ClientRequest::Switch(SwitchRequest::Prepare { id, target }) => {
                ensure!(
                    self.preparation.is_none(),
                    "A session switch is already pending"
                );
                remote_sessions::validate_name(&target.name)?;
                ensure!(
                    !target.saved || target.destination == Destination::CurrentServer,
                    "Only current-server saved sessions can be resurrected"
                );
                let machine = match target.destination {
                    Destination::CurrentServer => self.machine.clone(),
                    Destination::Remote(host) => {
                        ensure!(
                            self.hosts().contains(&host),
                            "SSH host is not remembered by this client"
                        );
                        Destination::Remote(host)
                    },
                    Destination::Local => Destination::Local,
                };
                ensure!(
                    machine != self.machine || target.name != self.name,
                    "Already attached to this session"
                );
                if machine == Destination::Local {
                    check_local_parent(&target.name)?;
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let stop = cancelled.clone();
                let mut assets = if machine == Destination::Local {
                    self.local_config.reload(self.cli_args);
                    crate::startup::StartupContext::detached(
                        self.cli_args,
                        &self.local_config.options,
                        &self.local_config.config.env,
                    )
                    .build_cli_assets(
                        &self.local_config.options,
                        None,
                        false,
                        None,
                    )
                } else {
                    CliAssets {
                        supports_session_requests: true,
                        ..Default::default()
                    }
                };
                assets.terminal_window_size = os.get_terminal_size();
                assets.host_terminal_env =
                    yinz_runtime::host_terminal_env::host_terminal_env_with_overrides(
                        &self.local_config.config.env,
                    );
                assets.timezone = yinz_runtime::client_time::capture_timezone();
                let config = self.cli_args.config.clone();
                let termios = os.original_termios();
                let env = self.local_config.config.env.clone();
                let (sender, receiver) = mpsc::channel();
                let worker = std::thread::Builder::new()
                    .name("session-prepare".into())
                    .spawn(move || {
                        let result = prepare(
                            machine,
                            target.name,
                            target.saved,
                            assets,
                            config.as_deref(),
                            termios,
                            &stop,
                            &env,
                        );
                        let _ = sender.send(result);
                    })
                    .context("start destination preparation")?;
                self.workers.push(worker);
                self.preparation = Some(Preparation {
                    id,
                    cancelled,
                    receiver,
                    result: None,
                });
                Ok(ClientReply::Pending)
            },
            ClientRequest::Switch(SwitchRequest::Status(id) | SwitchRequest::Commit(id)) => {
                let pending = self
                    .preparation
                    .as_mut()
                    .filter(|pending| pending.id == id)
                    .context("Session switch is no longer pending")?;
                pending.update();
                match pending.result.as_ref() {
                    None => Ok(ClientReply::Pending),
                    Some(Ok(_)) => Ok(ClientReply::Ready),
                    Some(Err(error)) => Err(anyhow!(error.clone())),
                }
            },
        })();
        (result, None)
    }

    pub fn commit(&mut self, id: &str) -> Result<PreparedConnection> {
        self.reap_finished_workers();
        let pending = self
            .preparation
            .as_mut()
            .filter(|pending| pending.id == id)
            .context("Session switch is no longer pending")?;
        pending.update();
        let result = pending.result.take().context("Destination is not ready")?;
        self.preparation = None;
        let mut prepared = result.map_err(anyhow::Error::msg)?;
        // Detect a peer that closed after its first frame but before the user committed.
        let receiver = prepared
            .receiver
            .as_mut()
            .context("Destination receiver is missing")?;
        let mut bytes = prepared
            .buffered
            .iter()
            .map(|(message, _)| output_size(message))
            .sum::<Result<usize>>()?;
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            match receiver.recv_server_msg_until(deadline) {
                IpcRecv::Idle => break,
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                    bail!("Destination closed before handoff: {exit_reason}")
                },
                IpcRecv::Msg(message, context) => {
                    bytes += output_size(&message)?;
                    prepared.buffered.push_back((message, context));
                    // Check closure even after a large first render, then leave later frames on the transport.
                    if bytes >= STARTUP_OUTPUT_BUDGET {
                        break;
                    }
                },
                _ => bail!("Destination disconnected before handoff"),
            }
        }
        prepared
            .receiver
            .as_ref()
            .context("Destination receiver is missing")?
            .set_recv_timeout(None)
            .context("remove destination startup receive timeout")?;
        prepared
            .sender
            .as_ref()
            .context("Destination sender is missing")?
            .set_send_timeout(None)
            .context("remove destination startup send timeout")?;
        Ok(prepared)
    }
}

impl Drop for Sessions<'_> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn join_worker(worker: JoinHandle<()>) {
    if worker.join().is_err() {
        log::error!("Session preparation worker panicked");
    }
}

fn prepare(
    machine: Destination,
    name: String,
    saved: bool,
    assets: CliAssets,
    config: Option<&std::path::Path>,
    termios: Option<WireTermios>,
    cancelled: &AtomicBool,
    env: &EnvironmentVariables,
) -> Result<PreparedConnection> {
    let forward = if let Destination::Remote(host) = &machine {
        Some(remote_sessions::own_forward(remote_sessions::connect(
            host, &name, saved, cancelled, env,
        )?))
    } else {
        None
    };
    let socket = if let Some(forward) = &forward {
        forward.socket().to_owned()
    } else {
        let socket = YINZ_SOCK_DIR.join(&name);
        if saved {
            let output = yinz_process::ssh::bootstrap_output(
                resurrection_command(&name, &assets, config, &termios)?.envs(env.inner()),
                Duration::from_secs(60),
                Some(cancelled),
            )
            .context("resurrect current-server session")?;
            ensure!(
                output.status.success(),
                "Session resurrection failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            remote_sessions::parse_endpoint(&output.stdout, Some(&name))?;
        }
        socket
    };
    attach_socket(
        machine,
        name,
        forward,
        &socket,
        assets,
        cancelled,
        Instant::now() + Duration::from_secs(25),
    )
}

fn resurrection_command(
    name: &str,
    assets: &CliAssets,
    config: Option<&std::path::Path>,
    termios: &Option<WireTermios>,
) -> Result<std::process::Command> {
    let mut command =
        std::process::Command::new(std::env::current_exe().context("locate Yinz executable")?);
    for (flag, path) in [
        ("--config", config),
        ("--config-dir", assets.config_dir.as_deref()),
        ("--data-dir", assets.data_dir.as_deref()),
    ] {
        if let Some(path) = path {
            command.arg(flag).arg(path);
        }
    }
    if let Some(max_panes) = assets.max_panes {
        command.arg("--max-panes").arg(max_panes.to_string());
    }
    if assets.is_debug {
        command.arg("--debug");
    }
    command.args([
        "remote-session",
        "--create",
        "--client-version",
        yinz_runtime::consts::VERSION,
        "--contract-version",
        &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
        "--bootstrap-version",
        &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
    ]);
    if let Some(options) = &assets.configuration_options {
        command
            .arg("--configuration-options")
            .arg(serde_json::to_string(options).context("encode local resurrection options")?);
    }
    command.arg("--client-termios").arg(
        serde_json::to_string(termios).context("encode local resurrection terminal settings")?,
    );
    command.arg("--terminal-window-size").arg(
        serde_json::to_string(&assets.terminal_window_size)
            .context("encode local resurrection terminal window size")?,
    );
    command.args(["--", name]);
    Ok(command)
}

pub(crate) fn prepare_recovery(
    host: String,
    name: String,
    assets: CliAssets,
    cancelled: &AtomicBool,
    deadline: Instant,
    env: &EnvironmentVariables,
) -> Result<PreparedConnection> {
    // Recovery never creates or resurrects, including after an original --create.
    let forward = remote_sessions::own_forward(remote_sessions::connect_until(
        &host, &name, false, cancelled, deadline, env,
    )?);
    let socket = forward.socket().to_owned();
    attach_socket(
        Destination::Remote(host),
        name,
        Some(forward),
        &socket,
        assets,
        cancelled,
        deadline,
    )
}

pub(crate) fn attach_socket(
    machine: Destination,
    name: String,
    forward: Option<remote_sessions::ClientForward>,
    socket: &std::path::Path,
    assets: CliAssets,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<PreparedConnection> {
    yinz_process::ssh::check_cancelled(Some(cancelled))?;
    // Connect exactly once: a vanished running session must never become a new session.
    let stream = yinz_ipc::consts::ipc_connect(socket).context("connect to destination session")?;
    stream
        .set_send_timeout(Some(
            remote_sessions::remaining(deadline)?.min(Duration::from_secs(3)),
        ))
        .context("bound destination attach request")?;
    stream
        .set_recv_timeout(Some(Duration::from_millis(100)))
        .context("bound destination attach reply")?;
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender
        .get_receiver()
        .context("read destination attachment")?;
    let mut prepared = PreparedConnection {
        sender: Some(sender),
        receiver: Some(receiver),
        buffered: VecDeque::new(),
        forward,
        machine,
        name,
    };
    prepared
        .sender
        .as_mut()
        .expect("prepared sender")
        .send_client_msg_until(
            ClientToServerMsg::AttachClient {
                cli_assets: assets,
                tab_number_to_focus: None,
                pane_to_focus: None,
            },
            deadline.min(Instant::now() + Duration::from_secs(3)),
        )
        .context("attach destination session")?;
    let mut bytes = 0;
    loop {
        yinz_process::ssh::check_cancelled(Some(cancelled))?;
        ensure!(
            Instant::now() < deadline,
            "Destination did not finish attachment"
        );
        match prepared
            .receiver
            .as_mut()
            .expect("prepared receiver")
            .recv_server_msg_until(deadline.min(Instant::now() + Duration::from_millis(100)))
        {
            IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                return Err(remote_sessions::attachment_rejected(exit_reason))
            },
            IpcRecv::Msg(message, context) => {
                let ready = matches!(message, ServerToClientMsg::Render { .. });
                if !ready {
                    bytes += output_size(&message)?;
                    ensure!(
                        bytes <= STARTUP_OUTPUT_BUDGET,
                        "Destination startup output exceeds limit"
                    );
                }
                prepared.buffered.push_back((message, context));
                if ready {
                    return Ok(prepared);
                }
            },
            IpcRecv::Idle => {},
            _ => bail!("Destination disconnected during attachment"),
        }
    }
}

pub(crate) fn legacy_reconnect(
    target: yinz_types::data::SessionConnection,
    host: Option<String>,
    options: &yinz_types::input::options::Options,
    os: &dyn ClientOsApi,
    env: &EnvironmentVariables,
) -> Result<
    (
        crate::ClientInfo,
        Option<remote_sessions::StartupCancellation>,
    ),
    crate::ClientOutcome,
> {
    let mut cancellation = None;
    let next = (|| {
        let cancelled = if host.is_some() {
            let guard = remote_sessions::StartupCancellation::new()?;
            let flag = guard.cancelled.clone();
            cancellation = Some(guard);
            os.unset_raw_mode()
                .context("restore terminal input before SSH bootstrap")?;
            flag
        } else {
            Arc::new(AtomicBool::new(false))
        };
        legacy_target(target, host, options, cancelled, env)
    })();
    match next {
        Ok(info) => Ok((info, cancellation)),
        Err(error) => {
            let interrupted = cancellation
                .as_ref()
                .is_some_and(remote_sessions::StartupCancellation::is_cancelled);
            let exit_msg = if interrupted {
                "Interrupted while starting up".into()
            } else {
                format!("{error:#}")
            };
            remote_sessions::set_forward(None);
            crate::terminal_escapes::ClientTeardown::new(
                os.get_terminal_size().rows,
                options.support_kitty_keyboard_protocol != Some(false),
            )
            .restore(os, &exit_msg);
            Err(crate::ClientOutcome {
                exit_status: if interrupted {
                    crate::INTERRUPTED_EXIT_STATUS
                } else {
                    1
                },
                exit_msg,
                reconnect_to_session: None,
                prepared_connection: None,
            })
        },
    }
}

fn legacy_target(
    target: yinz_types::data::SessionConnection,
    host: Option<String>,
    options: &yinz_types::input::options::Options,
    cancelled: Arc<AtomicBool>,
    env: &EnvironmentVariables,
) -> Result<crate::ClientInfo> {
    use yinz_types::internal_ids::SessionName;
    if let Some(host) = host {
        ensure!(
            target.layout.is_none() && target.cwd.is_none(),
            "Remote switching does not support layout or directory overrides"
        );
        let name = target
            .name
            .context("Remote switching requires a session name")?;
        let forward = remote_sessions::connect(&host, name.as_str(), true, &cancelled, env)?;
        let socket = forward.socket().to_owned();
        remote_sessions::set_forward(Some(forward));
        return Ok(crate::ClientInfo::Remote {
            host,
            name,
            socket,
            cancelled,
        });
    }
    remote_sessions::set_forward(None);
    let name = target
        .name
        .or_else(yinz_cli::sessions::generate_unique_session_name)
        .context("No unique session name is available")?;
    remote_sessions::validate_name(name.as_str())?;
    check_local_parent(name.as_str())?;
    match yinz_session::sessions::session_status(&name)
        .map_err(std::io::Error::from)
        .context("probe session")?
    {
        yinz_ipc::probe::SessionProbe::Alive => {
            Ok(crate::ClientInfo::Attach(name, options.clone()))
        },
        yinz_ipc::probe::SessionProbe::Absent => {
            if yinz_session::sessions::resurrection_layout(&name)
                .map_err(anyhow::Error::msg)?
                .is_some()
            {
                let path = yinz_runtime::consts::session_layout_cache_file_name(&name);
                Ok(crate::ClientInfo::Resurrect(name, path, false, target.cwd))
            } else {
                Ok(crate::ClientInfo::New(
                    SessionName::new(name.as_str()),
                    target.layout,
                    target.cwd,
                ))
            }
        },
        _ => bail!("Session is unresponsive or inaccessible"),
    }
}

fn check_local_parent(name: &str) -> Result<()> {
    if let Some((owner, _)) = yinz_process::pane_context::current_pane().ok().flatten() {
        ensure!(
            owner.as_str() != name,
            "Yinz does not support nesting a session in itself"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/session_switch.rs"]
mod tests;
