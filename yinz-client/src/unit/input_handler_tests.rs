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
