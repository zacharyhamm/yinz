//! A plugin's key intercept: keys typed by a client go to the plugin instead of the pane.

use super::*;
use crate::screen::PluginId;
use yinz_test_support::Watched;

use yinz_types::keys::{BareKey, KeyWithModifier};

fn typed(key: char, client_id: ClientId) -> ScreenInstruction {
    ScreenInstruction::WriteCharacter(
        Some(KeyWithModifier::new(BareKey::Char(key))),
        key.to_string().into_bytes(),
        false,
        client_id,
        None,
    )
}

fn intercepted_keys(instructions: &[PluginInstruction]) -> Vec<(PluginId, char)> {
    instructions
        .iter()
        .filter_map(|i| match i {
            PluginInstruction::Update(updates) => Some(updates),
            _ => None,
        })
        .flatten()
        .filter_map(|(plugin_id, _, event)| match (plugin_id, event) {
            (Some(plugin_id), Event::InterceptedKeyPress(key)) => match key.bare_key {
                BareKey::Char(c) => Some((*plugin_id, c)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A clear is plugin-scoped: the unload path sends one for every plugin it unloads, and a
/// client has at most one intercepting plugin, so a clear naming another plugin must leave
/// this one's intercept in place. Only the intercepting plugin's own clear ends it.
#[test]
fn clearing_another_plugins_intercept_leaves_this_ones_in_place() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(None, vec![]);
    let received = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(received, PluginInstruction::Exit, plugin_receiver);
    let (intercepting, other) = (2, 1);

    for step in [
        ScreenInstruction::InterceptKeyPresses(crate::PluginId::new(intercepting), client_id),
        ScreenInstruction::ClearKeyPressesIntercepts(crate::PluginId::new(other), client_id),
        typed('x', client_id),
        ScreenInstruction::ClearKeyPressesIntercepts(crate::PluginId::new(intercepting), client_id),
        typed('y', client_id),
    ] {
        let _ = mock_screen.to_screen.send(step);
    }
    received.wait_until(
        "the key typed while the intercept stood",
        yinz_test_support::default_timeout(),
        |instructions| {
            intercepted_keys(instructions)
                .contains(&(crate::PluginId::new(intercepting), 'x'))
                .then_some(())
        },
    );
    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    assert_eq!(
        intercepted_keys(&received.lock()),
        vec![(yinz_types::internal_ids::PluginId::new(intercepting), 'x')],
        "the other plugin's clear left the intercept standing; the plugin's own ended it"
    );
}

#[test]
fn only_client_keyboard_completion_waits_for_plugin_input() {
    use crate::route::NotificationEnd;
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsCommand, PaneTabsLayout};

    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let receiver = mock.plugin_receiver.take().unwrap();
    let client = mock.main_client_id;
    let plugin = PluginId::new(100);
    let screen_thread = mock.run(None, vec![]);
    mock.new_tab_with_plugins(vec![plugin]);
    let pane_tabs = |command| {
        let (response, result) = crossbeam::channel::bounded(1);
        mock.to_screen
            .send(ScreenInstruction::PaneTabs {
                command,
                client_id: client,
                response,
            })
            .unwrap();
        result
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .unwrap()
            .unwrap()
    };
    let info = pane_tabs(PaneTabsCommand::Mount {
        pane_id: PaneId::Plugin(plugin),
        layout: PaneTabsLayout {
            active: 0,
            tabs: vec![PaneTabDescriptor {
                label: "Plugin".into(),
                location: "yinz:strider".into(),
                initial_cwd: None,
                configuration: BTreeMap::new(),
            }],
        },
        existing: 0,
    });
    receiver.try_iter().for_each(drop);
    let key = KeyWithModifier::new(BareKey::Char('x'));
    for (parsed_key, bytes) in [
        (Some(key.clone()), b"x".to_vec()),
        (None, b"x".to_vec()),
        (Some(key.clone()), vec![]),
    ] {
        let (sender, mut completed) = tokio::sync::oneshot::channel();
        mock.to_screen
            .send(ScreenInstruction::WriteCharacter(
                parsed_key,
                bytes,
                false,
                client,
                Some(NotificationEnd::new_client_input(sender)),
            ))
            .unwrap();
        let mut keys = vec![];
        let barrier = loop {
            match receiver
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
                .0
            {
                PluginInstruction::Update(events) => {
                    for (target, target_client, event) in events {
                        if let Event::Key(key) = event {
                            assert_eq!((target, target_client), (Some(plugin), Some(client)));
                            keys.push(key);
                        }
                    }
                },
                PluginInstruction::InputBarrier(target, completion) => {
                    assert_eq!(target, plugin);
                    break completion;
                },
                _ => {},
            }
        };
        assert_eq!(keys, vec![key.clone()], "key must precede its barrier");
        assert!(matches!(
            completed.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        drop(barrier);
        assert!(completed.try_recv().is_ok());
    }

    for target_pane in [false, true] {
        for (parsed_key, bytes) in [
            (Some(key.clone()), b"x".to_vec()),
            (None, b"x".to_vec()),
            (Some(key.clone()), vec![]), // forwarded key
        ] {
            let (sender, mut completed) = tokio::sync::oneshot::channel();
            let completion = Some(NotificationEnd::new(sender));
            let instruction = if target_pane {
                if parsed_key.is_some() {
                    ScreenInstruction::WriteKeyToPaneId(
                        parsed_key,
                        bytes,
                        false,
                        PaneId::Plugin(plugin),
                        completion,
                    )
                } else {
                    ScreenInstruction::WriteToPaneId(bytes, PaneId::Plugin(plugin), completion)
                }
            } else {
                ScreenInstruction::WriteCharacter(parsed_key, bytes, false, client, completion)
            };
            mock.to_screen.send(instruction).unwrap();
            // Fence the write without processing plugin instructions: a synchronous plugin
            // caller must complete while its own executor is still busy with that call.
            pane_tabs(PaneTabsCommand::Focus {
                id: info.id,
                bar: false,
            });
            assert!(completed.try_recv().is_ok());
            let mut keys = vec![];
            for (instruction, _) in receiver.try_iter() {
                match instruction {
                    PluginInstruction::InputBarrier(..) => {
                        panic!("plugin writes must not queue an input barrier");
                    },
                    PluginInstruction::Update(events) => {
                        for (target, target_client, event) in events {
                            if let Event::Key(key) = event {
                                assert_eq!(target, Some(plugin));
                                assert_eq!(target_client, (!target_pane).then_some(client));
                                keys.push(key);
                            }
                        }
                    },
                    _ => {},
                }
            }
            assert_eq!(keys, vec![key.clone()]);
        }
    }

    pane_tabs(PaneTabsCommand::Focus {
        id: info.id,
        bar: true,
    });
    receiver.try_iter().for_each(drop);
    let (sender, mut completed) = tokio::sync::oneshot::channel();
    mock.to_screen
        .send(ScreenInstruction::WriteCharacter(
            Some(KeyWithModifier::new(BareKey::Right)),
            b"\x1b[C".to_vec(),
            false,
            client,
            Some(NotificationEnd::new_client_input(sender)),
        ))
        .unwrap();
    // The reply fences the preceding keyboard instruction on the screen thread.
    pane_tabs(PaneTabsCommand::Focus {
        id: info.id,
        bar: false,
    });
    assert!(completed.try_recv().is_ok());
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| match instruction {
            PluginInstruction::InputBarrier(..) => false,
            PluginInstruction::Update(events) => events
                .iter()
                .all(|(_, _, event)| !matches!(event, Event::Key(_))),
            _ => true,
        }));
    mock.teardown(vec![screen_thread]);
}
