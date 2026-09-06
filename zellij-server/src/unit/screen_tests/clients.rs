#[cfg(test)]
const PLUGIN_2: zellij_types::internal_ids::PluginId = zellij_types::internal_ids::PluginId::new(2);

use super::*;
use zellij_types::internal_ids::TabId;

#[test]
fn attaching_terminal_client_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .new_floating_pane(
            PaneId::Terminal(crate::TerminalId::new(3)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .unwrap();

    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(2)),
        Some(&TabId::new(1)),
        "terminal client is on the host tab"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(1))
            .unwrap()
            .are_floating_panes_visible(),
        "the floating surface remains visible"
    );
}

#[test]
fn attaching_watcher_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen
        .add_watcher_client(crate::ClientId::new(2))
        .expect("TEST");
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(2)),
        Some(&TabId::new(1)),
        "watcher client mirrors the host tab"
    );
}

#[test]
fn reattaching_the_only_client_tells_the_tab_plugins_they_are_visible_again() {
    // A tab whose last client leaves tells its plugins nobody is looking; a client
    // arriving in that tab has to tell them the opposite, otherwise a plugin that
    // went quiet on `Visible(false)` never wakes up again after a reattach.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(2)),
        false,
    );
    let _ = capture.drain_plugin_updates();

    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");

    let visibility_events: Vec<bool> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(|(pid, _cid, event)| match (pid, event) {
            (Some(PLUGIN_2), Event::Visible(is_visible)) => Some(is_visible),
            _ => None,
        })
        .collect();
    assert_eq!(
        visibility_events,
        vec![false, true],
        "the plugin should be told the tab went off screen and then came back"
    );
}

#[test]
fn reattaching_a_client_restores_floating_pane_visibility_notifications() {
    // While a tab has no clients its floating plugins are not told about the surface
    // being lowered; `add_client` marks the tab visible again, and from then on they
    // must be, otherwise a plugin idling on a timer keeps working while off screen.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    // focused, so that the floating surface actually comes up with the pane
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            PaneId::Plugin(crate::PluginId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    assert!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .are_floating_panes_visible(),
        "precondition: the floating surface is up"
    );

    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    let _ = capture.drain_plugin_updates();
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    let visibility_events: Vec<(Option<zellij_types::internal_ids::PluginId>, bool)> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(|(pid, _cid, event)| match event {
            Event::Visible(is_visible) => Some((pid, is_visible)),
            _ => None,
        })
        .collect();
    assert!(
        visibility_events.contains(&(Some(zellij_types::internal_ids::PluginId::new(2)), false)),
        "a floating plugin must still be told when its surface is hidden after a reattach, got {visibility_events:?}"
    );
}
