//! `Event::Visible` for plugins on the floating surface.
//!
//! A floating plugin goes off screen when the floating surface is lowered exactly as it does
//! when its tab is hidden, and a plugin idling on a timer (the session-manager polls the session
//! list once a second) keeps that timer armed until it is told. `show_floating_panes` and
//! `hide_floating_panes` therefore report real transitions of the surface, and `Tab::visible`
//! leaves floating plugins alone while the surface is down.

use super::*;
use crate::plugins::PluginInstruction;
use {zellij_types::data::Event, zellij_types::data::NewPanePlacement};

fn drain_visible_events(
    plugin_receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(Option<zellij_types::internal_ids::PluginId>, bool)> {
    let mut visible_events = vec![];
    while let Ok((instruction, _)) = plugin_receiver.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _client_id, event) in updates {
                if let Event::Visible(is_visible) = event {
                    visible_events.push((pid, is_visible));
                }
            }
        }
    }
    visible_events
}

fn tab_with_floating_plugin_pane(
    plugin_pid: u32,
) -> (Tab, Receiver<(PluginInstruction, ErrorContext)>) {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut tab, plugin_receiver) = create_new_tab_with_plugin_receiver(size, ModeInfo::default());
    tab.new_pane(
        PaneId::Plugin(crate::PluginId(plugin_pid)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(crate::ClientId(1)),
        None,
    )
    .unwrap();
    (tab, plugin_receiver)
}

#[test]
fn floating_plugin_panes_are_notified_when_the_floating_surface_is_hidden() {
    let (mut tab, plugin_receiver) = tab_with_floating_plugin_pane(1);
    drain_visible_events(&plugin_receiver);

    tab.hide_floating_panes();

    assert_eq!(
        drain_visible_events(&plugin_receiver),
        vec![(Some(zellij_types::internal_ids::PluginId(1)), false)],
        "a plugin in a floating pane should be sent Event::Visible(false) when the floating surface is hidden"
    );
}

#[test]
fn floating_plugin_panes_are_notified_when_the_floating_surface_is_shown() {
    let (mut tab, plugin_receiver) = tab_with_floating_plugin_pane(1);
    tab.hide_floating_panes();
    drain_visible_events(&plugin_receiver);

    tab.show_floating_panes();

    assert_eq!(
        drain_visible_events(&plugin_receiver),
        vec![(Some(zellij_types::internal_ids::PluginId(1)), true)],
        "a plugin in a floating pane should be sent Event::Visible(true) when the floating surface is shown"
    );
}

#[test]
fn toggling_the_floating_surface_twice_does_not_repeat_the_visibility_event() {
    let (mut tab, plugin_receiver) = tab_with_floating_plugin_pane(1);
    drain_visible_events(&plugin_receiver);

    tab.hide_floating_panes();
    tab.hide_floating_panes();
    tab.show_floating_panes();
    tab.show_floating_panes();

    assert_eq!(
        drain_visible_events(&plugin_receiver),
        vec![
            (Some(zellij_types::internal_ids::PluginId(1)), false),
            (Some(zellij_types::internal_ids::PluginId(1)), true)
        ],
        "only actual visibility transitions of the floating surface should be reported"
    );
}

#[test]
fn floating_surface_toggles_in_a_hidden_tab_do_not_notify_plugins() {
    let (mut tab, plugin_receiver) = tab_with_floating_plugin_pane(1);
    tab.visible(false).unwrap();
    drain_visible_events(&plugin_receiver);

    tab.hide_floating_panes();
    tab.show_floating_panes();

    assert!(
        drain_visible_events(&plugin_receiver).is_empty(),
        "a plugin in a hidden tab should not be told it became visible because the floating surface was toggled"
    );
}

#[test]
fn floating_plugin_panes_are_not_shown_again_when_their_tab_returns_with_the_surface_hidden() {
    let (mut tab, plugin_receiver) = tab_with_floating_plugin_pane(1);
    tab.hide_floating_panes();
    tab.visible(false).unwrap();
    drain_visible_events(&plugin_receiver);

    tab.visible(true).unwrap();

    assert!(
        !drain_visible_events(&plugin_receiver).contains(&(Some(crate::PluginId(1)), true)),
        "a plugin whose floating surface is hidden should not be told it is visible when its tab returns"
    );
}
