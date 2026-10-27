use super::*;
use crate::os_input_output::ServerConnection;
use crate::test_support::FakeClientOsApi;
use yinz_runtime::channels;
use yinz_types::internal_ids::SessionName;

#[test]
fn configured_mouse_mode_preserves_manual_toggles_until_configuration_changes() {
    for configured in [None, Some(true), Some(false)] {
        let (client_tx, _client_rx) = channels::unbounded();
        let (_input_tx, input_rx) = channels::unbounded();
        let reannounce = NestedReannounce::spawn(
            Box::new(FakeClientOsApi::default()),
            SessionName::from("mouse-mode-test"),
        );
        reannounce.stop();
        let mut handler = InputHandler::new(
            Box::new(FakeClientOsApi::default()),
            CommandIsExecuting::new(),
            Options {
                mouse_mode: configured,
                ..Default::default()
            },
            SenderWithContext::new(client_tx),
            input_rx,
            reannounce,
        );
        let enabled = configured.unwrap_or(true);
        handler.mouse_mode_active = enabled;
        handler.dispatch_action(Action::ToggleMouseMode, None);
        assert_eq!(handler.mouse_mode_active, !enabled);
        for _ in 0..2 {
            handler.set_configured_mouse_mode(enabled);
            assert_eq!(handler.mouse_mode_active, !enabled);
        }
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.options.mouse_mode, Some(!enabled));
        handler.dispatch_action(Action::ToggleMouseMode, None);
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.mouse_mode_active, enabled);
        handler.set_configured_mouse_mode(enabled);
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.mouse_mode_active, !enabled);
    }
}

#[test]
fn text_area_reports_carry_the_grid_observed_by_the_reporting_client() {
    use yinz_ipc::ipc::PixelDimensions;
    use yinz_types::pane_size::{Size, SizeInPixels};
    let os_api = FakeClientOsApi::default();
    let (client_tx, _client_rx) = channels::unbounded();
    let (_input_tx, input_rx) = channels::unbounded();
    let reannounce = NestedReannounce::spawn(
        Box::new(os_api.clone()),
        SessionName::from("pixel-report-test"),
    );
    reannounce.stop();
    let mut handler = InputHandler::new(
        Box::new(os_api.clone()),
        CommandIsExecuting::new(),
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        reannounce,
    );
    let area = SizeInPixels {
        width: 800,
        height: 480,
    };
    handler.handle_stdin_ansi_instruction(AnsiStdinInstruction::PixelDimensions(PixelDimensions {
        text_area_size: Some(area),
        ..Default::default()
    }));
    assert_eq!(
        os_api.sent_messages(),
        vec![ClientToServerMsg::TerminalPixelDimensions {
            pixel_dimensions: PixelDimensions {
                text_area_size: Some(area),
                text_area_grid_size: Some(Size { cols: 80, rows: 24 }),
                character_cell_size: None,
            }
        }]
    );
}

#[test]
fn offline_and_stale_keyboard_paste_and_mouse_input_are_discarded_after_replacement() {
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new("host".into(), command.clone());
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    let key = KeyWithModifier::new(yinz_types::keys::BareKey::Char('x'));
    for stamp in [(0, true), (0, false)] {
        for instruction in [
            InputInstruction::KeyWithModifierEvent(key.clone(), b"x".to_vec(), false),
            InputInstruction::KeyEvent(InputEvent::Paste("OFFLINE".into()), Vec::new()),
            InputInstruction::KeyEvent(
                InputEvent::Mouse(TermwizMouseEvent {
                    x: 10,
                    y: 10,
                    mouse_buttons: MouseButtons::LEFT,
                    modifiers: Modifiers::NONE,
                }),
                Vec::new(),
            ),
        ] {
            input_tx
                .send(InputInstruction::FromGeneration(
                    stamp,
                    Box::new(instruction),
                ))
                .unwrap();
        }
    }
    input_tx
        .send(InputInstruction::FromGeneration(
            (1, true),
            Box::new(InputInstruction::KeyEvent(
                InputEvent::Paste("CURRENT".into()),
                Vec::new(),
            )),
        ))
        .unwrap();
    input_tx.send(InputInstruction::Exit).unwrap();
    recovery.control.lost(0, "EOF".into());
    recovery.control.replacing();
    recovery.control.repainted();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "input-recovery".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        nested,
    )
    .handle_input();
    assert_eq!(
        os.sent_messages(),
        vec![ClientToServerMsg::Paste("CURRENT".into())]
    );
}

#[test]
fn offline_focus_changes_survive_recovery_and_queued_generation_changes() {
    use crate::remote_recovery::Phase;

    for phase in [Phase::Reconnecting, Phase::Repainting, Phase::Connected] {
        for (focused, final_focus) in [
            (false, InputInstruction::HostTerminalFocusChanged(false)),
            (
                false,
                InputInstruction::KeyEvent(InputEvent::FocusLost, Vec::new()),
            ),
            (true, InputInstruction::HostTerminalFocusChanged(true)),
            (
                true,
                InputInstruction::KeyEvent(InputEvent::FocusGained, Vec::new()),
            ),
        ] {
            let os = FakeClientOsApi::new();
            let command = CommandIsExecuting::new();
            let recovery = crate::remote_recovery::Recovery::new("host".into(), command.clone());
            os.set_remote_recovery(Some(recovery.control.clone()))
                .unwrap();
            os.send_to_server(ClientToServerMsg::HostTerminalFocusChanged { focused: true });
            recovery.control.lost(0, "EOF".into());
            if phase != Phase::Reconnecting {
                recovery.control.replacing();
            }
            if phase == Phase::Connected {
                recovery.control.repainted();
            }
            let (client_tx, _client_rx) = channels::unbounded();
            let (input_tx, input_rx) = channels::unbounded();
            let input_tx = SenderWithContext::new(input_tx);
            // Both focus representations describe this terminal even when queued
            // before a generation change or read during the outage.
            for (stamp, instruction) in [
                ((0, true), InputInstruction::HostTerminalFocusChanged(false)),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::FocusGained, Vec::new()),
                ),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::FocusLost, Vec::new()),
                ),
                ((0, false), final_focus),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::Paste("OFFLINE".into()), Vec::new()),
                ),
            ] {
                input_tx
                    .send(InputInstruction::FromGeneration(
                        stamp,
                        Box::new(instruction),
                    ))
                    .unwrap();
            }
            input_tx.send(InputInstruction::Exit).unwrap();
            let nested = NestedReannounce::spawn(Box::new(os.clone()), "focus-recovery".into());
            nested.stop();
            InputHandler::new(
                Box::new(os.clone()),
                command,
                Options::default(),
                SenderWithContext::new(client_tx),
                input_rx,
                nested,
            )
            .handle_input();
            let before_replay = os.sent_messages();
            assert!(!before_replay
                .iter()
                .any(|msg| matches!(msg, ClientToServerMsg::Paste(_))));
            if phase == Phase::Reconnecting {
                assert_eq!(before_replay.len(), 1, "offline focus must only be cached");
                recovery.control.replacing();
            }
            recovery.control.replay(&os);
            let replayed: Vec<_> = os.sent_messages()[before_replay.len()..]
                .iter()
                .filter_map(|msg| match msg {
                    ClientToServerMsg::HostTerminalFocusChanged { focused } => Some(*focused),
                    _ => None,
                })
                .collect();
            assert_eq!(replayed, vec![focused], "{phase:?}");
        }
    }
}

#[test]
fn fresh_keyboard_paste_and_ctrl_c_after_recovery_reach_the_healthy_server() {
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new("host".into(), command.clone());
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    recovery.control.lost(0, "outage".into());
    recovery.control.replacing();
    recovery.control.repainted();
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    let key = KeyWithModifier::new(yinz_types::keys::BareKey::Char('x'));
    let ctrl_c = KeyWithModifier::new(yinz_types::keys::BareKey::Char('c')).with_ctrl_modifier();
    for instruction in [
        InputInstruction::KeyWithModifierEvent(key.clone(), b"x".to_vec(), false),
        InputInstruction::KeyEvent(InputEvent::Paste("fresh".into()), Vec::new()),
        InputInstruction::KeyWithModifierEvent(ctrl_c.clone(), vec![3], false),
    ] {
        input_tx
            .send(InputInstruction::FromGeneration(
                (1, true),
                Box::new(instruction),
            ))
            .unwrap();
    }
    input_tx.send(InputInstruction::Exit).unwrap();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "fresh-input".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        nested,
    )
    .handle_input();
    assert!(!recovery.interrupted());
    assert_eq!(
        os.sent_messages(),
        vec![
            ClientToServerMsg::Key {
                key,
                raw_bytes: b"x".to_vec(),
                is_kitty_keyboard_protocol: false
            },
            ClientToServerMsg::Paste("fresh".into()),
            ClientToServerMsg::Key {
                key: ctrl_c,
                raw_bytes: vec![3],
                is_kitty_keyboard_protocol: false
            },
        ]
    );
}

#[test]
fn ctrl_c_during_recovery_cancels_locally_and_is_not_sent_to_the_server() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new("host".into(), command.clone());
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    recovery.control.lost(0, "EOF".into());
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    input_tx
        .send(InputInstruction::FromGeneration(
            (0, false),
            Box::new(InputInstruction::KeyWithModifierEvent(
                KeyWithModifier::new(yinz_types::keys::BareKey::Char('c')).with_ctrl_modifier(),
                b"\x1b[99;5u".to_vec(),
                true,
            )),
        ))
        .unwrap();
    input_tx.send(InputInstruction::Exit).unwrap();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "input-cancel".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        nested,
    )
    .handle_input();
    assert!(recovery.interrupted());
    assert!(os.sent_messages().is_empty());
}
