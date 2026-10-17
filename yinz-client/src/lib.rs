// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
pub mod os_input_output;

#[path = "os_input_output_unix.rs"]
mod os_input_output_unix;

#[cfg(target_os = "windows")]
compile_error!("Yinz does not support Windows");

pub mod cli_client;
mod client_loop;
pub mod repl;
pub use client_loop::ClientOutcome;
mod command_is_executing;
mod host_transport;
mod input_handler;
mod keyboard_parser;
mod nested_reannounce;
mod render_budget;
mod router;
mod startup;
mod startup_signals;
mod stdin_ansi_parser;
mod stdin_handler;
mod terminal_escapes;

use log::info;
use std::collections::BTreeSet;
use std::env::current_exe;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use yinz_runtime::errors::{FatalError, LoggableError};
use yinz_runtime::logging::startup_stage;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabNumber;

use crate::terminal_escapes::{
    announce_nested, claim_terminal, say_bye, write_seqs, ClientTeardown, CLEAR_SCREEN,
    ENABLE_BRACKETED_PASTE, ENABLE_FOCUS_REPORTING,
};

use crate::startup_signals::StartupSignalGuard;
use crate::stdin_ansi_parser::{AnsiStdinInstruction, StdinAnsiParser, SyncOutput};
use crate::{
    command_is_executing::CommandIsExecuting, input_handler::input_loop,
    os_input_output::ClientOsApi, stdin_handler::stdin_loop,
};
use yinz_cli::args::CliArgs;
use yinz_key_codec::vendored::termwiz::input::InputEvent;
use yinz_runtime::channels;
use yinz_runtime::channels::ChannelWithContext;
use yinz_runtime::channels::SenderWithContext;
use yinz_runtime::channels::Shutdown;
use yinz_runtime::envs;
use yinz_runtime::errors::ErrorInstruction;
use yinz_types::diagnostics::ClientContext;
use yinz_types::keys::{KeyModifier, KeyWithModifier};
use {
    yinz_config::config::Config, yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ExitReason,
    yinz_ipc::ipc::ServerToClientMsg, yinz_runtime::consts::set_permissions,
    yinz_runtime::consts::YINZ_SOCK_DIR, yinz_types::data::ClientId, yinz_types::data::LayoutInfo,
    yinz_types::data::SessionConnection, yinz_types::input::options::Options,
};

/// Instructions related to the client-side application
#[derive(Debug, Clone)]
pub(crate) enum ClientInstruction {
    Error(String),
    Render(String),
    UnblockInputThread,
    Exit(ExitReason),
    Connected,
    Log(Vec<String>),
    LogError(Vec<String>),
    SwitchSession(SessionConnection),
    SetSynchronizedOutput(Option<SyncOutput>),
    UnblockCliPipeInput(()), // String -> pipe name
    CliPipeOutput((), ()),   // String -> pipe name, String -> output
    QueryTerminalSize,
    #[allow(dead_code)] // we need the session name here even though we're not currently using it
    RenamedSession(SessionName), // String -> new session name
    ConfigFileUpdated,
    SetMouseMode(bool),
    /// Server asked us to forward `query_bytes` to the host terminal and
    /// collect the reply bytes into the window identified by `token`.
    ForwardQueryToHost {
        token: u32,
        query_bytes: Vec<u8>,
        resolve_async: bool,
    },
    EmitNestedSessionFrame(Vec<u8>),
}

impl From<ServerToClientMsg> for ClientInstruction {
    fn from(instruction: ServerToClientMsg) -> Self {
        match instruction {
            ServerToClientMsg::Exit { exit_reason } => ClientInstruction::Exit(exit_reason),
            ServerToClientMsg::Render { content } => ClientInstruction::Render(content),
            ServerToClientMsg::UnblockInputThread => ClientInstruction::UnblockInputThread,
            ServerToClientMsg::Connected { .. } => ClientInstruction::Connected,
            ServerToClientMsg::Log { lines } => ClientInstruction::Log(lines),
            ServerToClientMsg::LogError { lines } => ClientInstruction::LogError(lines),
            ServerToClientMsg::SwitchSession { connect_to_session } => {
                ClientInstruction::SwitchSession(connect_to_session)
            },
            ServerToClientMsg::UnblockCliPipeInput { .. } => {
                ClientInstruction::UnblockCliPipeInput(())
            },
            ServerToClientMsg::CliPipeOutput { .. } => ClientInstruction::CliPipeOutput((), ()),
            ServerToClientMsg::QueryTerminalSize => ClientInstruction::QueryTerminalSize,
            ServerToClientMsg::RenamedSession { name } => ClientInstruction::RenamedSession(name),
            ServerToClientMsg::SetMouseMode { enabled } => ClientInstruction::SetMouseMode(enabled),
            ServerToClientMsg::ConfigFileUpdated => ClientInstruction::ConfigFileUpdated,
            ServerToClientMsg::ForwardQueryToHost {
                token,
                query_bytes,
                resolve_async,
            } => ClientInstruction::ForwardQueryToHost {
                token,
                query_bytes,
                resolve_async,
            },
            ServerToClientMsg::EmitNestedSessionFrame { payload_bytes } => {
                ClientInstruction::EmitNestedSessionFrame(payload_bytes)
            },
            // Subscribe-only messages — not handled by regular interactive clients
            ServerToClientMsg::PaneRenderUpdate { .. } => ClientInstruction::UnblockInputThread,
            ServerToClientMsg::SubscribedPaneClosed { .. } => ClientInstruction::UnblockInputThread,
            ServerToClientMsg::AgentControl(_)
            | ServerToClientMsg::DataResponse { .. }
            | ServerToClientMsg::PluginResponse { .. }
            | ServerToClientMsg::AgentReportAccepted { .. }
            | ServerToClientMsg::KillSessionComplete { .. } => {
                ClientInstruction::UnblockInputThread
            },
            ServerToClientMsg::EventUpdate { .. } => ClientInstruction::UnblockInputThread,
        }
    }
}

impl From<&ClientInstruction> for ClientContext {
    fn from(client_instruction: &ClientInstruction) -> Self {
        match *client_instruction {
            ClientInstruction::Exit(_) => ClientContext::Exit,
            ClientInstruction::Error(_) => ClientContext::Error,
            ClientInstruction::Render(_) => ClientContext::Render,
            ClientInstruction::UnblockInputThread => ClientContext::UnblockInputThread,
            ClientInstruction::Connected => ClientContext::Connected,
            ClientInstruction::Log(_) => ClientContext::Log,
            ClientInstruction::LogError(_) => ClientContext::LogError,
            ClientInstruction::SwitchSession(..) => ClientContext::SwitchSession,
            ClientInstruction::SetSynchronizedOutput(..) => ClientContext::SetSynchronisedOutput,
            ClientInstruction::UnblockCliPipeInput(..) => ClientContext::UnblockCliPipeInput,
            ClientInstruction::CliPipeOutput(..) => ClientContext::CliPipeOutput,
            ClientInstruction::QueryTerminalSize => ClientContext::QueryTerminalSize,
            ClientInstruction::RenamedSession(..) => ClientContext::RenamedSession,
            ClientInstruction::SetMouseMode(_) => ClientContext::ConfigFileUpdated,
            ClientInstruction::ConfigFileUpdated => ClientContext::ConfigFileUpdated,
            ClientInstruction::ForwardQueryToHost { .. } => ClientContext::ForwardQueryToHost,
            ClientInstruction::EmitNestedSessionFrame(..) => ClientContext::EmitNestedSessionFrame,
        }
    }
}

impl ErrorInstruction for ClientInstruction {
    fn error(err: String) -> Self {
        ClientInstruction::Error(err)
    }
}

/// Hand the terminal back before the default panic handler prints.
///
/// The terminal is now claimed - raw mode, alternate screen, kitty keyboard - before the server is
/// spawned and the socket connected, so the stretch before the real panic hook (which needs the
/// client's channels) is long enough to be worth guarding. It restores all of it and then defers to
/// whatever hook was already installed, so the panic message and backtrace land on the screen the
/// user is left looking at rather than on an alternate screen that is never popped.
fn install_early_terminal_panic_guard(teardown: ClientTeardown) {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(
            "startup trace: client panic location={:?}\n{}",
            info.location(),
            std::backtrace::Backtrace::force_capture()
        );
        teardown.restore_startup(None);
        previous_hook(info);
    }));
}

fn check_ipc_pipe_length(ipc_pipe: &Path, teardown: Option<ClientTeardown>) {
    use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;
    let path_len = ipc_pipe.as_os_str().len();
    if path_len >= YINZ_SOCK_MAX_LENGTH {
        ClientTeardown::exit_after_startup_error(
            teardown,
            format!(
                "Error: the IPC socket path is too long ({} bytes, max {}):\n  {}\n\n\
                 The socket directory or session name is too long.\n\
                 To fix this, set a shorter socket directory, eg.:\n  \
                 YINZ_SOCKET_DIR=/tmp/yinz yinz",
                path_len,
                YINZ_SOCK_MAX_LENGTH - 1,
                ipc_pipe.display()
            ),
        );
    }
}

fn spawn_server_error_message(e: io::Error) -> String {
    format!(
        "Error: failed to start the Yinz server process:\n\n\
         Reason: {}\n\n\
         This can happen if the Yinz binary cannot be executed, or if the server \
         could not create its session socket - for example due to a permission issue \
         in the socket directory.\n\
         To fix a socket directory issue, set a writable socket directory, eg.:\n  \
         YINZ_SOCKET_DIR=/tmp/yinz-$USER yinz",
        e
    )
}

/// Create the socket directory and restrict it to its owner. The `Err` is the diagnostic to
/// show the user: both failures mean the directory is owned by someone else or is not writable,
/// and `YINZ_SOCKET_DIR` is the way out.
fn prepare_socket_dir(sock_dir: &Path) -> std::result::Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    if let Err(e) = std::fs::create_dir_all(sock_dir) {
        return Err(format!(
            "Error: failed to create the Yinz socket directory:\n  {}\n\n\
             Reason: {}\n\n\
             This usually means the directory (or one of its parents) is owned by \
             another user or is not writable - for example if Yinz was previously \
             run with `sudo`, or if $XDG_RUNTIME_DIR points to a directory you do not \
             own.\nTo fix this, remove or correct the offending directory, or set a \
             writable socket directory, eg.:\n  YINZ_SOCKET_DIR=/tmp/yinz-$USER yinz",
            sock_dir.display(),
            e
        ));
    }
    // An already secured directory can be a read-only mount in a Bubblewrap client.
    if std::fs::metadata(sock_dir).is_ok_and(|metadata| {
        metadata.uid() == nix::unistd::geteuid().as_raw() && metadata.mode() & 0o7777 == 0o700
    }) {
        return Ok(());
    }
    if let Err(e) = set_permissions(sock_dir, 0o700) {
        return Err(format!(
            "Error: failed to set permissions (0700) on the Yinz socket directory:\n  {}\n\n\
             Reason: {}\n\n\
             This usually means the directory is owned by another user.\n\
             To fix this, remove or correct the offending directory, or set a writable \
             socket directory, eg.:\n  YINZ_SOCKET_DIR=/tmp/yinz-$USER yinz",
            sock_dir.display(),
            e
        ));
    }
    Ok(())
}

/// The socket path for `session_name`, with the socket directory prepared first.
///
/// Exits with a startup diagnostic when the directory cannot be prepared or the path is too
/// long: through `teardown` when the terminal has been claimed, on stderr otherwise. Every
/// client that connects to a session by name goes through here, so a socket directory that
/// cannot be used produces the same message from `yinz action` as from `yinz attach`.
pub(crate) fn ipc_pipe_for_session(
    session_name: &SessionName,
    teardown: Option<ClientTeardown>,
) -> PathBuf {
    if let Err(message) = prepare_socket_dir(&YINZ_SOCK_DIR) {
        ClientTeardown::exit_after_startup_error(teardown, message);
    }
    let ipc_pipe = YINZ_SOCK_DIR.join(session_name);
    check_ipc_pipe_length(&ipc_pipe, teardown);
    ipc_pipe
}

/// Restores output post-processing for as long as it is held, and clears it again on the way out.
///
/// The child spawned inside this window inherits our terminal, and can print setup errors to it
/// before it daemonizes; without `ONLCR` those staircase down the screen. Only the output side is
/// touched - dropping back to cooked mode here would let the host's replies to the queries we have
/// already written echo onto the alternate screen, which is the very thing raw mode is on for.
struct OutputProcessingRestored;

impl OutputProcessingRestored {
    fn around_a_child() -> Self {
        crate::os_input_output_unix::set_output_processing(true);
        Self
    }
}

impl Drop for OutputProcessingRestored {
    fn drop(&mut self) {
        crate::os_input_output_unix::set_output_processing(false);
    }
}

/// Spawn the Yinz server process.
///
/// On Unix the server daemonizes (double-fork) inside start_server(), so
/// the intermediate child exits immediately and `cmd.status()` returns.
pub fn spawn_server(socket_path: &Path, debug: bool) -> io::Result<()> {
    spawn_server_with_termios(socket_path, debug, None)
}

fn spawn_server_with_termios(
    socket_path: &Path,
    debug: bool,
    termios: Option<&Option<yinz_runtime::termios::WireTermios>>,
) -> io::Result<()> {
    let mut cmd = startup_stage("client.server_command", || {
        server_command(socket_path, debug, termios)
    })?;
    let status = {
        let _output_processing = OutputProcessingRestored::around_a_child();
        let mut child = startup_stage("client.server_spawn", || cmd.spawn())?;
        log::info!(
            "startup trace: server launcher_pid={} socket={socket_path:?}",
            child.id()
        );
        startup_stage("client.server_wait", || child.wait())?
    };
    log::info!("startup trace: server launcher status={status}");
    if status.success() {
        Ok(())
    } else {
        let msg = "Process returned non-zero exit code";
        let err_msg = match status.code() {
            Some(c) => format!("{}: {}", msg, c),
            None => msg.to_string(),
        };
        Err(io::Error::other(err_msg))
    }
}

fn server_command(
    socket_path: &Path,
    debug: bool,
    termios: Option<&Option<yinz_runtime::termios::WireTermios>>,
) -> io::Result<Command> {
    let mut cmd = Command::new(current_exe()?);
    cmd.arg("--server").arg(socket_path);
    if let Some(termios) = termios {
        cmd.arg("--server-termios")
            .arg(serde_json::to_string(termios).map_err(io::Error::other)?);
    }
    if debug {
        cmd.arg("--debug");
    }
    Ok(cmd)
}

#[derive(Debug, Clone)]
pub enum ClientInfo {
    Attach(SessionName, Options),
    New(SessionName, Option<LayoutInfo>, Option<PathBuf>), // PathBuf -> explicit cwd
    Resurrect(SessionName, PathBuf, bool, Option<PathBuf>), // (name, path_to_layout, force_run_commands, cwd)
    Watch(SessionName, Options),                            // Watch mode (read-only)
}

impl ClientInfo {
    pub fn get_session_name(&self) -> &SessionName {
        match self {
            Self::Attach(ref name, _) => name,
            Self::New(ref name, _layout_info, _layout_cwd) => name,
            Self::Resurrect(ref name, _, _, _) => name,
            Self::Watch(ref name, _) => name,
        }
    }
    pub fn set_layout_info(&mut self, new_layout_info: LayoutInfo) {
        if let ClientInfo::New(_, layout_info, _) = self {
            *layout_info = Some(new_layout_info)
        }
    }
    pub fn set_cwd(&mut self, new_cwd: PathBuf) {
        match self {
            ClientInfo::New(_, _, cwd) => *cwd = Some(new_cwd),
            ClientInfo::Resurrect(_, _, _, cwd) => *cwd = Some(new_cwd),
            _ => {},
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum InputInstruction {
    SetMouseMode(bool),
    KeyEvent(InputEvent, Vec<u8>),
    KeyWithModifierEvent(KeyWithModifier, Vec<u8>, bool), // bool = is_kitty_keyboard_protocol
    AnsiStdinInstructions(Vec<AnsiStdinInstruction>),
    DesktopNotificationResponse(Vec<u8>),
    /// The continuous host-reply parser closed a forwarding window (barrier
    /// reply seen or timeout fired). Payload is the accumulated raw bytes
    /// to ship to the server.
    ForwardedReplyFromHostComplete {
        token: u32,
        reply_bytes: Vec<u8>,
    },
    NestedSessionFrameFromHost(Vec<u8>),
    /// The host terminal gained (`true`) or lost (`false`) keyboard focus.
    HostTerminalFocusChanged(bool),
    /// Which modifier keys the host says are physically down, sent when the set changes.
    ///
    /// Only ever non-empty on a terminal asked for `REPORT_ALL_KEYS_AS_ESCAPE_CODES`, which is
    /// the only way a modifier key on its own is ever reported at all.
    ModifiersHeldChanged(BTreeSet<KeyModifier>),
    Exit,
}

#[allow(clippy::too_many_arguments)]
pub fn start_client(
    mut os_input: Box<dyn ClientOsApi>,
    cli_args: CliArgs,
    config: Config,          // saved to disk (or default?)
    config_options: Options, // CLI options merged into (getting priority over) saved config options
    info: ClientInfo,
    tab_number_to_focus: Option<TabNumber>,
    pane_id_to_focus: Option<yinz_types::pane_id::PaneId>,
    is_a_reconnect: bool,
    start_detached_and_exit: bool,
) -> ClientOutcome {
    let startup_started = std::time::Instant::now();
    let own_session_name = info.get_session_name().to_owned();
    let kind = match &info {
        ClientInfo::New(..) => "new",
        ClientInfo::Attach(..) => "attach",
        ClientInfo::Resurrect(..) => "resurrect",
        ClientInfo::Watch(..) => "watch",
    };
    log::info!("startup trace: client session={own_session_name:?} kind={kind} detached={start_detached_and_exit} reconnect={is_a_reconnect}");
    if start_detached_and_exit {
        start_server_detached(os_input, cli_args, config, config_options, info);
        return ClientOutcome {
            exit_msg: String::new(),
            exit_status: 0,
            reconnect_to_session: None,
        };
    }
    info!("Starting Yinz client!");

    let explicitly_disable_kitty_keyboard_protocol = config_options
        .support_kitty_keyboard_protocol
        .map(|e| !e)
        .unwrap_or(false);
    // Opt-in, and only meaningful with the protocol on at all: the flags that report a modifier
    // key on its own also stop the terminal sending composed text. See
    // `ENTER_KITTY_KEYBOARD_MODE_WITH_MODIFIERS`.
    let report_modifier_keys = config_options.report_modifier_keys.unwrap_or(false)
        && !explicitly_disable_kitty_keyboard_protocol;
    // An explicit setting is the user's call either way - among other things it is the escape
    // hatch for a mosh fork that does pass graphics through. Unset, the default is "yes, unless
    // we can see we are behind something that would only mangle it".
    let support_kitty_graphics_protocol = config_options
        .support_kitty_graphics_protocol
        .unwrap_or_else(|| !host_transport::host_swallows_kitty_graphics());
    let terminal_teardown = ClientTeardown::at_startup(!explicitly_disable_kitty_keyboard_protocol);
    // Both of these cover the same stretch: the terminal is claimed before the server is spawned
    // and the socket connected, while the real panic hook (which needs channels that do not exist
    // yet) and the real signal listener are still some way off. A panic or a Ctrl-C in between
    // would otherwise hand the shell back raw, on an alternate screen.
    let mut startup_signal_guard = None;
    if os_input.should_install_panic_hook() {
        install_early_terminal_panic_guard(terminal_teardown);
        startup_signal_guard = StartupSignalGuard::arm(terminal_teardown);
    }
    startup_stage("client.claim_terminal", || {
        claim_terminal(
            &mut *os_input,
            !explicitly_disable_kitty_keyboard_protocol,
            report_modifier_keys,
            is_a_reconnect,
        )
    });
    envs::set_yinz("0".to_string());
    yinz_runtime::envs::set_vars(&config.env);

    let full_screen_ws = startup_stage("client.terminal_size", || os_input.get_terminal_size());
    log::info!("startup trace: terminal rows={} cols={} kitty_keyboard={} kitty_graphics={support_kitty_graphics_protocol}", full_screen_ws.rows, full_screen_ws.cols, !explicitly_disable_kitty_keyboard_protocol);

    let startup = crate::startup::StartupContext::attached(
        &cli_args,
        &config_options,
        full_screen_ws,
        terminal_teardown,
    );
    let (first_msg, ipc_pipe) = startup_stage("client.first_message", || {
        startup.first_message(
            &mut os_input,
            info,
            crate::startup::AttachFocus {
                tab_number: tab_number_to_focus,
                pane: pane_id_to_focus,
            },
        )
    });

    if let Err(e) = startup_stage("client.connect", || os_input.connect_to_server(&ipc_pipe)) {
        // The terminal is already on the alternate screen with the kitty
        // keyboard protocol pushed; a bare `exit` here would leave the shell
        // there, with the message gone along with the screen it was printed on.
        ClientTeardown::exit_after_startup_error(startup.teardown, e.to_string());
    }
    startup_stage("client.send_first_message", || {
        os_input.send_to_server(first_msg)
    });

    let mut command_is_executing = CommandIsExecuting::new();

    // The socket is up and we are about to start reading the keyboard, so take the signal keys
    // too: from here on Ctrl-C is a keystroke to forward to a pane, not a way out of a stuck
    // startup. `claim_terminal` left them working for exactly that window.
    startup_stage("client.raw_mode", || os_input.set_raw_mode());
    startup_stage("client.enable_terminal_events", || {
        write_seqs(
            &*os_input,
            &[ENABLE_BRACKETED_PASTE, ENABLE_FOCUS_REPORTING],
        )
    });
    // Announce unconditionally: whether there is a Yinz host on the other
    // side of the terminal is learnt from it answering, not from the
    // environment (which a remote pane, for one, does not carry over). The
    // reannounce thread gives up after a few unanswered tries.
    startup_stage("client.announce_nested", || {
        announce_nested(&*os_input, &own_session_name)
    });
    let nested_reannounce = crate::nested_reannounce::NestedReannounce::spawn(
        os_input.clone(),
        own_session_name.clone(),
    );

    let (send_client_instructions, receive_client_instructions): ChannelWithContext<
        ClientInstruction,
    > = channels::bounded(50);
    // Begun just before the receiver is dropped, so late sends from the router and input
    // threads are not reported as errors.
    let client_shutdown = Shutdown::default();
    let send_client_instructions = client_shutdown.sender(send_client_instructions);

    let (send_input_instructions, receive_input_instructions): ChannelWithContext<
        InputInstruction,
    > = channels::bounded(50);
    let send_input_instructions = SenderWithContext::new(send_input_instructions);

    if os_input.should_install_panic_hook() {
        std::panic::set_hook({
            use yinz_runtime::errors::handle_panic;
            let send_client_instructions = send_client_instructions.clone();
            let os_input = os_input.clone();
            Box::new(move |info| {
                // A paused host terminal must not keep a panicking client from tearing down.
                crate::terminal_escapes::abandon_stalled_output();
                os_input.disable_mouse().non_fatal();
                if let Ok(()) = os_input.unset_raw_mode() {
                    handle_panic(info, Some(&send_client_instructions));
                }
            })
        });
    }

    let on_force_close = config_options.on_force_close.unwrap_or_default();
    let stdin_ansi_parser = Arc::new(Mutex::new(StdinAnsiParser::new()));

    log::info!("startup trace: stage=client.workers event=begin");
    let workers_started = std::time::Instant::now();
    let _stdin_thread = thread::Builder::new()
        .name("stdin_handler".to_string())
        .spawn({
            let os_input = os_input.clone();
            let send_input_instructions = send_input_instructions.clone();
            let stdin_ansi_parser = stdin_ansi_parser.clone();
            move || {
                stdin_loop(
                    os_input,
                    send_input_instructions,
                    stdin_ansi_parser,
                    explicitly_disable_kitty_keyboard_protocol,
                    support_kitty_graphics_protocol,
                    report_modifier_keys,
                )
            }
        });

    // Apps running inside Yinz panes can issue a whitelisted set
    // of queries to the host terminal (bg/fg colour, palette
    // registers, window pixel dimensions). Each query opens a
    // "forward slot" on the client: we write the query + a
    // Primary-DA barrier to stdout, then collect any reply bytes
    // that arrive on stdin until the barrier reply closes the slot.
    // The pane that asked gets the captured bytes piped to its pty.
    //
    // If the host never answers, we must close the slot anyway so
    // the server can dispatch the next queued forward. A per-slot
    // timer task enforces that deadline: opening a forward spawns
    // an async sleep on `forward_timeout_runtime()`; on wake it
    // tries to close the slot for that specific token. If the
    // barrier (or a later forward) closed the slot first, the
    // timer's close call is a no-op — the token-guard makes
    // cancellation implicit. Spawn site: the
    // `ClientInstruction::ForwardQueryToHost` handler below.

    let _input_thread = thread::Builder::new()
        .name("input_handler".to_string())
        .spawn({
            let send_client_instructions = send_client_instructions.clone();
            let command_is_executing = command_is_executing.clone();
            let os_input = os_input.clone();
            let nested_reannounce = nested_reannounce.clone();
            move || {
                input_loop(
                    os_input,
                    config_options,
                    command_is_executing,
                    send_client_instructions,
                    receive_input_instructions,
                    nested_reannounce,
                )
            }
        });

    // Handing quit signals over from the startup guard to the real listener.
    if let Some(guard) = startup_signal_guard.take() {
        guard.disarm();
    }
    let pixel_size_queries = crate::terminal_escapes::PixelSizeQueries::spawn({
        let os_api = os_input.clone();
        move || os_api.get_stdout_writer()
    })
    .to_log()
    .ok();
    let _signal_thread = thread::Builder::new()
        .name("signal_listener".to_string())
        .spawn({
            let os_input = os_input.clone();
            move || {
                os_input.handle_signals(
                    Box::new({
                        let os_api = os_input.clone();
                        move || {
                            os_api.send_to_server(ClientToServerMsg::TerminalResize {
                                new_size: os_api.get_terminal_size(),
                            });
                            // Written elsewhere: this thread must not wait on the terminal.
                            if let Some(queries) = &pixel_size_queries {
                                queries.request();
                            }
                        }
                    }),
                    Box::new({
                        let os_api = os_input.clone();
                        move || {
                            // A paused terminal must not keep the client from quitting.
                            crate::terminal_escapes::abandon_stalled_output();
                            os_api.send_to_server(ClientToServerMsg::Action {
                                action: on_force_close.into(),
                                terminal_id: None,
                                client_id: None,
                                is_cli_client: false,
                            });
                        }
                    }),
                );
            }
        })
        .unwrap();

    let render_budget = std::sync::Arc::new(crate::render_budget::RenderBudget::new(
        crate::render_budget::RENDER_BUDGET_BYTES,
    ));
    let router_thread = crate::router::spawn(
        os_input.clone(),
        send_client_instructions,
        render_budget.clone(),
    );

    let teardown = ClientTeardown::new(
        full_screen_ws.rows,
        !explicitly_disable_kitty_keyboard_protocol,
    );

    // Alacritty answers no DECRPM probe for mode 2026, so its DCS-flavoured synchronised output
    // is recognised from TERM alone and never reaches the server the way a probe reply does. The
    // client loop tells the server what this comes to, and tells it again if a probe reply later
    // changes the answer - see `ClientLoop::announce_synchronised_output`.
    let synchronised_output = match os_input.env_variable("TERM").as_deref() {
        Some("alacritty") => Some(SyncOutput::Dcs),
        _ => None,
    };

    log::info!(
        "startup trace: stage=client.workers event=end elapsed_us={}; waiting for first render",
        workers_started.elapsed().as_micros()
    );
    let outcome = crate::client_loop::ClientLoop {
        os_input: &*os_input,
        command_is_executing: &mut command_is_executing,
        stdin_ansi_parser: &stdin_ansi_parser,
        send_input_instructions: &send_input_instructions,
        nested_reannounce: &nested_reannounce,
        teardown,
        synchronised_output,
        render_budget: &render_budget,
        startup_started,
    }
    .run(&receive_client_instructions);
    // The router may be waiting for frames to drain that nobody will now write,
    // or for room in a channel nobody will now read. Dropping the receiver fails
    // its next send, so it stops instead of holding up the join below.
    render_budget.close().non_fatal();
    client_shutdown.begin();
    drop(receive_client_instructions);

    router_thread.join().unwrap();

    if nested_reannounce.host_contacted() {
        say_bye(&*os_input);
    }

    if outcome.reconnect_to_session.is_none() {
        info!("{}", outcome.exit_msg);
        teardown.restore(&*os_input, &outcome.exit_msg);
    } else {
        write_seqs(&*os_input, &[CLEAR_SCREEN]);
    }

    let _ = send_input_instructions.send(InputInstruction::Exit);

    outcome
}

pub fn start_server_detached(
    mut os_input: Box<dyn ClientOsApi>,
    cli_args: CliArgs,
    config: Config,
    config_options: Options,
    info: ClientInfo,
) {
    envs::set_yinz("0".to_string());
    yinz_runtime::envs::set_vars(&config.env);

    // Only a session that does not exist yet can be started detached;
    // attaching needs a controlling terminal to attach to.
    if matches!(info, ClientInfo::Attach(..) | ClientInfo::Watch(..)) {
        eprintln!("Session already exists");
        std::process::exit(1);
    }

    let startup = crate::startup::StartupContext::detached(&cli_args, &config_options);
    let (first_msg, ipc_pipe) = startup_stage("client.first_message", || {
        startup.first_message(&mut os_input, info, Default::default())
    });

    if let Err(e) = startup_stage("client.connect", || os_input.connect_to_server(&ipc_pipe)) {
        // No terminal was claimed on this path (`teardown` is `None`), so this
        // is a plain message on stderr.
        ClientTeardown::exit_after_startup_error(startup.teardown, e.to_string());
    }
    startup_stage("client.send_first_message", || {
        os_input.send_to_server(first_msg)
    });
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod unit;

pub mod store_client;

pub mod wait_client;
