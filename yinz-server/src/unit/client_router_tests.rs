use super::*;
use crate::background_jobs::BackgroundJob;
use crate::capturing_os::CapturingOs;
use crate::pty::PtyInstruction;
use crate::pty_writer::PtyWriteInstruction;
use std::collections::HashMap;
use yinz_runtime::channels::{self, Receiver};
use yinz_types::diagnostics::ErrorContext;
use yinz_types::internal_ids::{ProcessId, TerminalId};

#[test]
fn subscriptions_received_during_initialization_are_retained_once() {
    for event_types in [
        vec!["PaneUpdate".into()],
        vec!["agent_lifecycle".into(), "PaneUpdate".into()],
    ] {
        let instruction = ClientToServerMsg::SubscribeToEvents {
            event_types,
            options: Default::default(),
        };
        let os = CapturingOs::default();
        let mut retry = VecDeque::new();
        subscribe_to_events(None, &os, CLIENT_1, instruction, Some(&mut retry)).unwrap();
        assert_eq!(
            retry.len(),
            1,
            "the complete subscription must wait for initialization"
        );
        let (session, threads) = paused_session();
        subscribe_to_events(
            Some(&session.senders),
            &os,
            CLIENT_1,
            retry.pop_front().unwrap(),
            Some(&mut retry),
        )
        .unwrap();
        assert!(retry.is_empty());
        assert!(matches!(
            threads.plugin_receiver.try_recv().unwrap().0,
            PluginInstruction::SubscribeCliClientToEvents {
                client_id: CLIENT_1,
                ..
            }
        ));
        assert!(threads.plugin_receiver.is_empty());
    }
}

#[test]
fn reports_are_accepted_while_consumers_are_paused_and_rejected_for_closed_queues() {
    use {
        yinz_runtime::channels::Shutdown, yinz_types::agent::report::AgentReport,
        yinz_types::agent::report::AgentReportState,
    };
    let shutdown = Shutdown::default();
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(tx)),
        ..Default::default()
    };
    let cloned_senders = senders.clone();
    let report = AgentReport {
        report_id: "00000000-0000-4000-8000-000000000001".into(),
        pane_id: yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        state: AgentReportState::Clear,
        event: None,
        trace: false,
        process_id: yinz_types::internal_ids::ProcessId::new(42),
        kind: "claude".into(),
    };
    let (release, gate) = std::sync::mpsc::channel();
    let (accepted, acknowledgement) = std::sync::mpsc::channel();
    let consumer = std::thread::spawn(move || {
        gate.recv().unwrap();
        rx.recv().unwrap().0
    });
    accept_agent_report(Some(&senders), report.clone(), |reply| {
        accepted.send(reply).unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(
        acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id.clone()
        }
    );
    release.send(()).unwrap();
    assert!(
        matches!(consumer.join().unwrap(), ScreenInstruction::AgentReport(received, _) if received == report)
    );
    for shutting_down in [false, true] {
        if shutting_down {
            shutdown.begin();
        }
        for sender in [None, Some(&senders), Some(&cloned_senders)] {
            accept_agent_report(sender, report.clone(), |reply| {
                assert!(matches!(reply, ServerToClientMsg::LogError { .. }));
                Ok(())
            })
            .unwrap();
        }
    }
}

fn modifiers(of: &[KeyModifier]) -> BTreeSet<KeyModifier> {
    of.iter().cloned().collect()
}

/// The bug this guards: every `Ctrl` chord in a burst of editing broadcast its own
/// `ModifiersHeld` to every plugin, restating a fact none of them had stopped believing.
#[test]
fn an_unchanged_inferred_modifier_set_is_restated_only_once_an_interval() {
    let start = Instant::now();
    let mut last = None;
    let ctrl = modifiers(&[KeyModifier::Ctrl]);
    assert!(
        should_report_inferred_modifiers(&mut last, &ctrl, start),
        "the first chord has nothing to be redundant with"
    );
    assert!(
        !should_report_inferred_modifiers(&mut last, &ctrl, start + Duration::from_millis(900)),
        "the same set inside the interval says nothing new"
    );
    assert!(
        should_report_inferred_modifiers(&mut last, &ctrl, start + Duration::from_secs(1)),
        "once the interval is up the hold has to be restated or a consumer times it out"
    );
}

#[test]
fn a_changed_inferred_modifier_set_is_reported_at_once() {
    let start = Instant::now();
    let mut last = None;
    assert!(should_report_inferred_modifiers(
        &mut last,
        &modifiers(&[KeyModifier::Ctrl]),
        start
    ));
    assert!(
        should_report_inferred_modifiers(
            &mut last,
            &modifiers(&[KeyModifier::Alt]),
            start + Duration::from_millis(10)
        ),
        "a different set is news no matter how soon it arrives"
    );
    assert!(
        should_report_inferred_modifiers(
            &mut last,
            &modifiers(&[KeyModifier::Alt, KeyModifier::Ctrl]),
            start + Duration::from_millis(20)
        ),
        "so is the same modifier joined by another"
    );
}

/// A suppressed chord must not push the deadline out, or a fast enough typist would keep the
/// hold from ever being restated and a consumer would time it out mid-hold.
#[test]
fn suppressing_a_chord_does_not_slide_the_interval() {
    let start = Instant::now();
    let mut last = None;
    let alt = modifiers(&[KeyModifier::Alt]);
    assert!(should_report_inferred_modifiers(&mut last, &alt, start));
    for elapsed_ms in [400, 800] {
        assert!(!should_report_inferred_modifiers(
            &mut last,
            &alt,
            start + Duration::from_millis(elapsed_ms)
        ));
    }
    assert!(should_report_inferred_modifiers(
        &mut last,
        &alt,
        start + Duration::from_millis(1200)
    ));
}

const CLIENT_1: ClientId = ClientId::new(1);
const CLIENT_99: ClientId = ClientId::new(99);

/// The receiving ends of a session's threads. Nothing consumes any of them, so the screen,
/// the plugins and the server are all paused: whatever a route thread does must not wait on
/// one of them.
struct PausedThreads {
    to_server: SenderWithContext<ServerInstruction>,
    screen_receiver: Receiver<(ScreenInstruction, ErrorContext)>,
    plugin_receiver: Receiver<(PluginInstruction, ErrorContext)>,
    server_receiver: Receiver<(ServerInstruction, ErrorContext)>,
    _pty_receiver: Receiver<(PtyInstruction, ErrorContext)>,
    _pty_writer_receiver: Receiver<(PtyWriteInstruction, ErrorContext)>,
    _background_jobs_receiver: Receiver<(BackgroundJob, ErrorContext)>,
}

/// A session under the default configuration whose threads are all paused.
fn paused_session() -> (SessionMetaData, PausedThreads) {
    let (to_server, server_receiver) = channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);
    let (to_screen, screen_receiver) = channels::unbounded();
    let (to_plugin, plugin_receiver) = channels::unbounded();
    let (to_pty, pty_receiver) = channels::unbounded();
    let (to_pty_writer, pty_writer_receiver) = channels::unbounded();
    let (to_background_jobs, background_jobs_receiver) = channels::unbounded();
    let session = SessionMetaData {
        shutdown: Default::default(),
        senders: ThreadSenders {
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_pty: Some(SenderWithContext::new(to_pty)),
            to_plugin: Some(SenderWithContext::new(to_plugin)),
            to_pty_writer: Some(SenderWithContext::new(to_pty_writer)),
            to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
            to_server: Some(to_server.clone()),
            session_checkpoint: Default::default(),
            session_ownership: None,
            should_silently_fail: true,
        },
        data_access: std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
            std::path::Path::new("/unused"),
        )),
        default_shell: None,
        current_input_modes: HashMap::new(),
        session_configuration: Default::default(),
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        command_palette_restore_modes: HashMap::new(),
        pending_mouse_updates: Vec::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        pty_writer_thread: None,
        background_jobs_thread: None,
        config_file_path: None,
        config_dir: None,
    };
    let threads = PausedThreads {
        to_server,
        screen_receiver,
        plugin_receiver,
        server_receiver,
        _pty_receiver: pty_receiver,
        _pty_writer_receiver: pty_writer_receiver,
        _background_jobs_receiver: background_jobs_receiver,
    };
    (session, threads)
}

#[test]
fn legacy_lf_identity_follows_live_mode_and_keybindings() {
    let (mut session, _threads) = paused_session();
    let ctrl_j = KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier();
    let enter = KeyWithModifier::new(BareKey::Enter);
    let mut config = yinz_config::config::Config::default();
    config.options.default_mode = Some(InputMode::Normal);
    config
        .keybinds
        .get_input_mode_mut(&InputMode::Pane)
        .insert(ctrl_j.clone(), vec![Action::NoOp]);
    session
        .session_configuration
        .set_client_runtime_configuration(CLIENT_1, config.clone());
    assert_eq!(legacy_lf_key(&session, CLIENT_1), enter);
    session
        .change_input_mode(CLIENT_1, InputMode::Pane, None)
        .unwrap();
    assert_eq!(legacy_lf_key(&session, CLIENT_1), ctrl_j);
    session
        .key_passthrough_clients
        .insert(CLIENT_1, PaneId::Terminal(TerminalId::new(9)));
    assert_eq!(legacy_lf_key(&session, CLIENT_1), ctrl_j);
    config
        .keybinds
        .get_input_mode_mut(&InputMode::Pane)
        .insert(ctrl_j.clone(), vec![]);
    session
        .session_configuration
        .set_client_runtime_configuration(CLIENT_1, config);
    assert_eq!(legacy_lf_key(&session, CLIENT_1), enter);
}

#[test]
fn interactive_paste_follows_live_mode_without_changing_cli_or_targeted_paste() {
    let (mut session, _threads) = paused_session();
    let mut config = yinz_config::config::Config::default();
    config.options.default_mode = Some(InputMode::Normal);
    session
        .session_configuration
        .set_saved_configuration(config);
    let paste = Action::Paste {
        chars: "pasted text\nλ".into(),
        pane_id: None,
    };
    let input = "pasted text\nλ".as_bytes().to_vec();
    let writes = [b"\x1b[200~".to_vec(), input.clone(), b"\x1b[201~".to_vec()]
        .into_iter()
        .map(|bytes| Action::Write {
            key_with_modifier: None,
            bytes,
            is_kitty_keyboard_protocol: false,
        })
        .collect::<Vec<_>>();
    // The initial/default mode stays Normal while the same client changes mode at runtime.
    assert_eq!(
        resolve_client_paste(paste.clone(), false, &session, CLIENT_1),
        writes
    );
    for (mode, expected) in [
        (
            InputMode::EnterSearch,
            vec![Action::SearchInput {
                input: input.clone(),
            }],
        ),
        (
            InputMode::RenamePane,
            vec![Action::PaneNameInput {
                input: input.clone(),
            }],
        ),
        (
            InputMode::RenameTab,
            vec![Action::TabNameInput {
                input: input.clone(),
            }],
        ),
        (InputMode::Pane, vec![]),
        (InputMode::Locked, writes.clone()),
        (InputMode::Normal, writes.clone()),
    ] {
        session.change_input_mode(CLIENT_1, mode, None).unwrap();
        assert_eq!(
            resolve_client_paste(paste.clone(), false, &session, CLIENT_1),
            expected,
            "interactive paste in {mode:?}",
        );
        assert_eq!(
            resolve_client_paste(paste.clone(), true, &session, CLIENT_1),
            vec![paste.clone()],
            "CLI paste must not depend on {mode:?}",
        );
        let targeted = Action::Paste {
            chars: "targeted".into(),
            pane_id: Some(PaneId::Terminal(TerminalId::new(9))),
        };
        assert_eq!(
            resolve_client_paste(targeted.clone(), false, &session, CLIENT_1),
            vec![targeted],
        );
    }
    session
        .change_input_mode(CLIENT_1, InputMode::EnterSearch, None)
        .unwrap();
    session
        .key_passthrough_clients
        .insert(CLIENT_1, PaneId::Terminal(TerminalId::new(9)));
    assert_eq!(
        resolve_client_paste(paste, false, &session, CLIENT_1),
        writes
    );
}

#[test]
fn report_router_acknowledges_with_screen_and_plugins_paused_and_no_active_client() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::IpcSenderWithContext;
    use {yinz_types::agent::report::AgentReport, yinz_types::agent::report::AgentReportState};
    let (session, threads) = paused_session();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.sock");
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let client =
        LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .unwrap();
    let server = listener.accept().unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
    let report = AgentReport {
        report_id: "00000000-0000-4000-8000-000000000001".into(),
        pane_id: yinz_types::data::PaneId::Terminal(TerminalId::new(2)),
        state: AgentReportState::Clear,
        event: None,
        trace: false,
        process_id: ProcessId::new(42),
        kind: "claude".into(),
    };
    sender
        .send_client_msg(ClientToServerMsg::AgentReport(report.clone()))
        .unwrap();
    let session = Arc::new(RwLock::new(Some(session)));
    // Keep the session owner alive while the router exits. Dropping SessionMetaData sends
    // thread Exit instructions, which are unrelated to reporting.
    let router_session = session.clone();
    let os = CapturingOs::default();
    let router_os = os.clone();
    let to_server = threads.to_server.clone();
    let (finished, done) = std::sync::mpsc::channel();
    let router = std::thread::spawn(move || {
        let result = route_thread_main(
            router_session,
            Arc::new(RwLock::new(SessionState::new())),
            Box::new(router_os),
            to_server,
            IpcReceiverWithContext::new(server),
            CLIENT_99,
            Duration::from_secs(1),
        );
        finished.send(result).unwrap();
    });
    // Neither consumer is running. The router must finish without their participation.
    done.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    router.join().unwrap();
    assert_eq!(
        os.server_to_client_messages.lock().get(&CLIENT_99).unwrap(),
        &vec![ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id.clone()
        }]
    );
    assert!(matches!(threads.screen_receiver.try_recv().unwrap().0,
        ScreenInstruction::AgentReport(queued, _) if queued == report));
    assert!(threads.plugin_receiver.is_empty());
    let removals = threads
        .server_receiver
        .try_iter()
        .filter(|(message, _)| matches!(message, ServerInstruction::RemoveClient(CLIENT_99)))
        .count();
    assert_eq!(removals, 1, "the route removes its client once");
}

#[test]
fn agent_control_routes_explicit_targets_replies_and_removes_the_client_once() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::IpcSenderWithContext;
    use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse};
    use yinz_types::agent::AgentSpawnSpec;

    for ready in [true, false] {
        let (session, threads) = paused_session();
        let session = Arc::new(RwLock::new(Some(session)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-control.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .unwrap();
        let server = listener.accept().unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
        let prompt = AgentRequest::Prompt {
            preserve_draft: false,
            submission_id: None,
            pane_id: PaneId::Terminal(TerminalId::new(2)),
            expected_session_id: Some("conversation-7".into()),
            prompt: "Review this diff.\nExplain the result.".into(),
            wait: true,
            timeout_ms: 60_000,
        };
        let clear = AgentRequest::ClearDraft {
            pane_id: PaneId::Terminal(TerminalId::new(2)),
            expected_session_id: "conversation-7".into(),
            input_revision: 42,
            confirm_discard: true,
            interrupted: false,
            timeout_ms: 30_000,
        };
        for request in [
            prompt.clone(),
            clear.clone(),
            AgentRequest::Spawn {
                placement: Default::default(),

                spec: AgentSpawnSpec {
                    kind: Some("zsh".into()),
                    ..Default::default()
                },
                cwd: Some(dir.path().to_path_buf()),
                project: None,
                remote_host: None,
            },
        ] {
            sender
                .send_client_msg(ClientToServerMsg::AgentControl(request))
                .unwrap();
        }
        sender
            .send_client_msg(ClientToServerMsg::ClientExited)
            .unwrap();
        let router_session = if ready {
            session.clone()
        } else {
            Arc::new(RwLock::new(None))
        };
        let os = CapturingOs::default();
        let router_os = os.clone();
        let to_server = threads.to_server.clone();
        let (finished, done) = std::sync::mpsc::channel();
        let router = std::thread::spawn(move || {
            let result = route_thread_main(
                router_session,
                Arc::new(RwLock::new(SessionState::new())),
                Box::new(router_os),
                to_server,
                IpcReceiverWithContext::new(server),
                CLIENT_99,
                Duration::from_secs(1),
            );
            finished.send(result).unwrap();
        });
        // A prompt is handed to Screen without waiting on it. The connection can close
        // while consumers are paused, and the router must preserve its target and identity.
        done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        router.join().unwrap();
        if ready {
            assert!(matches!(threads.screen_receiver.try_recv().unwrap().0,
                ScreenInstruction::AgentControl(queued, CLIENT_99) if queued == prompt));
            assert!(matches!(threads.screen_receiver.try_recv().unwrap().0,
                ScreenInstruction::AgentControl(queued, CLIENT_99) if queued == clear));
        }
        assert!(threads.screen_receiver.is_empty());
        let replies = os.server_to_client_messages.lock();
        let replies = replies.get(&CLIENT_99).unwrap();
        let mut expected = Vec::new();
        if !ready {
            expected.push(ServerToClientMsg::AgentControl(AgentResponse::new(
                AgentOutcome::StatusUnavailable,
            )));
        }
        expected.push(ServerToClientMsg::UnblockInputThread);
        if !ready {
            expected.push(ServerToClientMsg::AgentControl(AgentResponse::new(
                AgentOutcome::StatusUnavailable,
            )));
        }
        expected.push(ServerToClientMsg::UnblockInputThread);
        let mut rejected = AgentResponse::new(if ready {
            AgentOutcome::Rejected
        } else {
            AgentOutcome::StatusUnavailable
        });
        if ready {
            rejected.message = Some(format!("agent spawn requires a recognized agent command. Use --kind with a recognized kind: {}. Alternatively, use --file PATH (or --file - for stdin) with a JSON object containing a recognized executable in command and optional args.", yinz_types::agent::KINDS.join(", ")));
        }
        expected.push(ServerToClientMsg::AgentControl(rejected));
        expected.push(ServerToClientMsg::UnblockInputThread);
        assert_eq!(replies, &expected);
        assert!(threads.plugin_receiver.is_empty());
        assert_eq!(
            threads
                .server_receiver
                .try_iter()
                .filter(|(message, _)| matches!(
                    message,
                    ServerInstruction::RemoveClient(CLIENT_99)
                ))
                .count(),
            1
        );
    }
}

#[test]
fn kitty_probe_routing_preserves_client_lifecycle_order() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::{IpcSenderWithContext, PixelDimensions};
    use yinz_types::pane_size::{Size, SizeInPixels};

    for first_message in [
        ClientToServerMsg::FirstClientConnected {
            cli_assets: Default::default(),
        },
        ClientToServerMsg::AttachClient {
            cli_assets: Default::default(),
            tab_number_to_focus: None,
            pane_to_focus: None,
        },
    ] {
        let (session, threads) = paused_session();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kitty.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .unwrap();
        let server = listener.accept().unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
        let reported_pixels = PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
            text_area_grid_size: Some(Size { cols: 80, rows: 24 }),
            character_cell_size: None,
        };
        for message in [
            first_message,
            ClientToServerMsg::TerminalPixelDimensions {
                pixel_dimensions: reported_pixels,
            },
            ClientToServerMsg::KittyGraphicsSupport { supported: false },
            ClientToServerMsg::KittyGraphicsSupport { supported: true },
            ClientToServerMsg::ClientExited,
        ] {
            sender.send_client_msg(message).unwrap();
        }
        let session = Arc::new(RwLock::new(Some(session)));
        // Leave both consumers paused: the router must queue registration, answers and
        // removal together, even when the server has not begun handling the first message.
        route_thread_main(
            session.clone(),
            Arc::new(RwLock::new(SessionState::new())),
            Box::new(CapturingOs::default()),
            threads.to_server.clone(),
            IpcReceiverWithContext::new(server),
            CLIENT_1,
            Duration::from_secs(1),
        )
        .unwrap();
        let instructions: Vec<_> = threads
            .server_receiver
            .try_iter()
            .map(|(instruction, _)| instruction)
            .collect();
        assert!(
            matches!(
                instructions.as_slice(),
                [
                    ServerInstruction::FirstClientConnected(_, CLIENT_1)
                        | ServerInstruction::AttachClient(_, _, _, CLIENT_1),
                    ServerInstruction::TerminalPixelDimensions {
                        client_id: CLIENT_1,
                        ..
                    },
                    ServerInstruction::KittyGraphicsSupport {
                        client_id: CLIENT_1,
                        supported: false
                    },
                    ServerInstruction::KittyGraphicsSupport {
                        client_id: CLIENT_1,
                        supported: true
                    },
                    ServerInstruction::RemoveClient(CLIENT_1),
                ]
            ),
            "{instructions:?}"
        );
        assert!(
            matches!(instructions.get(1), Some(ServerInstruction::TerminalPixelDimensions {
            pixel_dimensions, ..
        }) if *pixel_dimensions == reported_pixels)
        );
        assert!(
            threads.screen_receiver.is_empty(),
            "the probe bypassed client registration"
        );
    }
}

#[test]
fn startup_retries_without_client_traffic_and_preserves_fifo_after_timeout() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::IpcSenderWithContext;

    for timeout_first in [false, true] {
        let (session, threads) = paused_session();
        let session_data = Arc::new(RwLock::new(None));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .unwrap();
        let server = listener.accept().unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
        let os = CapturingOs::default();
        let router_os = os.clone();
        let router_session = session_data.clone();
        let to_server = threads.to_server.clone();
        let router = thread::spawn(move || {
            route_thread_main(
                router_session,
                Arc::new(RwLock::new(SessionState::new())),
                Box::new(router_os),
                to_server,
                IpcReceiverWithContext::new(server),
                CLIENT_1,
                Duration::from_secs(5),
            )
        });
        sender
            .send_client_msg(ClientToServerMsg::SixelSupport { supported: true })
            .unwrap();
        if timeout_first {
            os.server_to_client_messages.wait_until(
                "the readiness timeout to unblock the client",
                Duration::from_secs(15),
                |messages| {
                    messages
                        .get(&CLIENT_1)
                        .filter(|messages| {
                            messages.contains(&ServerToClientMsg::UnblockInputThread)
                        })
                        .map(|_| ())
                },
            );
            assert!(threads.screen_receiver.is_empty());
        }
        sender
            .send_client_msg(ClientToServerMsg::SixelSupport { supported: false })
            .unwrap();
        // No startup instruction may reach Screen while session metadata is absent.
        assert!(threads
            .screen_receiver
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        *session_data.write().unwrap() = Some(session);
        // Publishing metadata is the only wakeup: send no further client traffic.
        for expected in [true, false] {
            assert!(
                matches!(threads.screen_receiver.recv_timeout(Duration::from_secs(5)).unwrap().0,
                ScreenInstruction::SetSixelSupport { client_id: CLIENT_1, supported } if supported == expected)
            );
        }
        sender
            .send_client_msg(ClientToServerMsg::ClientExited)
            .unwrap();
        router.join().unwrap().unwrap();
        assert!(threads.screen_receiver.is_empty());
        assert_eq!(
            threads
                .server_receiver
                .try_iter()
                .filter(|(message, _)| matches!(message, ServerInstruction::RemoveClient(CLIENT_1)))
                .count(),
            1
        );
    }
}
