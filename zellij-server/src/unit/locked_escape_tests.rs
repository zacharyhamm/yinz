use super::*;
use crate::thread_bus::ThreadSenders;
use crate::TerminalId;
use std::collections::HashMap;
use std::str::FromStr;
use zellij_config::config::Config;
use zellij_runtime::channels;
use zellij_types::internal_ids::{ClientId, TerminalId};

/// The default window, as `SessionConfiguration` resolves it for an unconfigured session.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS);

fn key(value: &str) -> KeyWithModifier {
    KeyWithModifier::from_str(value).unwrap()
}

fn session() -> SessionMetaData {
    SessionMetaData {
        senders: ThreadSenders {
            should_silently_fail: true,
            ..Default::default()
        },
        default_shell: None,
        current_input_modes: HashMap::from([
            (ClientId(1), InputMode::Locked),
            (ClientId(2), InputMode::Locked),
        ]),
        session_configuration: Default::default(),
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        pty_writer_thread: None,
        background_jobs_thread: None,
        session_recorder_thread: None,
        config_file_path: None,
        config_dir: None,
    }
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
            session
                .key_passthrough_clients
                .insert(ClientId(1), crate::panes::PaneId::Terminal(TerminalId(1)));
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
            ClientId(1),
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
                .contains_key(&ClientId(1)),
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
            ClientId(1),
        )
        .unwrap();
        let guard = session_data.read().unwrap();
        let session = guard.as_ref().unwrap();
        match expected {
            Some(timeout) => {
                assert!(result.is_none(), "{kdl:?} should hold the key");
                let deadline = session.locked_escapes[&ClientId(1)].deadline;
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
                assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
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
    let old = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    let new = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId(1), new);
    session.locked_escapes.insert(ClientId(2), old);
    session
        .resolve_locked_escape(ClientId(1), old.token)
        .unwrap();
    assert!(screen_rx.try_recv().is_err());
    assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
    assert_eq!(session.locked_escapes[&ClientId(1)].token, new.token);
    session
        .resolve_locked_escape(ClientId(1), new.token)
        .unwrap();
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::CloseContextMenu(ClientId(1))
    ));
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::ChangeMode(InputMode::Normal, _, ClientId(1), _)
    ));
    assert!(screen_rx.try_recv().is_err());
    assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Normal);
    assert_eq!(session.current_input_modes[&ClientId(2)], InputMode::Locked);
    assert!(!session.locked_escapes.contains_key(&ClientId(1)));
    session
        .change_input_mode(ClientId(1), InputMode::Locked, None)
        .unwrap();
    assert!(matches!(
        screen_rx.try_recv().unwrap().0,
        ScreenInstruction::ChangeMode(InputMode::Locked, _, ClientId(1), _)
    ));
    session
        .resolve_locked_escape(ClientId(1), new.token)
        .unwrap();
    assert!(screen_rx.try_recv().is_err());
    assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
}

#[test]
fn locked_escape_is_cancelled_by_mode_configuration_and_disconnect_changes() {
    let mut session = session();
    let pending = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId(1), pending);
    session
        .change_input_mode(ClientId(1), InputMode::Pane, None)
        .unwrap();
    assert!(!session.locked_escapes.contains_key(&ClientId(1)));
    session.locked_escapes.insert(ClientId(1), pending);
    session.change_mode_for_all_clients(InputMode::Locked);
    assert!(session.locked_escapes.is_empty());
    session.locked_escapes.insert(ClientId(1), pending);
    session.propagate_configuration_changes(vec![(ClientId(1), Config::default())], false);
    assert!(!session.locked_escapes.contains_key(&ClientId(1)));
    session.locked_escapes.insert(ClientId(1), pending);
    session.remove_key_passthrough_client(ClientId(1));
    session
        .resolve_locked_escape(ClientId(1), pending.token)
        .unwrap();
    assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
}

#[test]
fn locked_escape_completion_writes_raw_ctrl_g_for_both_encodings() {
    for is_kitty in [false, true] {
        let mut session = session();
        // The subject is byte routing, not the real clock or scheduler.
        session.locked_escapes.insert(
            ClientId(1),
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
            ClientId(1),
        )
        .unwrap();
        assert!(matches!(result, Some(ClientToServerMsg::Action {
            action: Action::Write { bytes, key_with_modifier: None, is_kitty_keyboard_protocol: false }, ..
        }) if bytes == [7]));
        let guard = session_data.read().unwrap();
        let session = guard.as_ref().unwrap();
        assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
        assert!(session.locked_escapes.is_empty());
    }
}

#[test]
fn locked_escape_fallback_updates_mode_and_queues_unlock_before_returning_input() {
    for instruction in [
        ClientToServerMsg::Key {
            key: key("Ctrl g"),
            raw_bytes: vec![7],
            is_kitty_keyboard_protocol: false,
        },
        ClientToServerMsg::Action {
            action: Action::Write {
                bytes: b"g".to_vec(),
                key_with_modifier: None,
                is_kitty_keyboard_protocol: false,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: false,
        },
    ] {
        let mut session = session();
        session.locked_escapes.insert(
            ClientId(1),
            LockedEscape::new(Instant::now(), Duration::from_secs(60)),
        );
        let (screen_tx, screen_rx) = channels::unbounded();
        session.senders.to_screen = Some(SenderWithContext::new(screen_tx));
        let session_data = Arc::new(RwLock::new(Some(session)));
        let (tx, _rx) = channels::unbounded();
        assert!(prepare_input(
            instruction,
            &session_data,
            &SenderWithContext::new(tx),
            ClientId(1)
        )
        .unwrap()
        .is_some());
        assert_eq!(
            session_data
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .current_input_modes[&ClientId(1)],
            InputMode::Normal
        );
        assert!(matches!(
            screen_rx.try_recv().unwrap().0,
            ScreenInstruction::CloseContextMenu(ClientId(1))
        ));
        assert!(matches!(
            screen_rx.try_recv().unwrap().0,
            ScreenInstruction::ChangeMode(InputMode::Normal, _, ClientId(1), _)
        ));
    }
}

#[test]
fn locked_escape_resize_does_not_resolve_or_extend_the_prefix() {
    let mut session = session();
    let pending = LockedEscape::new(Instant::now(), DEFAULT_TIMEOUT);
    session.locked_escapes.insert(ClientId(1), pending);
    let session_data = Arc::new(RwLock::new(Some(session)));
    let (tx, _rx) = channels::unbounded();
    assert!(matches!(
        prepare_input(
            ClientToServerMsg::TerminalResize {
                new_size: zellij_types::pane_size::Size { rows: 24, cols: 80 },
            },
            &session_data,
            &SenderWithContext::new(tx),
            ClientId(1)
        )
        .unwrap(),
        Some(ClientToServerMsg::TerminalResize { .. })
    ));
    let guard = session_data.read().unwrap();
    let session = guard.as_ref().unwrap();
    assert_eq!(
        session.locked_escapes[&ClientId(1)].deadline,
        pending.deadline
    );
    assert_eq!(session.current_input_modes[&ClientId(1)], InputMode::Locked);
}
