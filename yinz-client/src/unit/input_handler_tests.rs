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
