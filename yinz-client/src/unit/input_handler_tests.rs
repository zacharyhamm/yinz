use super::*;
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
