#[cfg(target_os = "windows")]
compile_error!("Yinz does not support Windows");

pub use yinz_bus::os_input_output;
pub use yinz_panes::{pane, panes};
pub use yinz_terminal::output;
use yinz_types::internal_ids::TabPosition;
use yinz_types::style::default_palette;

pub use yinz_bus::background_jobs;
pub(crate) use yinz_bus::global_async_runtime;
pub(crate) use yinz_plugin_host as plugins;
pub use yinz_plugin_host::inspect_agent_provider;
#[doc(hidden)]
pub use yinz_plugin_host::run_compilation_worker;
pub use yinz_plugin_host::testing as plugin_testing;
mod configuration;
use configuration::{
    rebind_client_keys, reconfigure_all_clients, reconfigure_client, report_changes_in_config_file,
    SessionConfiguration,
};
mod locked_escape;
pub(crate) use yinz_bus::pty;
pub use yinz_plugin_host::plugin_authoring;
#[cfg(unix)]
pub use yinz_process::pty_host;
mod client_router;
pub(crate) use yinz_bus::pty_writer;
mod route;
pub(crate) use yinz_bus::screen;
pub(crate) use yinz_bus::server_instruction::ServerInstruction;
pub(crate) use yinz_bus::thread_bus;

use background_jobs::{background_jobs_main, BackgroundJob};
use log::info;
use pty_writer::{pty_writer_main, PtyWriteInstruction};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, RwLock,
    },
    thread,
    time::Duration,
};

/// How long shutdown waits for queued client messages to reach their sockets.
/// Generous enough for a live client to drain, short enough that a wedged one
/// cannot keep the server process alive.
const CLIENT_WRITER_FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a freshly started server waits for its first client before giving
/// up. Must exceed the client's own `CONNECT_TO_SERVER_TIMEOUT` (30s) with
/// margin, so the server never leaves before a slow client finishes connecting.
const FIRST_CLIENT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long session shutdown waits for the plugin thread. It exits once every plugin is
/// unloaded, and a plugin that is looping is cancelled first (`PluginEnv::cancel`) — but a
/// guest that never calls the host cannot be, and joining it without a bound once held a dead
/// session's process open indefinitely, with every hook client that connected piling up
/// behind the join. Past this the thread is abandoned and the exit goes on.
const PLUGIN_THREAD_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How many client connections the server will serve at once.
///
/// Every accepted connection costs two threads — the router that reads it and
/// the writer that answers it — and nothing in the accept path could ever say
/// no: `SessionState::new_client` hands out an id unconditionally. A server
/// that leaked connections therefore leaked threads without bound, and one
/// observed here reached 65,538 of them, at which point its listen backlog was
/// permanently full and a blocking `connect` to it never returned. That hung
/// every `yinz`, `yinz ls` and `yinz attach` on the machine, for every
/// unrelated session, because they all sweep the socket directory.
///
/// Far above any real load — a session has a handful of clients, plus the
/// short-lived `yinz action` and hook invocations passing through — so
/// reaching it means something is wrong, and refusing is strictly better than
/// the alternative of taking the whole machine's yinz down with it.
const MAX_CONCURRENT_CLIENT_CONNECTIONS: usize = 512;

/// How long a freshly accepted connection may stay silent before its router
/// gives up on it.
///
/// The cap above is only a fix if a slot cannot be held forever, and a peer
/// that connects and never writes would hold one exactly that long: the
/// router blocks reading it, and every `RemoveClient` — hence every
/// `shutdown_reads` that would unblock the router — is sent by the router
/// itself. So until the first message the read is bounded by this deadline,
/// after which the connection is treated as abandoned and its slot released.
///
/// Only the first message: every real client writes the moment it connects
/// (`AttachClient`, `ConnStatus`, an `Action`), and a later read must not be
/// bounded, because a deadline expiring halfway through a large frame would
/// desynchronise a connection that was merely slow. Equal to the client's
/// `CONNECT_TO_SERVER_TIMEOUT`, so it cannot fire on a client that is only
/// slow to be scheduled.
const FIRST_CLIENT_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the accept loop pauses after an error before trying again, so that
/// a persistent failure (`EMFILE`, typically) does not become a spin.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Holds one of the `MAX_CONCURRENT_CLIENT_CONNECTIONS` slots for as long as a
/// router thread lives.
///
/// A guard rather than a decrement at the end of the router, so that a router
/// which panics still gives its slot back — the panic hook turns a router panic
/// into a server error rather than an unwind that skips cleanup, but a leaked
/// slot would be permanent either way.
struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
use yinz_runtime::envs;
use yinz_storage::data_access::DataAccess;

use yinz_types::input::cli_assets::CliAssets;

use crate::{
    os_input_output::ServerOsApi,
    panes::PaneId,
    plugins::{plugin_thread_main, PluginInstruction},
    pty::{get_default_shell, PtyInstruction},
    screen::ScreenInstruction,
    thread_bus::{Bus, ThreadSenders},
};
use route::{route_thread_main, NotificationEnd};
use yinz_pty::{pty_thread_main, Pty};
use yinz_runtime::channels;
use yinz_runtime::channels::ChannelWithContext;
use yinz_runtime::channels::{SenderWithContext, Shutdown};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::errors::FatalError;
use yinz_runtime::home::get_default_data_dir;
use yinz_runtime::home::layout_dir_or_default;
use yinz_runtime::home::user_plugin_dir_or_default;
use yinz_screen::screen_thread_main;
use yinz_types::diagnostics::ContextType;
use yinz_types::style::Style;
use {
    yinz_config::config::Config, yinz_ipc::ipc::ClientAttributes, yinz_ipc::ipc::ExitReason,
    yinz_ipc::ipc::ServerToClientMsg, yinz_runtime::consts::DEFAULT_SCROLL_BUFFER_SIZE,
    yinz_runtime::consts::SCROLL_BUFFER_SIZE, yinz_types::data::InputMode,
    yinz_types::input::actions::Action, yinz_types::input::command::RunCommand,
    yinz_types::input::command::TerminalAction, yinz_types::input::keybinds::Keybinds,
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Layout,
    yinz_types::input::layout::PluginAlias, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::options::Options,
    yinz_types::input::plugins::PluginAliases,
};

pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};

pub(crate) struct SessionMetaData {
    shutdown: Shutdown,
    pub senders: ThreadSenders,
    pub data_access: Arc<DataAccess>,
    pub default_shell: Option<TerminalAction>,
    pub current_input_modes: HashMap<ClientId, InputMode>,
    pub session_configuration: SessionConfiguration,
    pub key_passthrough_clients: HashMap<ClientId, PaneId>,
    pub locked_escapes: HashMap<ClientId, locked_escape::LockedEscape>,
    // Holds Locked through palette note editing; final dismissal resumes notification browsing.
    command_palette_restore_modes: HashMap<ClientId, InputMode>,
    pending_mouse_updates: Vec<(ClientId, bool)>,
    screen_thread: Option<thread::JoinHandle<()>>,
    pty_thread: Option<thread::JoinHandle<()>>,
    plugin_thread: Option<thread::JoinHandle<()>>,
    pty_writer_thread: Option<thread::JoinHandle<()>>,
    background_jobs_thread: Option<thread::JoinHandle<()>>,
    config_file_path: Option<PathBuf>,
    /// The session's `--config-dir` / `YINZ_CONFIG_DIR`, kept so that a config reload can
    /// re-derive the folders that hang off it. Without it, reloading a config that does not
    /// name `user_plugin_dir` explicitly would wipe the one derived at startup.
    config_dir: Option<PathBuf>,
}

impl SessionMetaData {
    fn change_input_mode(
        &mut self,
        client_id: ClientId,
        input_mode: InputMode,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        self.locked_escapes.remove(&client_id);
        let input_mode = if self.command_palette_restore_modes.contains_key(&client_id) {
            InputMode::Locked
        } else {
            input_mode
        };
        let base_mode = self
            .session_configuration
            .get_client_default_input_mode(&client_id);
        self.current_input_modes.insert(client_id, input_mode);
        self.senders
            .send_to_screen(ScreenInstruction::ChangeMode(
                input_mode,
                Some(base_mode),
                client_id,
                completion,
            ))
            .context("changing client input mode")
    }

    fn set_command_palette_open(&mut self, client_id: ClientId, open: bool) -> Result<()> {
        let mode = if open {
            let previous_mode = self
                .current_input_modes
                .get(&client_id)
                .copied()
                .unwrap_or_else(|| {
                    self.session_configuration
                        .get_client_default_input_mode(&client_id)
                });
            self.command_palette_restore_modes
                .entry(client_id)
                .or_insert(
                    if matches!(previous_mode, InputMode::Locked | InputMode::Notifications) {
                        previous_mode
                    } else {
                        InputMode::Normal
                    },
                );
            InputMode::Locked
        } else if let Some(mode) = self.command_palette_restore_modes.remove(&client_id) {
            mode
        } else {
            return Ok(());
        };
        self.change_input_mode(client_id, mode, None)
            .context("updating command palette input lock")
    }

    pub fn get_client_keybinds_and_mode(
        &self,
        client_id: &ClientId,
    ) -> Option<(&Keybinds, &InputMode, InputMode)> {
        // (keybinds, current_input_mode,
        // default_input_mode)
        let client_keybinds = self.session_configuration.get_client_keybinds(client_id);
        let default_input_mode = self
            .session_configuration
            .get_client_default_input_mode(client_id);
        match self.current_input_modes.get(client_id) {
            Some(client_input_mode) => {
                Some((client_keybinds, client_input_mode, default_input_mode))
            },
            _ => None,
        }
    }
    pub fn remove_key_passthrough_client(&mut self, client_id: ClientId) {
        self.remove_key_passthrough_client_with_notify(client_id, true);
    }
    /// Drop the per-client state of a client that has gone. A pane it was passing keys through
    /// to hears that it lost focus.
    fn forget_client(&mut self, client_id: ClientId) {
        self.remove_key_passthrough_client(client_id);
        self.current_input_modes.remove(&client_id);
        self.command_palette_restore_modes.remove(&client_id);
        self.pending_mouse_updates
            .retain(|(client, _)| *client != client_id);
        self.session_configuration.remove_client(client_id);
    }
    pub fn remove_key_passthrough_client_with_notify(
        &mut self,
        client_id: ClientId,
        notify_guest: bool,
    ) {
        self.locked_escapes.remove(&client_id);
        if let Some(pane_id) = self.key_passthrough_clients.remove(&client_id) {
            let pane_still_active = self.key_passthrough_clients.values().any(|p| *p == pane_id);
            if !pane_still_active
                && notify_guest
                && let PaneId::Terminal(terminal_id) = pane_id
            {
                let frame = yinz_ipc::nested_session::encode_frame(
                    &yinz_ipc::nested_session::NestedSessionMessage::FocusLost,
                );
                let _ = self.senders.send_to_pty_writer(PtyWriteInstruction::Write(
                    frame,
                    terminal_id,
                    None,
                ));
            }
        }
    }
    pub fn change_mode_for_all_clients(&mut self, input_mode: InputMode) {
        self.locked_escapes.clear();
        let all_clients: Vec<ClientId> = self.current_input_modes.keys().copied().collect();
        for client_id in all_clients {
            if !self.command_palette_restore_modes.contains_key(&client_id) {
                self.current_input_modes.insert(client_id, input_mode);
            }
        }
    }
}

impl SessionMetaData {
    /// Queue the screen's and plugins' cleanup for a departed connection.
    /// A failed send means that consumer has already exited.
    fn detach_client(&self, client_id: ClientId) {
        self.senders
            .send_to_screen(ScreenInstruction::RemoveClient(client_id))
            .with_context(|| format!("removing client {client_id} from the screen"))
            .non_fatal();
        self.senders
            .send_to_plugin(PluginInstruction::RemoveClient(client_id))
            .with_context(|| format!("removing client {client_id} from the plugins"))
            .non_fatal();
    }
}

impl Drop for SessionMetaData {
    fn drop(&mut self) {
        let started = std::time::Instant::now();
        log::info!("shutdown: worker joins starting");
        self.shutdown.begin();
        let screen_thread = self.screen_thread.take();
        let plugin_thread = self.plugin_thread.take();
        self.senders.exit_threads(
            || {
                if let Some(screen_thread) = screen_thread {
                    log::info!("shutdown: joining screen at {:?}", started.elapsed());
                    let _ = screen_thread.join();
                    log::info!("shutdown: joined screen at {:?}", started.elapsed());
                }
            },
            || {
                if let Some(plugin_thread) = plugin_thread {
                    log::info!("shutdown: joining plugin at {:?}", started.elapsed());
                    join_with_timeout(plugin_thread, PLUGIN_THREAD_EXIT_TIMEOUT, "plugin");
                    log::info!("shutdown: plugin join returned at {:?}", started.elapsed());
                }
            },
        );
        if let Some(pty_thread) = self.pty_thread.take() {
            log::info!("shutdown: joining pty at {:?}", started.elapsed());
            let _ = pty_thread.join();
            log::info!("shutdown: joined pty at {:?}", started.elapsed());
        }
        if let Some(pty_writer_thread) = self.pty_writer_thread.take() {
            log::info!("shutdown: joining pty_writer at {:?}", started.elapsed());
            let _ = pty_writer_thread.join();
            log::info!("shutdown: joined pty_writer at {:?}", started.elapsed());
        }
        if let Some(background_jobs_thread) = self.background_jobs_thread.take() {
            log::info!(
                "shutdown: joining background_jobs at {:?}",
                started.elapsed()
            );
            let _ = background_jobs_thread.join();
            log::info!(
                "shutdown: joined background_jobs at {:?}",
                started.elapsed()
            );
        }
    }
}

/// Join `handle`, giving up after `timeout` with an error in the log rather than waiting on a
/// thread that may never finish. See `PLUGIN_THREAD_EXIT_TIMEOUT`.
fn join_with_timeout(handle: thread::JoinHandle<()>, timeout: Duration, name: &str) {
    let started = std::time::Instant::now();
    while !handle.is_finished() {
        if started.elapsed() >= timeout {
            log::error!(
                "the {name} thread did not exit within {timeout:?} of being told to; leaving it \
                 behind"
            );
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = handle.join();
}

/// Remove a client from session state and synthesize empty
/// `ForwardedReplyFromHost` instructions for any host-query forwards
/// that had been dispatched to it. Without this, a client that goes
/// away while holding an in-flight forward leaves `Screen`'s
/// `forward_in_flight` flag stuck — the flag is only released by
/// replies, and no reply will ever come.
fn remove_client_and_flush_forwards(
    client_id: ClientId,
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    let _ = os_input.remove_client(client_id);
    let pipes: Vec<_> = session_state
        .read()
        .to_anyhow()
        .context("access client state")?
        .pipes
        .iter()
        .filter(|(_, owner)| **owner == client_id)
        .map(|(pipe, _)| pipe.clone())
        .collect();
    with_session_mut(session_data, |session| {
        if !pipes.is_empty() {
            session
                .senders
                .send_to_plugin(PluginInstruction::CliPipesDisconnected(pipes))
                .context("retiring disconnected CLI pipes")
                .non_fatal();
        }
        session.forget_client(client_id);
        session.detach_client(client_id);
    })?;
    let stuck_tokens = session_state
        .write()
        .to_anyhow()
        .context("access client state")?
        .remove_client(client_id);
    if stuck_tokens.is_empty() {
        return Ok(());
    }
    if let Some(session) = session_data
        .read()
        .to_anyhow()
        .context("access client state")?
        .as_ref()
    {
        for token in stuck_tokens {
            let _ = session
                .senders
                .send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                    token,
                    reply_bytes: Vec::new(),
                });
        }
    }
    Ok(())
}

fn router_spawn_failed(
    error: std::io::Error,
    client_id: ClientId,
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) {
    Err::<(), _>(error)
        .with_context(|| format!("start router for client {client_id}"))
        .non_fatal();
    remove_client_and_flush_forwards(client_id, os_input, session_state, session_data).non_fatal();
}

/// Whether `client_id` is an attached interactive client. A plugin keeps the id of the client
/// that loaded it, and an instruction can be queued before its client leaves and handled after,
/// so a per-client instruction can name a client that has gone. Handling it would leave state
/// behind for a client that never comes back. A CLI or warp connection is not attached either:
/// it has no mode, keybinds or focus.
fn is_attached(client_id: ClientId, session_state: &Arc<RwLock<SessionState>>) -> Result<bool> {
    Ok(session_state
        .read()
        .to_anyhow()
        .context("failed to read which clients are attached")?
        .is_interactive(&client_id))
}

/// `ChangeMode`. Only an attached client has a mode. Any other id fails the request, which a
/// CLI caller reports; it names no attached client only when none is attached.
fn change_client_mode(
    client_id: ClientId,
    input_mode: InputMode,
    completion: Option<NotificationEnd>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    if !is_attached(client_id, session_state)? {
        if let Some(mut completion) = completion {
            completion.set_exit_status(1);
            completion.set_error_message("No attached client to switch mode for".to_owned());
        }
        return Ok(());
    }
    with_session_mut(session_data, |session| {
        session
            .change_input_mode(client_id, input_mode, completion)
            .non_fatal()
    })?;
    Ok(())
}

/// `KeyPassthroughChanged`. Only an attached client has focus to pass keys through.
fn change_key_passthrough(
    client_id: ClientId,
    new_pane_id: PaneId,
    should_route: bool,
    entered_from_direction: Option<yinz_types::geometry::Direction>,
    notify_guest: bool,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    if !is_attached(client_id, session_state)? {
        return Ok(());
    }
    with_session_mut(session_data, |session| {
        session.locked_escapes.remove(&client_id);
        let previous_pane = session.key_passthrough_clients.get(&client_id).copied();

        if should_route {
            if previous_pane != Some(new_pane_id) {
                if previous_pane.is_some() {
                    session.remove_key_passthrough_client_with_notify(client_id, notify_guest);
                }
                let pane_already_active = session
                    .key_passthrough_clients
                    .values()
                    .any(|p| *p == new_pane_id);
                session
                    .key_passthrough_clients
                    .insert(client_id, new_pane_id);
                if !pane_already_active && let PaneId::Terminal(terminal_id) = new_pane_id {
                    let frame = yinz_ipc::nested_session::encode_frame(
                        &yinz_ipc::nested_session::NestedSessionMessage::FocusGained {
                            from_direction: entered_from_direction,
                        },
                    );
                    let _ = session
                        .senders
                        .send_to_pty_writer(PtyWriteInstruction::Write(frame, terminal_id, None));
                }
            }
        } else if previous_pane.is_some() {
            session.remove_key_passthrough_client_with_notify(client_id, notify_guest);
        }
    })?;
    Ok(())
}

/// Remove a watcher, and any state its id picked up while it was a plain connection.
fn remove_watcher_client(
    client_id: ClientId,
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    with_session_mut(session_data, |session| {
        session.forget_client(client_id);
        let _ = session
            .senders
            .send_to_screen(ScreenInstruction::RemoveWatcherClient(client_id));
        // Whatever the plugins kept for the connection before it became a watcher.
        session
            .senders
            .send_to_plugin(PluginInstruction::RemoveClient(client_id))
            .with_context(|| format!("removing watcher {client_id} from the plugins"))
            .non_fatal();
    })?;
    os_input
        .remove_client(client_id)
        .context("removing a watcher client")
        .non_fatal();
    session_state
        .write()
        .to_anyhow()
        .context("access client state")?
        .remove_watcher(client_id);
    Ok(())
}

/// The live session, if there is one. `None` from server start until `FirstClientConnected`
/// builds it, and a `yinz ls` probe, a watcher, and the `RemoveClient` that ends every route
/// can all arrive in that window: an arm that finds no session is a no-op then. A read lock:
/// sending and cloning need no exclusivity, and the route threads read this lock too.
fn with_session<R>(
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    f: impl FnOnce(&SessionMetaData) -> R,
) -> Result<Option<R>> {
    Ok(session_data
        .read()
        .to_anyhow()
        .context("read session metadata")?
        .as_ref()
        .map(f))
}

/// As [`with_session`], for the arms that change the session.
fn with_session_mut<R>(
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    f: impl FnOnce(&mut SessionMetaData) -> R,
) -> Result<Option<R>> {
    Ok(session_data
        .write()
        .to_anyhow()
        .context("write session metadata")?
        .as_mut()
        .map(f))
}

macro_rules! remove_client {
    ($client_id:expr_2021, $os_input:expr_2021, $session_state:expr_2021, $session_data:expr_2021) => {
        $crate::remove_client_and_flush_forwards(
            $client_id,
            &mut $os_input,
            &$session_state,
            &$session_data,
        )
        .non_fatal();
    };
}

type SessionReplySender =
    std::sync::mpsc::Sender<Result<yinz_types::session_destination::ClientReply, String>>;
/// Plugin session requests awaiting their client's reply, by client and request token.
type PendingSessionRequests = HashMap<(ClientId, String), (SessionReplySender, std::time::Instant)>;
/// How long an unanswered session request is remembered. The plugin stops waiting sooner.
const SESSION_REQUEST_RETENTION: Duration = Duration::from_secs(5);

macro_rules! send_to_client {
    ($client_id:expr_2021, $os_input:expr_2021, $msg:expr_2021, $session_state:expr_2021, $session_data:expr_2021) => {
        let send_to_client_res = $os_input.send_to_client($client_id, $msg);
        if let Err(e) = send_to_client_res {
            // Try to recover the message
            let context = match e.downcast_ref::<YinzError>() {
                Some(YinzError::ClientTooSlow { .. }) => {
                    format!(
                        "client {} is processing server messages too slow",
                        $client_id
                    )
                },
                _ => {
                    format!("failed to route server message to client {}", $client_id)
                },
            };
            // Log it so it isn't lost
            Err::<(), _>(e).context(context).non_fatal();
            // failed to send to client, remove it
            remove_client!($client_id, $os_input, $session_state, $session_data);
        }
    };
}

/// Forward a plugin's session request to its acting client. A client that has left, or that
/// predates session requests and would discard one unread, is never sent it: the plugin gets
/// the reason at once instead of waiting out its timeout.
fn route_session_request(
    client: ClientId,
    request: yinz_types::session_destination::Request,
    reply: SessionReplySender,
    pending: &mut PendingSessionRequests,
    mut os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) {
    pending.retain(|_, (_, at)| at.elapsed() < SESSION_REQUEST_RETENTION);
    let refusal = match session_state.read() {
        Ok(state) if !state.is_interactive(&client) => {
            Some("The requesting client is no longer attached")
        },
        Ok(state) if !state.supports_session_requests(&client) => {
            Some(yinz_types::session_destination::UNSUPPORTED_CLIENT)
        },
        Ok(_) => None,
        Err(_) => Some("Cannot read which clients are attached"),
    };
    if let Some(refusal) = refusal {
        let _ = reply.send(Err(refusal.to_owned()));
        return;
    }
    pending.insert(
        (client, request.token.clone()),
        (reply, std::time::Instant::now()),
    );
    send_to_client!(
        client,
        os_input,
        ServerToClientMsg::SessionRequest(request),
        session_state,
        session_data
    );
}

fn flush_pending_mouse_updates(
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    let updates = with_session_mut(session_data, |session| {
        std::mem::take(&mut session.pending_mouse_updates)
    })?
    .unwrap_or_default();
    if updates.is_empty() {
        return Ok(());
    }
    let attached: HashSet<ClientId> = session_state
        .read()
        .to_anyhow()
        .context("failed to read which clients are attached")?
        .interactive_client_ids()
        .into_iter()
        .collect();
    // Client delivery can remove disconnected clients and must run outside both session locks.
    for (client, enabled) in updates {
        if !attached.contains(&client) {
            continue;
        }
        send_to_client!(
            client,
            *os_input,
            ServerToClientMsg::SetMouseMode { enabled },
            session_state,
            session_data
        );
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionState {
    clients: HashSet<ClientId>,
    /// The id the next connection gets, or `None` once `u32::MAX` has been handed out. Ids are
    /// never reused, so an instruction that names a departed connection can never act on a
    /// newer one.
    next_client_id: Option<u32>,
    interactive_clients: HashSet<ClientId>,
    pipes: HashMap<String, ClientId>,     // String => pipe_id
    watchers: HashSet<ClientId>,          // watcher clients (read-only observers)
    last_active_client: Option<ClientId>, // last client that sent a Key message
    /// Host-query forward tokens that have been dispatched to a
    /// specific client and are waiting for a reply. Used to clean up
    /// when that client disconnects (or when there's no client to
    /// dispatch to in the first place) — each stuck token gets an
    /// empty synthetic reply so `Screen`'s `forward_in_flight` slot
    /// releases and the queued forwards keep moving.
    forwards_in_flight: HashMap<u32, ClientId>,
    /// Attached clients that advertised `CliAssets::supports_session_requests`. Older clients
    /// share the contract version but cannot decode `ServerToClientMsg::SessionRequest`.
    session_request_clients: HashSet<ClientId>,
}

impl SessionState {
    pub fn new() -> Self {
        SessionState {
            clients: HashSet::new(),
            next_client_id: Some(1),
            interactive_clients: HashSet::new(),
            pipes: HashMap::new(),
            watchers: HashSet::new(),
            last_active_client: None,
            forwards_in_flight: HashMap::new(),
            session_request_clients: HashSet::new(),
        }
    }
    /// Number a new connection, from 1 up. `None` once every id has been used: handing one out
    /// twice would let a departed connection's late instructions reach the new one.
    pub fn new_client(&mut self) -> Option<ClientId> {
        let id = self.next_client_id?;
        self.next_client_id = id.checked_add(1);
        let client_id = ClientId::new(id);
        self.clients.insert(client_id);
        Some(client_id)
    }
    pub fn associate_pipe_with_client(&mut self, pipe_id: String, client_id: ClientId) {
        self.pipes.insert(pipe_id, client_id);
    }
    /// Remove a client and return any host-query tokens that had
    /// been dispatched to this client and were still awaiting a
    /// reply. Callers must synthesize an empty reply for each token
    /// (via `ScreenInstruction::ForwardedReplyFromHost`) so
    /// `Screen`'s in-flight slot releases and any queued forwards
    /// can proceed.
    pub fn remove_client(&mut self, client_id: ClientId) -> Vec<u32> {
        self.clients.remove(&client_id);
        self.interactive_clients.remove(&client_id);
        self.session_request_clients.remove(&client_id);
        self.pipes.retain(|_p_id, c_id| c_id != &client_id);
        self.clear_last_active_client(client_id);
        let stuck: Vec<u32> = self
            .forwards_in_flight
            .iter()
            .filter_map(|(token, owner)| (*owner == client_id).then_some(*token))
            .collect();
        for token in &stuck {
            self.forwards_in_flight.remove(token);
        }
        stuck
    }
    pub fn client_ids(&self) -> Vec<ClientId> {
        self.clients.iter().copied().collect()
    }
    fn disconnect_targets(&self, initiating_client: ClientId) -> Vec<ClientId> {
        let mut targets: Vec<_> = self
            .clients
            .iter()
            .copied()
            .filter(|client| *client != initiating_client)
            .collect();
        targets.sort_unstable();
        targets
    }
    fn interactive_client_ids(&self) -> Vec<ClientId> {
        self.interactive_clients.iter().copied().collect()
    }
    fn is_interactive(&self, client_id: &ClientId) -> bool {
        self.interactive_clients.contains(client_id)
    }
    fn has_interactive_clients(&self) -> bool {
        !self.interactive_clients.is_empty()
    }
    pub fn watcher_client_ids(&self) -> Vec<ClientId> {
        self.watchers.iter().copied().collect()
    }
    pub fn get_pipe(&self, pipe_name: &str) -> Option<ClientId> {
        self.pipes.get(pipe_name).copied()
    }
    pub fn active_clients_are_connected(&self) -> bool {
        self.has_interactive_clients()
    }
    pub fn convert_client_to_watcher(&mut self, client_id: ClientId) {
        self.clients.remove(&client_id);
        self.interactive_clients.remove(&client_id);
        self.session_request_clients.remove(&client_id);
        self.watchers.insert(client_id);
    }
    pub fn is_watcher(&self, client_id: &ClientId) -> bool {
        self.watchers.contains(client_id)
    }
    pub fn remove_watcher(&mut self, client_id: ClientId) {
        self.watchers.remove(&client_id);
    }
    pub fn set_last_active_client(&mut self, client_id: ClientId) {
        self.last_active_client = Some(client_id);
    }
    pub fn get_last_active_client(&self) -> Option<ClientId> {
        self.last_active_client
    }
    /// The client a CLI action acts for: the attached client that last pressed a key, else the
    /// client the action names, else the longest-attached client. With nobody attached it is
    /// the CLI connection itself, so an action that needs an attached client says there is none.
    pub fn cli_action_target(
        &self,
        cli_client_id: ClientId,
        requested: Option<ClientId>,
    ) -> ClientId {
        self.last_active_client
            .filter(|client| self.interactive_clients.contains(client))
            .or(requested)
            .or_else(|| self.interactive_clients.iter().min().copied())
            .unwrap_or(cli_client_id)
    }
    pub fn clear_last_active_client(&mut self, client_id: ClientId) {
        if self.last_active_client == Some(client_id) {
            self.last_active_client = None;
        }
    }
    pub fn mark_forward_in_flight(&mut self, token: u32, client_id: ClientId) {
        self.forwards_in_flight.insert(token, client_id);
    }
    pub fn clear_forward_in_flight(&mut self, token: u32) {
        self.forwards_in_flight.remove(&token);
    }
    /// Mark `client_id` attached, recording whether it can answer session requests.
    fn attach_interactive(&mut self, client_id: ClientId, supports_session_requests: bool) {
        self.interactive_clients.insert(client_id);
        if supports_session_requests {
            self.session_request_clients.insert(client_id);
        } else {
            self.session_request_clients.remove(&client_id);
        }
    }
    fn supports_session_requests(&self, client_id: &ClientId) -> bool {
        self.session_request_clients.contains(client_id)
    }
    /// Route host queries to the last-active interactive client, falling back to the
    /// lowest interactive client ID. CLI, pipe, subscription and watcher clients are excluded.
    pub fn pick_forward_target(&self) -> Option<ClientId> {
        self.last_active_client
            .filter(|client| self.interactive_clients.contains(client))
            .or_else(|| self.interactive_clients.iter().min().copied())
    }
}

pub fn start_server(os_input: Box<dyn ServerOsApi>, socket_path: PathBuf) -> Result<()> {
    use nix::sys::stat::{umask, Mode};
    let working_directory = std::env::current_dir().context("read server startup directory")?;
    // preserve the current umask: read current value by setting to another mode, and then restoring it
    let current_umask = umask(Mode::all());
    umask(current_umask);
    yinz_runtime::logging::startup_stage("server.daemonize", || {
        daemonize::Daemonize::new()
            .working_directory(working_directory)
            .umask(current_umask.bits() as u32)
            .start()
    })
    .context("daemonize the server process")?;

    start_server_impl(os_input, socket_path, true);
    Ok(())
}

pub fn start_server_impl(
    mut os_input: Box<dyn ServerOsApi>,
    socket_path: PathBuf,
    install_panic_hook: bool,
) {
    use yinz_runtime::logging::startup_stage;
    let session_identity = envs::SessionState::new(
        socket_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
            .into(),
    );
    os_input.set_session_state(session_identity.clone());
    // Here rather than in `start_server`, so every way in - daemonized or not - says it once,
    // and a debug install, whose own code runs unoptimized, is one grep away when a session
    // feels wrong.
    log::info!(
        "Starting Yinz server {} ({} build)",
        yinz_runtime::consts::VERSION,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    log::info!(
        "startup trace: server ppid={} socket={socket_path:?}",
        nix::unistd::getppid()
    );

    let ownership = match startup_stage("server.session_ownership", || {
        yinz_ipc::session_ownership::SessionOwnership::acquire_for_startup(&socket_path)
    })
    .context("acquire server session ownership")
    {
        Ok(Some(ownership)) => Arc::new(ownership),
        Ok(None) => return,
        Err(error) => {
            Err::<(), _>(error).non_fatal();
            return;
        },
    };
    let listener = match startup_stage("server.bind_socket", || ownership.bind()) {
        Ok(listener) => listener,
        Err(error) => {
            Err::<(), _>(error)
                .context("start session listener")
                .non_fatal();
            return;
        },
    };
    // Keep the socket from being aged out of XDG_RUNTIME_DIR.
    yinz_runtime::shared::set_permissions(&socket_path, 0o1700)
        .context("set session socket permissions")
        .non_fatal();

    let shutdown = Shutdown::default();
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::bounded(50);
    let to_server = shutdown.sender(to_server);
    // A panic hook may still hold a session lock. Its report must not wait for the main loop.
    let (to_panic, panic_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    let to_panic = shutdown.sender(to_panic);
    let session_data: Arc<RwLock<Option<SessionMetaData>>> = Arc::new(RwLock::new(None));
    let session_state = Arc::new(RwLock::new(SessionState::new()));
    let mut startup_traces_waiting_for_first_render: HashMap<ClientId, std::time::Instant> =
        HashMap::new();

    if install_panic_hook {
        install_server_panic_hook(to_panic.clone());
    }

    let listener_thread = thread::Builder::new()
        .name("server_listener".to_string())
        .spawn({
            use interprocess::local_socket::prelude::*;

            let os_input = os_input.clone();
            let session_data = session_data.clone();
            let session_state = session_state.clone();
            let to_server = to_server.clone();
            let live_connections = Arc::new(AtomicUsize::new(0));
            move || {
                let mut reported_exhausted_ids = false;
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let mut os_input = os_input.clone();
                            let accepted_at = std::time::Instant::now();
                            // Claim a slot before anything is allocated for this
                            // connection. Over the cap the stream is dropped
                            // unserved, which the peer sees as an immediate EOF —
                            // the same thing it would see from a server that had
                            // gone away, and far better than being accepted into a
                            // process that is already past saving.
                            let in_flight =
                                live_connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let slot = ConnectionSlot(live_connections.clone());
                            if in_flight > MAX_CONCURRENT_CLIENT_CONNECTIONS {
                                if in_flight == MAX_CONCURRENT_CLIENT_CONNECTIONS + 1 {
                                    // Once per crossing, not once per refusal: a
                                    // peer hammering the socket must not also
                                    // flood the log.
                                    log::error!(
                                        "refusing client connections: {MAX_CONCURRENT_CLIENT_CONNECTIONS} already open. \
                                         Something is opening connections without closing them."
                                    );
                                }
                                drop(slot);
                                drop(stream);
                                continue;
                            }
                            let Some(client_id) = (match session_state.write() {
                                Ok(mut state) => state.new_client(),
                                Err(error) => {
                                    let message = format!("stop listener after poisoned session state: {error}");
                                    drop(error);
                                    to_server.send(ServerInstruction::Error(message)).context("report listener shutdown").non_fatal();
                                    break;
                                },
                            })
                            else {
                                if !std::mem::replace(&mut reported_exhausted_ids, true) {
                                    log::error!(
                                        "refusing client connections: this session has used every client id"
                                    );
                                }
                                drop(slot);
                                drop(stream);
                                continue;
                            };
                            log::info!(
                                "attach trace: server listener allocated client_id={client_id} after {:?}; registering IPC client",
                                accepted_at.elapsed()
                            );

                            let receiver = match os_input.new_client(client_id, stream) {
                                Ok(receiver) => receiver,
                                Err(err) => {
                                    // The same outcome as the refusal above, and
                                    // for the same reason: this used to `unwrap`,
                                    // which took the listener thread down with it
                                    // and left the session running but
                                    // unreachable. Registering the client dups
                                    // its socket, so `EMFILE` reaches here too.
                                    Err::<(), _>(err)
                                        .with_context(|| {
                                            format!("failed to register client {client_id}")
                                        })
                                        .non_fatal();
                                    // Forget the connection. Nothing has been
                                    // dispatched to a client this new, so there are
                                    // no in-flight forwards to flush, and no sender
                                    // was registered for `os_input.remove_client`
                                    // to remove.
                                    session_state.write().to_anyhow().map(|mut state| state.remove_client(client_id)).context("remove failed connection").non_fatal();
                                    drop(slot);
                                    continue;
                                },
                            };
                            log::info!(
                                "attach trace: server listener registered IPC client_id={client_id} after {:?}; spawning router",
                                accepted_at.elapsed()
                            );

                            let router_session_data = session_data.clone();
                            let router_session_state = session_state.clone();
                            let router_os_input = os_input.clone();
                            let to_server = to_server.clone();
                            let router = thread::Builder::new()
                                .name("server_router".to_string())
                                .spawn(move || {
                                    // Held for the life of the router, released
                                    // however it ends.
                                    let _slot = slot;
                                    route_thread_main(
                                        router_session_data,
                                        router_session_state,
                                        router_os_input,
                                        to_server,
                                        receiver,
                                        client_id,
                                        FIRST_CLIENT_MESSAGE_TIMEOUT,
                                    )
                                    .fatal()
                                });
                            if let Err(error) = router {
                                router_spawn_failed(
                                    error,
                                    client_id,
                                    &mut os_input,
                                    &session_state,
                                    &session_data,
                                );
                                continue;
                            }
                            log::info!(
                                "attach trace: server listener router spawned client_id={client_id} after {:?}",
                                accepted_at.elapsed()
                            );
                        },
                        Err(err) => {
                            // Not fatal. This used to panic, which killed the
                            // listener thread outright and left the session
                            // running but unreachable, with nothing in the log
                            // to say why. The realistic cause is `EMFILE` from
                            // connections piling up, which is transient once
                            // the cap above starts refusing them.
                            log::error!("failed to accept a client connection: {err:?}");
                            thread::sleep(ACCEPT_ERROR_BACKOFF);
                        },
                    }
                }
            }
        });
    if let Err(error) = listener_thread {
        Err::<(), _>(error)
            .context("start server listener thread")
            .non_fatal();
        return;
    }

    // Why the loop below stopped. Set at every break so that the reason a
    // session ended is written down: without it, a server shutdown leaves
    // nothing in the log at all, and "the session died" is indistinguishable
    // from "the session was closed" after the fact.
    let mut exit_reason = "server instruction channel closed";
    let mut starting_session = None;
    let mut kill_session = false;
    let mut session_requests = PendingSessionRequests::new();
    let mut kill_completion_client = None;
    let mut kill_progress_client = None;
    // Until a first client has initialised the session, the wait is bounded by
    // this one deadline. The client that spawned this server gives up on it
    // after `CONNECT_TO_SERVER_TIMEOUT` (30s) and exits with no way to kill
    // what it started; a server left like that would sit forever with a bound
    // socket and no session state. A fixed deadline rather than a per-message
    // timeout, so that probes from `yinz ls` — which arrive here too, and
    // are answered without a session — cannot keep an orphan alive by turning
    // up every so often. Once there is a session the loop waits without a
    // deadline as before: a detached session is meant to outlive its clients.
    let first_client_deadline = std::time::Instant::now() + FIRST_CLIENT_TIMEOUT;
    log::info!("startup trace: server listening socket={socket_path:?}; waiting for first client");
    let outcome = (|| -> Result<()> {
        loop {
            flush_pending_mouse_updates(&mut os_input, &session_state, &session_data)
                .context("flush pending client updates")?;
            let mut pending = channels::Select::new_biased();
            let panic_index = pending.recv(&panic_receiver);
            pending.recv(&server_receiver);
            let selected = if session_data
                .read()
                .to_anyhow()
                .context("access server session state")?
                .is_none()
            {
                match pending.select_deadline(first_client_deadline) {
                    Ok(selected) => selected,
                    Err(_) => {
                        exit_reason = "no client attached within the startup timeout";
                        break;
                    },
                }
            } else {
                pending.select()
            };
            let recv = if selected.index() == panic_index {
                selected.recv(&panic_receiver)
            } else {
                selected.recv(&server_receiver)
            };
            let (instruction, mut err_ctx) = recv.context("receive server instruction")?;
            let instruction = match instruction {
                ServerInstruction::FirstClientConnected(assets, client_id)
                    if session_data
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .is_some() =>
                {
                    ServerInstruction::AttachClient(assets, None, None, client_id)
                },
                instruction => instruction,
            };
            yinz_runtime::errors::add_call(
                &mut err_ctx,
                ContextType::IPCServer((&instruction).into()),
            );
            match &instruction {
                ServerInstruction::AttachClient(_, _, _, client_id) => log::info!(
                    "attach trace: server main loop received AttachClient client_id={client_id}"
                ),
                ServerInstruction::ConnStatus(client_id) => log::info!(
                    "attach trace: server main loop received ConnStatus client_id={client_id}"
                ),
                _ => {},
            }
            match instruction {
                ServerInstruction::FirstClientConnected(cli_assets, client_id) => {
                    let startup_started = std::time::Instant::now();
                    startup_traces_waiting_for_first_render.insert(client_id, startup_started);
                    log::info!(
                        "startup trace: server received FirstClientConnected client_id={client_id}"
                    );
                    let host_terminal_env = cli_assets.host_terminal_env.clone();
                    let timezone = cli_assets.timezone.clone();
                    let supports_session_requests = cli_assets.supports_session_requests;
                    let (config, layout) = match startup_stage("server.config_and_layout", || {
                        yinz_config::cli_assets::load_config_and_layout(&cli_assets)
                    }) {
                        Ok(config_and_layout) => config_and_layout,
                        Err(error) => {
                            let _ = os_input.send_to_client(
                                client_id,
                                ServerToClientMsg::Exit {
                                    exit_reason: ExitReason::Error(format!(
                                        "failed to load session checkpoint: {error:#}"
                                    )),
                                },
                            );
                            Err::<(), _>(error)
                                .context("failed to load session checkpoint")
                                .non_fatal();
                            remove_client!(client_id, os_input, session_state, session_data);
                            exit_reason = "failed to load session checkpoint";
                            break;
                        },
                    };
                    // A default config that cannot be written faithfully is not written at all -
                    // and then there is nothing for the setup wizard to edit.
                    let successfully_written_config =
                        startup_stage("server.write_initial_config", || {
                            match config.to_string(true) {
                                Ok(text) => Config::write_config_to_disk_if_it_does_not_exist(
                                    text,
                                    &cli_assets.config_file_path,
                                ),
                                Err(e) => {
                                    Err::<(), _>(anyhow!("{e}"))
                                        .context("not writing the initial config")
                                        .non_fatal();
                                    false
                                },
                            }
                        });
                    // if we successfully wrote the config to disk, it means two things:
                    // 1. It did not exist beforehand
                    // 2. The config folder is writeable
                    //
                    // If these two are true, we should launch the setup wizard, if even one of them is
                    // false, we should never launch it.
                    let should_launch_setup_wizard = successfully_written_config;

                    let runtime_config_options = match &cli_assets.configuration_options {
                        Some(configuration_options) => {
                            config.options.merge(configuration_options.clone())
                        },
                        None => config.options.clone(),
                    };

                    let client_attributes = ClientAttributes {
                        size: cli_assets.terminal_window_size,
                        style: Style {
                            agent_theme_mapping: runtime_config_options
                                .agent_theme_mapping
                                .unwrap_or_default(),
                            terminal: config
                                .terminal_theme_config(runtime_config_options.theme.as_ref()),
                            colors: config
                                .theme_config(runtime_config_options.theme.as_ref())
                                .unwrap_or_else(|| default_palette().into()),
                            rounded_corners: config.ui.pane_frames.rounded_corners,
                            hide_session_name: config.ui.pane_frames.hide_session_name,
                        },
                    };

                    info!("FirstClientConnected: client={client_id} initializing session");
                    startup_stage("server.init_session", || {
                        init_session(
                            &mut starting_session,
                            os_input.clone(),
                            to_server.clone(),
                            shutdown.clone(),
                            ownership.clone(),
                            session_identity.clone(),
                            client_attributes.clone(),
                            runtime_config_options.clone(),
                            layout.clone(),
                            cli_assets.clone(),
                            config.clone(),
                            config.plugins.clone(),
                            client_id,
                        )
                    })
                    .context("initialize session workers")?;
                    let session = starting_session
                        .as_mut()
                        .context("session workers were initialized")?;
                    info!("FirstClientConnected: client={client_id} session initialized after {:?}, spawning tabs", startup_started.elapsed());
                    let mut runtime_configuration = config.clone();
                    runtime_configuration.options = runtime_config_options.clone();
                    session
                        .session_configuration
                        .set_saved_configuration(config.clone());
                    session
                        .session_configuration
                        .set_client_runtime_configuration(client_id, runtime_configuration);
                    let default_input_mode =
                        runtime_config_options.default_mode.unwrap_or_default();
                    session
                        .current_input_modes
                        .insert(client_id, default_input_mode);
                    publish_session(
                        &mut starting_session,
                        &session_state,
                        &session_data,
                        client_id,
                        supports_session_requests,
                    )?;
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::RecomputeTabSize(
                                client_id,
                                client_attributes.size,
                            ))
                            .context("sizing the first client's tabs")
                            .non_fatal()
                    })?;

                    let cwd = session_start_cwd(cli_assets.cwd.as_deref(), &runtime_config_options);
                    let default_shell = runtime_config_options.default_shell.map(|shell| {
                        TerminalAction::RunCommand(RunCommand {
                            command: shell,
                            cwd: config.options.default_cwd.clone(),
                            use_terminal_title: true,
                            ..Default::default()
                        })
                    });

                    let spawn_tabs = |tab_layout,
                                      floating_panes_layout,
                                      tab_name,
                                      swap_layouts,
                                      should_focus_tab|
                     -> Result<()> {
                        with_session(&session_data, |session| {
                            session
                                .senders
                                .send_to_screen(ScreenInstruction::NewTab(
                                    cwd.clone(),
                                    default_shell.clone(),
                                    tab_layout,
                                    floating_panes_layout,
                                    tab_name,
                                    swap_layouts,
                                    None,  // initial_panes
                                    false, // block_on_first_terminal
                                    should_focus_tab,
                                    client_id,
                                    None,
                                ))
                                .context("opening a tab for the first client")
                                .non_fatal()
                        })?;
                        Ok(())
                    };

                    if layout.has_tabs() {
                        let focused_tab_index =
                            layout.focused_tab_index().unwrap_or(TabPosition::new(0));
                        for (tab_index, (tab_name, tab_layout, floating_panes_layout)) in
                            layout.tabs().into_iter().enumerate()
                        {
                            let should_focus_tab = TabPosition::new(tab_index) == focused_tab_index;
                            spawn_tabs(
                                Some(tab_layout.clone()),
                                floating_panes_layout.clone(),
                                tab_name,
                                (
                                    Some(layout.swap_tiled_layouts.clone()),
                                    Some(layout.swap_floating_layouts.clone()),
                                ),
                                should_focus_tab,
                            )?;
                        }
                    } else {
                        let mut floating_panes =
                            layout.template.map(|t| t.1).clone().unwrap_or_default();
                        if should_launch_setup_wizard {
                            // we only do this here (and only once) because otherwise it will be
                            // intrusive
                            let setup_wizard = setup_wizard_floating_pane();
                            floating_panes.push(setup_wizard);
                        }
                        spawn_tabs(
                            None,
                            floating_panes,
                            None,
                            (
                                Some(layout.swap_tiled_layouts.clone()),
                                Some(layout.swap_floating_layouts.clone()),
                            ),
                            true,
                        )?;
                    }
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_plugin(PluginInstruction::AddClient(client_id))
                            .context("adding the first client to the plugins")
                            .non_fatal();
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::SetClientTimezone(
                                client_id, timezone,
                            ))
                            .context("recording the first client timezone")
                            .non_fatal();
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::SetClientHostTerminalEnv(
                                client_id,
                                host_terminal_env,
                            ))
                            .context("recording the first client's host terminal")
                            .non_fatal();
                    })?;
                    log::info!("startup trace: server first client dispatch complete client_id={client_id} elapsed_us={}; waiting for first render", startup_started.elapsed().as_micros());
                },
                ServerInstruction::AttachClient(
                    cli_assets,
                    tab_number_to_focus,
                    pane_id_to_focus,
                    client_id,
                ) => {
                    let attach_started_at = std::time::Instant::now();
                    startup_traces_waiting_for_first_render.insert(client_id, attach_started_at);
                    log::info!(
                    "attach trace: server main client_id={client_id} waiting for session_data write lock"
                );
                    let mut rlock = session_data
                        .write()
                        .to_anyhow()
                        .context("access server session state")?;
                    log::info!(
                    "attach trace: server main client_id={client_id} acquired session_data write lock after {:?}",
                    attach_started_at.elapsed()
                );
                    let Some(session_data) = rlock.as_mut() else {
                        // No first client ever initialised this session — the one
                        // that spawned the server gave up before its first message
                        // arrived. There is nothing to attach to; tell the client
                        // rather than unwrap and take the whole server down.
                        drop(rlock);
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Error(
                                    "the session was never initialised, nothing to attach to"
                                        .to_owned(),
                                ),
                            },
                        );
                        remove_client!(client_id, os_input, session_state, session_data);
                        startup_traces_waiting_for_first_render.remove(&client_id);
                        continue;
                    };
                    let config = session_data.session_configuration.saved_config.clone();
                    let host_terminal_env = cli_assets.host_terminal_env.clone();
                    let timezone = cli_assets.timezone.clone();
                    let supports_session_requests = cli_assets.supports_session_requests;
                    let runtime_config_options = match cli_assets.configuration_options {
                        Some(configuration_options) => config.options.merge(configuration_options),
                        None => config.options.clone(),
                    };

                    let client_attributes = ClientAttributes {
                        size: cli_assets.terminal_window_size,
                        style: Style {
                            agent_theme_mapping: runtime_config_options
                                .agent_theme_mapping
                                .unwrap_or_default(),
                            terminal: config
                                .terminal_theme_config(runtime_config_options.theme.as_ref()),
                            colors: config
                                .theme_config(runtime_config_options.theme.as_ref())
                                .unwrap_or_else(|| default_palette().into()),
                            rounded_corners: config.ui.pane_frames.rounded_corners,
                            hide_session_name: config.ui.pane_frames.hide_session_name,
                        },
                    };

                    let mut runtime_configuration = config.clone();
                    runtime_configuration.options = runtime_config_options.clone();
                    session_data
                        .session_configuration
                        .set_client_runtime_configuration(client_id, runtime_configuration);

                    let effective = session_data
                        .session_configuration
                        .get_client_configuration(&client_id);
                    let default_mode = effective.options.default_mode.unwrap_or_default();
                    session_data
                        .current_input_modes
                        .insert(client_id, default_mode);
                    // Before the client counts as attached: from then on a CLI action can act for it
                    // and load a plugin for it, which must find its attachment token.
                    session_data
                        .senders
                        .send_to_plugin(PluginInstruction::ReserveClient(client_id))
                        .context("reserving the attaching client in the plugins")
                        .non_fatal();
                    session_state
                        .write()
                        .to_anyhow()
                        .context("access server session state")?
                        .attach_interactive(client_id, supports_session_requests);

                    log::info!(
                    "attach trace: server main client_id={client_id} waiting for session_state write lock"
                );
                    log::info!(
                    "attach trace: server main client_id={client_id} session state updated after {:?}",
                    attach_started_at.elapsed()
                );

                    log::info!(
                    "attach trace: server main client_id={client_id} enqueue SetClientHostTerminalEnv"
                );
                    session_data
                        .senders
                        .send_to_screen(ScreenInstruction::SetClientTimezone(client_id, timezone))
                        .context("recording the attaching client timezone")
                        .non_fatal();
                    session_data
                        .senders
                        .send_to_screen(ScreenInstruction::SetClientHostTerminalEnv(
                            client_id,
                            host_terminal_env,
                        ))
                        .context("recording the attaching client's host terminal")
                        .non_fatal();
                    log::info!("attach trace: server main client_id={client_id} enqueue AddClient");
                    session_data
                        .senders
                        .send_to_screen(ScreenInstruction::AddClient(
                            client_id,
                            client_attributes.size,
                            tab_number_to_focus,
                            pane_id_to_focus,
                        ))
                        .context("adding the attaching client to the screen")
                        .non_fatal();
                    session_data.propagate_configuration_changes(
                        vec![(client_id, effective.clone())],
                        false,
                    );
                    // ModeUpdate broadcast is handled by the screen thread via
                    // change_mode() -> update_input_modes()
                    log::info!(
                        "attach trace: server main client_id={client_id} enqueue ChangeMode"
                    );
                    session_data
                        .senders
                        .send_to_screen(ScreenInstruction::ChangeMode(
                            default_mode,
                            Some(default_mode),
                            client_id,
                            None,
                        ))
                        .context("setting the attaching client's mode")
                        .non_fatal();
                    log::info!(
                    "attach trace: server main client_id={client_id} attach dispatch complete after {:?}",
                    attach_started_at.elapsed()
                );
                },
                ServerInstruction::AttachWatcherClient(client_id, terminal_size) => {
                    if session_data
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .is_none()
                    {
                        // As for `AttachClient`: no first client ever initialised this session, so
                        // there is nothing to watch. Tell the client rather than unwrap and take
                        // the whole server down.
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Error(
                                    "the session was never initialised, nothing to watch"
                                        .to_owned(),
                                ),
                            },
                        );
                        remove_client!(client_id, os_input, session_state, session_data);
                        continue;
                    }
                    // the client_id was inserted into clients upon ipc tunnel initialization
                    // now that it identified itself as a watcher, we need to convert it

                    // Convert to watcher in SessionState (needed for input filtering in route.rs)
                    session_state
                        .write()
                        .to_anyhow()
                        .context("access server session state")?
                        .convert_client_to_watcher(client_id);

                    // Also notify Screen to add this as a watcher client (for rendering) with the terminal size
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::AddWatcherClient(
                                client_id,
                                terminal_size,
                            ))
                            .context("adding a watcher client to the screen")
                            .non_fatal()
                    })?;
                },
                ServerInstruction::UnblockInputThread => {
                    // Pane closes and plugin reloads release interactive input only. A CLI
                    // must wait for its own route/completion, otherwise it exits before its
                    // reply and that reply can reach a newly allocated client with the same id.
                    let client_ids = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .interactive_client_ids();
                    for client_id in client_ids {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::UnblockInputThread,
                            session_state,
                            session_data
                        );
                    }
                },
                ServerInstruction::UnblockCliPipeInput(pipe_name) => {
                    let pipe = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .get_pipe(&pipe_name);
                    if let Some(client_id) = pipe {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::UnblockCliPipeInput { pipe_name },
                            session_state,
                            session_data
                        );
                    }
                },
                ServerInstruction::CliPipeOutput(pipe_name, output) => {
                    // Association is queued before dispatching the pipe to the plugin. An unknown
                    // pipe has disconnected; broadcasting its late output can corrupt a probe's reply.
                    let pipe = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .get_pipe(&pipe_name);
                    if let Some(client_id) = pipe {
                        send_to_client!(
                            client_id,
                            os_input,
                            cli_pipe_reply(pipe_name, output),
                            session_state,
                            session_data
                        );
                    }
                },
                ServerInstruction::CliPipeError(pipe_name, error) => {
                    let pipe = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .get_pipe(&pipe_name);
                    if let Some(client_id) = pipe {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::LogError { lines: vec![error] },
                            session_state,
                            session_data
                        );
                    }
                },
                ServerInstruction::EventUpdateToClient(client_id, event_type, event_json) => {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::EventUpdate {
                            event_type: event_type.clone(),
                            event_json: event_json.clone(),
                        },
                        session_state,
                        session_data
                    );
                },
                ServerInstruction::ClientExit(client_id, completion_tx) => {
                    startup_traces_waiting_for_first_render.remove(&client_id);
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Normal,
                        },
                    );

                    // Check if this is a watcher
                    let is_watcher = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .is_watcher(&client_id);
                    if is_watcher {
                        remove_watcher_client(
                            client_id,
                            &mut os_input,
                            &session_state,
                            &session_data,
                        )?;
                    } else {
                        // Handle regular client removal
                        remove_client!(client_id, os_input, session_state, session_data);
                        drop(completion_tx); // cleanup is queued before the action completes
                        if !session_state
                            .read()
                            .to_anyhow()
                            .context("access server session state")?
                            .active_clients_are_connected()
                        {
                            // Refuse new connections now; join workers after leaving the server
                            // loop, once its bounded receiver has been disconnected.
                            ownership.shutdown().non_fatal();
                            let client_ids_to_cleanup: Vec<ClientId> = session_state
                                .read()
                                .to_anyhow()
                                .context("access server session state")?
                                .clients
                                .iter()
                                .copied()
                                .collect();
                            // Only CLI, pipe and subscription connections remain.
                            for client_id in client_ids_to_cleanup {
                                remove_client!(client_id, os_input, session_state, session_data);
                            }

                            let watcher_client_ids: Vec<ClientId> = session_state
                                .read()
                                .to_anyhow()
                                .context("access server session state")?
                                .watcher_client_ids();
                            for watcher_id in watcher_client_ids {
                                let _ = os_input.send_to_client(
                                    watcher_id,
                                    ServerToClientMsg::Exit {
                                        exit_reason: ExitReason::Normal,
                                    },
                                );
                            }

                            exit_reason = "the last interactive client exited";
                            break;
                        }
                    }
                },
                ServerInstruction::RemoveClient(client_id) => {
                    startup_traces_waiting_for_first_render.remove(&client_id);
                    // Check if this is a watcher
                    let is_watcher = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .is_watcher(&client_id);
                    if is_watcher {
                        remove_watcher_client(
                            client_id,
                            &mut os_input,
                            &session_state,
                            &session_data,
                        )?;
                    } else {
                        // Handle regular client removal
                        remove_client!(client_id, os_input, session_state, session_data);
                    }
                },
                ServerInstruction::KillSession
                | ServerInstruction::KillSessionWithCompletion { .. } => {
                    log::info!("shutdown: KillSession received");
                    if let ServerInstruction::KillSessionWithCompletion {
                        client_id,
                        report_progress,
                    } = instruction
                    {
                        kill_completion_client = Some(client_id);
                        if report_progress {
                            kill_progress_client = Some(client_id);
                            os_input
                                .send_to_client(client_id, ServerToClientMsg::KillSessionProgress)
                                .non_fatal();
                        }
                    }
                    // Keep clients registered until exit; disconnect cleanup would cancel plugin unload.
                    let client_ids = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .client_ids();
                    for client_id in client_ids {
                        if Some(client_id) == kill_completion_client {
                            continue;
                        }
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Normal,
                            },
                        );
                        let _ = os_input.remove_client(client_id);
                    }
                    kill_session = true;
                    exit_reason = "KillSession";
                    break;
                },
                ServerInstruction::GetDisconnectTargets(client_id, response) => {
                    let _ = response.send(
                        session_state
                            .read()
                            .to_anyhow()
                            .context("access server session state")?
                            .disconnect_targets(client_id),
                    );
                },
                ServerInstruction::DisconnectAllClientsExcept(client_id) => {
                    let client_ids = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .disconnect_targets(client_id);
                    for client_id in client_ids {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::KickedByHost,
                            },
                        );
                        remove_client!(client_id, os_input, session_state, session_data);
                    }
                },
                ServerInstruction::DetachSession(client_ids, completion_tx) => {
                    for client_id in &client_ids {
                        let _ = os_input.send_to_client(
                            *client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Normal,
                            },
                        );
                        remove_client!(*client_id, os_input, session_state, session_data);
                    }
                    drop(completion_tx); // cleanup is queued before the action completes
                },
                ServerInstruction::Render(serialized_output) => {
                    let client_ids = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .client_ids();
                    // If `Some(_)`- unwrap it and forward it to the clients to render.
                    // If `None`- Send an exit instruction. This is the case when a user closes the last Tab/Pane.
                    if let Some(output) = &serialized_output {
                        for (client_id, client_render_instruction) in output.iter() {
                            let attach_started_at = startup_traces_waiting_for_first_render
                                .get(client_id)
                                .copied();
                            if let Some(started_at) = attach_started_at {
                                log::info!(
                                "startup trace: server main client_id={client_id} received first Render bytes={} after {:?}; client queue begin",
                                client_render_instruction.len(),
                                started_at.elapsed()
                            );
                            }
                            send_to_client!(
                                *client_id,
                                os_input,
                                ServerToClientMsg::Render {
                                    content: client_render_instruction.clone()
                                },
                                session_state,
                                session_data
                            );
                            if let Some(started_at) = attach_started_at {
                                log::info!(
                                "startup trace: server main client_id={client_id} first Render queued after {:?}",
                                started_at.elapsed()
                            );
                                startup_traces_waiting_for_first_render.remove(client_id);
                            }
                        }
                    } else {
                        // Keep IDs reserved for shutdown so plugin unload can finish.
                        for client_id in client_ids {
                            let _ = os_input.send_to_client(
                                client_id,
                                ServerToClientMsg::Exit {
                                    exit_reason: ExitReason::Normal,
                                },
                            );
                            let _ = os_input.remove_client(client_id);
                        }

                        // Also disconnect all watchers
                        let watcher_ids: Vec<ClientId> = session_state
                            .read()
                            .to_anyhow()
                            .context("access server session state")?
                            .watchers
                            .iter()
                            .copied()
                            .collect();
                        for watcher_id in watcher_ids {
                            let _ = os_input.send_to_client(
                                watcher_id,
                                ServerToClientMsg::Exit {
                                    exit_reason: ExitReason::Normal,
                                },
                            );
                            let _ = os_input.remove_client(watcher_id);
                        }
                        exit_reason = "the last pane closed";
                        break;
                    }
                },
                ServerInstruction::Error(backtrace) => {
                    notify_shutdown_error(&mut os_input, &session_state, &backtrace);
                    exit_reason = "a fatal error in the server";
                    break;
                },
                ServerInstruction::ConnStatus(client_id) => {
                    // A liveness probe from `assert_socket` in `sessions.rs`. It is
                    // answered from this loop on purpose: the probe has a receive
                    // deadline, so a loop that is parked — joining threads, stuck
                    // on a lock — does not answer in time and reads as unresponsive.
                    // That is the property callers rely on: a server that answers
                    // here can also take an `AttachClient` next.
                    let started_at = std::time::Instant::now();
                    log::info!(
                    "attach trace: server main client_id={client_id} ConnStatus reply queue begin"
                );
                    let result = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Connected {
                            warp_companions: true,
                            warp_held_terminals: true,
                        },
                    );
                    log::info!(
                    "attach trace: server main client_id={client_id} ConnStatus reply queued after {:?} ({})",
                    started_at.elapsed(),
                    if result.is_ok() { "ok" } else { "error" }
                );
                    // The router queues RemoveClient after ConnStatus and owns the removal.
                },
                ServerInstruction::Log(lines_to_log, client_id, completion_tx) => {
                    // A timed-out route has already answered its request; a late reply would
                    // follow that answer.
                    if completion_tx
                        .as_ref()
                        .is_some_and(NotificationEnd::is_cancelled)
                    {
                        continue;
                    }
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::Log {
                            lines: lines_to_log
                        },
                        session_state,
                        session_data
                    );
                },
                ServerInstruction::LogError(lines_to_log, client_id, completion_tx) => {
                    // A timed-out route has already answered its request; a late reply would
                    // follow that answer.
                    if completion_tx
                        .as_ref()
                        .is_some_and(NotificationEnd::is_cancelled)
                    {
                        continue;
                    }
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::LogError {
                            lines: lines_to_log
                        },
                        session_state,
                        session_data
                    );
                },
                ServerInstruction::SessionRequest {
                    client,
                    request,
                    reply,
                } => {
                    route_session_request(
                        client,
                        request,
                        reply,
                        &mut session_requests,
                        &mut os_input,
                        &session_state,
                        &session_data,
                    );
                },
                ServerInstruction::SessionReply(client, reply) => {
                    if let Some((sender, _)) = session_requests.remove(&(client, reply.token)) {
                        let _ = sender.send(reply.result);
                    }
                },
                ServerInstruction::SwitchSession(
                    mut connect_to_session,
                    client_id,
                    completion_tx,
                ) => {
                    let current_session_name = session_identity.current();
                    if connect_to_session.name.as_ref() == Some(&current_session_name) {
                        log::error!("Cannot attach to same session");
                    } else {
                        let layout_dir = with_session(&session_data, |session| {
                            layout_dir_or_default(
                                session
                                    .session_configuration
                                    .get_client_configuration(&client_id)
                                    .options
                                    .layout_dir,
                                session.config_dir.clone(),
                            )
                        })?
                        .flatten();
                        if let Some(layout_dir) = layout_dir {
                            connect_to_session.apply_layout_dir(&layout_dir);
                        }

                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::SwitchSession { connect_to_session },
                            session_state,
                            session_data
                        );
                        remove_client!(client_id, os_input, session_state, session_data);
                        drop(completion_tx); // cleanup is queued before the action completes
                    }
                },
                ServerInstruction::AssociatePipeWithClient { pipe_id, client_id } => {
                    session_state
                        .write()
                        .to_anyhow()
                        .context("access server session state")?
                        .associate_pipe_with_client(pipe_id, client_id);
                },
                ServerInstruction::ChangeMode(client_id, input_mode, completion) => {
                    change_client_mode(
                        client_id,
                        input_mode,
                        completion,
                        &session_state,
                        &session_data,
                    )
                    .non_fatal();
                },
                ServerInstruction::SetCommandPaletteOpen(client_id, open) => {
                    if !open
                        && session_state
                            .read()
                            .to_anyhow()
                            .context("failed to read which clients take session requests")
                            .to_log()
                            .is_ok_and(|state| state.supports_session_requests(&client_id))
                    {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::SessionRequest(
                                yinz_types::session_destination::Request {
                                    token: String::new(),
                                    request:
                                        yinz_types::session_destination::ClientRequest::CancelAll,
                                }
                            ),
                            session_state,
                            session_data
                        );
                    }
                    if is_attached(client_id, &session_state)
                        .to_log()
                        .unwrap_or(false)
                    {
                        with_session_mut(&session_data, |session| {
                            session
                                .set_command_palette_open(client_id, open)
                                .non_fatal();
                        })?;
                    }
                },
                ServerInstruction::ResolveLockedEscape(client_id, token) => {
                    let mut guard = session_data
                        .write()
                        .to_anyhow()
                        .context("locking escape state")?;
                    if let Some(session) = guard.as_mut() {
                        session.resolve_locked_escape(client_id, token).non_fatal();
                    }
                },
                ServerInstruction::ChangeModeForAllClients(input_mode) => {
                    with_session_mut(&session_data, |session| {
                        session.change_mode_for_all_clients(input_mode)
                    })?;
                },
                ServerInstruction::ConfigurationEditor { edit, theme, reply } => {
                    let result = with_session_mut(&session_data, |session| {
                        if edit.is_none() {
                            session.inspect_configuration(theme.as_deref())
                        } else {
                            session.configuration_editor(edit)
                        }
                    })?
                    .unwrap_or_else(|| Err("Session is unavailable".into()));
                    let _ = reply.send(result);
                },
                ServerInstruction::SavePaletteTheme {
                    cancelled,
                    client,
                    request,
                    name,
                    permanent,
                } => {
                    with_session_mut(&session_data, |session| {
                        let result = session
                            .apply_palette_theme(client, request, &name, permanent, &cancelled);
                        if let Err(error) = result {
                            session
                                .senders
                                .send_to_screen(ScreenInstruction::PaletteThemeSaved {
                                    client,
                                    request,
                                    result: Err(error),
                                })
                                .context("replying to theme edit")
                                .non_fatal();
                        }
                    })?;
                },
                ServerInstruction::Reconfigure {
                    client_id,
                    config,
                    write_config_to_disk,
                } => {
                    reconfigure_client(
                        client_id,
                        config,
                        write_config_to_disk,
                        &session_state,
                        &session_data,
                    )
                    .non_fatal();
                },
                ServerInstruction::ReconfigureAllClients {
                    config,
                    write_config_to_disk,
                    completion,
                } => {
                    reconfigure_all_clients(
                        config,
                        write_config_to_disk,
                        completion,
                        &session_state,
                        &session_data,
                    );
                },
                ServerInstruction::ConfigWrittenToDisk(new_config) => {
                    with_session_mut(&session_data, |session| {
                        let changes = session
                            .session_configuration
                            .change_saved_config(new_config);
                        let config_was_written_to_disk = true;
                        session
                            .propagate_configuration_changes(changes, config_was_written_to_disk);
                    })?;
                    let client_ids = session_state
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .interactive_client_ids();
                    for client_id in client_ids {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::ConfigFileUpdated,
                            session_state,
                            session_data
                        );
                    }
                },
                ServerInstruction::RebindKeys {
                    client_id,
                    keys_to_rebind,
                    keys_to_unbind,
                    write_config_to_disk,
                    plugin_id,
                } => {
                    if let Some(plugin_id) = plugin_id {
                        let mut keybinds = Keybinds::default();
                        for (mode, key, actions) in keys_to_rebind {
                            keybinds.0.entry(mode).or_default().insert(key, actions);
                        }
                        with_session_mut(&session_data, |session| {
                            let changes = session
                                .session_configuration
                                .register_plugin_default_keybinds(plugin_id, keybinds);
                            session.propagate_configuration_changes(changes, false);
                        })?;
                        continue;
                    }
                    rebind_client_keys(
                        client_id,
                        keys_to_rebind,
                        keys_to_unbind,
                        write_config_to_disk,
                        &session_state,
                        &session_data,
                    )
                    .non_fatal();
                },
                ServerInstruction::TerminalPixelDimensions {
                    client_id,
                    pixel_dimensions,
                } => {
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::TerminalPixelDimensions(
                                client_id,
                                pixel_dimensions,
                            ))
                            .context("recording the client's pixel geometry")
                            .non_fatal();
                    })?;
                },
                ServerInstruction::KittyGraphicsSupport {
                    client_id,
                    supported,
                } => {
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::SetKittyGraphicsSupport {
                                client_id,
                                supported,
                            })
                            .context("recording the client's kitty graphics support")
                            .non_fatal();
                    })?;
                },
                ServerInstruction::SynchronizedOutputSupport {
                    client_id,
                    supported,
                } => {
                    // Unwrapped like its neighbours would be unsafe here: this one can arrive while
                    // the session is being torn down, having come from a client socket rather than
                    // from inside the server.
                    if let Some(session_data) = session_data
                        .read()
                        .to_anyhow()
                        .context("access server session state")?
                        .as_ref()
                    {
                        let _ = session_data.senders.send_to_screen(
                            ScreenInstruction::SetSynchronizedOutputSupport {
                                client_id,
                                supported,
                            },
                        );
                    }
                },
                ServerInstruction::ClearMouseHelpText(client_id) => {
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::ClearMouseHelpText(client_id))
                            .context("clearing the mouse help text")
                            .non_fatal()
                    })?;
                },
                ServerInstruction::ClearCommandOutputFlash(pane_id) => {
                    with_session(&session_data, |session| {
                        session
                            .senders
                            .send_to_screen(ScreenInstruction::ClearCommandOutputFlash(pane_id))
                            .context("clearing a command output flash")
                            .non_fatal()
                    })?;
                },
                ServerInstruction::ForwardQueryToHost(token, query_bytes, resolve_async) => {
                    // The last-active interactive client's terminal is the best proxy for
                    // what the user is looking at; otherwise use the lowest interactive ID.
                    let target_client_id = {
                        let mut session = session_state
                            .write()
                            .to_anyhow()
                            .context("access server session state")?;
                        let picked = session.pick_forward_target();
                        if let Some(cid) = picked {
                            session.mark_forward_in_flight(token, cid);
                        }
                        picked
                    };
                    if let Some(client_id) = target_client_id {
                        send_to_client!(
                            client_id,
                            os_input,
                            ServerToClientMsg::ForwardQueryToHost {
                                token,
                                query_bytes,
                                resolve_async
                            },
                            session_state,
                            session_data
                        );
                    } else {
                        // No interactive client to ask — synthesize an empty reply
                        // so `Screen`'s in-flight slot releases. Without
                        // this, a detached session (apps still running)
                        // accumulates forwards in `forward_queue` with
                        // no way for them to drain until someone
                        // reattaches.
                        log::warn!(
                        "No interactive client to forward host query (token={}); returning empty reply",
                        token
                    );
                        if let Some(session) = session_data
                            .read()
                            .to_anyhow()
                            .context("access server session state")?
                            .as_ref()
                        {
                            let _ = session.senders.send_to_screen(
                                ScreenInstruction::ForwardedReplyFromHost {
                                    token,
                                    reply_bytes: Vec::new(),
                                },
                            );
                        }
                    }
                },
                ServerInstruction::KeyPassthroughChanged(
                    client_id,
                    _old_pane_id,
                    new_pane_id,
                    should_route,
                    entered_from_direction,
                    notify_guest,
                ) => {
                    change_key_passthrough(
                        client_id,
                        new_pane_id,
                        should_route,
                        entered_from_direction,
                        notify_guest,
                        &session_state,
                        &session_data,
                    )
                    .non_fatal();
                },
                ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload_bytes) => {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::EmitNestedSessionFrame { payload_bytes },
                        session_state,
                        session_data
                    );
                },
            }
        }

        Ok(())
    })();
    if let Err(error) = outcome {
        exit_reason = "failed to initialize or access server session state";
        notify_shutdown_error(&mut os_input, &session_state, &format!("{error:#}"));
        Err::<(), _>(error)
            .context("server shutting down")
            .non_fatal();
    }

    let shutdown_started = std::time::Instant::now();
    let mut progress = || {
        if let Some(client_id) = kill_progress_client {
            os_input
                .send_to_client(client_id, ServerToClientMsg::KillSessionProgress)
                .non_fatal();
        }
    };
    log::info!("Server exiting: {exit_reason}");
    shutdown.begin();
    // No one drains this bounded channel now. Disconnect it before joining workers that
    // may still send to the server, so a full queue cannot hold up their shutdown.
    drop(server_receiver);
    drop(panic_receiver);

    ownership.shutdown().non_fatal();
    progress();

    // Drop cached session data before exit. This joins the pty thread, which
    // drops `Pty` and closes every pane — which is what terminates each pane's
    // supervisor, via `clear_terminal_id`.
    //
    // Taken out under the lock and dropped outside it: those joins must not run with the write
    // lock held, or route threads still blocked on `session_data.read()` stay
    // blocked for the whole shutdown instead of seeing the socket close.
    let tracking = if kill_completion_client.is_some() {
        os_input.begin_confirmed_pty_host_shutdown()
    } else {
        Ok(())
    };
    progress();
    log::info!(
        "shutdown: acquiring session-data lock at {:?}",
        shutdown_started.elapsed()
    );
    let session_metadata = take_session_for_shutdown(&session_data);
    log::info!(
        "shutdown: acquired session-data lock at {:?}",
        shutdown_started.elapsed()
    );
    drop(starting_session);
    drop(session_metadata);
    progress();
    log::info!(
        "shutdown: workers dropped at {:?}",
        shutdown_started.elapsed()
    );

    // Belt and braces. A supervisor that outlived the sweep above would be a
    // leaked user process holding a live shell, invisible until `pty-host list`.
    os_input.shutdown_pty_hosts();
    progress();
    log::info!(
        "shutdown: PTY hosts stopped at {:?}",
        shutdown_started.elapsed()
    );
    // A kill by name also reaches what an earlier incarnation of this session
    // left detached when it died without shutting down. Only on a kill: a detach
    // or a last-client exit is not the user saying those panes are done.
    let owned_teardown = tracking.and_then(|_| os_input.confirm_pty_host_shutdown());
    progress();
    log::info!(
        "shutdown: PTY host shutdown confirmed at {:?}",
        shutdown_started.elapsed()
    );
    let cleanup = if kill_session {
        os_input.terminate_orphaned_pty_hosts(session_identity.current().as_str(), &mut progress)
    } else {
        Ok(())
    };

    log::info!(
        "shutdown: orphan cleanup returned at {:?}",
        shutdown_started.elapsed()
    );
    if let Some(client_id) = kill_completion_client {
        let error = owned_teardown
            .and(cleanup)
            .err()
            .map(|e| format!("shutdown incomplete: {e:#}"));
        os_input
            .send_to_client(client_id, ServerToClientMsg::KillSessionComplete { error })
            .non_fatal();
    } else {
        owned_teardown.and(cleanup).non_fatal();
    }

    // Every break above sends the clients an `Exit` first, but sending only
    // queues it — the writer threads are detached, so without this wait the
    // process can exit before the bytes are written and a client that was told
    // why the session ended sees a bare hangup instead.
    os_input.flush_client_writers(CLIENT_WRITER_FLUSH_TIMEOUT);
    log::info!(
        "shutdown: client writers flushed at {:?}",
        shutdown_started.elapsed()
    );
}

/// The directory a new session's first panes open in, when one was asked for: the client's
/// explicit directory, else the configured `default_cwd`.
fn session_start_cwd(client_cwd: Option<&Path>, options: &Options) -> Option<PathBuf> {
    client_cwd
        .map(Path::to_path_buf)
        .or_else(|| options.default_cwd.clone())
}

fn install_server_panic_hook(to_panic: SenderWithContext<ServerInstruction>) {
    std::panic::set_hook(Box::new(move |info| {
        yinz_runtime::errors::handle_panic(info, Some(&to_panic));
    }));
}

/// Keep partial startup owned by the shutdown path until both shared locks are available.
fn publish_session(
    starting: &mut Option<SessionMetaData>,
    session_state: &RwLock<SessionState>,
    session_data: &RwLock<Option<SessionMetaData>>,
    client_id: ClientId,
    supports_session_requests: bool,
) -> Result<()> {
    let mut data = session_data
        .write()
        .to_anyhow()
        .context("publish initialized session")?;
    let mut state = session_state
        .write()
        .to_anyhow()
        .context("attach first client")?;
    state.attach_interactive(client_id, supports_session_requests);
    *data = starting.take();
    Ok(())
}

fn spawn_session_worker(
    name: &'static str,
    task: impl FnOnce() + Send + 'static,
) -> Result<thread::JoinHandle<()>> {
    #[cfg(test)]
    if let Some(result) = session_shutdown_tests::injected_worker_spawn(name) {
        return result.with_context(|| format!("start {name} thread"));
    }
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(task)
        .with_context(|| format!("start {name} thread"))
}

/// Only shutdown may inspect poisoned state, to close sockets without resuming session work.
fn notify_shutdown_error(
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &RwLock<SessionState>,
    message: &str,
) {
    let clients = {
        let state = session_state
            .read()
            .unwrap_or_else(|error| error.into_inner());
        state
            .clients
            .union(&state.watchers)
            .copied()
            .collect::<Vec<_>>()
    };
    for client_id in clients {
        os_input
            .send_to_client(
                client_id,
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Error(message.to_owned()),
                },
            )
            .context("notify client of server shutdown")
            .non_fatal();
        os_input
            .remove_client(client_id)
            .context("close client during server shutdown")
            .non_fatal();
    }
}

/// Extract ownership for disposal; the poison stays set and no live operation is resumed.
fn take_session_for_shutdown(
    session_data: &RwLock<Option<SessionMetaData>>,
) -> Option<SessionMetaData> {
    session_data
        .write()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

fn init_session(
    starting_session: &mut Option<SessionMetaData>,
    os_input: Box<dyn ServerOsApi>,
    to_server: SenderWithContext<ServerInstruction>,
    shutdown: Shutdown,
    ownership: Arc<yinz_ipc::session_ownership::SessionOwnership>,
    session_identity: envs::SessionState,
    client_attributes: ClientAttributes,
    config_options: Options,
    layout: Layout,
    cli_assets: CliAssets,
    mut config: Config,
    plugin_aliases: PluginAliases,
    client_id: ClientId,
) -> Result<()> {
    use yinz_runtime::logging::startup_stage;
    config.options = config.options.merge(config_options.clone());

    let _ = SCROLL_BUFFER_SIZE.set(
        config_options
            .scroll_buffer_size
            .unwrap_or(DEFAULT_SCROLL_BUFFER_SIZE),
    );

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = shutdown.sender(to_screen);

    let (to_screen_bounded, bounded_screen_receiver): ChannelWithContext<ScreenInstruction> =
        channels::bounded(50);
    let to_screen_bounded = shutdown.sender(to_screen_bounded);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = shutdown.sender(to_plugin);
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = shutdown.sender(to_pty);

    let (to_pty_writer, pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = shutdown.sender(to_pty_writer);

    let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = shutdown.sender(to_background_jobs);

    let startup_started = std::time::Instant::now();
    log::info!("startup: host identity initialization starting");
    // Determine and initialize the data directory
    let data_dir = cli_assets.data_dir.unwrap_or_else(get_default_data_dir);
    // Before anything can write to the plugin data store: this is the one place that holds both
    // the data dir and the options the id is resolved from, and every later reader takes what is
    // cached here. See `yinz_runtime::host_identity`.
    startup_stage("server.host_identity", || {
        yinz_runtime::host_identity::init_host_id(&data_dir, config.options.host_id.as_ref())
    });
    log::info!(
        "startup: host identity initialized at {:?}",
        startup_started.elapsed()
    );
    let data_access = Arc::new(startup_stage("server.data_access", || {
        DataAccess::sqlite(&data_dir)
    }));

    let serialization_interval = config_options.serialization_interval;
    let disable_session_metadata = config_options.disable_session_metadata.unwrap_or(false);
    let session_checkpoint = Arc::new(background_jobs::SessionCheckpointWriter::default());

    // Config only exists once a client has connected, long after the pty backend
    // was built, so this is the first point at which we can know. It is safe to
    // be this late: the pty thread does not exist yet, so no pane can have been
    // spawned. Session-scoped by construction — attaching later with different
    // config will not change it.
    #[cfg(unix)]
    os_input.set_pty_host_enabled(
        config
            .options
            .detachable_panes
            .unwrap_or(pty_host::DEFAULT_DETACHABLE_PANES),
    );
    let default_shell = config_options.default_shell.clone().map(|command| {
        TerminalAction::RunCommand(RunCommand {
            command,
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let path_to_default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or_else(get_default_shell);

    let default_mode = config_options.default_mode.unwrap_or_default();
    let default_keybinds = config.keybinds.clone();

    let session = starting_session.insert(SessionMetaData {
        shutdown: shutdown.clone(),
        data_access: data_access.clone(),
        senders: ThreadSenders {
            session: session_identity.clone(),
            to_screen: Some(to_screen.clone()),
            to_pty: Some(to_pty.clone()),
            to_plugin: Some(to_plugin.clone()),
            to_pty_writer: Some(to_pty_writer.clone()),
            to_background_jobs: Some(to_background_jobs.clone()),
            to_server: Some(to_server.clone()),
            session_checkpoint: session_checkpoint.clone(),
            session_ownership: Some(Arc::downgrade(&ownership)),
            should_silently_fail: false,
        },
        default_shell: default_shell.clone(),
        session_configuration: Default::default(),
        current_input_modes: HashMap::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        pty_writer_thread: None,
        background_jobs_thread: None,
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        command_palette_restore_modes: HashMap::new(),
        pending_mouse_updates: Vec::new(),
        config_file_path: cli_assets.config_file_path.clone(),
        config_dir: cli_assets.config_dir.clone(),
    });

    log::info!(
        "startup: spawning pty thread at {:?}",
        startup_started.elapsed()
    );
    session.pty_thread = Some(spawn_session_worker("pty", {
        let layout = Box::new(layout.clone());
        let mut bus = Bus::new(
            vec![pty_receiver],
            Some(&to_screen_bounded),
            Some(&to_pty),
            Some(&to_plugin),
            Some(&to_server),
            Some(&to_pty_writer),
            Some(&to_background_jobs),
            Some(os_input.clone()),
        );
        bus.senders.session = session_identity.clone();
        bus.senders.session_checkpoint = session_checkpoint.clone();
        let pty = Pty::new(
            bus,
            cli_assets.is_debug,
            config_options.scrollback_editor.clone(),
            config_options.post_command_discovery_hook.clone(),
        );

        move || pty_thread_main(pty, layout.clone()).fatal()
    })?);

    log::info!(
        "startup: spawning screen thread at {:?}",
        startup_started.elapsed()
    );
    session.screen_thread = Some(spawn_session_worker("screen", {
        let mut screen_bus = Bus::new(
            vec![screen_receiver, bounded_screen_receiver],
            Some(&to_screen), // there are certain occasions (eg. caching) where the screen
            // needs to send messages to itself
            Some(&to_pty),
            Some(&to_plugin),
            Some(&to_server),
            Some(&to_pty_writer),
            Some(&to_background_jobs),
            Some(os_input.clone()),
        );
        screen_bus.senders.session = session_identity.clone();
        screen_bus.senders.session_checkpoint = session_checkpoint.clone();
        screen_bus.senders.session_ownership = Some(Arc::downgrade(&ownership));
        let max_panes = cli_assets.max_panes;

        let client_attributes_clone = client_attributes.clone();
        let debug = cli_assets.is_debug;
        let layout = Box::new(layout.clone());
        let config = config.clone();
        let config_file_path = cli_assets.config_file_path.clone();
        let config_dir = cli_assets.config_dir.clone();
        let screen_data_dir = data_dir.clone();
        move || {
            screen_thread_main(
                screen_bus,
                max_panes,
                client_attributes_clone,
                config,
                debug,
                config_file_path,
                config_dir,
                Some(screen_data_dir),
                layout,
            )
            .fatal();
        }
    })?);

    let yinz_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut session_env_vars: std::collections::BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    session_env_vars.insert("YINZ".into(), "0".into());
    session_env_vars.insert(
        "YINZ_SESSION_NAME".into(),
        session_identity.current().to_string(),
    );
    // CLI paths are not necessarily present in the server's inherited environment.
    // Expose them to plugins so their child CLI commands use this session's config.
    for (key, path) in [
        (
            yinz_runtime::consts::YINZ_CONFIG_FILE_ENV,
            cli_assets.config_file_path.as_ref(),
        ),
        (
            yinz_runtime::consts::YINZ_CONFIG_DIR_ENV,
            cli_assets.config_dir.as_ref(),
        ),
    ] {
        if let Some(path) = path {
            session_env_vars.insert(key.to_owned(), yinz_cwd.join(path).display().to_string());
        }
    }

    let default_layout_name = config_options
        .default_layout
        .as_ref()
        .map(|layout| layout.display().to_string());
    let layout_dir = layout_dir_or_default(
        config_options.layout_dir.clone(),
        cli_assets.config_dir.clone(),
    );
    let (available_layouts, available_layout_errors) =
        startup_stage("server.available_layouts", || {
            yinz_config::layout::list_available_layouts(layout_dir.clone(), &default_layout_name)
        });

    log::info!(
        "startup: spawning plugin thread at {:?}",
        startup_started.elapsed()
    );
    session.plugin_thread = Some(spawn_session_worker("plugins", {
        let mut plugin_bus = Bus::new(
            vec![plugin_receiver],
            Some(&to_screen_bounded),
            Some(&to_pty),
            Some(&to_plugin),
            Some(&to_server),
            Some(&to_pty_writer),
            Some(&to_background_jobs),
            None,
        );
        plugin_bus.senders.session = session_identity.clone();
        let plugin_session = plugins::ServerSession::Shared(session_identity.clone());
        // A plugin worker can outlive its bounded shutdown wait.
        let ownership = ownership.clone();

        let layout = Box::new(layout.clone());
        let default_shell = default_shell.clone();
        // The session's *active* config dir, not whichever conventional one happens to exist:
        // a session started with `--config-dir` must load plugins from the folder
        // `yinz setup --check` names for it.
        let user_plugin_dir = user_plugin_dir_or_default(
            config_options.user_plugin_dir.clone(),
            cli_assets.config_dir.clone(),
        );
        let mut background_plugins = config.background_plugins.clone();
        // Agent support is a host requirement independent of configuration ownership.
        background_plugins.insert(RunPluginOrAlias::RunPlugin(
            yinz_types::input::layout::RunPlugin {
                location: yinz_types::input::layout::RunPluginLocation::Yinz(
                    yinz_types::data::PluginTag::new("agent-opencode"),
                ),
                ..Default::default()
            },
        ));
        let hooks = config.hooks.clone();
        let context_menu = config.context_menu.clone();
        let session_env_vars = session_env_vars.clone();
        let data_access = data_access.clone();
        move || {
            let _ownership = ownership;
            plugin_thread_main(
                plugin_bus,
                data_dir,
                data_access,
                layout,
                layout_dir,
                user_plugin_dir,
                available_layouts,
                available_layout_errors,
                path_to_default_shell,
                yinz_cwd,
                session_env_vars,
                default_shell,
                plugin_aliases,
                default_mode,
                default_keybinds,
                background_plugins,
                hooks,
                context_menu,
                client_id,
                plugin_session,
            )
            .fatal()
        }
    })?);

    log::info!(
        "startup: spawning pty_writer thread at {:?}",
        startup_started.elapsed()
    );
    session.pty_writer_thread = Some(spawn_session_worker("pty_writer", {
        let pty_writer_bus = Bus::new(
            vec![pty_writer_receiver],
            Some(&to_screen),
            Some(&to_pty),
            Some(&to_plugin),
            Some(&to_server),
            None,
            Some(&to_background_jobs),
            Some(os_input.clone()),
        );
        || pty_writer_main(pty_writer_bus).fatal()
    })?);

    log::info!(
        "startup: spawning background_jobs thread at {:?}",
        startup_started.elapsed()
    );
    session.background_jobs_thread = Some(spawn_session_worker("background_jobs", {
        let mut background_jobs_bus = Bus::new(
            vec![background_jobs_receiver],
            Some(&to_screen),
            Some(&to_pty),
            Some(&to_plugin),
            Some(&to_server),
            Some(&to_pty_writer),
            // this thread sends to itself: streamed_web_request reports back with
            // WebRequestStreamFinished so the stream terminator is emitted from the loop
            Some(&to_background_jobs),
            Some(os_input.clone()),
        );
        background_jobs_bus.senders.session = session_identity.clone();
        background_jobs_bus.senders.session_checkpoint = session_checkpoint.clone();
        move || {
            background_jobs_main(
                background_jobs_bus,
                serialization_interval,
                disable_session_metadata,
            )
            .fatal()
        }
    })?);
    if let Some(config_file_path) = cli_assets.config_file_path.clone() {
        startup_stage("server.watch_config", || {
            report_changes_in_config_file(
                config_file_path,
                cli_assets.config_dir.as_deref(),
                to_server.clone(),
            )
        });
    }

    log::info!(
        "startup: worker threads spawned at {:?}",
        startup_started.elapsed()
    );
    Ok(())
}

fn setup_wizard_floating_pane() -> FloatingPaneLayout {
    let mut setup_wizard_pane = FloatingPaneLayout::new();
    let configuration = BTreeMap::from_iter([("is_setup_wizard".to_owned(), "true".to_owned())]);
    setup_wizard_pane.run = Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::new(
        "configuration",
        &Some(configuration),
        None,
    ))));
    setup_wizard_pane
}

/// What the CLI waiting on `pipe_name` is sent for a plugin's `output` to it: the output, or an
/// error when it does not fit in one IPC frame. The thread writing to the client would refuse
/// that frame but only log it, and `yinz pipe` then exited 0 with nothing on stdout.
fn cli_pipe_reply(pipe_name: String, output: String) -> ServerToClientMsg {
    ServerToClientMsg::cli_pipe_output(pipe_name, output)
        .context("cannot send the plugin's output to the CLI pipe")
        .unwrap_or_else(|error| ServerToClientMsg::LogError {
            lines: vec![format!("{error:#}")],
        })
}

#[cfg(test)]
#[path = "unit/capturing_os.rs"]
mod capturing_os;

#[cfg(test)]
#[path = "unit/session_shutdown_tests.rs"]
mod session_shutdown_tests;

#[cfg(test)]
#[path = "unit/cli_pipe_tests.rs"]
mod cli_pipe_tests;

#[cfg(test)]
#[path = "unit/session_start_cwd_tests.rs"]
mod session_start_cwd_tests;

#[cfg(test)]
#[path = "unit/client_selection_tests.rs"]
mod client_selection_tests;

#[cfg(test)]
#[path = "unit/session_request_tests.rs"]
mod session_request_tests;
