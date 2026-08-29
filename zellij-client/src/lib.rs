pub mod os_input_output;

#[path = "os_input_output_unix.rs"]
mod os_input_output_unix;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");

pub mod cli_client;
mod client_loop;
mod command_is_executing;
mod input_handler;
mod keyboard_parser;
mod nested_reannounce;
mod router;
mod startup;
mod stdin_ansi_parser;
mod stdin_handler;
mod terminal_escapes;

use log::info;
use std::env::current_exe;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use zellij_utils::errors::FatalError;

use crate::terminal_escapes::{
    announce_nested, enter_terminal, say_bye, write_seqs, ClientTeardown, CLEAR_SCREEN,
    ENABLE_BRACKETED_PASTE, ENABLE_FOCUS_REPORTING,
};

use crate::stdin_ansi_parser::{AnsiStdinInstruction, StdinAnsiParser, SyncOutput};
use crate::{
    command_is_executing::CommandIsExecuting, input_handler::input_loop,
    os_input_output::ClientOsApi, stdin_handler::stdin_loop,
};
use zellij_utils::cli::CliArgs;
use zellij_utils::{
    channels::{self, ChannelWithContext, SenderWithContext},
    consts::{set_permissions, ZELLIJ_SOCK_DIR},
    data::{ClientId, ConnectToSession, KeyWithModifier, LayoutInfo},
    envs,
    errors::{ClientContext, ErrorInstruction},
    input::{config::Config, options::Options},
    ipc::{ClientToServerMsg, ExitReason, ServerToClientMsg},
    vendored::termwiz::input::InputEvent,
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
    SwitchSession(ConnectToSession),
    SetSynchronizedOutput(Option<SyncOutput>),
    UnblockCliPipeInput(()), // String -> pipe name
    CliPipeOutput((), ()),   // String -> pipe name, String -> output
    QueryTerminalSize,
    #[allow(dead_code)] // we need the session name here even though we're not currently using it
    RenamedSession(String), // String -> new session name
    ConfigFileUpdated,
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
            ServerToClientMsg::Connected => ClientInstruction::Connected,
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

fn check_ipc_pipe_length(ipc_pipe: &Path) {
    use zellij_utils::consts::ZELLIJ_SOCK_MAX_LENGTH;
    let path_len = ipc_pipe.as_os_str().len();
    if path_len >= ZELLIJ_SOCK_MAX_LENGTH {
        eprintln!(
            "Error: the IPC socket path is too long ({} bytes, max {}):\n  {}\n\n\
             This is usually caused by a long $TMPDIR path.\n\
             To fix this, set a shorter socket directory, eg.:\n  \
             ZELLIJ_SOCKET_DIR=/tmp/zellij zellij",
            path_len,
            ZELLIJ_SOCK_MAX_LENGTH - 1,
            ipc_pipe.display()
        );
        std::process::exit(1);
    }
}

fn spawn_server_error_message(e: io::Error) -> String {
    format!(
        "Error: failed to start the Zellij server process:\n\n\
         Reason: {}\n\n\
         This can happen if the Zellij binary cannot be executed, or if the server \
         could not create its session socket - for example due to a permission issue \
         in the socket directory.\n\
         To fix a socket directory issue, set a writable socket directory, eg.:\n  \
         ZELLIJ_SOCKET_DIR=/tmp/zellij-$USER zellij",
        e
    )
}

fn create_ipc_pipe(teardown: Option<ClientTeardown>) -> PathBuf {
    let mut sock_dir = ZELLIJ_SOCK_DIR.clone();
    if let Err(e) = std::fs::create_dir_all(&sock_dir) {
        ClientTeardown::exit_after_startup_error(
            teardown,
            format!(
                "Error: failed to create the Zellij socket directory:\n  {}\n\n\
                 Reason: {}\n\n\
                 This usually means the directory (or one of its parents) is owned by \
                 another user or is not writable - for example if Zellij was previously \
                 run with `sudo`, or if $XDG_RUNTIME_DIR points to a directory you do not \
                 own.\nTo fix this, remove or correct the offending directory, or set a \
                 writable socket directory, eg.:\n  ZELLIJ_SOCKET_DIR=/tmp/zellij-$USER zellij",
                sock_dir.display(),
                e
            ),
        );
    }
    if let Err(e) = set_permissions(&sock_dir, 0o700) {
        ClientTeardown::exit_after_startup_error(
            teardown,
            format!(
                "Error: failed to set permissions (0700) on the Zellij socket directory:\n  {}\n\n\
                 Reason: {}\n\n\
                 This usually means the directory is owned by another user.\n\
                 To fix this, remove or correct the offending directory, or set a writable \
                 socket directory, eg.:\n  ZELLIJ_SOCKET_DIR=/tmp/zellij-$USER zellij",
                sock_dir.display(),
                e
            ),
        );
    }
    sock_dir.push(envs::get_session_name().unwrap());
    check_ipc_pipe_length(&sock_dir);
    sock_dir
}

/// Spawn the Zellij server process.
///
/// On Unix the server daemonizes (double-fork) inside start_server(), so
/// the intermediate child exits immediately and `cmd.status()` returns.
pub fn spawn_server(socket_path: &Path, debug: bool) -> io::Result<()> {
    let mut cmd = Command::new(current_exe()?);
    cmd.arg("--server").arg(socket_path);
    if debug {
        cmd.arg("--debug");
    }
    let status = cmd.status()?;
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

#[derive(Debug, Clone)]
pub enum ClientInfo {
    Attach(String, Options),
    New(String, Option<LayoutInfo>, Option<PathBuf>), // PathBuf -> explicit cwd
    Resurrect(String, PathBuf, bool, Option<PathBuf>), // (name, path_to_layout, force_run_commands, cwd)
    Watch(String, Options),                            // Watch mode (read-only)
}

impl ClientInfo {
    pub fn get_session_name(&self) -> &str {
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
    Exit,
}

#[allow(clippy::too_many_arguments)]
pub fn start_client(
    mut os_input: Box<dyn ClientOsApi>,
    cli_args: CliArgs,
    config: Config,          // saved to disk (or default?)
    config_options: Options, // CLI options merged into (getting priority over) saved config options
    info: ClientInfo,
    tab_position_to_focus: Option<usize>,
    pane_id_to_focus: Option<(u32, bool)>, // (pane_id, is_plugin)
    is_a_reconnect: bool,
    start_detached_and_exit: bool,
) -> Option<ConnectToSession> {
    if start_detached_and_exit {
        start_server_detached(os_input, cli_args, config, config_options, info);
        return None;
    }
    info!("Starting Zellij client!");

    let own_session_name = info.get_session_name().to_owned();
    let tracing_attach = matches!(&info, ClientInfo::Attach(..));
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} session={own_session_name:?} startup begin",
            std::process::id()
        );
    }

    let explicitly_disable_kitty_keyboard_protocol = config_options
        .support_kitty_keyboard_protocol
        .map(|e| !e)
        .unwrap_or(false);
    let support_kitty_graphics_protocol = config_options
        .support_kitty_graphics_protocol
        .unwrap_or(true);
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} terminal preparation begin",
            std::process::id()
        );
    }
    os_input.unset_raw_mode().unwrap();

    if !is_a_reconnect {
        // we don't do this for a reconnect because our controlling terminal already has the
        // attributes we want from it, and some terminals don't treat these atomically.
        enter_terminal(&*os_input, !explicitly_disable_kitty_keyboard_protocol);
    }
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} terminal preparation complete",
            std::process::id()
        );
    }
    envs::set_zellij("0".to_string());
    config.env.set_vars();

    let full_screen_ws = os_input.get_terminal_size();

    let terminal_teardown = ClientTeardown::at_startup(!explicitly_disable_kitty_keyboard_protocol);

    let startup = crate::startup::StartupContext::attached(
        &cli_args,
        &config_options,
        full_screen_ws,
        terminal_teardown,
    );
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} preparing AttachClient message",
            std::process::id()
        );
    }
    let (first_msg, ipc_pipe) = startup.first_message(
        &mut os_input,
        info,
        crate::startup::AttachFocus {
            tab_position: tab_position_to_focus,
            pane: pane_id_to_focus,
        },
    );

    let connect_started_at = std::time::Instant::now();
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} socket connect begin path={}",
            std::process::id(),
            ipc_pipe.display()
        );
    }
    if let Err(e) = os_input.connect_to_server(&ipc_pipe) {
        // The terminal is already on the alternate screen with the kitty
        // keyboard protocol pushed; a bare `exit` here would leave the shell
        // there, with the message gone along with the screen it was printed on.
        ClientTeardown::exit_after_startup_error(startup.teardown, e.to_string());
    }
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} socket connected after {:?}; AttachClient send begin",
            std::process::id(),
            connect_started_at.elapsed()
        );
    }
    os_input.send_to_server(first_msg);
    if tracing_attach {
        log::info!(
            "attach trace: client pid={} AttachClient send call complete after {:?}",
            std::process::id(),
            connect_started_at.elapsed()
        );
    }

    let mut command_is_executing = CommandIsExecuting::new();

    os_input.set_raw_mode();
    write_seqs(
        &*os_input,
        &[ENABLE_BRACKETED_PASTE, ENABLE_FOCUS_REPORTING],
    );
    // Announce unconditionally: whether there is a Zellij host on the other
    // side of the terminal is learnt from it answering, not from the
    // environment (which a remote pane, for one, does not carry over). The
    // reannounce thread gives up after a few unanswered tries.
    announce_nested(&*os_input, &own_session_name);
    let nested_reannounce = crate::nested_reannounce::NestedReannounce::spawn(
        os_input.clone(),
        own_session_name.clone(),
    );

    let (send_client_instructions, receive_client_instructions): ChannelWithContext<
        ClientInstruction,
    > = channels::bounded(50);
    let send_client_instructions = SenderWithContext::new(send_client_instructions);

    let (send_input_instructions, receive_input_instructions): ChannelWithContext<
        InputInstruction,
    > = channels::bounded(50);
    let send_input_instructions = SenderWithContext::new(send_input_instructions);

    if os_input.should_install_panic_hook() {
        std::panic::set_hook({
            use zellij_utils::errors::handle_panic;
            let send_client_instructions = send_client_instructions.clone();
            let os_input = os_input.clone();
            Box::new(move |info| {
                os_input.disable_mouse().non_fatal();
                if let Ok(()) = os_input.unset_raw_mode() {
                    handle_panic(info, Some(&send_client_instructions));
                }
            })
        });
    }

    let on_force_close = config_options.on_force_close.unwrap_or_default();
    let stdin_ansi_parser = Arc::new(Mutex::new(StdinAnsiParser::new()));

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
                )
            }
        });

    // Apps running inside Zellij panes can issue a whitelisted set
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
            let default_mode = config_options.default_mode.unwrap_or_default();
            let nested_reannounce = nested_reannounce.clone();
            move || {
                input_loop(
                    os_input,
                    config,
                    config_options,
                    command_is_executing,
                    send_client_instructions,
                    default_mode,
                    receive_input_instructions,
                    nested_reannounce,
                )
            }
        });

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
                            let _ = os_api
                                .get_stdout_writer()
                                .write(crate::stdin_handler::PIXEL_SIZE_QUERY.as_bytes());
                        }
                    }),
                    Box::new({
                        let os_api = os_input.clone();
                        move || {
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

    let router_thread = crate::router::spawn(os_input.clone(), send_client_instructions);

    let teardown = ClientTeardown::new(
        full_screen_ws.rows,
        !explicitly_disable_kitty_keyboard_protocol,
    );

    let outcome = crate::client_loop::ClientLoop {
        os_input: &*os_input,
        command_is_executing: &mut command_is_executing,
        stdin_ansi_parser: &stdin_ansi_parser,
        send_input_instructions: &send_input_instructions,
        nested_reannounce: &nested_reannounce,
        teardown,
        synchronised_output: match os_input.env_variable("TERM").as_deref() {
            Some("alacritty") => Some(SyncOutput::DCS),
            _ => None,
        },
    }
    .run(&receive_client_instructions);
    let exit_msg = outcome.exit_msg;
    let reconnect_to_session = outcome.reconnect_to_session;

    router_thread.join().unwrap();

    if nested_reannounce.host_contacted() {
        say_bye(&*os_input);
    }

    if reconnect_to_session.is_none() {
        info!("{}", exit_msg);
        teardown.restore(&*os_input, &exit_msg);
    } else {
        write_seqs(&*os_input, &[CLEAR_SCREEN]);
    }

    let _ = send_input_instructions.send(InputInstruction::Exit);

    reconnect_to_session
}

pub fn start_server_detached(
    mut os_input: Box<dyn ClientOsApi>,
    cli_args: CliArgs,
    config: Config,
    config_options: Options,
    info: ClientInfo,
) {
    envs::set_zellij("0".to_string());
    config.env.set_vars();

    // Only a session that does not exist yet can be started detached;
    // attaching needs a controlling terminal to attach to.
    if matches!(info, ClientInfo::Attach(..) | ClientInfo::Watch(..)) {
        eprintln!("Session already exists");
        std::process::exit(1);
    }

    let startup = crate::startup::StartupContext::detached(&cli_args, &config_options);
    let (first_msg, ipc_pipe) = startup.first_message(&mut os_input, info, Default::default());

    if let Err(e) = os_input.connect_to_server(&ipc_pipe) {
        // No terminal was claimed on this path (`teardown` is `None`), so this
        // is a plain message on stderr.
        ClientTeardown::exit_after_startup_error(startup.teardown, e.to_string());
    }
    os_input.send_to_server(first_msg);
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod unit;
