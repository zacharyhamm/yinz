//! Confirming a quit: the `Ctrl-q` route, the last pane closing, and the notices both carry.

#[cfg(test)]
const CLIENT_2: zellij_types::internal_ids::ClientId = zellij_types::internal_ids::ClientId::new(2);

use super::*;
use crate::screen::PluginId;
use zellij_types::internal_ids::TabId;
use {zellij_types::data::OverlayKind, zellij_types::data::OverlayPlacement};

type ScreenReceiver = Receiver<(ScreenInstruction, ErrorContext)>;

/// Close a pane the way the pty and the close-pane action both end up doing. What that leaves
/// the session with is decided by `settle_tabs`, which the screen thread runs after every
/// instruction.
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
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    (screen, screen_receiver)
}

/// Whether the server was told the session is over.
fn session_ended(receiver: &Receiver<(ServerInstruction, ErrorContext)>) -> bool {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None)))
}

/// The tabs `settle_tabs` asked the plugin thread to build, in order.
fn requested_tabs(receiver: &Receiver<(PluginInstruction, ErrorContext)>) -> Vec<TabId> {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::NewTab(_, _, _, _, tab_id, ..) => Some(tab_id),
            _ => None,
        })
        .collect()
}

/// The replacement tab comes up the way any new tab does: the plugin thread spawns its panes
/// and the layout is applied. Here that is one terminal, `terminal_id`.
fn load_replacement_tab(screen: &mut Screen, tab_id: TabId, terminal_id: u32) {
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(crate::TerminalId::new(terminal_id), None)],
            vec![],
            HashMap::new(),
            tab_id,
            false,
            crate::ClientId::new(1),
            None,
        )
        .expect("TEST");
}

/// The number of quit confirmations raised so far.
fn launches(receiver: &ScreenReceiver) -> usize {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|(instruction, _)| matches!(instruction, ScreenInstruction::LaunchPlugin(..)))
        .count()
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
/// to leave. With no guard, the last pane closing still ends the session - decided by the settle
/// step after the instruction, not by a frame.
#[test]
fn the_last_pane_closing_still_exits_when_nothing_guards() {
    let (mut screen, _screen_receiver) = screen_with_a_single_pane();
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty(), "the emptied tab is closed");
    assert!(
        session_ended(&server_receiver),
        "the session ends when the last pane closes"
    );
}

/// A session nobody is looking at ends the same way, at once: the settle step does not wait
/// for a frame, and a frame is only ever drawn for someone.
#[test]
fn the_last_pane_closing_exits_with_nobody_attached() {
    let (mut screen, _screen_receiver, server_receiver, _plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[]);
    assert!(screen.active_tab_ids.is_empty());

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty());
    assert!(session_ended(&server_receiver));
}

/// A watcher is not a client a tab is built for, and it does not keep an emptied one either.
#[test]
fn the_last_pane_closing_exits_with_only_a_watcher_attached() {
    let (mut screen, _screen_receiver, server_receiver, _plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[]);
    screen
        .add_watcher_client(crate::ClientId::new(9))
        .expect("TEST");

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty());
    assert!(session_ended(&server_receiver));
}

/// A terminal a plugin parked is still running, and the session keeps it: not by holding an
/// empty tab open - that is a blank screen nobody can type into - but by opening a fresh tab and
/// moving the hidden pane into it. Once the hidden terminal exits too, nothing is left to keep.
#[test]
fn a_hidden_terminal_keeps_the_session_alive_until_it_exits() {
    let (mut screen, screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    let client_id = crate::ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .suppress_pane(pane_id, None);
    assert!(!screen.tabs[&TabId::new(1)].has_selectable_panes());

    screen.settle_tabs().expect("TEST");
    let replacement = match requested_tabs(&plugin_receiver).as_slice() {
        [id] => *id,
        other => panic!("one replacement tab is requested, got {other:?}"),
    };
    assert_eq!(
        screen.tabs.len(),
        2,
        "the emptied tab waits for its replacement"
    );
    assert!(!session_ended(&server_receiver));

    load_replacement_tab(&mut screen, replacement, 2);
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![replacement],
        "the emptied tab closes into the replacement"
    );
    assert_eq!(screen.active_tab_ids.get(&client_id), Some(&replacement));
    assert!(
        screen.tabs[&replacement]
            .get_all_pane_ids()
            .contains(&pane_id),
        "the hidden terminal moves with it"
    );
    assert_eq!(
        launches(&screen_receiver),
        0,
        "no plugin asked for a confirmation"
    );
    assert!(!session_ended(&server_receiver));

    // The hidden terminal exits, and then the shell in front of it does.
    screen.tabs.get_mut(&replacement).expect("TEST").hold_pane(
        pane_id,
        Some(0),
        false,
        RunCommand::default(),
    );
    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(2)));
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty());
    assert!(session_ended(&server_receiver));
}

/// With a guard up the session survives: a fresh tab replaces the emptied one, the client and
/// its overlays land there, and only then does the confirmation go up - in the tab the user is
/// on, saying why it is asking. Cancelling it leaves the user with a shell, not a blank tab.
#[test]
fn the_last_pane_closing_confirms_when_a_plugin_guards() {
    let (mut screen, screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    let client_id = crate::ClientId::new(1);
    screen.set_quit_notice(
        HOCUS_PLUGIN_ID,
        Some("hocus is still working".to_owned()),
        true,
    );

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");

    let replacement = match requested_tabs(&plugin_receiver).as_slice() {
        [id] => *id,
        other => panic!("one replacement tab is requested, got {other:?}"),
    };
    assert_eq!(
        screen.tabs.len(),
        2,
        "the emptied tab waits for its replacement"
    );
    assert!(
        !session_ended(&server_receiver),
        "the session must not end before the user answers"
    );
    assert_eq!(
        launches(&screen_receiver),
        0,
        "nothing to confirm in until the replacement is the tab the client is on"
    );

    load_replacement_tab(&mut screen, replacement, 2);
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![replacement]
    );
    assert_eq!(screen.active_tab_ids.get(&client_id), Some(&replacement));
    assert!(screen.tabs[&replacement].has_selectable_panes());
    assert!(!session_ended(&server_receiver));
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

/// Several tabs can empty in one instruction. One replacement serves them all.
#[test]
fn closing_all_tabs_at_once_still_confirms_when_a_plugin_guards() {
    let (mut screen, screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(
            &[TabId::new(1), TabId::new(2)],
            &[crate::ClientId::new(1)],
        );
    screen.set_quit_notice(HOCUS_PLUGIN_ID, Some("hocus is still working".into()), true);

    for (tab_id, terminal_id) in [(1, 1), (2, 2)] {
        screen
            .tabs
            .get_mut(&TabId::new(tab_id))
            .expect("TEST")
            .close_pane(
                PaneId::Terminal(crate::TerminalId::new(terminal_id)),
                false,
                Some(0),
            );
    }
    screen.settle_tabs().expect("TEST");
    screen.settle_tabs().expect("TEST");

    let replacement = match requested_tabs(&plugin_receiver).as_slice() {
        [id] => *id,
        other => panic!("one replacement tab is requested, got {other:?}"),
    };
    assert_eq!(screen.tabs.len(), 3);
    load_replacement_tab(&mut screen, replacement, 3);
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![replacement]
    );
    assert_eq!(launches(&screen_receiver), 1);
    assert!(!session_ended(&server_receiver));
}

/// The settle step runs after every instruction, and the emptied tab stays empty across all of
/// them while the replacement loads. One replacement, one confirmation, however many passes.
#[test]
fn the_confirmation_is_raised_once_and_not_once_per_pass() {
    let (mut screen, screen_receiver, _server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    for _ in 0..4 {
        screen.settle_tabs().expect("TEST");
    }
    let requested = requested_tabs(&plugin_receiver);
    assert_eq!(requested.len(), 1, "one replacement, however many passes");

    load_replacement_tab(&mut screen, requested[0], 2);
    for _ in 0..4 {
        screen.settle_tabs().expect("TEST");
    }
    assert_eq!(
        launches(&screen_receiver),
        1,
        "one modal, however many passes"
    );
    assert!(requested_tabs(&plugin_receiver).is_empty());
}

/// Closing the replacement's shell with the guard still up asks again rather than exiting
/// behind the user's back - the same cycle, with another fresh tab.
#[test]
fn closing_the_replacements_shell_asks_again() {
    let (mut screen, screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");
    let first = requested_tabs(&plugin_receiver)[0];
    load_replacement_tab(&mut screen, first, 2);
    screen.settle_tabs().expect("TEST");
    assert_eq!(launches(&screen_receiver), 1);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(2)));
    screen.settle_tabs().expect("TEST");
    let second = match requested_tabs(&plugin_receiver).as_slice() {
        [id] => *id,
        other => panic!("another replacement is requested, got {other:?}"),
    };
    assert_ne!(first, second);
    load_replacement_tab(&mut screen, second, 3);
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![second]
    );
    assert_eq!(
        launches(&screen_receiver),
        1,
        "the second close is asked about too"
    );
    assert!(!session_ended(&server_receiver));
}

/// A replacement is built for a client. With nobody attached the emptied tab simply waits,
/// unseen, and the client that attaches is the one the replacement is built for.
#[test]
fn a_guarded_session_with_nobody_attached_waits_for_a_client() {
    let (mut screen, _screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert_eq!(screen.tabs.len(), 1, "the emptied tab is kept");
    assert!(requested_tabs(&plugin_receiver).is_empty());
    assert!(!session_ended(&server_receiver));

    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert_eq!(requested_tabs(&plugin_receiver).len(), 1);
    assert!(!session_ended(&server_receiver));
}

/// A replacement the user closes before it has loaded is not the end of the story: the session
/// still has to survive, so another is requested.
#[test]
fn a_replacement_closed_before_it_loads_is_requested_again() {
    let (mut screen, _screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");
    let first = requested_tabs(&plugin_receiver)[0];
    screen.close_tab_by_id(first).expect("TEST");
    screen.settle_tabs().expect("TEST");

    // Tab ids are handed out from the highest live one, so the second request may well reuse
    // the first's id; that it was made at all is the point.
    assert_eq!(
        requested_tabs(&plugin_receiver).len(),
        1,
        "another replacement is requested"
    );
    assert_eq!(screen.tabs.len(), 2);
    assert!(!session_ended(&server_receiver));
}

/// A replacement that comes up with nothing selectable in it - a layout of bars and nothing
/// else - would only ask for another. The session ends instead of looping.
#[test]
fn a_replacement_that_loads_unusable_ends_the_session() {
    let (mut screen, _screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);

    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().expect("TEST");
    let replacement = requested_tabs(&plugin_receiver)[0];
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            replacement,
            true,
            crate::ClientId::new(1),
            None,
        )
        .expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty());
    assert!(session_ended(&server_receiver));
    assert!(requested_tabs(&plugin_receiver).is_empty());
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

/// A tab closing takes its plugin panes down without a per-pane close, and their notices have
/// to go with them all the same.
#[test]
fn closing_a_tab_drops_the_guards_of_the_plugins_in_it() {
    let (mut screen, _screen_receiver, _server_receiver, _plugin_receiver) =
        screen_with_plugin_senders_and_clients(
            &[TabId::new(1), TabId::new(2)],
            &[crate::ClientId::new(1)],
        );
    let plugin_id = PluginId::new(42);
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        plugin_id,
    );
    screen.set_quit_notice(plugin_id, Some("working".to_owned()), true);
    assert!(screen.quit_is_guarded());

    screen.close_tab_by_id(TabId::new(1)).expect("TEST");

    assert!(!screen.quit_is_guarded());
    assert_every_client_has_a_tab(&screen);
}

// ---------------------------------------------------------------------------
// A hook pane standing in for the pane it replaced
//
// `agent_exit` opens its plugin in place of the exited agent pane, which is suppressed
// behind it (`add_plugin_pane_in_place_of` -> `suppress_pane_and_replace_with_pid`). Closing
// that hook pane with the close-pane binding is the gesture these pin: the tab it was the
// last pane of has to go away, and the client has to land somewhere it can see and type.
// ---------------------------------------------------------------------------

/// A screen whose tabs can hold plugin panes: `Tab` clones the senders when it is built, so
/// the plugin channel has to be in place before any tab is made. Otherwise as
/// `screen_with_a_single_pane`, with the given clients attached.
fn screen_with_plugin_senders_and_clients(
    tab_ids: &[TabId],
    client_ids: &[crate::ClientId],
) -> (
    Screen,
    ScreenReceiver,
    Receiver<(ServerInstruction, ErrorContext)>,
    Receiver<(PluginInstruction, ErrorContext)>,
) {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_new_screen(size, true, true);
    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));
    for (index, tab_id) in tab_ids.iter().enumerate() {
        new_tab(&mut screen, index as u32 + 1, *tab_id);
    }
    // `new_tab` applies each layout for client 1, which attaches it; a screen with nobody on it
    // is one that client has since left.
    if !client_ids.contains(&crate::ClientId::new(1)) {
        screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    }
    for client_id in client_ids {
        screen.add_client(*client_id).expect("TEST");
    }
    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    (screen, screen_receiver, server_receiver, plugin_receiver)
}

/// As `screen_with_plugin_senders_and_clients`, with client 1 attached.
fn screen_with_plugin_senders(
    tab_ids: &[TabId],
) -> (
    Screen,
    ScreenReceiver,
    Receiver<(ServerInstruction, ErrorContext)>,
) {
    let (screen, screen_receiver, server_receiver, _plugin_receiver) =
        screen_with_plugin_senders_and_clients(tab_ids, &[crate::ClientId::new(1)]);
    (screen, screen_receiver, server_receiver)
}

/// The shape `agent_exit` leaves behind: `tab_id` holds a plugin pane standing in for the
/// held, exited terminal it replaced.
fn hook_pane_over_a_held_terminal(
    screen: &mut Screen,
    tab_id: TabId,
    terminal_id: crate::TerminalId,
    plugin_id: PluginId,
) {
    let tab = screen.tabs.get_mut(&tab_id).expect("TEST");
    tab.suppress_pane_and_replace_with_pid(
        PaneId::Terminal(terminal_id),
        PaneId::Plugin(plugin_id),
        false, // close_replaced_pane
        None,  // run
        None,  // completion_tx
        None,  // borderless
    )
    .expect("TEST");
    // The agent's command exited before the hook fired, which is what makes the pane held.
    let is_first_run = false;
    tab.hold_pane(
        PaneId::Terminal(terminal_id),
        Some(0),
        is_first_run,
        RunCommand::default(),
    );
}

/// Every client can still see a tab and type into it: its `active_tab_ids` entry names a live
/// tab, and that tab counts it among its connected clients. A client that fails either half
/// gets no output and no input handling, with nothing to repair it short of a reattach.
fn assert_every_client_has_a_tab(screen: &Screen) {
    for (client_id, tab_id) in &screen.active_tab_ids {
        let tab = screen.tabs.get(tab_id).unwrap_or_else(|| {
            panic!("client {client_id} is on tab {tab_id}, which no longer exists")
        });
        assert!(
            tab.has_connected_client(client_id),
            "client {client_id} names tab {tab_id}, which does not count it as connected",
        );
    }
}

/// The last tab: closing the hook pane takes the dead terminal with it and ends the session.
#[test]
fn closing_a_hook_pane_over_a_dead_terminal_closes_the_last_tab() {
    let (mut screen, _screen_receiver, server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1)]);
    let client_id = crate::ClientId::new(1);
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        PluginId::new(42),
    );

    screen.close_focused_pane(client_id, None).expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert!(
        screen.tabs.is_empty(),
        "the hook pane and the dead terminal go together, and the tab with them",
    );
    assert!(
        std::iter::from_fn(|| server_receiver.try_recv().ok())
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))),
        "the session ends when the last tab closes",
    );
}

/// With another tab open, the client lands on it - seeing it and able to type into it.
#[test]
fn closing_a_hook_pane_over_a_dead_terminal_rehomes_the_client() {
    let (mut screen, _screen_receiver, _server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1), TabId::new(2)]);
    let client_id = crate::ClientId::new(1);
    screen
        .go_to_tab(zellij_types::internal_ids::TabNumber::new(1), client_id)
        .expect("TEST");
    assert_eq!(screen.active_tab_ids.get(&client_id), Some(&TabId::new(1)));

    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        PluginId::new(42),
    );

    screen.close_focused_pane(client_id, None).expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2)],
        "the emptied tab is swept and the other one survives",
    );
    assert_every_client_has_a_tab(&screen);
}

/// The destination matters as much as the tab that closes. A tab whose only pane is floating
/// has nothing tiled in it, and used to refuse the arriving client outright - leaving it named
/// in `active_tab_ids` and counted by no tab, which is a client that receives no frames and
/// whose keys reach nothing until it reattaches.
#[test]
fn a_client_re_homed_into_a_tab_with_only_floating_panes_can_still_see_and_type() {
    let (mut screen, _screen_receiver, _server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1), TabId::new(2)]);
    let client_id = crate::ClientId::new(1);
    // Tab 2 keeps a floating pane and nothing tiled, so it survives the sweep with no tiled
    // pane for an arriving client to be focused on.
    let tab_two = screen.tabs.get_mut(&TabId::new(2)).expect("TEST");
    tab_two
        .new_floating_pane(
            PaneId::Terminal(crate::TerminalId::new(9)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .expect("TEST");
    tab_two.close_pane(PaneId::Terminal(crate::TerminalId::new(2)), false, None);

    screen
        .go_to_tab(zellij_types::internal_ids::TabNumber::new(1), client_id)
        .expect("TEST");
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        PluginId::new(42),
    );

    screen.close_focused_pane(client_id, None).expect("TEST");
    screen.settle_tabs().expect("TEST");

    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2)],
        "the emptied tab is swept and the floating-only one survives",
    );
    assert_every_client_has_a_tab(&screen);
}

/// A plugin pane closed with the close-pane binding drops whatever it said about a quit.
///
/// `Screen::close_focused_pane` reaches `Tab::close_focused_pane` directly rather than through
/// `ScreenInstruction::ClosePane`, so it used to skip `drop_quit_notice` - and a guard nothing
/// can lift is a session that can never close its last pane again.
#[test]
fn closing_a_guarding_plugin_pane_by_keybinding_drops_its_guard() {
    let (mut screen, _screen_receiver, _server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1)]);
    let client_id = crate::ClientId::new(1);
    let plugin_id = PluginId::new(42);
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        plugin_id,
    );
    screen.set_quit_notice(plugin_id, Some("still working".to_owned()), true);
    assert!(screen.quit_is_guarded(), "precondition");

    screen.close_focused_pane(client_id, None).expect("TEST");

    assert!(
        !screen.quit_is_guarded(),
        "the guard goes with the pane that raised it",
    );
}

/// The close-pane gesture from a client with no tab of its own falls back to the first client
/// there is - and the teardown has to follow it to *that* client's pane.
///
/// `Screen::close_focused_pane` used to read the pane off the requesting client while letting
/// `active_tab_and_connected_client_id!` close the fallback client's pane, so the pane that
/// actually went away kept its quit guard: a session that can never close its last pane again.
#[test]
fn closing_a_pane_for_a_client_with_no_tab_tears_down_the_pane_that_closed() {
    let (mut screen, _screen_receiver, _server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1)]);
    let plugin_id = PluginId::new(42);
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        plugin_id,
    );
    screen.set_quit_notice(plugin_id, Some("still working".to_owned()), true);
    assert!(screen.quit_is_guarded(), "precondition");
    assert!(
        screen.get_active_tab(CLIENT_2).is_err(),
        "precondition: client 2 is not attached",
    );

    let closed = screen.close_focused_pane(CLIENT_2, None).expect("TEST");

    assert_eq!(
        closed,
        Some(PaneId::Plugin(plugin_id)),
        "the fallback client's pane is the one that closed, and is what is reported",
    );
    assert!(
        !screen.quit_is_guarded(),
        "the guard goes with the pane that actually closed",
    );
}

/// The dead terminal a hook pane takes with it takes its nested-guest state too.
///
/// `Tab::close_dead_subject_of` destroys the suppressed terminal from inside the tab, so the
/// screen only ever heard about the stand-in - and the guest state keyed on a terminal id that
/// gets recycled becomes some later pane's passthrough and ascend keys.
#[test]
fn closing_a_hook_pane_over_a_dead_terminal_clears_the_terminals_guest_state() {
    let (mut screen, _screen_receiver, _server_receiver) =
        screen_with_plugin_senders(&[TabId::new(1)]);
    let client_id = crate::ClientId::new(1);
    let subject = PaneId::Terminal(crate::TerminalId::new(1));
    hook_pane_over_a_held_terminal(
        &mut screen,
        TabId::new(1),
        crate::TerminalId::new(1),
        PluginId::new(42),
    );
    screen
        .nested_guest_tracker
        .on_announce(subject, std::time::Instant::now());
    screen.nested_guest_choices.insert(
        (client_id, subject),
        super::super::NestedGuestChoice::Descend,
    );
    screen
        .pane_output_activity
        .insert(subject, std::time::Instant::now());

    screen.close_focused_pane(client_id, None).expect("TEST");

    assert!(
        !screen.nested_guest_tracker.is_tracked(subject),
        "the terminal that went with the hook pane is no longer a tracked guest",
    );
    assert!(
        !screen
            .nested_guest_choices
            .contains_key(&(client_id, subject)),
        "and no client is left descended into it",
    );
    assert!(
        !screen.pane_output_activity.contains_key(&subject),
        "and nothing is left keyed on an id that will be handed out again",
    );
}

#[test]
fn disconnected_pty_does_not_interrupt_tab_teardown() {
    let (mut screen, _screen_receiver, _server_receiver, _plugin_receiver) =
        screen_with_plugin_senders_and_clients(
            &[TabId::new(1), TabId::new(2), TabId::new(3)],
            &[crate::ClientId::new(1)],
        );
    let (sender, receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    drop(receiver);
    screen.bus.senders.should_silently_fail = false;
    let (background_sender, _background_receiver) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background_sender));
    assert!(screen
        .bus
        .senders
        .send_to_pty(PtyInstruction::Exit)
        .is_err());
    screen
        .go_to_tab(
            zellij_types::internal_ids::TabNumber::new(1),
            crate::ClientId::new(1),
        )
        .unwrap();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_every_client_has_a_tab(&screen);
    assert_eq!(screen.tabs[&TabId::new(2)].position.get(), 0);
    assert_eq!(screen.tabs[&TabId::new(3)].position.get(), 1);
    screen
        .go_to_tab(
            zellij_types::internal_ids::TabNumber::new(2),
            crate::ClientId::new(1),
        )
        .unwrap();
    assert_eq!(
        screen.active_tab_ids[&crate::ClientId::new(1)],
        TabId::new(3)
    );
}

#[test]
fn pending_tabs_are_not_relocation_destinations_and_cancel_obsolete_confirmation() {
    let (mut screen, receiver, _, plugins) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);
    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().unwrap();
    let replacement = requested_tabs(&plugins)[0];
    let other = screen.get_new_tab_id();
    screen
        .new_tab(other, (vec![], vec![]), None, Some(crate::ClientId::new(1)))
        .unwrap();
    screen.settle_tabs().unwrap();
    assert!(screen.tabs.contains_key(&TabId::new(1)));
    assert_eq!(
        screen.active_tab_ids[&crate::ClientId::new(1)],
        TabId::new(1)
    );
    load_replacement_tab(&mut screen, other, 3);
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&crate::ClientId::new(1)], other);
    assert!(!screen.tabs.contains_key(&TabId::new(1)));
    assert_eq!(launches(&receiver), 0);
    load_replacement_tab(&mut screen, replacement, 2);
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&crate::ClientId::new(1)], other);
    assert_eq!(launches(&receiver), 0);
}

#[test]
fn internal_tab_closure_waits_for_a_loaded_destination() {
    for empty_first in [true, false] {
        let source = TabId::new(1);
        let destination = TabId::new(2);
        let client = crate::ClientId::new(1);
        let hidden = PaneId::Terminal(crate::TerminalId::new(8));
        let overlay = PaneId::Plugin(HOCUS_PLUGIN_ID);
        let respawn = (RunCommand::default(), true);
        let (mut screen, receiver, server, plugins) =
            screen_with_plugin_senders_and_clients(&[source], &[client, CLIENT_2]);
        let (to_pty, pty_receiver) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        let tab = screen.tabs.get_mut(&source).unwrap();
        for (id, placement) in [
            (hidden, NewPanePlacement::default()),
            (
                overlay,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            ),
        ] {
            tab.new_pane(id, None, None, false, true, placement, Some(client), None)
                .unwrap();
        }
        tab.suppress_pane(hidden, None);
        tab.restore_pending_respawn(hidden, respawn.clone());
        screen.set_quit_notice(HOCUS_PLUGIN_ID, Some("working".into()), true);
        screen
            .new_tab(destination, (vec![], vec![]), None, Some(client))
            .unwrap();

        if empty_first {
            screen.empty_and_close_tab(source);
        } else {
            screen.close_tab_by_id(source).unwrap();
        }
        screen.settle_tabs().unwrap();
        assert!(screen.deferred_tab_closes.contains(&source));
        assert_eq!(screen.active_tab_ids[&client], source);
        assert_eq!(screen.active_tab_ids[&CLIENT_2], source);
        assert_every_client_has_a_tab(&screen);
        let tab = screen.tabs.get_mut(&source).unwrap();
        assert!(tab.has_pane_with_pid(&hidden));
        assert!(tab.pane_is_overlay(overlay));
        assert_eq!(tab.take_pending_respawn(hidden), Some(respawn.clone()));
        tab.restore_pending_respawn(hidden, respawn.clone());
        assert_eq!(launches(&receiver), 0);
        assert!(requested_tabs(&plugins).is_empty());
        assert!(pty_receiver.try_recv().is_err());
        assert!(!session_ended(&server));

        // ApplyLayout must leave focus alone: settlement owns the client relocation.
        load_replacement_tab(&mut screen, destination, 2);
        assert_eq!(screen.active_tab_ids[&client], source);
        screen.settle_tabs().unwrap();
        assert!(!screen.tabs.contains_key(&source));
        assert!(screen.deferred_tab_closes.is_empty());
        assert_eq!(screen.active_tab_ids[&client], destination);
        assert_eq!(screen.active_tab_ids[&CLIENT_2], destination);
        assert_every_client_has_a_tab(&screen);
        let tab = screen.tabs.get_mut(&destination).unwrap();
        assert_eq!(tab.position.get(), 0);
        assert!(tab.has_pane_with_pid(&hidden));
        assert!(tab.pane_is_overlay(overlay));
        assert_eq!(tab.take_pending_respawn(hidden), Some(respawn.clone()));
        assert!(screen.quit_is_guarded());
        assert_eq!(launches(&receiver), 0);
        assert!(!session_ended(&server));
        let closed: Vec<_> = pty_receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PtyInstruction::CloseTab(ids) => ids,
                PtyInstruction::ClosePane(id, _) => vec![id],
                _ => vec![],
            })
            .collect();
        assert_eq!(
            closed,
            if empty_first {
                vec![]
            } else {
                vec![PaneId::Terminal(crate::TerminalId::new(1))]
            }
        );
    }
}

#[test]
fn failed_pending_destination_does_not_block_internal_tab_closure() {
    let source = TabId::new(1);
    let failed = TabId::new(2);
    let (mut screen, receiver, server, plugins) =
        screen_with_plugin_senders_and_clients(&[source], &[crate::ClientId::new(1)]);
    screen
        .new_tab(failed, (vec![], vec![]), None, None)
        .unwrap();
    screen.empty_and_close_tab(source);
    screen.settle_tabs().unwrap();
    assert!(screen.tabs.contains_key(&source));

    screen.close_tab_by_id(failed).unwrap();
    assert!(!screen.has_pending_tabs());
    assert_eq!(screen.tabs[&source].position.get(), 0);
    screen.settle_tabs().unwrap();
    assert!(screen.tabs.is_empty());
    assert!(screen.deferred_tab_closes.is_empty());
    assert!(screen.active_tab_ids.is_empty());
    assert!(session_ended(&server));
    assert_eq!(launches(&receiver), 0);
    assert!(requested_tabs(&plugins).is_empty());
}

#[test]
fn hidden_only_non_last_tab_keeps_its_identity_and_terminal() {
    let (mut screen, _, _, plugins) = screen_with_plugin_senders_and_clients(
        &[TabId::new(1), TabId::new(2)],
        &[crate::ClientId::new(1)],
    );
    let id = PaneId::Terminal(crate::TerminalId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .suppress_pane(id, None);
    screen.settle_tabs().unwrap();
    assert!(screen.tabs[&TabId::new(1)].has_pane_with_pid(&id));
    assert!(requested_tabs(&plugins).is_empty());
}

#[test]
fn homeless_plugin_drops_its_notice_before_settlement() {
    let (mut screen, _, server, plugins) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);
    screen.close_homeless_pane(PaneId::Plugin(HOCUS_PLUGIN_ID));
    assert!(!screen.quit_is_guarded());
    close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
    screen.settle_tabs().unwrap();
    assert!(session_ended(&server));
    assert!(requested_tabs(&plugins).is_empty());
}

#[test]
fn deferred_focus_uses_stable_ids_after_earlier_tab_removal() {
    let (mut screen, _, _, _) = screen_with_plugin_senders_and_clients(
        &[TabId::new(1), TabId::new(2), TabId::new(3)],
        &[crate::ClientId::new(1)],
    );
    let mut switches = HashSet::from([(TabId::new(3), crate::ClientId::new(1))]);
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    screen.apply_deferred_tab_switches(&mut switches).unwrap();
    assert_eq!(
        screen.active_tab_ids[&crate::ClientId::new(1)],
        TabId::new(3)
    );
    switches.insert((TabId::new(1), crate::ClientId::new(1)));
    screen.apply_deferred_tab_switches(&mut switches).unwrap();
    assert_eq!(
        screen.active_tab_ids[&crate::ClientId::new(1)],
        TabId::new(3)
    );
}

#[test]
fn explicit_last_tab_close_checks_guards_and_preserves_hidden_work() {
    for focused in [false, true] {
        let (mut screen, receiver, server, plugins) =
            screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
        screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);
        if focused {
            screen.close_tab(crate::ClientId::new(1)).unwrap();
        } else {
            screen.request_close_tab(TabId::new(1), None).unwrap();
        }
        assert_eq!(launches(&receiver), 1);
        assert!(screen.tabs.contains_key(&TabId::new(1)));
        screen.drop_quit_notice(HOCUS_PLUGIN_ID);
        let id = PaneId::Terminal(crate::TerminalId::new(1));
        screen
            .tabs
            .get_mut(&TabId::new(1))
            .unwrap()
            .suppress_pane(id, None);
        if focused {
            screen.close_tab(crate::ClientId::new(1)).unwrap();
        } else {
            screen.request_close_tab(TabId::new(1), None).unwrap();
        }
        let replacement = requested_tabs(&plugins)[0];
        assert!(screen.tabs[&TabId::new(1)].has_pane_with_pid(&id));
        load_replacement_tab(&mut screen, replacement, 2);
        screen.settle_tabs().unwrap();
        assert!(screen.tabs[&replacement].has_pane_with_pid(&id));
        assert!(!screen.tabs.contains_key(&TabId::new(1)));
        assert!(!session_ended(&server));
    }
}

#[test]
fn automatic_replacement_preserves_guarding_plugins_in_every_layer() {
    for layer in 0..5 {
        let (mut screen, receiver, _, plugins) =
            screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
        let id = PaneId::Plugin(HOCUS_PLUGIN_ID);
        let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
        let placement = match layer {
            1 => NewPanePlacement::Floating(None),
            3 => NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            4 => NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
            _ => NewPanePlacement::default(),
        };
        tab.new_pane(
            id,
            None,
            None,
            false,
            true,
            placement,
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
        if layer == 2 {
            tab.suppress_pane(id, None);
        } else {
            tab.set_pane_selectable(id, false);
        }
        screen.set_quit_notice(HOCUS_PLUGIN_ID, Some("working".into()), true);
        close_pane(&mut screen, PaneId::Terminal(crate::TerminalId::new(1)));
        screen.settle_tabs().unwrap();
        let replacement = requested_tabs(&plugins)[0];
        load_replacement_tab(&mut screen, replacement, 2);
        screen.settle_tabs().unwrap();
        assert!(
            screen.tabs[&replacement].has_pane_with_pid(&id),
            "layer {layer}"
        );
        assert!(screen.quit_is_guarded(), "layer {layer}");
        assert_eq!(launches(&receiver), 1, "layer {layer}");
    }
}

#[test]
fn all_tab_close_instructions_confirm_before_destroying_the_last_tab() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let client = mock.main_client_id;
    let screen_thread = mock.run(None, vec![]);
    let receiver = mock.pty_receiver.take().unwrap();
    mock.to_screen
        .send(ScreenInstruction::SetQuitNotice(
            HOCUS_PLUGIN_ID,
            None,
            true,
        ))
        .unwrap();
    let requests = [
        ScreenInstruction::CloseTab(client, None),
        ScreenInstruction::CloseTabWithId(TabId::new(0), None),
        ScreenInstruction::CloseTabWithIndex(zellij_types::internal_ids::TabPosition::new(0)),
    ];
    for request in requests {
        mock.to_screen.send(request).unwrap();
        loop {
            let (instruction, _) = receiver
                .recv_timeout(zellij_test_support::default_timeout())
                .unwrap();
            if matches!(instruction, PtyInstruction::FillPluginCwd(..)) {
                break;
            }
        }
    }
    mock.teardown(vec![screen_thread]);
}

#[test]
fn replacement_exit_buffered_before_layout_is_held_after_settlement() {
    let size = Size { cols: 80, rows: 24 };
    let mut mock = MockScreen::new(size);
    let client = mock.main_client_id;
    let screen_thread = mock.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugins = mock.plugin_receiver.take().unwrap();
    mock.to_screen
        .send(ScreenInstruction::SetQuitNotice(
            HOCUS_PLUGIN_ID,
            None,
            true,
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Terminal(crate::TerminalId::new(0)),
            None,
            None,
            Some(0),
        ))
        .unwrap();
    let replacement = loop {
        let (instruction, _) = plugins
            .recv_timeout(zellij_test_support::default_timeout())
            .unwrap();
        if let PluginInstruction::NewTab(_, _, _, _, id, ..) = instruction {
            if id != TabId::new(0) {
                break id;
            }
        }
    };
    // The screen receives this before it knows which tab terminal 2 belongs to.
    mock.to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            Some(23),
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(crate::TerminalId::new(2), None)],
            vec![],
            vec![],
            HashMap::new(),
            replacement,
            false,
            client,
            None,
            None,
        ))
        .unwrap();
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "buffered replacement exit is held",
        |frame| frame.contains("EXIT CODE: 23"),
    );
    assert!(std::iter::from_fn(|| plugins.try_recv().ok())
        .all(|(instruction, _)| !matches!(instruction, PluginInstruction::NewTab(..))));
    mock.teardown(vec![server_thread, screen_thread]);
}

#[test]
fn focused_last_tab_confirmation_goes_to_the_requesting_client() {
    let (mut screen, receiver, _, _) = screen_with_plugin_senders_and_clients(
        &[TabId::new(1)],
        &[crate::ClientId::new(1), CLIENT_2],
    );
    screen.set_quit_notice(HOCUS_PLUGIN_ID, None, true);
    screen.close_tab(CLIENT_2).unwrap();
    assert!(
        matches!(launched_plugin(&receiver), Some(ScreenInstruction::LaunchPlugin(
        _, _, _, _, _, _, _, client, ..
    )) if client == CLIENT_2)
    );
}

#[test]
fn background_tab_overlays_skip_pending_destinations() {
    let (mut screen, _, _, _) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    let id = PaneId::Plugin(HOCUS_PLUGIN_ID);
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .new_pane(
            id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    screen
        .new_tab(
            TabId::new(2),
            (vec![], vec![]),
            None,
            Some(crate::ClientId::new(1)),
        )
        .unwrap();
    new_tab(&mut screen, 3, TabId::new(3));
    screen
        .go_to_tab(
            zellij_types::internal_ids::TabNumber::new(3),
            crate::ClientId::new(1),
        )
        .unwrap();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert!(screen.tabs[&TabId::new(3)].has_pane_with_pid(&id));
    assert!(!screen.tabs[&TabId::new(2)].has_pane_with_pid(&id));
}
