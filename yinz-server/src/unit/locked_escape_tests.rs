#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);

use super::*;
use crate::capturing_os::CapturingOs;
use crate::plugins::PluginInstruction;
use crate::thread_bus::ThreadSenders;
use std::collections::HashMap;
use std::str::FromStr;
use yinz_config::config::Config;
use yinz_runtime::channels;
use yinz_types::data::Event;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::internal_ids::{ClientId, TerminalId};

/// The default window, as `SessionConfiguration` resolves it for an unconfigured session.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS);

fn key(value: &str) -> KeyWithModifier {
    KeyWithModifier::from_str(value).unwrap()
}

fn session() -> SessionMetaData {
    SessionMetaData {
        shutdown: Default::default(),
        senders: ThreadSenders {
            should_silently_fail: true,
            ..Default::default()
        },
        data_access: std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
            std::path::Path::new("/unused"),
        )),
        default_shell: None,
        current_input_modes: HashMap::from([
            (ClientId::new(1), InputMode::Locked),
            (ClientId::new(2), InputMode::Locked),
        ]),
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
    }
}

/// The plugin half of an unlock: the mode switch is recorded as an action the plugins can see,
/// and every subscribed plugin is woken with `InputReceived`, in that order and nothing else.
fn assert_unlock_reported_to_plugins(
    plugin_rx: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) {
    match plugin_rx
        .try_recv()
        .expect("the unlock was not logged as an action")
        .0
    {
        PluginInstruction::UserInput {
            client_id,
            action: Action::SwitchToMode { input_mode },
            terminal_id: None,
            cli_client_id: None,
        } => {
            assert_eq!(client_id, CLIENT_1);
            assert_eq!(input_mode, InputMode::Normal);
        },
        other => panic!("expected the unlock to be logged as UserInput, got {other:?}"),
    }
    match plugin_rx
        .try_recv()
        .expect("the plugins were not told about the input")
        .0
    {
        PluginInstruction::Update(updates) => {
            assert!(
                matches!(
                    updates.as_slice(),
                    [(None, Some(CLIENT_1), Event::InputReceived)]
                ),
                "expected one InputReceived for client 1, got {updates:?}"
            );
        },
        other => panic!("expected an InputReceived update, got {other:?}"),
    }
    assert!(
        plugin_rx.try_recv().is_err(),
        "an unlock sends the plugins exactly two instructions"
    );
}

#[test]
fn locked_escape_matches_only_the_standard_unlock_binding() {
    let unlock = Action::SwitchToMode {
        input_mode: InputMode::Normal,
    };
    assert!(starts_escape(
        &key("Ctrl g"),
        InputMode::Locked,
        std::slice::from_ref(&unlock)
    ));
    for mode in [InputMode::Normal, InputMode::Pane] {
        assert!(!starts_escape(
            &key("Ctrl g"),
            mode,
            std::slice::from_ref(&unlock)
        ));
    }
    for value in ["g", "Alt g", "Ctrl Alt g", "Ctrl Shift g", "Ctrl h"] {
        assert!(!starts_escape(
            &key(value),
            InputMode::Locked,
            std::slice::from_ref(&unlock)
        ));
    }
    for actions in [
        vec![],
        vec![Action::NoOp],
        vec![unlock.clone(), Action::NoOp],
        // the binding must lead *out* of locked mode into Normal: a user who bound Ctrl+g in
        // locked mode to some other mode must get that mode, not a swallowed prefix
        vec![Action::SwitchToMode {
            input_mode: InputMode::Pane,
        }],
        vec![Action::SwitchToMode {
            input_mode: InputMode::Locked,
        }],
    ] {
        assert!(!starts_escape(&key("Ctrl g"), InputMode::Locked, &actions));
    }
}

#[test]
fn locked_escape_routing_respects_custom_bindings_and_explicit_passthrough() {
    for (config, passthrough, should_hold) in [
        (Config::from_default_assets().unwrap(), false, true),
        (Config::from_default_assets().unwrap(), true, false),
        (Config::default(), false, false),
        (
            Config::from_kdl(
                "keybinds { locked { bind \"Ctrl g\" { Write 7; }; }; }",
                None,
            )
            .unwrap(),
            false,
            false,
        ),
    ] {
        let mut session = session();
        session
            .session_configuration
            .set_saved_configuration(config);
        if passthrough {
            session.key_passthrough_clients.insert(
                ClientId::new(1),
                crate::panes::PaneId::Terminal(TerminalId::new(1)),
            );
        }
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        let result = prepare_input(
            ClientToServerMsg::Key {
                key: key("Ctrl g"),
                raw_bytes: vec![7],
                is_kitty_keyboard_protocol: false,
            },
            &session_data,
            &SenderWithContext::new(tx),
            ClientId::new(1),
        )
        .unwrap();
        assert_eq!(result.is_none(), should_hold);
        assert_eq!(
            session_data
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .locked_escapes
                .contains_key(&ClientId::new(1)),
            should_hold
        );
    }
}

/// The config's `locked_escape_timeout_ms` sizes the window, and 0 turns the prefix off
/// entirely: Ctrl+g is handed on to its binding rather than held.
#[test]
fn locked_escape_window_follows_the_configured_timeout() {
    for (kdl, expected) in [
        ("", Some(DEFAULT_TIMEOUT)),
        (
            "locked_escape_timeout_ms 1000",
            Some(Duration::from_secs(1)),
        ),
        ("locked_escape_timeout_ms 0", None),
    ] {
        let mut session = session();
        let config = Config::from_kdl(kdl, Some(Config::from_default_assets().unwrap())).unwrap();
        session
            .session_configuration
            .set_saved_configuration(config);
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        let before = Instant::now();
        let result = prepare_input(
            ClientToServerMsg::Key {
                key: key("Ctrl g"),
                raw_bytes: vec![7],
                is_kitty_keyboard_protocol: false,
            },
            &session_data,
            &SenderWithContext::new(tx),
            ClientId::new(1),
        )
        .unwrap();
        let guard = session_data.read().unwrap();
        let session = guard.as_ref().unwrap();
        match expected {
            Some(timeout) => {
                assert!(result.is_none(), "{kdl:?} should hold the key");
                let deadline = session.locked_escapes[&ClientId::new(1)].deadline;
                assert!(
                    deadline >= before + timeout && deadline <= Instant::now() + timeout,
                    "{kdl:?} deadline is not {timeout:?} out"
                );
            },
            None => {
                assert!(
                    matches!(result, Some(ClientToServerMsg::Key { .. })),
                    "{kdl:?} should pass the key straight through"
                );
                assert!(session.locked_escapes.is_empty());
                assert_eq!(
                    session.current_input_modes[&ClientId::new(1)],
                    InputMode::Locked
                );
            },
        }
    }
}

#[test]
fn locked_escape_deadline_and_modifiers_are_exact() {
    let now = Instant::now();
    let pending = LockedEscape::new(now, DEFAULT_TIMEOUT);
    assert!(pending.completes(&key("g"), now));
    assert!(pending.completes(&key("g"), pending.deadline - Duration::from_nanos(1)));
    assert!(!pending.completes(&key("g"), pending.deadline));
    assert!(!pending.completes(&key("g"), pending.deadline + Duration::from_nanos(1)));
    for value in ["G", "Shift g", "Ctrl g", "Alt g", "x"] {
        assert!(!pending.completes(&key(value), now));
    }
}

#[test]
fn locked_escape_resolution_is_per_client_and_stale_tokens_are_harmless() {
    let mut session = session();
    let (screen_tx, screen_rx) = channels::unbounded();
    session.senders.to_screen = Some(SenderWithContext::new(screen_tx));
    let (plugin_tx, plugin_rx) = channels::unbounded();
    session.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let old = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    let new = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId::new(1), new);
    session.locked_escapes.insert(ClientId::new(2), old);
    session
        .resolve_locked_escape(ClientId::new(1), old.token)
        .unwrap();
    assert!(screen_rx.try_recv().is_err());
    assert!(plugin_rx.try_recv().is_err());
    assert_eq!(
        session.current_input_modes[&ClientId::new(1)],
        InputMode::Locked
    );
    assert_eq!(session.locked_escapes[&ClientId::new(1)].token, new.token);
    session
        .resolve_locked_escape(ClientId::new(1), new.token)
        .unwrap();
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::CloseContextMenu(CLIENT_1)
    ));
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::ChangeMode(InputMode::Normal, _, CLIENT_1, _)
    ));
    assert!(screen_rx.try_recv().is_err());
    assert_unlock_reported_to_plugins(&plugin_rx);
    assert_eq!(
        session.current_input_modes[&ClientId::new(1)],
        InputMode::Normal
    );
    assert_eq!(
        session.current_input_modes[&ClientId::new(2)],
        InputMode::Locked
    );
    assert!(!session.locked_escapes.contains_key(&ClientId::new(1)));
    session
        .change_input_mode(ClientId::new(1), InputMode::Locked, None)
        .unwrap();
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::ChangeMode(InputMode::Locked, _, CLIENT_1, _)
    ));
    session
        .resolve_locked_escape(ClientId::new(1), new.token)
        .unwrap();
    assert!(screen_rx.try_recv().is_err());
    assert!(plugin_rx.try_recv().is_err());
    assert_eq!(
        session.current_input_modes[&ClientId::new(1)],
        InputMode::Locked
    );
}

/// The timer is the only thing that unlocks an abandoned prefix on the clock: without it a user
/// who presses Ctrl+g and walks away stays visibly locked until the next keystroke. It reports
/// to the server thread with the token of the escape it was armed for, so a prefix that was
/// resolved or replaced in the meantime is not touched.
#[test]
fn locked_escape_expiry_timer_reports_to_the_server_with_its_token() {
    let mut session = session();
    let config = Config::from_kdl(
        "locked_escape_timeout_ms 20",
        Some(Config::from_default_assets().unwrap()),
    )
    .unwrap();
    session
        .session_configuration
        .set_saved_configuration(config);
    let session_data = Arc::new(RwLock::new(Some(session)));
    let (tx, rx) = channels::unbounded();
    let result = prepare_input(
        ClientToServerMsg::Key {
            key: key("Ctrl g"),
            raw_bytes: vec![7],
            is_kitty_keyboard_protocol: false,
        },
        &session_data,
        &SenderWithContext::new(tx),
        CLIENT_1,
    )
    .unwrap();
    assert!(result.is_none(), "the key must be held as a prefix");
    let armed_token = session_data
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .locked_escapes[&CLIENT_1]
        .token;

    let (instruction, _) = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the expiry timer never reported to the server");
    match instruction {
        ServerInstruction::ResolveLockedEscape(client_id, token) => {
            assert_eq!(client_id, CLIENT_1);
            assert_eq!(
                token, armed_token,
                "the timer resolves the escape it was armed for"
            );
        },
        other => panic!("expected ResolveLockedEscape, got {other:?}"),
    }
}

/// A client's route thread ending - the peer closed its connection - drops that client's
/// pending escape before the server is told to remove the client, since the id is reused by
/// the next connection and a stale prefix would then hold *its* first Ctrl+g.
#[test]
fn a_client_disconnect_cancels_its_pending_escape() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::IpcReceiverWithContext;

    let mut session = session();
    session.locked_escapes.insert(
        CLIENT_1,
        LockedEscape::new(Instant::now(), Duration::from_secs(60)),
    );
    let session_data = Arc::new(RwLock::new(Some(session)));
    let session_state = Arc::new(RwLock::new(crate::SessionState::new()));
    let (to_server, server_rx) = channels::unbounded();

    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("client.sock");
    let listener = ListenerOptions::new()
        .name(
            socket_path
                .as_path()
                .to_fs_name::<GenericFilePath>()
                .unwrap(),
        )
        .create_sync()
        .unwrap();
    let client = interprocess::local_socket::Stream::connect(
        socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .unwrap(),
    )
    .unwrap();
    let accepted = listener.incoming().next().unwrap().unwrap();
    // the client says nothing and goes away, which the route thread reads as EOF
    drop(client);

    let os = CapturingOs::default();
    crate::route::route_thread_main(
        session_data.clone(),
        session_state,
        Box::new(os.clone()),
        SenderWithContext::new(to_server),
        IpcReceiverWithContext::new(accepted),
        CLIENT_1,
        Duration::from_secs(5),
    )
    .unwrap();

    assert!(
        session_data
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .locked_escapes
            .is_empty(),
        "the disconnected client's escape is still pending"
    );
    let (instruction, _) = server_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the route thread did not tell the server the client left");
    assert!(
        matches!(instruction, ServerInstruction::RemoveClient(CLIENT_1)),
        "expected RemoveClient, got {instruction:?}"
    );
    // there is nobody left to write to: not even the unblock that follows a handled message
    let sent = os.server_to_client_messages.lock();
    assert!(
        sent.is_empty(),
        "a client that only hung up was sent {sent:?}"
    );
}

/// The four `SessionMetaData` helpers that cancel a pending escape. The two other cancel
/// sites - the `KeyPassthroughChanged` server arm and a client's route thread ending - are
/// driven elsewhere: the second by `a_client_disconnect_cancels_its_pending_escape`.
#[test]
fn locked_escape_is_cancelled_by_mode_configuration_and_passthrough_changes() {
    let mut session = session();
    let pending = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId::new(1), pending);
    session
        .change_input_mode(ClientId::new(1), InputMode::Pane, None)
        .unwrap();
    assert!(!session.locked_escapes.contains_key(&ClientId::new(1)));
    session.locked_escapes.insert(ClientId::new(1), pending);
    session.change_mode_for_all_clients(InputMode::Locked);
    assert!(session.locked_escapes.is_empty());
    session.locked_escapes.insert(ClientId::new(1), pending);
    session.propagate_configuration_changes(vec![(ClientId::new(1), Config::default())], false);
    assert!(!session.locked_escapes.contains_key(&ClientId::new(1)));
    session.locked_escapes.insert(ClientId::new(1), pending);
    session.remove_key_passthrough_client(ClientId::new(1));
    session
        .resolve_locked_escape(ClientId::new(1), pending.token)
        .unwrap();
    assert_eq!(
        session.current_input_modes[&ClientId::new(1)],
        InputMode::Locked
    );
}

#[test]
fn locked_escape_completion_writes_raw_ctrl_g_for_both_encodings() {
    for is_kitty in [false, true] {
        let mut session = session();
        // The subject is byte routing, not the real clock or scheduler.
        session.locked_escapes.insert(
            ClientId::new(1),
            LockedEscape::new(Instant::now(), Duration::from_secs(60)),
        );
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        let result = prepare_input(
            ClientToServerMsg::Key {
                key: key("g"),
                raw_bytes: if is_kitty {
                    b"\x1b[103u".to_vec()
                } else {
                    vec![b'g']
                },
                is_kitty_keyboard_protocol: is_kitty,
            },
            &session_data,
            &SenderWithContext::new(tx),
            ClientId::new(1),
        )
        .unwrap();
        assert!(matches!(result, Some(ClientToServerMsg::Action {
            action: Action::Write { bytes, key_with_modifier: None, is_kitty_keyboard_protocol: false }, ..
        }) if bytes == [7]));
        let guard = session_data.read().unwrap();
        let session = guard.as_ref().unwrap();
        assert_eq!(
            session.current_input_modes[&ClientId::new(1)],
            InputMode::Locked
        );
        assert!(session.locked_escapes.is_empty());
    }
}

/// A key or an interactive action that is not the completing `g` resolves the pending prefix
/// as an unlock (screen and plugins told, mode switched) and is then passed on *unchanged*. A
/// cli client's action - `yinz action` from a script - is passed on without touching the
/// prefix at all, so a script cannot cancel a user's half-typed escape.
#[test]
fn locked_escape_fallback_updates_mode_and_queues_unlock_before_returning_input() {
    let interactive_write = ClientToServerMsg::Action {
        action: Action::Write {
            bytes: b"g".to_vec(),
            key_with_modifier: None,
            is_kitty_keyboard_protocol: false,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    };
    let cli_write = match interactive_write.clone() {
        ClientToServerMsg::Action {
            action,
            terminal_id,
            client_id,
            ..
        } => ClientToServerMsg::Action {
            action,
            terminal_id,
            client_id,
            is_cli_client: true,
        },
        _ => unreachable!(),
    };
    for (instruction, should_resolve) in [
        (
            ClientToServerMsg::Key {
                key: key("Ctrl g"),
                raw_bytes: vec![7],
                is_kitty_keyboard_protocol: false,
            },
            true,
        ),
        (ClientToServerMsg::Paste("g".into()), true),
        (interactive_write, true),
        (cli_write, false),
    ] {
        let mut session = session();
        let pending = LockedEscape::new(Instant::now(), Duration::from_secs(60));
        session.locked_escapes.insert(ClientId::new(1), pending);
        let (screen_tx, screen_rx) = channels::unbounded();
        session.senders.to_screen = Some(SenderWithContext::new(screen_tx));
        let (plugin_tx, plugin_rx) = channels::unbounded();
        session.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        let returned = prepare_input(
            instruction.clone(),
            &session_data,
            &SenderWithContext::new(tx),
            ClientId::new(1),
        )
        .unwrap();
        assert_eq!(
            returned,
            Some(instruction.clone()),
            "the input is handed on exactly as it came in"
        );
        let guard = session_data.read().unwrap();
        let session = guard.as_ref().unwrap();
        if should_resolve {
            assert_eq!(
                session.current_input_modes[&ClientId::new(1)],
                InputMode::Normal
            );
            assert!(!session.locked_escapes.contains_key(&ClientId::new(1)));
            assert!(matches!(
                screen_rx.try_recv().unwrap().0,
                ScreenInstruction::CloseContextMenu(CLIENT_1)
            ));
            assert!(matches!(
                screen_rx.try_recv().unwrap().0,
                ScreenInstruction::ChangeMode(InputMode::Normal, _, CLIENT_1, _)
            ));
            assert!(screen_rx.try_recv().is_err());
            assert_unlock_reported_to_plugins(&plugin_rx);
        } else {
            assert_eq!(
                session.current_input_modes[&ClientId::new(1)],
                InputMode::Locked,
                "a cli client's action must not unlock the user"
            );
            assert_eq!(
                session.locked_escapes[&ClientId::new(1)].token,
                pending.token,
                "a cli client's action must leave the pending prefix alone"
            );
            assert!(screen_rx.try_recv().is_err());
            assert!(plugin_rx.try_recv().is_err());
        }
    }
}

#[test]
fn locked_escape_resize_does_not_resolve_or_extend_the_prefix() {
    let mut session = session();
    let pending = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId::new(1), pending);
    let session_data = Arc::new(RwLock::new(Some(session)));
    let (tx, _rx) = channels::unbounded();
    assert!(matches!(
        prepare_input(
            ClientToServerMsg::TerminalResize {
                new_size: yinz_types::pane_size::Size { rows: 24, cols: 80 },
            },
            &session_data,
            &SenderWithContext::new(tx),
            ClientId::new(1)
        )
        .unwrap(),
        Some(ClientToServerMsg::TerminalResize { .. })
    ));
    let guard = session_data.read().unwrap();
    let session = guard.as_ref().unwrap();
    assert_eq!(
        session.locked_escapes[&ClientId::new(1)].deadline,
        pending.deadline
    );
    assert_eq!(
        session.current_input_modes[&ClientId::new(1)],
        InputMode::Locked
    );
}

#[test]
fn command_palette_holds_input_locked_and_restores_lock_state_or_notification_browsing() {
    for original in [
        InputMode::Normal,
        InputMode::Locked,
        InputMode::Pane,
        InputMode::Scroll,
        InputMode::Notifications,
    ] {
        let mut session = session();
        let (screen_tx, screen_rx) = channels::unbounded();
        session.senders.to_screen = Some(SenderWithContext::new(screen_tx));
        session.current_input_modes.insert(CLIENT_1, original);
        let restored = if matches!(original, InputMode::Locked | InputMode::Notifications) {
            original
        } else {
            InputMode::Normal
        };
        for _ in 0..2 {
            session.set_command_palette_open(CLIENT_1, true).unwrap();
            // Repeated open notifications must not replace the saved state with Locked.
            session.set_command_palette_open(CLIENT_1, true).unwrap();
            assert_eq!(session.current_input_modes[&CLIENT_1], InputMode::Locked);
            assert_eq!(session.command_palette_restore_modes[&CLIENT_1], restored);
            for requested in [InputMode::Normal, InputMode::Pane, InputMode::Locked] {
                let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
                session
                    .change_input_mode(
                        CLIENT_1,
                        requested,
                        Some(yinz_bus::route::NotificationEnd::new(completion_tx)),
                    )
                    .unwrap();
                assert_eq!(session.current_input_modes[&CLIENT_1], InputMode::Locked);
                for (instruction, _) in screen_rx.try_iter() {
                    assert!(matches!(
                        instruction,
                        ScreenInstruction::ChangeMode(InputMode::Locked, _, CLIENT_1, _)
                    ));
                }
                assert!(
                    completion_rx.try_recv().is_ok(),
                    "blocked mode changes must still complete"
                );
                session.change_mode_for_all_clients(requested);
                assert_eq!(session.current_input_modes[&CLIENT_1], InputMode::Locked);
                assert_eq!(session.current_input_modes[&ClientId::new(2)], requested);
            }
            session.set_command_palette_open(CLIENT_1, false).unwrap();
            assert_eq!(session.current_input_modes[&CLIENT_1], restored);
            assert!(session.command_palette_restore_modes.is_empty());
            assert!(matches!(screen_rx.try_recv().unwrap().0,
                ScreenInstruction::ChangeMode(mode, _, CLIENT_1, _) if mode == restored));
            session.set_command_palette_open(CLIENT_1, false).unwrap();
            assert!(screen_rx.try_recv().is_err());
        }
    }
}

#[test]
fn command_palette_cancels_pending_unlocks_and_bypasses_ctrl_g_prefix() {
    for timeout_ms in [0, 250] {
        let mut session = session();
        let config = Config::from_kdl(
            &format!("locked_escape_timeout_ms {timeout_ms}"),
            Some(Config::from_default_assets().unwrap()),
        )
        .unwrap();
        session
            .session_configuration
            .set_saved_configuration(config);
        let pending = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
        session.locked_escapes.insert(CLIENT_1, pending);
        session.set_command_palette_open(CLIENT_1, true).unwrap();
        assert!(session.locked_escapes.is_empty());
        session
            .resolve_locked_escape(CLIENT_1, pending.token)
            .unwrap();
        assert_eq!(session.current_input_modes[&CLIENT_1], InputMode::Locked);
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        let to_server = SenderWithContext::new(tx);
        for (value, bytes) in [("Ctrl g", vec![7]), ("g", vec![b'g'])] {
            let result = prepare_input(
                ClientToServerMsg::Key {
                    key: key(value),
                    raw_bytes: bytes.clone(),
                    is_kitty_keyboard_protocol: false,
                },
                &session_data,
                &to_server,
                CLIENT_1,
            )
            .unwrap();
            assert!(
                matches!(result, Some(ClientToServerMsg::Key { key: actual, raw_bytes, .. })
                if actual == key(value) && raw_bytes == bytes)
            );
            let guard = session_data.read().unwrap();
            let session = guard.as_ref().unwrap();
            assert!(session.locked_escapes.is_empty());
            assert_eq!(session.current_input_modes[&CLIENT_1], InputMode::Locked);
        }
    }
}

#[test]
fn command_palette_disconnect_discards_restore_state_without_unlocking_other_clients() {
    let mut session = session();
    session
        .current_input_modes
        .insert(CLIENT_1, InputMode::Normal);
    session.set_command_palette_open(CLIENT_1, true).unwrap();
    session
        .set_command_palette_open(ClientId::new(2), true)
        .unwrap();
    let (screen_tx, screen_rx) = channels::unbounded();
    session.senders.to_screen = Some(SenderWithContext::new(screen_tx));
    let session_data = Arc::new(RwLock::new(Some(session)));
    let session_state = Arc::new(RwLock::new(crate::SessionState::new()));
    let mut os: Box<dyn crate::os_input_output::ServerOsApi> = Box::new(CapturingOs::default());
    crate::remove_client_and_flush_forwards(CLIENT_1, &mut os, &session_state, &session_data);
    let mut guard = session_data.write().unwrap();
    let session = guard.as_mut().unwrap();
    assert!(!session
        .command_palette_restore_modes
        .contains_key(&CLIENT_1));
    assert!(session
        .command_palette_restore_modes
        .contains_key(&ClientId::new(2)));
    // Screen's later dismissal notification must not restore the departed client's mode.
    session.set_command_palette_open(CLIENT_1, false).unwrap();
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::RemoveClient(CLIENT_1)
    ));
    assert!(screen_rx.try_recv().is_err());
    assert_eq!(
        session.current_input_modes[&ClientId::new(2)],
        InputMode::Locked
    );
    session
        .set_command_palette_open(ClientId::new(2), false)
        .unwrap();
    assert_eq!(
        session.current_input_modes[&ClientId::new(2)],
        InputMode::Locked
    );
}
