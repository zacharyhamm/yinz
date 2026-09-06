//! Confirming a quit: the `Ctrl-q` route, the last pane closing, and the notices both carry.

#[cfg(test)]
const CLIENT_2: zellij_types::internal_ids::ClientId = zellij_types::internal_ids::ClientId::new(2);

use super::*;
use crate::screen::PluginId;
use zellij_types::internal_ids::TabId;
use {zellij_types::data::OverlayKind, zellij_types::data::OverlayPlacement};

type ScreenReceiver = Receiver<(ScreenInstruction, ErrorContext)>;

/// Close a pane the way the pty and the close-pane action both end up doing, then let the render
/// pass decide what that leaves the session with.
fn close_pane(screen: &mut Screen, pane_id: PaneId) {
    let tab_id = *screen.tabs.keys().next().expect("a tab to close it in");
    screen
        .tabs
        .get_mut(&tab_id)
        .expect("TEST")
        .close_pane(pane_id, false, None);
}

const HOCUS_PLUGIN_ID: PluginId = crate::PluginId::new(7);

/// A screen with one tab, one pane, and a channel onto its own instruction queue - which is
/// where `confirm_quit` puts the modal.
fn screen_with_a_single_pane() -> (Screen, ScreenReceiver) {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(1));
    // The sweep that decides an emptied tab's fate only runs for regular clients, so the screen
    // needs one attached for any of this to happen at all.
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    (screen, screen_receiver)
}

/// The first `LaunchPlugin` on the queue, or `None` if nothing was launched.
fn launched_plugin(receiver: &ScreenReceiver) -> Option<ScreenInstruction> {
    while let Ok((instruction, _)) = receiver.try_recv() {
        if matches!(instruction, ScreenInstruction::LaunchPlugin(..)) {
            return Some(instruction);
        }
    }
    None
}

/// The plugin tag, configuration and overlay placement of a `LaunchPlugin`.
fn launch_details(
    instruction: &ScreenInstruction,
) -> (String, BTreeMap<String, String>, Option<OverlayPlacement>) {
    match instruction {
        ScreenInstruction::LaunchPlugin(
            RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::Zellij(plugin_tag),
                configuration,
                ..
            }),
            ..,
            placement,
        ) => (
            plugin_tag.to_string(),
            configuration.inner().clone(),
            placement.clone(),
        ),
        other => panic!("expected a plugin launch, got {other:?}"),
    }
}

/// A keybinding has neither an originating pane nor a CLI client, and is the only quit that gets
/// asked about. The modal itself is built on the screen thread, which is where the notices are.
#[test]
fn quit_action_asks_the_screen_to_confirm() {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    route_arbitrary_action_to_server(
        &mock_screen.clone_session_metadata(),
        Action::Quit,
        crate::ClientId::new(2),
    );

    let screen_receiver = mock_screen.screen_receiver.take().unwrap();
    let instruction = loop {
        let (instruction, _) = screen_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("quit confirmation instruction");
        if matches!(instruction, ScreenInstruction::ConfirmQuit(..)) {
            break instruction;
        }
    };
    assert!(matches!(
        instruction,
        ScreenInstruction::ConfirmQuit(CLIENT_2, _)
    ));

    mock_screen.teardown(vec![]);
}

/// The shape the confirmation has always had, now built one thread over. The placement is stated
/// by the launcher rather than spelled as a `modal` key in the plugin's configuration for the
/// server to read back out, and a quit nobody had anything to say about carries no configuration
/// at all.
#[test]
fn confirming_a_plain_quit_launches_the_modal_with_no_configuration() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen
        .confirm_quit(crate::ClientId::new(1), false, None)
        .expect("TEST");

    let launch = launched_plugin(&screen_receiver).expect("the modal is launched");
    let (tag, configuration, placement) = launch_details(&launch);
    assert_eq!(tag, "quit-confirmation");
    assert!(
        configuration.is_empty(),
        "nothing had anything to say: {configuration:?}"
    );
    assert_eq!(
        placement,
        Some(OverlayPlacement {
            kind: OverlayKind::Modal,
            coordinates: None,
        })
    );
}

/// A guard the user is never told about is a session that refuses to close for no stated reason,
/// so what the plugin said reaches the dialog it will be read in.
#[test]
fn a_notice_reaches_the_confirmation() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(
        HOCUS_PLUGIN_ID,
        Some("hocus is still working".to_owned()),
        true,
    );
    screen
        .confirm_quit(crate::ClientId::new(1), false, None)
        .expect("TEST");

    let launch = launched_plugin(&screen_receiver).expect("the modal is launched");
    let (_, configuration, _) = launch_details(&launch);
    assert_eq!(
        configuration.get("notice").map(String::as_str),
        Some("hocus is still working")
    );
    assert_eq!(
        configuration.get("reason"),
        None,
        "this one was asked for with Ctrl-q"
    );
}

/// Two plugins with something outstanding both get a line, in plugin id order so the same set of
/// notices always reads the same way.
#[test]
fn notices_from_several_plugins_are_joined_in_a_stable_order() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(crate::PluginId::new(9), Some("second".to_owned()), false);
    screen.set_quit_notice(crate::PluginId::new(3), Some("first".to_owned()), false);
    screen
        .confirm_quit(crate::ClientId::new(1), false, None)
        .expect("TEST");

    let launch = launched_plugin(&screen_receiver).expect("the modal is launched");
    let (_, configuration, _) = launch_details(&launch);
    assert_eq!(
        configuration.get("notice").map(String::as_str),
        Some("first\nsecond")
    );
}

/// A plugin that says nothing and guards nothing has withdrawn, and its entry goes rather than
/// sitting there empty and leaving a blank line in the dialog.
#[test]
fn a_withdrawn_notice_leaves_nothing_behind() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(HOCUS_PLUGIN_ID, Some("working".to_owned()), true);
    assert!(screen.quit_is_guarded());

    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, false);
    assert!(!screen.quit_is_guarded());

    screen
        .confirm_quit(crate::ClientId::new(1), false, None)
        .expect("TEST");
    let launch = launched_plugin(&screen_receiver).expect("the modal is launched");
    let (_, configuration, _) = launch_details(&launch);
    assert!(configuration.is_empty(), "{configuration:?}");
}

/// The regression that matters: nothing about this feature may make an ordinary session harder
/// to leave. With no guard, the last pane closing still ends the session.
#[test]
fn the_last_pane_closing_still_exits_when_nothing_guards() {
    let (mut screen, _screen_receiver) = screen_with_a_single_pane();
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.render_to_clients().expect("TEST");

    assert!(screen.tabs.is_empty(), "the emptied tab is swept");
    let exited = std::iter::from_fn(|| server_receiver.try_recv().ok())
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None)));
    assert!(exited, "the session ends when the last pane closes");
}

/// With a guard up the tab is held open instead, the session survives, and the confirmation goes
/// up saying why it is asking. Cancelling needs no signal of its own - the tab simply stays.
#[test]
fn the_last_pane_closing_confirms_when_a_plugin_guards() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));
    screen.set_quit_notice(
        HOCUS_PLUGIN_ID,
        Some("hocus is still working".to_owned()),
        true,
    );

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.render_to_clients().expect("TEST");

    assert_eq!(screen.tabs.len(), 1, "the last tab is held open");
    let exited = std::iter::from_fn(|| server_receiver.try_recv().ok())
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None)));
    assert!(!exited, "the session must not end before the user answers");

    let launch = launched_plugin(&screen_receiver).expect("the confirmation is raised");
    let (tag, configuration, placement) = launch_details(&launch);
    assert_eq!(tag, "quit-confirmation");
    assert_eq!(
        configuration.get("reason").map(String::as_str),
        Some("last_pane"),
        "the dialog says which gesture it is answering"
    );
    assert_eq!(
        configuration.get("notice").map(String::as_str),
        Some("hocus is still working")
    );
    assert_eq!(
        placement,
        Some(OverlayPlacement {
            kind: OverlayKind::Modal,
            coordinates: None,
        })
    );
}

/// Renders are frequent and the held tab stays empty across all of them, so the raise has to be
/// an edge rather than a state - otherwise every frame stacks another modal on the last.
#[test]
fn the_confirmation_is_raised_once_and_not_once_per_frame() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    for _ in 0..4 {
        screen.render_to_clients().expect("TEST");
    }

    let launches = std::iter::from_fn(|| screen_receiver.try_recv().ok())
        .filter(|(instruction, _)| matches!(instruction, ScreenInstruction::LaunchPlugin(..)))
        .count();
    assert_eq!(launches, 1, "one modal, however many frames");
}

/// The hold is not permanent: opening something selectable in the held tab puts the session back
/// to normal, and closing that pane asks again rather than exiting behind the user's back.
#[test]
fn opening_a_pane_in_the_held_tab_makes_a_later_close_ask_again() {
    let (mut screen, screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.render_to_clients().expect("TEST");
    let tab_id = *screen.tabs.keys().next().expect("the tab is held");
    assert!(launched_plugin(&screen_receiver).is_some());

    add_pane_to_tab(
        &mut screen,
        tab_id,
        PaneId::Terminal(crate::TerminalId::new(2)),
        false,
    );
    screen.render_to_clients().expect("TEST");

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(2)));
    screen.render_to_clients().expect("TEST");
    assert_eq!(screen.tabs.len(), 1, "still held");
    assert!(
        launched_plugin(&screen_receiver).is_some(),
        "the second close is asked about too"
    );
}

/// A plugin that crashed or was closed mid-reply must not leave a guard behind that nothing can
/// lift - that is a session that will not close its last pane and cannot say why.
#[test]
fn a_notice_is_dropped_with_its_plugin() {
    let (mut screen, _screen_receiver) = screen_with_a_single_pane();
    screen.set_quit_notice(HOCUS_PLUGIN_ID, Some("working".to_owned()), true);
    assert!(screen.quit_is_guarded());

    screen.drop_quit_notice(HOCUS_PLUGIN_ID);
    assert!(!screen.quit_is_guarded());
}
