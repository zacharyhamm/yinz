use super::*;
use std::time::Duration;
use yinz_runtime::channels::SenderWithContext;
use yinz_types::{
    internal_ids::PluginId,
    keys::{BareKey, KeyWithModifier},
};

#[test]
fn only_client_writes_require_an_input_barrier() {
    let pane_id = PaneId::Plugin(PluginId::new(100));
    let key = KeyWithModifier::new(BareKey::Char('x'));
    for is_client_input in [false, true] {
        for action in [
            Action::Write {
                key_with_modifier: Some(key.clone()),
                bytes: b"x".to_vec(),
                is_kitty_keyboard_protocol: false,
            },
            Action::Write {
                key_with_modifier: None,
                bytes: b"x".to_vec(),
                is_kitty_keyboard_protocol: false,
            },
            Action::Write {
                key_with_modifier: Some(key.clone()),
                bytes: vec![],
                is_kitty_keyboard_protocol: false,
            },
            Action::WriteChars { chars: "x".into() },
            Action::WriteToPaneId {
                bytes: b"x".to_vec(),
                pane_id,
            },
            Action::WriteCharsToPaneId {
                chars: "x".into(),
                pane_id,
            },
            Action::Paste {
                chars: "x".into(),
                pane_id: Some(pane_id),
            },
            Action::Paste {
                chars: "x".into(),
                pane_id: None,
            },
            Action::ToggleTab,
        ] {
            let expected_barrier = is_client_input
                && matches!(action, Action::Write { .. } | Action::WriteChars { .. });
            let expected_scroll_target = match &action {
                Action::WriteToPaneId { pane_id, .. }
                | Action::WriteCharsToPaneId { pane_id, .. } => Some(*pane_id),
                Action::Paste { pane_id, .. } => *pane_id,
                _ => None,
            };
            let clears_scroll = !matches!(action, Action::ToggleTab);
            let (screen_tx, screen_rx) = yinz_runtime::channels::unbounded();
            let senders = ThreadSenders {
                to_screen: Some(SenderWithContext::new(screen_tx)),
                should_silently_fail: true,
                ..Default::default()
            };
            let screen = std::thread::spawn(move || {
                if clears_scroll {
                    let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(matches!(instruction,
                        ScreenInstruction::ClearScroll(target, client)
                        if target == expected_scroll_target && client == ClientId::new(1)
                    ));
                }
                let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let mut completion = match instruction {
                    ScreenInstruction::WriteCharacter(_, _, _, _, Some(completion))
                    | ScreenInstruction::WriteToPaneId(_, _, Some(completion))
                    | ScreenInstruction::Paste(_, _, _, Some(completion))
                    | ScreenInstruction::ToggleTab(_, Some(completion)) => completion,
                    _ => panic!("unexpected screen instruction"),
                };
                assert_eq!(completion.requires_input_barrier(), expected_barrier);
                assert_eq!(
                    completion.clone().requires_input_barrier(),
                    expected_barrier
                );
                completion.set_affected_pane_id(pane_id);
            });
            let (_, result) = route_action(
                action,
                ClientId::new(1),
                None,
                None,
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
            assert_eq!(result.unwrap().affected_pane_id, Some(pane_id));
        }
    }
}
