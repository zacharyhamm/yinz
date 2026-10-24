use super::*;
use std::time::Duration;
use yinz_runtime::channels::SenderWithContext;
use yinz_types::internal_ids::{PluginId, TerminalId};

#[test]
fn notifications_preserve_pane_context_and_plugin_origin() {
    let plugin_id = PluginId::new(7);
    for (pane_id, is_client_input, expected_plugin) in [
        (Some(PaneId::Terminal(TerminalId::new(42))), true, None),
        (None, true, None),
        (Some(PaneId::Plugin(plugin_id)), false, Some(plugin_id)),
    ] {
        let (screen_tx, screen_rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            should_silently_fail: true,
            ..Default::default()
        };
        let screen = std::thread::spawn(move || {
            let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let (text, on_click, completion) = match (expected_plugin, instruction) {
                (None, ScreenInstruction::Notify(text, target, on_click, completion)) => {
                    assert_eq!(target, pane_id);
                    (text, on_click, completion)
                },
                (
                    Some(expected_plugin),
                    ScreenInstruction::PluginNotify {
                        text,
                        pane_id,
                        on_click,
                        plugin_id,
                        completion,
                    },
                ) => {
                    assert_eq!(plugin_id, expected_plugin);
                    assert_eq!(pane_id, None);
                    (text, on_click, completion)
                },
                _ => panic!("unexpected notification instruction"),
            };
            assert_eq!(text, "task finished");
            assert!(on_click.is_none());
            assert!(completion.is_some());
        });
        let (_, result) = route_action(
            Action::Notify("task finished".into()),
            ClientId::new(1),
            is_client_input.then_some(ClientId::new(2)),
            pane_id,
            senders,
            None,
            None,
            InputMode::Normal,
            None,
            false,
            is_client_input,
        )
        .unwrap();
        screen.join().unwrap();
        assert!(result.unwrap().confirmed);
    }
}
