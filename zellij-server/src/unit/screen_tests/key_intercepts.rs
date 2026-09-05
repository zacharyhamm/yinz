//! A plugin's key intercept: keys typed by a client go to the plugin instead of the pane.

use super::*;
use crate::screen::PluginId;
use zellij_test_support::Watched;
use zellij_utils::data::{BareKey, KeyWithModifier};

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
        ScreenInstruction::InterceptKeyPresses(crate::PluginId(intercepting), client_id),
        ScreenInstruction::ClearKeyPressesIntercepts(crate::PluginId(other), client_id),
        typed('x', client_id),
        ScreenInstruction::ClearKeyPressesIntercepts(crate::PluginId(intercepting), client_id),
        typed('y', client_id),
    ] {
        let _ = mock_screen.to_screen.send(step);
    }
    received.wait_until(
        "the key typed while the intercept stood",
        zellij_test_support::default_timeout(),
        |instructions| {
            intercepted_keys(instructions)
                .contains(&(crate::PluginId(intercepting), 'x'))
                .then_some(())
        },
    );
    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    assert_eq!(
        intercepted_keys(&received.lock()),
        vec![(zellij_utils::internal_ids::PluginId(intercepting), 'x')],
        "the other plugin's clear left the intercept standing; the plugin's own ended it"
    );
}
