//! The `[cli_client]` is used to attach to a running server session
//! and dispatch actions, that are specified through the command line.
use std::collections::{BTreeMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process;
use std::str::FromStr;
use yinz_types::internal_ids::SessionName;

use crate::os_input_output::CliClientOs;
use uuid::Uuid;
use {
    yinz_cli::args::EventsCli, yinz_cli::args::SubscribeCli, yinz_cli::args::SubscribeFormat,
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ExitReason, yinz_ipc::ipc::IpcRecv,
    yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::EventType, yinz_types::data::PaneId,
    yinz_types::input::actions::Action,
};

pub fn start_cli_client(
    mut os_input: Box<dyn CliClientOs>,
    session_name: &SessionName,
    actions: Vec<Action>,
    pane_id: Option<yinz_types::internal_ids::TerminalId>,
) -> i32 {
    let yinz_ipc_pipe = crate::ipc_pipe_for_session(session_name, None);
    if let Err(e) = os_input.connect_to_server(&yinz_ipc_pipe) {
        eprintln!("{e}");
        return 1;
    }

    for action in actions {
        match action {
            Action::CliPipe {
                focus_plugin,
                pipe_id,
                name,
                payload,
                plugin,
                args,
                configuration,
                launch_new,
                skip_cache,
                floating,
                in_place,
                cwd,
                pane_title,
            } => {
                if let Some(exit_status) = pipe_client(
                    &mut os_input,
                    focus_plugin,
                    pipe_id,
                    name,
                    payload,
                    plugin,
                    args,
                    configuration,
                    launch_new,
                    skip_cache,
                    floating,
                    in_place,
                    pane_id,
                    cwd,
                    pane_title,
                ) {
                    return exit_status;
                }
            },
            action => {
                if let Some(exit_status) =
                    individual_messages_client(&mut os_input, action, pane_id)
                {
                    return exit_status;
                }
            },
        }
    }
    os_input.send_to_server(ClientToServerMsg::ClientExited);
    0
}

#[allow(clippy::too_many_arguments)]
fn pipe_client(
    os_input: &mut Box<dyn CliClientOs>,
    focus_plugin: bool,
    pipe_id: String,
    mut name: Option<String>,
    mut payload: Option<String>,
    plugin: Option<String>,
    args: Option<BTreeMap<String, String>>,
    mut configuration: Option<BTreeMap<String, String>>,
    launch_new: bool,
    skip_cache: bool,
    floating: Option<bool>,
    in_place: Option<bool>,
    pane_id: Option<yinz_types::internal_ids::TerminalId>,
    cwd: Option<PathBuf>,
    pane_title: Option<String>,
) -> Option<i32> {
    let mut stdin = os_input.get_stdin_reader();
    let name = name
        // first we try to take the explicitly supplied message name
        .take()
        // then we use the plugin, to facilitate using aliases
        .or_else(|| plugin.clone())
        // then we use a uuid to at least have some sort of identifier for this message
        .or_else(|| Some(Uuid::new_v4().to_string()));
    if launch_new {
        // we do this to make sure the plugin is unique (has a unique configuration parameter) so
        // that a new one would be launched, but we'll still send it to the same instance rather
        // than launching a new one in every iteration of the loop
        configuration
            .get_or_insert_with(BTreeMap::new)
            .insert("_yinz_id".to_owned(), Uuid::new_v4().to_string());
    }
    let create_msg = |payload: Option<String>| -> ClientToServerMsg {
        ClientToServerMsg::Action {
            action: Action::CliPipe {
                focus_plugin,
                pipe_id: pipe_id.clone(),
                name: name.clone(),
                payload,
                args: args.clone(),
                plugin: plugin.clone(),
                configuration: configuration.clone(),
                floating,
                in_place,
                launch_new,
                skip_cache,
                cwd: cwd.clone(),
                pane_title: pane_title.clone(),
            },
            terminal_id: pane_id,
            client_id: None,
            is_cli_client: true,
        }
    };
    let is_piped = !os_input.stdin_is_terminal();
    loop {
        let mut input_finished = false;
        if let Some(payload) = payload.take() {
            let msg = create_msg(Some(payload));
            os_input.send_to_server(msg);
        } else if !is_piped {
            // here we send an empty message to trigger the plugin, because we don't have any more
            // data
            let msg = create_msg(None);
            os_input.send_to_server(msg);
        } else {
            // we didn't get payload from the command line, meaning we listen on STDIN because this
            // signifies the user is about to pipe more (eg. cat my-large-file | yinz pipe ...)
            let mut buffer = String::new();
            if let Err(error) = stdin.read_line(&mut buffer) {
                eprintln!("Failed to read pipe input: {error}");
                return Some(2);
            }
            if buffer.is_empty() {
                let msg = create_msg(None);
                os_input.send_to_server(msg);
                input_finished = true;
            } else {
                // we've got data! send it down the pipe (most common)
                let msg = create_msg(Some(buffer));
                os_input.send_to_server(msg);
            }
        }
        loop {
            // wait for a response and act accordingly
            match os_input.recv_from_server() {
                IpcRecv::Msg(ServerToClientMsg::UnblockCliPipeInput { pipe_name }, _) => {
                    // unblock this pipe, meaning we need to stop waiting for a response and read
                    // once more from STDIN
                    if pipe_name == pipe_id {
                        if input_finished {
                            return None;
                        } else if !is_piped {
                            // if this client is not piped, we need to exit the process completely
                            // rather than wait for more data
                            return Some(0);
                        } else {
                            break;
                        }
                    }
                },
                IpcRecv::Msg(ServerToClientMsg::CliPipeOutput { pipe_name, output }, _) => {
                    // send data to STDOUT, this *does not* mean we need to unblock the input
                    if pipe_name == pipe_id {
                        let mut stdout = os_input.get_stdout_writer();
                        if let Err(error) = stdout
                            .write_all(output.as_bytes())
                            .and_then(|_| stdout.flush())
                        {
                            return Some(output_error_status(error));
                        }
                    }
                },
                IpcRecv::Msg(ServerToClientMsg::Log { lines: log_lines }, _) => {
                    return Some(
                        write_log_lines(&mut os_input.get_stdout_writer(), &log_lines)
                            .map_or_else(output_error_status, |()| 0),
                    );
                },
                IpcRecv::Msg(ServerToClientMsg::LogError { lines: log_lines }, _) => {
                    log_lines.iter().for_each(|line| eprintln!("{line}"));
                    return Some(2);
                },
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => match exit_reason {
                    ExitReason::Error(e) => {
                        eprintln!("{}", e);
                        return Some(2);
                    },
                    ExitReason::CustomExitStatus(exit_status) => {
                        return Some(exit_status);
                    },
                    _ => {
                        return Some(0);
                    },
                },
                IpcRecv::Eof => {
                    eprintln!("Server disconnected before completing the pipe request");
                    return Some(2);
                },
                _ => {},
            }
        }
    }
}

fn individual_messages_client(
    os_input: &mut Box<dyn CliClientOs>,
    action: Action,
    pane_id: Option<yinz_types::internal_ids::TerminalId>,
) -> Option<i32> {
    // Every blocking pane waits for its own `Exit`, condition or no condition. A session running
    // an older server broadcasts `UnblockInputThread` to every connection whenever any pane
    // closes anywhere in the session, so a plain `--blocking` that returned on it would report
    // success the moment an unrelated pane went away. This server unblocks interactive clients
    // only.
    let is_blocking = matches!(&action, Action::NewBlockingPane { .. });
    // An older server's warp placement and rollback also broadcast input unblocks before their
    // result arrives.
    let wait_for_result = is_blocking
        || matches!(
            &action,
            Action::WarpPane { .. }
                | Action::WarpTab { .. }
                | Action::WarpTabFrom { .. }
                | Action::AdoptPane { .. }
                | Action::AdoptTab { .. }
        );
    let msg = ClientToServerMsg::Action {
        action,
        terminal_id: pane_id,
        client_id: None,
        is_cli_client: true,
    };
    os_input.send_to_server(msg);
    loop {
        match os_input.recv_from_server() {
            IpcRecv::Msg(ServerToClientMsg::UnblockInputThread, _) if !wait_for_result => {
                return None;
            },
            IpcRecv::Msg(ServerToClientMsg::Log { lines: log_lines }, _) => {
                if let Err(error) = write_log_lines(&mut os_input.get_stdout_writer(), &log_lines) {
                    return Some(output_error_status(error));
                }
                // A blocking pane's ids arrive ahead of its exit status; the status is the
                // answer, so keep reading for it.
                if !is_blocking {
                    return None;
                }
            },
            IpcRecv::Msg(ServerToClientMsg::LogError { lines: log_lines }, _) => {
                log_lines.iter().for_each(|line| eprintln!("{line}"));
                return Some(2);
            },
            IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => match exit_reason {
                ExitReason::Error(e) => {
                    eprintln!("{}", e);
                    return Some(2);
                },
                ExitReason::CustomExitStatus(exit_status) => {
                    return Some(exit_status);
                },
                _ => {
                    return None;
                },
            },
            IpcRecv::Eof => {
                eprintln!("Server disconnected before completing the action");
                return Some(2);
            },
            _ => {},
        }
    }
}

fn write_log_lines(stdout: &mut impl Write, lines: &[String]) -> io::Result<()> {
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    stdout.flush()
}

fn output_error_status(error: io::Error) -> i32 {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return 0;
    }
    eprintln!("Failed to write output: {error}");
    2
}

pub fn start_subscribe_client(
    os_input: Box<dyn CliClientOs>,
    session_name: &SessionName,
    subscribe_cli: SubscribeCli,
) {
    let yinz_ipc_pipe = crate::ipc_pipe_for_session(session_name, None);
    if let Err(e) = os_input.connect_to_server(&yinz_ipc_pipe) {
        eprintln!("{e}");
        std::process::exit(1);
    }

    // Parse pane IDs
    let pane_ids: Vec<PaneId> = subscribe_cli
        .pane_id
        .iter()
        .map(|s| {
            PaneId::from_str(s).unwrap_or_else(|e| {
                eprintln!("Invalid pane ID '{}': {}", s, e);
                process::exit(2);
            })
        })
        .collect();

    // Send subscribe message
    os_input.send_to_server(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: pane_ids.clone(),
        scrollback: subscribe_cli.scrollback,
        ansi: subscribe_cli.ansi,
    });

    // Track remaining panes for exit-on-all-closed
    let mut remaining_panes: HashSet<PaneId> = pane_ids.into_iter().collect();

    // Streaming receive loop
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    loop {
        match os_input.recv_from_server() {
            IpcRecv::Msg(
                ServerToClientMsg::PaneRenderUpdate {
                    pane_id,
                    viewport,
                    scrollback,
                    is_initial,
                },
                _,
            ) => match subscribe_cli.format {
                SubscribeFormat::Raw => {
                    if let Some(ref scrollback_lines) = scrollback {
                        for line in scrollback_lines {
                            let _ = writeln!(stdout, "{}", line);
                        }
                    }
                    for line in &viewport {
                        let _ = writeln!(stdout, "{}", line);
                    }
                    let _ = stdout.flush();
                },
                SubscribeFormat::Json => {
                    let json = serde_json::json!({
                        "event": "pane_update",
                        "pane_id": pane_id.to_string(),
                        "viewport": viewport,
                        "scrollback": scrollback,
                        "is_initial": is_initial,
                    });
                    let _ = writeln!(stdout, "{}", json);
                    let _ = stdout.flush();
                },
            },
            IpcRecv::Msg(ServerToClientMsg::SubscribedPaneClosed { pane_id }, _) => {
                remaining_panes.remove(&pane_id);
                match subscribe_cli.format {
                    SubscribeFormat::Raw => {},
                    SubscribeFormat::Json => {
                        let json = serde_json::json!({
                            "event": "pane_closed",
                            "pane_id": pane_id.to_string(),
                        });
                        let _ = writeln!(stdout, "{}", json);
                        let _ = stdout.flush();
                    },
                }
                if remaining_panes.is_empty() {
                    break;
                }
            },
            IpcRecv::Msg(ServerToClientMsg::Exit { .. }, _) | IpcRecv::Eof => break,
            IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                for line in lines {
                    eprintln!("{}", line);
                }
                process::exit(2);
            },
            _ => {},
        }
    }

    os_input.send_to_server(ClientToServerMsg::ClientExited);
}

/// `yinz events`: every state event the session reports to plugins, one JSON object per line
/// on stdout, until the session ends or stdout goes away.
pub fn start_events_client(
    os_input: Box<dyn CliClientOs>,
    session_name: &SessionName,
    events_cli: EventsCli,
) {
    let yinz_ipc_pipe = crate::ipc_pipe_for_session(session_name, None);

    // Refused here, before connecting, so a typo costs nothing but the message.
    let event_types: Vec<String> = events_cli
        .event_type
        .iter()
        .map(|name| match EventType::from_cli_name(name) {
            _ if matches!(name.as_str(), "agent_lifecycle" | "AgentLifecycle") => "agent_lifecycle".into(),
            Some(event_type) => event_type.to_string(),
            None => {
                eprintln!(
                    "Unknown or unstreamable event type '{}'. Streamable events: {}, agent_lifecycle",
                    name,
                    EventType::cli_streamable_names().join(", ")
                );
                process::exit(2);
            },
        })
        .collect();

    if let Err(e) = os_input.connect_to_server(&yinz_ipc_pipe) {
        eprintln!("{e}");
        process::exit(1);
    }
    os_input.send_to_server(ClientToServerMsg::SubscribeToEvents {
        event_types,
        options: yinz_ipc::ipc::EventSubscription {
            panes: events_cli
                .filters
                .pane
                .iter()
                .map(ToString::to_string)
                .collect(),
            projects: events_cli.filters.project,
            kinds: events_cli.filters.kind,
            lifecycle_types: events_cli.filters.lifecycle_type,
            cursor: events_cli.filters.cursor,
        },
    });

    // Locked per line rather than for the loop: a lock held while blocking on the server would
    // stop anything else in the process printing, which is one client too many in a test.
    loop {
        match os_input.recv_from_server() {
            IpcRecv::Msg(
                ServerToClientMsg::EventUpdate {
                    event_type,
                    event_json,
                },
                _,
            ) => {
                let mut stdout = os_input.get_stdout_writer();
                // The payload is already JSON, so the line is assembled around it rather than
                // parsed and re-serialized. A write that fails is a reader that went away
                // (`yinz events | head`), which is how this stream ends.
                if events_cli.text {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event_json)
                        && writeln!(
                            stdout,
                            "{} {} {}",
                            payload["pane_id"].as_str().unwrap_or("session"),
                            payload["event_type"].as_str().unwrap_or(&event_type),
                            payload["state"].as_str().unwrap_or("")
                        )
                        .and_then(|_| stdout.flush())
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                let event_type = serde_json::to_string(&event_type).unwrap_or_default();
                if writeln!(
                    stdout,
                    "{{\"event\":{event_type},\"payload\":{event_json}}}"
                )
                .and_then(|_| stdout.flush())
                .is_err()
                {
                    break;
                }
            },
            IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                for line in lines {
                    eprintln!("{}", line);
                }
                process::exit(2);
            },
            IpcRecv::Msg(ServerToClientMsg::Exit { .. }, _) | IpcRecv::Eof => break,
            _ => {},
        }
    }
    os_input.send_to_server(ClientToServerMsg::ClientExited);
}
