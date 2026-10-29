use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

use crate::thread_bus::ThreadSenders;
use crate::{
    os_input_output::ServerOsApi, panes::PaneId, plugins::PluginInstruction,
    screen::ScreenInstruction, ServerInstruction, SessionMetaData, SessionState,
};
use std::thread;
use std::time::{Duration, Instant};
use yinz_runtime::{channels::SenderWithContext, errors::prelude::*};
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use {
    yinz_ipc::ipc::ClientToServerMsg,
    yinz_ipc::ipc::ExitReason,
    yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcRecv,
    yinz_ipc::ipc::ServerToClientMsg,
    yinz_types::data::{Event, InputMode},
    yinz_types::input::actions::Action,
};

use crate::route::{
    actions_dismiss_context_menu, actions_leave_clipboard_message, route_action,
    wait_for_action_completion, NotificationEnd,
};
use crate::ClientId;

/// How many unreadable messages in a row we tolerate from one client before
/// concluding the stream has desynchronised. Malformed frames only — a hangup
/// is terminal and handled separately, so it never reaches this counter.
const MAX_CONSECUTIVE_MALFORMED_MESSAGES: usize = 1000;

/// Between malformed frames, so a stream that produces them without ever
/// blocking cannot spin a core on the way to giving up.
const MALFORMED_MESSAGE_BACKOFF: Duration = Duration::from_millis(1);

const SESSION_READY_WAIT: Duration = Duration::from_secs(10);
const SESSION_READY_POLL_INTERVAL: Duration = Duration::from_millis(5);

fn wait_for_session_to_be_ready(session_data: &Arc<RwLock<Option<SessionMetaData>>>) -> bool {
    let deadline = Instant::now() + SESSION_READY_WAIT;
    loop {
        let ready = session_data
            .read()
            .map(|session_data| session_data.is_some())
            .unwrap_or(false);
        if ready {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(SESSION_READY_POLL_INTERVAL);
    }
}

macro_rules! send_to_screen_or_retry_queue {
    ($senders:expr_2021, $message:expr_2021, $instruction: expr_2021, $retry_queue:expr_2021) => {{
        match $senders.as_ref() {
            Some(senders) => senders.send_to_screen($message),
            None => {
                log::warn!("Server not ready, trying to place instruction in retry queue...");
                if let Some(retry_queue) = $retry_queue.as_mut() {
                    retry_queue.push_back($instruction);
                }
                Ok(())
            },
        }
    }};
}

fn subscribe_to_events(
    senders: Option<&ThreadSenders>,
    os_input: &dyn ServerOsApi,
    client_id: ClientId,
    instruction: ClientToServerMsg,
    retry_queue: Option<&mut VecDeque<ClientToServerMsg>>,
) -> Result<()> {
    let ClientToServerMsg::SubscribeToEvents {
        ref event_types,
        ref options,
    } = instruction
    else {
        unreachable!("event subscription handler");
    };
    let lifecycle = event_types.iter().any(|t| t == "agent_lifecycle");
    if !lifecycle && options != &yinz_ipc::ipc::EventSubscription::default() {
        os_input
            .send_to_client(
                client_id,
                ServerToClientMsg::LogError {
                    lines: vec!["lifecycle filters require --event-type agent_lifecycle".into()],
                },
            )
            .context("reject event filters")?;
        return Ok(());
    }
    let Some(senders) = senders else {
        if let Some(retry_queue) = retry_queue {
            retry_queue.push_back(instruction);
        }
        return Ok(());
    };
    let event_types: Vec<_> = event_types
        .iter()
        .filter(|t| *t != "agent_lifecycle")
        .cloned()
        .collect();
    if lifecycle {
        senders
            .send_to_screen(ScreenInstruction::SubscribeAgentEvents(
                client_id,
                options.clone(),
            ))
            .context("subscribe to agent events")?;
    }
    if lifecycle && event_types.is_empty() {
        return Ok(());
    }
    senders
        .send_to_plugin(PluginInstruction::SubscribeCliClientToEvents {
            client_id,
            event_types,
        })
        .context("subscribe to CLI events")?;
    Ok(())
}

fn enqueue_agent_report(
    senders: Option<&ThreadSenders>,
    report: yinz_types::agent::report::AgentReport,
    accepted_at: std::time::Instant,
) -> Result<()> {
    yinz_runtime::agent::report::validate_report(&report)?;
    // Acknowledgement requires a successful enqueue, even during shutdown or when
    // test senders enable `should_silently_fail`.
    senders
        .context("no session for report")?
        .to_screen
        .as_ref()
        .context("no screen queue for report")?
        .send_required(ScreenInstruction::AgentReport(report, accepted_at))
        .map_err(|_| anyhow!("screen report queue closed"))
}

fn accept_agent_report(
    senders: Option<&ThreadSenders>,
    report: yinz_types::agent::report::AgentReport,
    reply: impl FnOnce(ServerToClientMsg) -> Result<()>,
) -> Result<()> {
    let accepted_at = std::time::Instant::now();
    let trace = yinz_runtime::agent::report::report_diagnostics(&report);
    if trace.enabled {
        trace.record("acceptance", Duration::ZERO, "enter");
    }
    let report_id = report.report_id.clone();
    let result = enqueue_agent_report(senders, report, accepted_at);
    let message = if result.is_ok() {
        ServerToClientMsg::AgentReportAccepted { report_id }
    } else {
        ServerToClientMsg::LogError {
            lines: vec!["report queue unavailable".into()],
        }
    };
    let reply_result = reply(message).context("reply to agent report");
    trace.record(
        "acceptance",
        accepted_at.elapsed(),
        if result.is_ok() && reply_result.is_ok() {
            "ok"
        } else {
            "error"
        },
    );
    reply_result
}

/// How long the same chord-inferred modifier set is allowed to stand before it is restated.
///
/// A chord says only "these were down when this was struck", so a consumer has to time the hold
/// out itself and needs the fact restated to keep it alive - but restating it once per keystroke
/// puts a plugin `Update` on the bus for every `Ctrl` chord in a burst of editing, which is
/// exactly the queue pressure `docs/PLUGIN_RENDER_PIPELINE.md` blames for plugin input lag. This
/// has to stay comfortably under the shortest consumer window, which is the status bar's
/// `ALT_HINT_WINDOW_MS` (3s).
const INFERRED_MODIFIERS_REPEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Whether this chord's modifiers are worth telling the plugins about, recording the report in
/// `last` when they are.
///
/// A change is always news. An unchanged set is news again only once the interval has run out -
/// and a suppressed chord deliberately does not move the recorded instant, so the interval is
/// measured from the last report actually sent rather than sliding forward with every keystroke.
fn should_report_inferred_modifiers(
    last: &mut Option<(BTreeSet<KeyModifier>, Instant)>,
    modifiers: &BTreeSet<KeyModifier>,
    now: Instant,
) -> bool {
    let is_news = match last {
        Some((previous, sent_at)) => {
            previous != modifiers
                || now.duration_since(*sent_at) >= INFERRED_MODIFIERS_REPEAT_INTERVAL
        },
        None => true,
    };
    if is_news {
        *last = Some((modifiers.clone(), now));
    }
    is_news
}

fn legacy_lf_key(session: &SessionMetaData, client_id: ClientId) -> KeyWithModifier {
    let ctrl_j = KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier();
    let mode = session
        .current_input_modes
        .get(&client_id)
        .copied()
        .unwrap_or_else(|| {
            session
                .session_configuration
                .get_client_default_input_mode(&client_id)
        });
    let bound = session
        .session_configuration
        .get_client_keybinds(&client_id)
        .get_actions_for_key_in_mode(&mode, &ctrl_j)
        .is_some_and(|actions| !actions.is_empty());
    if bound {
        ctrl_j
    } else {
        KeyWithModifier::new(BareKey::Enter)
    }
}

fn resolve_client_paste(
    action: Action,
    is_cli_client: bool,
    session: &SessionMetaData,
    client_id: ClientId,
) -> Vec<Action> {
    let Action::Paste {
        chars,
        pane_id: None,
    } = action
    else {
        return vec![action];
    };
    if is_cli_client {
        return vec![Action::Paste {
            chars,
            pane_id: None,
        }];
    }
    let mode = if session.key_passthrough_clients.contains_key(&client_id) {
        InputMode::Normal
    } else {
        session
            .current_input_modes
            .get(&client_id)
            .copied()
            .unwrap_or_else(|| {
                session
                    .session_configuration
                    .get_client_default_input_mode(&client_id)
            })
    };
    let input = chars.into_bytes();
    match mode {
        InputMode::Normal | InputMode::Locked => {
            // Keep the markers separate: panes drop them when bracketed paste is disabled,
            // and plugin panes use them to collect one paste event. Write also preserves
            // synchronized input and the client-input completion barrier.
            [b"\x1b[200~".to_vec(), input, b"\x1b[201~".to_vec()]
                .into_iter()
                .map(|bytes| Action::Write {
                    key_with_modifier: None,
                    bytes,
                    is_kitty_keyboard_protocol: false,
                })
                .collect()
        },
        InputMode::EnterSearch => vec![Action::SearchInput { input }],
        InputMode::RenameTab => vec![Action::TabNameInput { input }],
        InputMode::RenamePane => vec![Action::PaneNameInput { input }],
        _ => vec![],
    }
}

pub(crate) fn route_thread_main(
    session_data: Arc<RwLock<Option<SessionMetaData>>>,
    session_state: Arc<RwLock<SessionState>>,
    os_input: Box<dyn ServerOsApi>,
    to_server: SenderWithContext<ServerInstruction>,
    mut receiver: IpcReceiverWithContext<ClientToServerMsg>,
    client_id: ClientId,
    first_message_timeout: Duration,
) -> Result<()> {
    let mut retry_queue = VecDeque::new();
    // The last chord-inferred modifier set reported to the plugins for this connection, and when.
    // Per connection rather than per message, which is what lets the rate limit hold across a
    // burst of chords - see `should_report_inferred_modifiers`.
    let mut last_inferred_modifiers: Option<(BTreeSet<KeyModifier>, Instant)> = None;
    let err_context = || format!("failed to handle instruction for client {client_id}");
    let mut seen_cli_pipes = HashSet::new();
    let mut consecutive_malformed_messages_received = 0;
    // Until the peer has said something the read is bounded, so that a
    // connection which is never going to speak does not hold its thread and
    // its connection slot for the life of the server. See
    // `FIRST_CLIENT_MESSAGE_TIMEOUT` for why only the first read is bounded.
    let mut received_first_message = false;
    receiver
        .set_recv_timeout(Some(first_message_timeout))
        .with_context(|| format!("failed to bound the first read from client {client_id}"))
        .non_fatal();
    'route_loop: loop {
        match receiver.recv_client_msg() {
            IpcRecv::Msg(instruction, err_ctx) => {
                consecutive_malformed_messages_received = 0;
                yinz_runtime::errors::update_thread_ctx(&err_ctx);
                if !received_first_message {
                    received_first_message = true;
                    receiver
                        .set_recv_timeout(None)
                        .with_context(|| {
                            format!("failed to lift the read deadline for client {client_id}")
                        })
                        .non_fatal();
                }

                let mut handle_instruction = |instruction: ClientToServerMsg,
                                              mut retry_queue: Option<
                    &mut VecDeque<ClientToServerMsg>,
                >|
                 -> Result<bool> {
                    let mut should_break = false;
                    let senders = session_data
                        .read()
                        .to_anyhow()
                        .with_context(err_context)
                        .to_log()
                        .ok()
                        .and_then(|r| r.as_ref().map(|r| r.senders.clone()));

                    if let ClientToServerMsg::AgentReport(report) = instruction {
                        accept_agent_report(senders.as_ref(), report, |reply| {
                            os_input.send_to_client(client_id, reply)
                        })
                        .non_fatal();
                        // The route-loop cleanup removes this connection exactly once.
                        return Ok(true);
                    }

                    // Check if this is a watcher client and ignore input messages
                    let is_watcher = session_state.read().unwrap().is_watcher(&client_id);
                    if is_watcher {
                        let should_exit = match &instruction {
                            ClientToServerMsg::Key { key, .. } => {
                                (key.bare_key == BareKey::Char('q')
                                    && key.key_modifiers.contains(&KeyModifier::Ctrl))
                                    || key.bare_key == BareKey::Esc
                                    || (key.bare_key == BareKey::Char('c')
                                        && key.key_modifiers.contains(&KeyModifier::Ctrl))
                            },
                            ClientToServerMsg::Action { action, .. } => {
                                matches!(action, Action::Quit | Action::Detach)
                            },
                            _ => false,
                        };
                        if should_exit {
                            os_input
                                .send_to_client(
                                    client_id,
                                    ServerToClientMsg::Exit {
                                        exit_reason: ExitReason::Normal,
                                    },
                                )
                                .with_context(err_context)
                                .non_fatal();
                            senders
                                .as_ref()
                                .map(|s| {
                                    s.send_to_screen(ScreenInstruction::RemoveWatcherClient(
                                        client_id,
                                    ))
                                })
                                .transpose()
                                .with_context(err_context)
                                .non_fatal();
                            return Ok(true);
                        }
                        match &instruction {
                            ClientToServerMsg::TerminalResize { new_size } => {
                                // For watchers: send size to Screen for rendering adjustments, but
                                // this does not affect the screen size
                                send_to_screen_or_retry_queue!(
                                    senders,
                                    ScreenInstruction::WatcherTerminalResize(client_id, *new_size),
                                    instruction.clone(),
                                    retry_queue
                                )
                                .with_context(err_context)?;
                            },
                            _ => {
                                // Ignore all input from watcher clients
                            },
                        }
                        // don't do anything else for watchers
                        return Ok(false);
                    }

                    if matches!(&instruction, ClientToServerMsg::Key { .. }) {
                        session_state
                            .write()
                            .to_anyhow()
                            .context("tracking active client")?
                            .set_last_active_client(client_id);
                    }
                    if let ClientToServerMsg::Key { key, .. } = &instruction
                        && (key.bare_key != yinz_types::keys::BareKey::Esc
                            || !key.has_no_modifiers())
                        && let Some(senders) = &senders
                    {
                        senders
                            .send_to_screen(ScreenInstruction::ResetAgentEscape(client_id))
                            .non_fatal();
                    }
                    let Some(instruction) = crate::locked_escape::prepare_input(
                        instruction,
                        &session_data,
                        &to_server,
                        client_id,
                    )?
                    else {
                        return Ok(false);
                    };
                    match instruction {
                        ClientToServerMsg::SessionReply(reply) => {
                            to_server.send(ServerInstruction::SessionReply(client_id, reply)).context("route client session reply")?;
                        },
                        ClientToServerMsg::AgentControl(request) => {
                            use yinz_types::agent::control::{AgentOutcome, AgentResponse};
                            if matches!(&request, yinz_types::agent::control::AgentRequest::Spawn { .. } | yinz_types::agent::control::AgentRequest::SpawnNewProject { .. } | yinz_types::agent::control::AgentRequest::List { project: Some(_) } | yinz_types::agent::control::AgentRequest::Usage { .. }) {
                            let assets = session_data.read().to_anyhow().context("read session for agent control")?.as_ref().map(|session| {
                                (session.senders.clone(), session.data_access.clone())
                            });
                            let response = if let Some((senders, data_access)) = assets {
                                let active_client = session_state.read().to_anyhow().context("read active client for agent control")?.get_last_active_client().unwrap_or(client_id);
                                yinz_plugin_host::control_agent(request, active_client, &senders, &data_access)
                            } else {
                                AgentResponse::new(AgentOutcome::StatusUnavailable)
                            };
                            os_input.send_to_client(client_id, ServerToClientMsg::AgentControl(response)).context("reply to agent control")?;
                            } else if let Some(senders) = &senders {
                                senders.send_to_screen(ScreenInstruction::AgentControl(request, client_id)).context("route agent control to screen")?;
                            } else {
                                os_input.send_to_client(client_id, ServerToClientMsg::AgentControl(AgentResponse::new(AgentOutcome::StatusUnavailable))).context("reply to unavailable agent control")?;
                            }
                        },
                        ClientToServerMsg::PluginRequest { request_id, request } => {
                            let result = if let Some(senders) = &senders {
                                let (reply, receiver) = std::sync::mpsc::channel();
                                senders.send_to_plugin(PluginInstruction::PluginRequest { request, client_id, reply }).context("route CLI plugin request")?;
                                receiver.recv_timeout(std::time::Duration::from_secs(30))
                                    .unwrap_or_else(|_| Err("Plugin request outcome unknown; read before retrying a mutation".into()))
                            } else {
                                Err("Session is not ready for plugin requests".into())
                            };
                            os_input.send_to_client(client_id, ServerToClientMsg::PluginResponse { request_id, result }).context("reply to CLI plugin request")?;
                        },
                        ClientToServerMsg::DataRequest { request_id, request } => {
                            let result = if let Some(senders) = &senders {
                                let (reply, receiver) = std::sync::mpsc::channel();
                                senders.send_to_plugin(PluginInstruction::DataRequest { request, reply }).context("route CLI data request")?;
                                receiver.recv_timeout(std::time::Duration::from_secs(30))
                                    .unwrap_or_else(|_| Err("Data request outcome unknown; read before retrying a mutation".into()))
                            } else {
                                Err("Session is not ready for data requests".into())
                            };
                            os_input.send_to_client(client_id, ServerToClientMsg::DataResponse { request_id, result }).context("reply to CLI data request")?;
                        },
                        ClientToServerMsg::AgentReport(_) => unreachable!("handled before action dispatch"),
                        ClientToServerMsg::Paste(text) => {
                            if let Some(senders) = &senders {
                                senders.send_to_screen(ScreenInstruction::ResetAgentEscape(client_id))
                                    .with_context(err_context)?;
                                senders.send_to_plugin(PluginInstruction::UserInput {
                                    client_id,
                                    action: Action::Paste { chars: text.clone(), pane_id: None },
                                    terminal_id: None,
                                    cli_client_id: None,
                                }).with_context(err_context)?;
                                senders.send_to_plugin(PluginInstruction::Update(vec![(
                                    None, Some(client_id), Event::InputReceived,
                                )])).with_context(err_context)?;
                                let (completion_tx, completion_rx) = oneshot::channel();
                                senders.send_to_screen(ScreenInstruction::PasteFromClient(
                                    text, client_id, Some(NotificationEnd::new_client_input(completion_tx)),
                                )).with_context(err_context)?;
                                wait_for_action_completion(completion_rx, "PasteFromClient", false);
                            }
                        },
                        ClientToServerMsg::Key {
                            key,
                            raw_bytes,
                            is_kitty_keyboard_protocol,
                        } => {
                            // Legacy LF is Enter unless Ctrl-J is bound in the current mode.
                            // Resolve its identity before menus, modifiers and passthrough see it.
                            let key = if raw_bytes == [10] && !is_kitty_keyboard_protocol {
                                session_data.read().unwrap().as_ref()
                                    .map(|session| legacy_lf_key(session, client_id))
                                    .unwrap_or(key)
                            } else {
                                key
                            };
                            // Keep the original key for overlays when bindings replace it.
                            let key_for_overlay_check = key.clone();

                            // A chord is the only evidence of a held modifier most terminals
                            // ever give us: `Alt+n` says Alt was down when it was pressed, and
                            // nothing will say when it came back up. `is_live: false` is that
                            // caveat, and it is why the receiving plugin has to time the hold
                            // out itself. The precise signal, where the terminal reports the
                            // modifier key itself, arrives as its own message with `true`.
                            //
                            // Rate limited, because a broadcast is not free: it crosses the bus
                            // and takes a lock per plugin instance before anything is filtered,
                            // and an unchanged set restated on every keystroke buys a consumer
                            // nothing. See `should_report_inferred_modifiers`.
                            if !key.key_modifiers.is_empty()
                                && should_report_inferred_modifiers(
                                    &mut last_inferred_modifiers,
                                    &key.key_modifiers,
                                    Instant::now(),
                                )
                            {
                                senders.as_ref().map(|senders| {
                                    senders.send_to_plugin(PluginInstruction::Update(vec![(
                                        None,
                                        Some(client_id),
                                        Event::ModifiersHeld {
                                            modifiers: key.key_modifiers.clone(),
                                            is_live: false,
                                        },
                                    )]))
                                }).transpose().with_context(err_context).non_fatal();
                            }

                            // The read guard ends as a temporary in this expression so
                            // `route_action` runs without holding `session_data.read()` —
                            // see the doc comment on `route_action` for why this matters.
                            let dispatch_inputs =
                                session_data.read().unwrap().as_ref().and_then(|s| {
                                    let in_passthrough =
                                        s.key_passthrough_clients.contains_key(&client_id);
                                    if in_passthrough {
                                        return Some((
                                            s.senders.clone(),
                                            s.default_shell.clone(),
                                            s.session_configuration
                                                .get_client_default_input_mode(&client_id),
                                            vec![Action::Write {
                                                key_with_modifier: Some(key),
                                                bytes: raw_bytes,
                                                is_kitty_keyboard_protocol,
                                            }],
                                        ));
                                    }
                                    let (kb, im, dim) =
                                        s.get_client_keybinds_and_mode(&client_id)?;
                                    let actions: Vec<Action> = kb
                                        .get_actions_for_key_in_mode_or_default_action(
                                            im,
                                            &key,
                                            raw_bytes,
                                            dim,
                                            is_kitty_keyboard_protocol,
                                        );
                                    Some((
                                        s.senders.clone(),
                                        s.default_shell.clone(),
                                        s.session_configuration
                                            .get_client_default_input_mode(&client_id),
                                        actions,
                                    ))
                                });
                            if let Some((senders, default_shell, client_input_mode, actions)) =
                                dispatch_inputs
                            {
                                // Only a single write retaining the original key can go straight
                                // to WriteCharacter. Other bindings may replace the key or run
                                // more actions after an overlay consumes it, so ask first.
                                let mut overlay_handled_key = false;
                                if !matches!(actions.as_slice(), [Action::Write {
                                    key_with_modifier: Some(write_key), ..
                                }] if write_key == &key_for_overlay_check) {
                                    senders.send_to_screen(ScreenInstruction::ResetAgentEscape(client_id)).non_fatal();
                                    let (completion_tx, completion_rx) = oneshot::channel();
                                    let sent = senders.send_to_screen(
                                        ScreenInstruction::ContextMenuHandleKeyIfOpen {
                                            key: Some(key_for_overlay_check),
                                            toggle_command_palette: actions.contains(&Action::ToggleCommandPalette),
                                            open_agents: actions.contains(&Action::OpenAgents),
                                            client_id,
                                            completion: Some(NotificationEnd::new(completion_tx)),
                                        },
                                    );
                                    if sent.is_ok() {
                                        overlay_handled_key = wait_for_action_completion(
                                            completion_rx,
                                            "ContextMenuHandleKeyIfOpen",
                                            false,
                                        )
                                        .stdout_message
                                        .as_deref()
                                            == Some("true");
                                    }
                                    sent.with_context(err_context).non_fatal();
                                    if !overlay_handled_key && actions_dismiss_context_menu(&actions) {
                                        senders.send_to_screen(
                                            ScreenInstruction::CloseContextMenu(client_id),
                                        ).with_context(err_context).non_fatal();
                                    }
                                }
                                if !overlay_handled_key {
                                    let leaves_clipboard_message =
                                        actions_leave_clipboard_message(&actions);
                                    for action in actions {
                                        // Send user input to plugin thread for logging
                                        senders.send_to_plugin(PluginInstruction::UserInput {
                                                client_id,
                                                action: action.clone(),
                                                terminal_id: None,
                                                cli_client_id: None,
                                            }).with_context(err_context).non_fatal();

                                        match route_action(
                                            action,
                                            client_id,
                                            None,
                                            None,
                                            senders.clone(),
                                            default_shell.clone(),
                                            Some(&mut seen_cli_pipes),
                                            client_input_mode,
                                            Some(os_input.clone()),
                                            leaves_clipboard_message,
                                            true, // is_client_input
                                        ) {
                                            Ok(route_action_should_break) => {
                                                if route_action_should_break.0 {
                                                    should_break = true;
                                                }
                                            },
                                            Err(e) => {
                                                log::error!("{}", e);
                                            },
                                        }
                                    }
                                }
                            }
                        },
                        ClientToServerMsg::Action {
                            action,
                            terminal_id: maybe_pane_id,
                            client_id: maybe_client_id,
                            is_cli_client,
                        } => {
                            let cli_client_id = client_id;
                            let client_id = if is_cli_client {
                                // A CLI connection has no focus, mode or tab of its own: actions
                                // such as move focus, switch mode or quit act for one interactive client.
                                session_state
                                    .read()
                                    .unwrap()
                                    .cli_action_target(client_id, maybe_client_id)
                            } else {
                                maybe_client_id.unwrap_or(client_id)
                            };

                            // A CLI copy is one action rather than a key's batch, so its own
                            // `InputReceived` would precede the copy rather than clear it - but
                            // the rule is about the request that copies, not about how it arrived.
                            let action_leaves_clipboard_message = action.leaves_clipboard_message();

                            // The read guard ends as a temporary in this expression so
                            // `route_action` runs without holding `session_data.read()` —
                            // see the doc comment on `route_action` for why this matters.
                            let session_data_assets =
                                session_data.read().unwrap().as_ref().map(|s| {
                                    (
                                        s.senders.clone(),
                                        s.default_shell.clone(),
                                        s.session_configuration
                                            .get_client_default_input_mode(&client_id),
                                        resolve_client_paste(action, is_cli_client, s, client_id),
                                    )
                                });
                            if let Some((senders, default_shell, client_input_mode, actions)) =
                                session_data_assets
                            {
                                for action in actions {
                                    senders.send_to_plugin(PluginInstruction::UserInput {
                                        client_id,
                                        action: action.clone(),
                                        terminal_id: maybe_pane_id,
                                        cli_client_id: is_cli_client.then_some(cli_client_id),
                                    }).with_context(err_context).non_fatal();
                                    match route_action(
                                        action,
                                        client_id,
                                        Some(cli_client_id),
                                        maybe_pane_id.map(PaneId::Terminal),
                                        senders.clone(),
                                        default_shell.clone(),
                                        Some(&mut seen_cli_pipes),
                                        client_input_mode,
                                        Some(os_input.clone()),
                                        action_leaves_clipboard_message,
                                        true, // is_client_input
                                    ) {
                                        Ok(route_action_should_break) => {
                                            if route_action_should_break.0 {
                                                should_break = true;
                                            }
                                        },
                                        Err(e) => {
                                            log::error!("{}", e);
                                        },
                                    }
                                }
                            }
                        },
                        ClientToServerMsg::TerminalResize { new_size } => {
                            // Check if this is a watcher or regular client
                            if is_watcher {
                                // For watchers: send size to Screen for tracking, don't affect screen size
                                send_to_screen_or_retry_queue!(
                                    senders.clone(),
                                    ScreenInstruction::WatcherTerminalResize(client_id, new_size),
                                    instruction,
                                    retry_queue
                                )
                                .with_context(err_context)?;
                            } else {
                                // Per-tab sizing: Screen owns the client
                                // sizes. A resize arriving before AddClient
                                // is recorded but does not reach the layout,
                                // which only counts connected clients, so no
                                // session-level gating is needed.
                                senders.as_ref().map(|s| {
                                    s.send_to_screen(ScreenInstruction::RecomputeTabSize(
                                        client_id, new_size,
                                    ))
                                }).transpose().with_context(err_context).non_fatal();
                            }
                        },
                        ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } => {
                            to_server
                                .send(ServerInstruction::TerminalPixelDimensions {
                                    client_id,
                                    pixel_dimensions,
                                })
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::KittyGraphicsSupport { supported } => {
                            // A fast probe must not overtake FirstClientConnected/AttachClient.
                            to_server
                                .send(ServerInstruction::KittyGraphicsSupport {
                                    client_id,
                                    supported,
                                })
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::SynchronizedOutputSupport { supported } => {
                            // Through the server's main loop rather than straight to Screen, so
                            // that it keeps its place in the queue relative to this client's own
                            // `AttachClient` and `RemoveClient` - see the instruction's own note.
                            to_server
                                .send(ServerInstruction::SynchronizedOutputSupport {
                                    client_id,
                                    supported,
                                })
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::SixelSupport { supported } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::SetSixelSupport {
                                    client_id,
                                    supported
                                },
                                instruction,
                                retry_queue
                            )
                            .with_context(err_context)?;
                        },
                        ClientToServerMsg::BackgroundColor {
                            color: ref background_color_instruction,
                        } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::TerminalBackgroundColor(
                                    background_color_instruction.clone()
                                ),
                                instruction,
                                retry_queue
                            )
                            .with_context(err_context)?;
                        },
                        ClientToServerMsg::ForegroundColor {
                            color: ref foreground_color_instruction,
                        } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::TerminalForegroundColor(
                                    foreground_color_instruction.clone()
                                ),
                                instruction,
                                retry_queue
                            )
                            .with_context(err_context)?;
                        },
                        ClientToServerMsg::ColorRegisters {
                            ref color_registers,
                        } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::TerminalColorRegisters(
                                    color_registers
                                        .iter()
                                        .map(|c| (c.index, c.color.clone()))
                                        .collect()
                                ),
                                instruction,
                                retry_queue
                            )
                            .with_context(err_context)?;
                        },
                        ClientToServerMsg::FirstClientConnected { cli_assets } => {
                            let new_client_instruction = ServerInstruction::FirstClientConnected(
                                cli_assets,
                                client_id,
                            );
                            to_server
                                .send(new_client_instruction)
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::AttachClient {
                            cli_assets,
                            tab_number_to_focus,
                            pane_to_focus: pane_id_to_focus,
                        } => {
                            let attach_client_instruction = ServerInstruction::AttachClient(
                                cli_assets,
                                tab_number_to_focus,
                                pane_id_to_focus,
                                client_id,
                            );
                            to_server
                                .send(attach_client_instruction)
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::AttachWatcherClient { terminal_size } => {
                            let attach_watcher_instruction =
                                ServerInstruction::AttachWatcherClient(client_id, terminal_size);
                            to_server
                                .send(attach_watcher_instruction)
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::ClientExited => return Ok(true),
                        ClientToServerMsg::KillSessionWithCompletion { report_progress } => {
                            to_server.send(ServerInstruction::KillSessionWithCompletion { client_id, report_progress })
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::KillSession => {
                            to_server
                                .send(ServerInstruction::KillSession)
                                .with_context(err_context)?;
                        },
                        ClientToServerMsg::ConnStatus => {
                            // A liveness probe. Deliberately answered by the main
                            // loop and not here: the probe carries a receive
                            // deadline (`PROBE_TIMEOUT` in `sessions.rs`), so a
                            // main loop that has stopped turning shows up as
                            // unresponsive. Answering from this thread would only
                            // prove that the accept path works, and a wedged
                            // server would pass as alive.
                            let started_at = std::time::Instant::now();
                            log::info!(
                                "attach trace: server router client_id={client_id} received ConnStatus; main-loop enqueue begin"
                            );
                            let result = to_server.send(ServerInstruction::ConnStatus(client_id));
                            log::info!(
                                "attach trace: server router client_id={client_id} ConnStatus enqueue finished after {:?} ({})",
                                started_at.elapsed(),
                                if result.is_ok() { "ok" } else { "error" }
                            );
                            result.with_context(err_context).non_fatal();
                            should_break = true;
                        },
                        ClientToServerMsg::DetachSession { client_ids } => {
                            to_server.send(ServerInstruction::DetachSession(client_ids, None)).with_context(err_context).non_fatal();
                            should_break = true;
                        },
                        ClientToServerMsg::DesktopNotificationResponse { ref raw_bytes } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::DesktopNotificationResponse(
                                    raw_bytes.clone(),
                                    client_id,
                                ),
                                instruction,
                                retry_queue
                            ).with_context(err_context).non_fatal();
                        },
                        ClientToServerMsg::ForwardedReplyFromHost {
                            token,
                            ref reply_bytes,
                        } => {
                            // The client that owns this forward
                            // answered — drop the in-flight entry
                            // first so a later disconnect of that
                            // client can't synthesize a spurious
                            // empty reply for the same token.
                            session_state
                                .write()
                                .unwrap()
                                .clear_forward_in_flight(token);
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::ForwardedReplyFromHost {
                                    token,
                                    reply_bytes: reply_bytes.clone(),
                                },
                                instruction,
                                retry_queue
                            ).with_context(err_context).non_fatal();
                        },
                        ClientToServerMsg::HostTerminalThemeChanged { mode } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::HostTerminalThemeChanged(mode),
                                instruction,
                                retry_queue
                            ).with_context(err_context).non_fatal();
                        },
                        ClientToServerMsg::SubscribeToPaneRenders {
                            ref pane_ids,
                            ref scrollback,
                            ansi,
                        } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::SubscribeToPaneRenders {
                                    client_id,
                                    pane_ids: pane_ids.clone(),
                                    scrollback: *scrollback,
                                    ansi,
                                },
                                instruction,
                                retry_queue
                            ).with_context(err_context).non_fatal();
                        },
                        ClientToServerMsg::SubscribeToEvents { .. } => {
                            subscribe_to_events(senders.as_ref(), os_input.as_ref(), client_id, instruction, retry_queue.as_deref_mut())?;
                        },
                        ClientToServerMsg::NestedSessionFrameFromHost { ref payload_bytes } => {
                            match yinz_ipc::nested_session::decode_payload(payload_bytes) {
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::AnnounceAck { .. }) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::FocusGained { .. }) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::FocusLost) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::ShortcutUpdate { .. }) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::FullscreenState { .. }) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message @ yinz_ipc::nested_session::NestedSessionMessage::AncestryUpdate { .. }) => {
                                    send_to_screen_or_retry_queue!(
                                        senders,
                                        ScreenInstruction::NestedSessionMessageFromHost { client_id, message },
                                        instruction,
                                        retry_queue
                                    ).with_context(err_context).non_fatal();
                                },
                                Some(message) => {
                                    log::debug!(
                                        "dropping unsupported nested session frame relayed from host: {:?}",
                                        message
                                    );
                                },
                                None => {
                                    log::debug!("dropping undecodable nested session frame relayed from host");
                                },
                            }
                        },
                        ClientToServerMsg::RequestSessionList => {
                            if let Some(senders) = senders.as_ref()
                                && let Some(scan_state) =
                                    crate::background_jobs::session_scan_state()
                                {
                                    let (session_name, available_layouts, plugin_list) = {
                                        let name =
                                            scan_state.current_session_name.lock().unwrap().clone();
                                        let info =
                                            scan_state.current_session_info.lock().unwrap().clone();
                                        let plugins = scan_state
                                            .current_session_plugin_list
                                            .lock()
                                            .unwrap()
                                            .clone();
                                        (name, info.available_layouts, plugins)
                                    };
                                    let (live_sessions_map, resurrectable_sessions_map) =
                                        crate::background_jobs::scan_session_list_default_dirs(
                                            &session_name,
                                            &available_layouts,
                                            &plugin_list,
                                        );
                                    senders.send_to_screen(
                                        ScreenInstruction::UpdateSessionInfos(
                                            live_sessions_map,
                                            resurrectable_sessions_map,
                                        ),
                                    ).with_context(err_context).non_fatal();
                                }
                        },
                        ClientToServerMsg::ModifiersHeld { modifiers } => {
                            // Straight to the plugins, with no `ScreenInstruction` hop: unlike
                            // the focus and theme signals this changes nothing about any pane,
                            // and the screen thread has no use for it.
                            senders.as_ref().map(|senders| {
                                senders.send_to_plugin(PluginInstruction::Update(vec![(
                                    None,
                                    Some(client_id),
                                    Event::ModifiersHeld {
                                        modifiers,
                                        is_live: true,
                                    },
                                )]))
                            }).transpose().with_context(err_context).non_fatal();
                        },
                        ClientToServerMsg::HostTerminalFocusChanged { focused } => {
                            send_to_screen_or_retry_queue!(
                                senders,
                                ScreenInstruction::HostTerminalFocusChanged(client_id, focused),
                                instruction,
                                retry_queue
                            ).with_context(err_context).non_fatal();
                        },
                    }
                    Ok(should_break)
                };
                // the parked instructions and the one just received form a single
                // FIFO sequence: the server can become ready in the middle of the
                // retry pass below, and handing it the fresh instruction while
                // older ones are still parked would deliver them out of order (a
                // stale nested AnnounceAck landing after the AncestryUpdate that
                // supersedes it, for instance). Once one instruction has to be
                // parked, every instruction behind it is parked too.
                let mut retried_count = retry_queue.len();
                let mut pending_instructions = std::mem::take(&mut retry_queue);
                pending_instructions.push_back(instruction);
                loop {
                    let mut deferred_instructions = VecDeque::new();
                    for (index, pending_instruction) in pending_instructions.into_iter().enumerate()
                    {
                        if !deferred_instructions.is_empty() {
                            deferred_instructions.push_back(pending_instruction);
                            continue;
                        }
                        if index < retried_count {
                            log::warn!("Server ready, retrying sending instruction.");
                            thread::sleep(Duration::from_millis(5));
                        }
                        let should_break = handle_instruction(
                            pending_instruction,
                            Some(&mut deferred_instructions),
                        )?;
                        if should_break {
                            break 'route_loop;
                        }
                    }
                    if deferred_instructions.is_empty()
                        || !wait_for_session_to_be_ready(&session_data)
                    {
                        // shortcut: after timeout, retry on client traffic; use a wakeup if startup can exceed 10s.
                        retry_queue = deferred_instructions;
                        break;
                    }
                    retried_count = deferred_instructions.len();
                    pending_instructions = deferred_instructions;
                }
            },
            IpcRecv::Eof => {
                // The client is gone. This returns instantly and will keep
                // doing so, so there is nothing to retry and nobody left to
                // write an Exit to — leave, and let the cleanup below take the
                // client out of the session.
                log::info!("client {client_id} closed its connection");
                break 'route_loop;
            },
            IpcRecv::Malformed => {
                consecutive_malformed_messages_received += 1;
                if consecutive_malformed_messages_received == 1 {
                    log::error!("Received an unreadable message from client {client_id}.");
                }
                if consecutive_malformed_messages_received >= MAX_CONSECUTIVE_MALFORMED_MESSAGES {
                    log::error!("Client sent over {MAX_CONSECUTIVE_MALFORMED_MESSAGES} consecutive unreadable messages, the connection has desynchronised - logging client out");
                    os_input
                        .send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Error(
                                    "The connection to this client desynchronised.".to_string(),
                                ),
                            },
                        )
                        .with_context(err_context)
                        .non_fatal();
                    break 'route_loop;
                }
                // Don't fall through to the unblock below: nothing was
                // processed, and a stream that produces malformed frames
                // without blocking would otherwise have us write to it as fast
                // as the CPU allows.
                thread::sleep(MALFORMED_MESSAGE_BACKOFF);
                continue 'route_loop;
            },
            IpcRecv::Idle => {
                if !received_first_message {
                    log::warn!(
                        "client {client_id} connected but sent nothing within \
                         {first_message_timeout:?}; dropping the connection"
                    );
                    break 'route_loop;
                }
                continue 'route_loop;
            },
        }

        // signal to the client that the action has finished processing and it can either exit (if
        // it's a cli client) or allow the user to perform another action (if it's an actively
        // connected user)
        os_input
            .send_to_client(client_id, ServerToClientMsg::UnblockInputThread)
            .with_context(err_context)
            .non_fatal();
    }
    // One removal per route, queued after everything the route sent.
    session_data
        .write()
        .to_anyhow()
        .map(|mut session| {
            if let Some(session) = session.as_mut() {
                session.locked_escapes.remove(&client_id);
            }
        })
        .context("cancelling disconnected escape")
        .non_fatal();
    to_server
        .send(ServerInstruction::RemoveClient(client_id))
        .with_context(err_context)
        .non_fatal();
    Ok(())
}

#[cfg(test)]
#[path = "unit/client_router_tests.rs"]
mod tests;
