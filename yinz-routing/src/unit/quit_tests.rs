use super::*;
use std::time::Duration;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::internal_ids::PluginId;

#[test]
fn quit_confirmation_depends_on_input_origin() {
    let client = ClientId::new(1);
    let plugin_pane = Some(PaneId::Plugin(PluginId::new(7)));
    for (origin, is_client_input, cli_client, pane, confirm) in [
        ("keybinding", true, None, None, true),
        ("CLI", true, Some(ClientId::new(2)), None, false),
        ("headless plugin", false, None, None, false),
        ("plugin pane", false, None, plugin_pane, false),
        ("context menu", true, None, plugin_pane, false),
    ] {
        let (screen_tx, screen_rx) = channels::unbounded();
        let (server_tx, server_rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_server: Some(SenderWithContext::new(server_tx)),
            ..Default::default()
        }
        .silently_fail_on_send();
        let receiver = std::thread::spawn(move || {
            let (routed_client, completion) = crossbeam::select! {
                recv(screen_rx) -> instruction => {
                    assert!(confirm, "{origin} must quit immediately");
                    match instruction.unwrap().0 {
                        ScreenInstruction::ConfirmQuit(client, completion) => (client, completion),
                        other => panic!("unexpected screen instruction: {other:?}"),
                    }
                },
                recv(server_rx) -> instruction => {
                    assert!(!confirm, "{origin} must ask for confirmation");
                    match instruction.unwrap().0 {
                        ServerInstruction::ClientExit(client, completion) => (client, completion),
                        other => panic!("unexpected server instruction: {other:?}"),
                    }
                },
                default(Duration::from_secs(5)) => panic!("{origin} quit was not routed"),
            };
            assert_eq!(routed_client, client);
            assert!(completion.is_some());
        });
        let (should_break, result) = route_action(
            Action::Quit,
            client,
            cli_client,
            pane,
            senders,
            None,
            None,
            InputMode::Normal,
            None,
            false,
            is_client_input,
        )
        .unwrap();
        receiver.join().unwrap();
        assert_eq!(should_break, !confirm, "{origin}");
        assert!(result.unwrap().confirmed, "{origin}");
    }
}
