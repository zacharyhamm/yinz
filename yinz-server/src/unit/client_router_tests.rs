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

#[test]
fn a_sealed_source_asks_the_reporter_to_retry_without_queueing() {
    use yinz_types::agent::report::{AgentReport, AgentReportState};
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(tx)),
        ..Default::default()
    };
    let report = AgentReport {
        report_id: "00000000-0000-4000-8000-000000000002".into(),
        pane_id: yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        state: AgentReportState::Clear,
        event: None,
        trace: false,
        process_id: yinz_types::internal_ids::ProcessId::new(42),
        kind: "claude".into(),
    };
    for sealed in [true, false] {
        *senders.agent_reports_sealed.lock().unwrap() = sealed;
        let mut replies = Vec::new();
        accept_agent_report(Some(&senders), report.clone(), |reply| {
            replies.push(reply);
            Ok(())
        })
        .unwrap();
        let report_id = report.report_id.clone();
        if sealed {
            assert_eq!(
                replies,
                vec![ServerToClientMsg::AgentReportRetry { report_id }]
            );
            assert!(rx.try_recv().is_err(), "a sealed report must not be queued");
        } else {
            assert_eq!(
                replies,
                vec![ServerToClientMsg::AgentReportAccepted { report_id }]
            );
            assert!(matches!(
                rx.try_recv().unwrap().0,
                ScreenInstruction::AgentReport(received, _) if received == report
            ));
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
            launches: Default::default(),
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_pty: Some(SenderWithContext::new(to_pty)),
            to_plugin: Some(SenderWithContext::new(to_plugin)),
            to_pty_writer: Some(SenderWithContext::new(to_pty_writer)),
            to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
            to_server: Some(to_server.clone()),
            session: Default::default(),
            session_checkpoint: Default::default(),
            session_ownership: None,
            session_update_quiesced: Default::default(),
            agent_reports_sealed: Default::default(),
            should_silently_fail: true,
        },
        data_access: std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
            std::path::Path::new("/unused"),
        )),
        default_shell: None,
        current_input_modes: HashMap::new(),
        session_configuration: Default::default(),
        effective_layout: Default::default(),
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        command_palette_restore_modes: HashMap::new(),
        pending_mouse_updates: Vec::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        web_server: None,
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
fn closed_router_queues_are_logged_without_skipping_client_cleanup() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::IpcSenderWithContext;

    thread_local! {
        static LOGS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
    }
    struct Logger;
    impl log::Log for Logger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() == log::Level::Error
        }
        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.with(|logs| {
                    if let Some(logs) = logs.borrow_mut().as_mut() {
                        logs.push(record.args().to_string());
                    }
                });
            }
        }
        fn flush(&self) {}
    }
    log::set_logger(&Logger).unwrap();
    log::set_max_level(log::LevelFilter::Error);

    for (shutting_down, poisoned) in [(false, false), (true, false), (false, true)] {
        let (mut session, threads) = paused_session();
        session.senders.should_silently_fail = false;
        let (screen, screen_receiver) = channels::unbounded();
        let (plugin, plugin_receiver) = channels::unbounded();
        session.senders.to_screen = Some(session.shutdown.sender(screen));
        session.senders.to_plugin = Some(session.shutdown.sender(plugin));
        drop((screen_receiver, plugin_receiver));
        if shutting_down {
            session.shutdown.begin();
        }
        let session = Arc::new(RwLock::new(Some(session)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("closed-queues.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
        if poisoned {
            let poisoned_session = session.clone();
            assert!(thread::spawn(move || {
                let _guard = poisoned_session.write().unwrap();
                panic!("poison session metadata before client disconnect");
            })
            .join()
            .is_err());
            drop(sender);
        } else {
            for message in [
                ClientToServerMsg::HostTerminalFocusChanged { focused: true },
                ClientToServerMsg::ModifiersHeld {
                    modifiers: BTreeSet::new(),
                },
                ClientToServerMsg::ClientExited,
            ] {
                sender.send_client_msg(message).unwrap();
            }
        }
        LOGS.with(|logs| *logs.borrow_mut() = Some(Vec::new()));
        let result = route_thread_main(
            session.clone(),
            Arc::new(RwLock::new(SessionState::new())),
            Box::new(CapturingOs::default()),
            threads.to_server.clone(),
            IpcReceiverWithContext::new(listener.accept().unwrap()),
            CLIENT_1,
            Duration::from_secs(5),
        );
        let logs = LOGS.with(|logs| logs.borrow_mut().take().unwrap());
        result.unwrap();
        if poisoned {
            assert!(logs
                .iter()
                .any(|line| line.contains("cancelling disconnected escape")));
        } else if shutting_down {
            assert!(
                logs.is_empty(),
                "expected shutdown sends were logged: {logs:?}"
            );
        } else {
            for target in ["screen", "plugin"] {
                assert!(
                    logs.iter().any(|line| {
                        line.contains("failed to handle instruction for client 1")
                            && line.contains(&format!("failed to send message to {target}"))
                    }),
                    "missing {target} send error: {logs:?}"
                );
            }
        }
        assert!(matches!(
            threads.server_receiver.try_recv().unwrap().0,
            ServerInstruction::RemoveClient(CLIENT_1)
        ));
        assert!(threads.server_receiver.is_empty());
    }
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
    use yinz_types::agent::control::{
        AgentOutcome, AgentRequest, AgentResponse, ProviderUsageStatus,
    };
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
            AgentRequest::Usage {
                provider: "unsupported-fixture".into(),
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
        let mut replies = os
            .server_to_client_messages
            .lock()
            .get(&CLIENT_99)
            .unwrap()
            .clone();
        assert_eq!(replies.pop(), Some(ServerToClientMsg::UnblockInputThread));
        let Some(ServerToClientMsg::AgentControl(usage_response)) = replies.pop() else {
            panic!("expected provider usage response");
        };
        if ready {
            assert_eq!(usage_response.outcome, AgentOutcome::Ok);
            let usage = usage_response.usage.unwrap();
            assert_eq!(usage.provider, "unsupported-fixture");
            assert_eq!(usage.status, ProviderUsageStatus::Unsupported);
            assert!(usage.windows.is_empty());
        } else {
            assert_eq!(
                usage_response,
                AgentResponse::new(AgentOutcome::StatusUnavailable)
            );
        }
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
        assert_eq!(replies, expected);
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

#[test]
fn watcher_signal_quit_exits_normally_without_terminating_the_session() {
    use interprocess::local_socket::traits::Listener;
    use yinz_ipc::ipc::IpcSenderWithContext;

    let (session, threads) = paused_session();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watcher.sock");
    let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
    let client = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let server = listener.accept().unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client);
    sender
        .send_client_msg(ClientToServerMsg::SixelSupport { supported: true })
        .unwrap();
    sender
        .send_client_msg(ClientToServerMsg::SignalQuit)
        .unwrap();
    // Bound a broken implementation: ignoring SignalQuit must not hang this test.
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .unwrap();
    drop(sender);
    let mut state = SessionState::new();
    let watcher = state.new_client().unwrap();
    state.convert_client_to_watcher(watcher);
    let interactive = state.new_client().unwrap();
    state.interactive_clients.insert(interactive);
    let state = Arc::new(RwLock::new(state));
    let os = CapturingOs::default();
    let session = Arc::new(RwLock::new(Some(session)));
    route_thread_main(
        session.clone(),
        state.clone(),
        Box::new(os.clone()),
        threads.to_server.clone(),
        IpcReceiverWithContext::new(server),
        watcher,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(
        os.server_to_client_messages.get().get(&watcher),
        Some(&vec![
            ServerToClientMsg::UnblockInputThread,
            ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal
            },
        ])
    );
    let instructions: Vec<_> = threads
        .server_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    assert!(
        matches!(instructions.as_slice(), [ServerInstruction::RemoveClient(id)] if *id == watcher),
        "{instructions:?}"
    );
    let screen: Vec<_> = threads
        .screen_receiver
        .try_iter()
        .map(|(message, _)| message)
        .collect();
    assert!(
        matches!(screen.as_slice(), [
        ScreenInstruction::SetSixelSupport { client_id, supported: true },
        ScreenInstruction::RemoveWatcherClient(removed),
    ] if *client_id == watcher && *removed == watcher),
        "{screen:?}"
    );
    assert!(state
        .read()
        .unwrap()
        .interactive_client_ids()
        .contains(&interactive));
}

const EMPTY_ACTION: &[u8] = &[0x4a, 2, 0x0a, 0];
const RETIRED_ARRANGE: &[u8] = &[0x4a, 5, 0x0a, 3, 0xa2, 0x0a, 0];
const ROUTER_TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn retired_action_router(
    watch: bool,
    first_message_timeout: Duration,
) -> (
    interprocess::local_socket::Stream,
    CapturingOs,
    PausedThreads,
    ClientId,
    ClientId,
    thread::JoinHandle<Result<()>>,
) {
    use interprocess::local_socket::traits::Listener;

    let (session, threads) = paused_session();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retired.sock");
    let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
    let client = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let server = listener.accept().unwrap();
    let mut state = SessionState::new();
    let origin = state.new_client().unwrap();
    if watch {
        state.convert_client_to_watcher(origin);
    } else {
        state.interactive_clients.insert(origin);
    }
    let target = state.new_client().unwrap();
    state.interactive_clients.insert(target);
    let os = CapturingOs::default();
    let router_os = os.clone();
    let to_server = threads.to_server.clone();
    let router = thread::spawn(move || {
        route_thread_main(
            Arc::new(RwLock::new(Some(session))),
            Arc::new(RwLock::new(state)),
            Box::new(router_os),
            to_server,
            IpcReceiverWithContext::new(server),
            origin,
            first_message_timeout,
        )
    });
    (client, os, threads, origin, target, router)
}

fn send_raw_action(stream: &mut impl std::io::Write, bytes: &[u8]) {
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
    stream.flush().unwrap();
}

fn wait_for_router_remove(threads: &PausedThreads, origin: ClientId) {
    assert!(matches!(
        threads.server_receiver.recv_timeout(ROUTER_TEST_TIMEOUT).unwrap().0,
        ServerInstruction::RemoveClient(id) if id == origin
    ));
}

#[test]
fn retired_arrange_ipc_action_gets_explicit_error() {
    use interprocess::TryClone;
    use yinz_ipc::ipc::IpcSenderWithContext;

    let (mut client, os, threads, origin, target, router) =
        retired_action_router(false, ROUTER_TEST_TIMEOUT);
    // Original ActionMsg includes a different live client ID in its metadata.
    let mut retired_with_target = vec![0x4a, 0, 0x0a, 3, 0xa2, 0x0a, 0, 0x18];
    retired_with_target.push(u8::try_from(target.get()).unwrap());
    assert!(target.get() < 128);
    retired_with_target[1] = (retired_with_target.len() - 2) as u8;

    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client.try_clone().unwrap());
    for (iteration, bytes) in [&retired_with_target[..], RETIRED_ARRANGE]
        .into_iter()
        .enumerate()
    {
        send_raw_action(&mut client, bytes);
        let expected_offset = iteration * 3;
        os.server_to_client_messages.wait_until(
            "retired action rejection",
            ROUTER_TEST_TIMEOUT,
            |messages| {
                let output = messages.get(&origin)?;
                let [ServerToClientMsg::LogError { lines }, ServerToClientMsg::UnblockInputThread] =
                    output.get(expected_offset..expected_offset + 2)?
                else {
                    return None;
                };
                assert!(
                    lines[0].contains("Arrange panes by project") && lines[0].contains("version")
                );
                assert!(!messages.contains_key(&target));
                Some(())
            },
        );
        assert!(threads.screen_receiver.is_empty());
        assert!(threads.plugin_receiver.is_empty());
        assert!(threads.server_receiver.is_empty());

        sender
            .send_client_msg(ClientToServerMsg::SixelSupport { supported: true })
            .unwrap();
        assert!(matches!(
            threads.screen_receiver.recv_timeout(ROUTER_TEST_TIMEOUT).unwrap().0,
            ScreenInstruction::SetSixelSupport { client_id, supported: true } if client_id == origin
        ));
        os.server_to_client_messages.wait_until(
            "supported request unblock",
            ROUTER_TEST_TIMEOUT,
            |messages| {
                (messages.get(&origin)?.get(expected_offset + 2)
                    == Some(&ServerToClientMsg::UnblockInputThread))
                .then_some(())
            },
        );
    }
    drop(sender);
    drop(client);
    wait_for_router_remove(&threads, origin);
    router.join().unwrap().unwrap();
    assert!(threads.server_receiver.is_empty());

    let (mut client, os, threads, watcher, target, router) =
        retired_action_router(true, ROUTER_TEST_TIMEOUT);
    send_raw_action(&mut client, RETIRED_ARRANGE);
    os.server_to_client_messages.wait_until(
        "watcher input unblock",
        ROUTER_TEST_TIMEOUT,
        |messages| {
            let output = messages.get(&watcher)?;
            assert_eq!(output, &[ServerToClientMsg::UnblockInputThread]);
            assert!(!messages.contains_key(&target));
            Some(())
        },
    );
    assert!(threads.screen_receiver.is_empty());
    assert!(threads.plugin_receiver.is_empty());
    drop(client);
    wait_for_router_remove(&threads, watcher);
    router.join().unwrap().unwrap();
}

#[test]
fn empty_and_retired_ipc_actions_preserve_first_read_timeout() {
    for bytes in [EMPTY_ACTION, RETIRED_ARRANGE] {
        let first_read_timeout = Duration::from_millis(250);
        let started = std::time::Instant::now();
        let (mut client, os, threads, origin, _, router) =
            retired_action_router(false, first_read_timeout);
        send_raw_action(&mut client, bytes);
        wait_for_router_remove(&threads, origin);
        assert!(started.elapsed() >= first_read_timeout);
        router.join().unwrap().unwrap();

        let messages = os.server_to_client_messages.get();
        if bytes == EMPTY_ACTION {
            assert!(!messages.contains_key(&origin));
        } else {
            assert!(matches!(
                messages[&origin].as_slice(),
                [
                    ServerToClientMsg::LogError { .. },
                    ServerToClientMsg::UnblockInputThread
                ]
            ));
        }
        // Keep the peer open through cleanup so EOF cannot satisfy the timeout test.
        drop(client);
    }
}

#[test]
fn empty_and_retired_ipc_actions_count_toward_malformed_budget() {
    use interprocess::TryClone;
    use yinz_ipc::ipc::IpcSenderWithContext;

    let (mut client, os, threads, origin, _, router) =
        retired_action_router(false, ROUTER_TEST_TIMEOUT);
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(client.try_clone().unwrap());
    sender
        .send_client_msg(ClientToServerMsg::SixelSupport { supported: true })
        .unwrap();
    assert!(matches!(
        threads
            .screen_receiver
            .recv_timeout(ROUTER_TEST_TIMEOUT)
            .unwrap()
            .0,
        ScreenInstruction::SetSixelSupport { .. }
    ));
    for _ in 0..MAX_CONSECUTIVE_MALFORMED_MESSAGES - 2 {
        send_raw_action(&mut client, EMPTY_ACTION);
    }
    send_raw_action(&mut client, RETIRED_ARRANGE);
    send_raw_action(&mut client, EMPTY_ACTION);
    wait_for_router_remove(&threads, origin);
    router.join().unwrap().unwrap();
    assert!(threads.server_receiver.is_empty());
    assert!(
        matches!(os.server_to_client_messages.get()[&origin].as_slice(), [
        ServerToClientMsg::UnblockInputThread,
        ServerToClientMsg::LogError { .. },
        ServerToClientMsg::UnblockInputThread,
        ServerToClientMsg::Exit { exit_reason: ExitReason::Error(error) },
    ] if error.contains("desynchronised"))
    );
}

/// Run both socket routers and writers through a forced numeric collision. Retain the
/// real render states so an accepted ACK has an observable filesystem consequence.
#[test]
fn attachment_generation_refusal_and_replacement_have_independent_frame_one() {
    use interprocess::{local_socket::traits::Listener, TryClone};
    use yinz_bus::os_input_output::ClientTransport;
    use yinz_ipc::ipc::IpcSenderWithContext;
    use yinz_runtime::native_media::{MediaBudget, MediaRoot, MediaUsage};
    use yinz_terminal::output::{NativeRenderState, Staging};
    use yinz_types::{
        render_negotiation::{Decision, Offer, Request},
        structured_render::FrameBuilder,
    };

    fn next_native(
        rx: &mut IpcReceiverWithContext<ServerToClientMsg>,
    ) -> Option<ServerToClientMsg> {
        let deadline = Instant::now() + ROUTER_TEST_TIMEOUT;
        loop {
            match rx.recv_native_server_msg(Some(deadline)).msg()?.0 {
                ServerToClientMsg::UnblockInputThread => continue,
                msg => return Some(msg),
            }
        }
    }

    for acknowledge_source in [false, true] {
        let (session, threads) = paused_session();
        let state = Arc::new(RwLock::new(SessionState {
            attachments: session.senders.session.clone(),
            ..SessionState::new()
        }));
        let data = Arc::new(RwLock::new(Some(session)));
        let os = yinz_server_os::get_server_os_input().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("attachment.sock");
        let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
        let next = || {
            threads
                .server_receiver
                .recv_timeout(ROUTER_TEST_TIMEOUT)
                .unwrap()
                .0
        };
        let attach = || {
            let id = {
                let mut state = state.write().unwrap();
                state.next_client_id = Some(1);
                let id = state.new_client().unwrap();
                state.attach_interactive(id, true);
                id
            };
            let mut backend = os.clone();
            let raw = yinz_ipc::consts::ipc_connect(&socket).unwrap();
            let mut sender =
                IpcSenderWithContext::<ClientToServerMsg>::new(raw.try_clone().unwrap());
            let mut reader = sender.get_receiver::<ServerToClientMsg>().unwrap();
            reader.set_recv_timeout(Some(ROUTER_TEST_TIMEOUT)).unwrap();
            let receiver = backend.new_client(id, listener.accept().unwrap()).unwrap();
            receiver
                .bind_server_attachment(id.generation().unwrap())
                .unwrap();
            let router = thread::spawn({
                let state = state.clone();
                let data = data.clone();
                let bus = threads.to_server.clone();
                move || {
                    route_thread_main(
                        data,
                        state,
                        Box::new(backend),
                        bus,
                        receiver,
                        id,
                        ROUTER_TEST_TIMEOUT,
                    )
                }
            });
            os.send_to_client(
                id,
                ServerToClientMsg::Connected {
                    native_render_revision: Some(1),
                    warp_companions: false,
                    warp_held_terminals: false,
                    warp_agent_tracking: false,
                    warp_terminal_state: 0,
                    signal_quit: false,
                    update_protocol: 0,
                    server_version: String::new(),
                    server_host: String::new(),
                },
            )
            .unwrap();
            assert!(matches!(
                next_native(&mut reader),
                Some(ServerToClientMsg::Connected { .. })
            ));
            sender
                .send_client_msg(ClientToServerMsg::NativeRenderRequest(Request::Offer(
                    Offer::reader(None),
                )))
                .unwrap();
            let ServerInstruction::NativeRenderRequest(owner, Request::Offer(offer)) = next()
            else {
                panic!("expected Offer")
            };
            assert_eq!(owner, id);
            let agreement = offer.select(None).unwrap();
            os.send_to_client(
                id,
                ServerToClientMsg::NativeRenderResult(Decision::Selected(agreement.clone())),
            )
            .unwrap();
            assert!(matches!(
                next_native(&mut reader),
                Some(ServerToClientMsg::NativeRenderResult(Decision::Selected(_)))
            ));
            // Each attachment owns its budget, so only its own ACKs can return it.
            let mut render = NativeRenderState::negotiated(
                agreement,
                Some(MediaRoot::open(dir.path()).unwrap()),
                Some(MediaBudget::viewer()),
            );
            render.seq = 1;
            let Staging::Shared(path) = render.stage_image(&vec![3; 256 * 1024]) else {
                panic!("expected a staged image")
            };
            let mut frame = FrameBuilder::new(1, 1, 1);
            frame.set_full_repaint(true);
            os.send_to_client(
                id,
                ServerToClientMsg::RenderFrame {
                    frame: frame.finish(),
                },
            )
            .unwrap();
            let received = next_native(&mut reader);
            assert!(
                matches!(received, Some(ServerToClientMsg::RenderFrame { .. })),
                "frame one: {received:?}"
            );
            (id, sender, reader, raw, router, render, path)
        };
        let (old, mut old_sender, mut old_reader, _raw, old_router, mut old_render, old_path) =
            attach();
        os.send_to_client(
            old,
            ServerToClientMsg::RenderFrame {
                frame: Vec::with_capacity(64 * 1024 * 1024 + 1),
            },
        )
        .unwrap_err();
        assert!(matches!(
            next_native(&mut old_reader),
            Some(ServerToClientMsg::Exit { .. })
        ));
        assert!(matches!(
            old_reader.recv_native_server_msg(None),
            IpcRecv::Eof
        ));
        let mut drains = HashMap::new();
        crate::observe_native_refusal(old, &os, &state, &data, &mut drains).unwrap();
        let ScreenInstruction::SealNativePrefix(owner, high) =
            threads.screen_receiver.try_recv().unwrap().0
        else {
            panic!("missing seal")
        };
        assert_eq!((owner, high), (old, 1));
        old_render.retain_prefix(high);
        let mut retired = HashMap::from([(old, old_render)]);
        let (new, mut sender, mut reader, mut wire, router, render, new_path) = attach();
        let mut active = HashMap::from([(new, render)]);
        let held = MediaUsage {
            entries: 1,
            bytes: 256 * 1024,
        };
        let usage = |states: &HashMap<_, NativeRenderState>, id| states[&id].media_usage();
        assert_eq!(usage(&active, new), held);
        assert_eq!(usage(&retired, old), held);
        assert_eq!(old.get(), new.get());
        assert_ne!(old, new);
        let before = os.native_drain(old).unwrap();
        let ack = |expected, seq| {
            assert!(
                matches!(next(), ServerInstruction::NativeFrameAck(client, number) if client == expected && number == seq)
            );
            crate::route_native_ack(expected, seq, &os, &data).unwrap();
        };
        // Encoded RenderFrameAck(1), followed by A's full top-level generation on B.
        let mut replay = vec![0xda, 2, 2, 8, 1, 0xf2, 2, 16];
        replay.extend_from_slice(&old.generation().unwrap().0);
        send_raw_action(&mut wire, &replay);
        sender
            .send_client_msg(ClientToServerMsg::RenderFrameAck { seq: 2 })
            .unwrap();
        ack(new, 2);
        old_sender
            .send_client_msg(ClientToServerMsg::RenderFrameAck { seq: 2 })
            .unwrap();
        ack(old, 2);
        assert!(threads.screen_receiver.try_recv().is_err());
        assert_eq!(os.native_drain(old), Some(before));
        assert!(old_path.exists() && new_path.exists());
        assert_eq!(usage(&active, new), held);
        if acknowledge_source {
            old_sender
                .send_client_msg(ClientToServerMsg::RenderFrameAck { seq: 1 })
                .unwrap();
            ack(old, 1);
            let ScreenInstruction::RenderFrameAck { client_id, seq } =
                threads.screen_receiver.try_recv().unwrap().0
            else {
                panic!("missing A ACK")
            };
            assert!(NativeRenderState::acknowledge_recipient(
                &mut active,
                &mut retired,
                client_id,
                seq
            )
            .is_empty());
            assert!(!old_path.exists() && new_path.exists());
            assert_eq!(usage(&retired, old), MediaUsage::default());
            assert_eq!(usage(&active, new), held);
            old_sender
                .send_client_msg(ClientToServerMsg::ClientExited)
                .unwrap();
        } else {
            crate::native_drain_deadline(&mut drains, &os, before.1 + Duration::from_secs(31));
        }
        wait_for_router_remove(&threads, old);
        old_router.join().unwrap().unwrap();
        os.clone().remove_client(old).unwrap();
        state.write().unwrap().remove_client(old);
        retired.remove(&old);
        assert!(!old_path.exists() && new_path.exists());
        assert_eq!(usage(&active, new), held);
        assert!(os.abort_native_drain(old, Some(before.1)));
        sender
            .send_client_msg(ClientToServerMsg::RenderFrameAck { seq: 1 })
            .unwrap();
        ack(new, 1);
        let ScreenInstruction::RenderFrameAck { client_id, seq } =
            threads.screen_receiver.try_recv().unwrap().0
        else {
            panic!("missing B ACK")
        };
        NativeRenderState::acknowledge_recipient(&mut active, &mut retired, client_id, seq);
        assert!(!new_path.exists());
        assert_eq!(usage(&active, new), MediaUsage::default());
        os.send_to_client(
            new,
            ServerToClientMsg::RenderFrame {
                frame: FrameBuilder::new(1, 1, 2).finish(),
            },
        )
        .unwrap();
        assert!(matches!(
            next_native(&mut reader),
            Some(ServerToClientMsg::RenderFrame { .. })
        ));
        sender
            .send_client_msg(ClientToServerMsg::ClientExited)
            .unwrap();
        wait_for_router_remove(&threads, new);
        router.join().unwrap().unwrap();
    }
}
