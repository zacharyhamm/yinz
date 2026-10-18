//! Confirming a quit: the `Ctrl-q` route, the last pane closing, and the notices both carry.

#[cfg(test)]
const CLIENT_2: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(2);

use super::*;
use crate::screen::PluginId;
use yinz_types::internal_ids::TabId;
use {yinz_types::data::OverlayKind, yinz_types::data::OverlayPlacement};

type ScreenReceiver = Receiver<(ScreenInstruction, ErrorContext)>;

#[test]
fn last_pane_exit_returns_each_client_to_the_previous_usable_tab() {
    for unavailable in [None, Some("missing"), Some("pending"), Some("hidden")] {
        let client = ClientId::new(1);
        let (mut screen, _, _, _plugins) = screen_with_plugin_senders_and_clients(
            &[TabId::new(1), TabId::new(2), TabId::new(3)],
            &[client, CLIENT_2],
        );
        screen.session_is_mirrored = false;
        screen
            .go_to_tab(yinz_types::internal_ids::TabNumber::new(2), client)
            .unwrap();
        screen
            .go_to_tab(yinz_types::internal_ids::TabNumber::new(3), client)
            .unwrap();
        assert_eq!(screen.tab_history[&client].last(), Some(&TabId::new(2)));
        screen.tab_history.insert(CLIENT_2, vec![TabId::new(1)]);
        match unavailable {
            Some("missing") => {
                screen.tab_history.insert(client, vec![TabId::new(99)]);
            },
            Some("pending") => {
                let pending = TabId::new(4);
                screen
                    .new_tab(pending, (vec![], vec![]), None, None)
                    .unwrap();
                screen.tab_history.insert(client, vec![pending]);
            },
            Some("hidden") => {
                screen
                    .tabs
                    .get_mut(&TabId::new(2))
                    .unwrap()
                    .suppress_pane(PaneId::Terminal(TerminalId::new(2)), None);
            },
            _ => {},
        }
        screen.tabs.get_mut(&TabId::new(3)).unwrap().close_pane(
            PaneId::Terminal(TerminalId::new(3)),
            false,
            Some(0),
        );
        screen.settle_tabs().unwrap();
        assert_eq!(
            screen.active_tab_ids[&client],
            TabId::new(if unavailable.is_some() { 1 } else { 2 })
        );
        assert_eq!(screen.active_tab_ids[&CLIENT_2], TabId::new(1));
        assert!(!screen.tabs.contains_key(&TabId::new(3)));
        assert!(screen.tabs.contains_key(&TabId::new(2)));
        assert_every_client_has_a_tab(&screen);
    }
}

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

const AGENT_PLUGIN_ID: PluginId = crate::PluginId::new(7);

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
                location: RunPluginLocation::Yinz(plugin_tag),
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
            background_opacity: None,
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
        AGENT_PLUGIN_ID,
        Some("agent is still working".to_owned()),
        true,
    );
    screen
        .confirm_quit(crate::ClientId::new(1), false, None)
        .expect("TEST");

    let launch = launched_plugin(&screen_receiver).expect("the modal is launched");
    let (_, configuration, _) = launch_details(&launch);
    assert_eq!(
        configuration.get("notice").map(String::as_str),
        Some("agent is still working")
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
    screen.set_quit_notice(AGENT_PLUGIN_ID, Some("working".to_owned()), true);
    assert!(screen.quit_is_guarded());

    screen.set_quit_notice(AGENT_PLUGIN_ID, None, false);
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

#[test]
fn last_pane_close_ends_the_session_with_hidden_terminals() {
    for explicit in [false, true] {
        for hidden_in_other_tab in [false, true] {
            let client = ClientId::new(1);
            let source = TabId::new(1);
            let tab_ids = if hidden_in_other_tab {
                vec![source, TabId::new(2)]
            } else {
                vec![source]
            };
            let (mut screen, receiver, server, plugins) =
                screen_with_plugin_senders_and_clients(&tab_ids, &[client]);
            let hidden = PaneId::Terminal(TerminalId::new(1));
            let visible = PaneId::Terminal(TerminalId::new(2));
            let tab = screen.tabs.get_mut(&source).unwrap();
            if !hidden_in_other_tab {
                tab.new_pane(
                    visible,
                    None,
                    None,
                    false,
                    false,
                    NewPanePlacement::default(),
                    Some(client),
                    None,
                )
                .unwrap();
            }
            tab.suppress_pane(hidden, None);
            screen.settle_tabs().unwrap();
            let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
            for tab in screen.tabs.values_mut() {
                tab.senders.to_pty = screen.bus.senders.to_pty.clone();
            }

            screen
                .close_pane_from_instruction(
                    visible,
                    explicit.then_some(client),
                    None,
                    (!explicit).then_some(0),
                    &mut Default::default(),
                )
                .unwrap();
            screen.settle_tabs().unwrap();

            assert!(screen.tabs.is_empty());
            assert!(session_ended(&server));
            assert!(requested_tabs(&plugins).is_empty());
            assert_eq!(launches(&receiver), 0);
            let closed: HashSet<_> = pty_receiver
                .try_iter()
                .flat_map(|(instruction, _)| match instruction {
                    PtyInstruction::ClosePane(id, _) => vec![id],
                    PtyInstruction::CloseTab(ids) => ids,
                    _ => vec![],
                })
                .collect();
            assert!(closed.contains(&visible));
            assert!(closed.contains(&hidden));
        }
    }
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
        AGENT_PLUGIN_ID,
        Some("agent is still working".to_owned()),
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
        Some("agent is still working")
    );
    assert_eq!(
        placement,
        Some(OverlayPlacement {
            background_opacity: None,
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
    screen.set_quit_notice(AGENT_PLUGIN_ID, Some("agent is still working".into()), true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);

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
    screen.set_quit_notice(AGENT_PLUGIN_ID, Some("working".to_owned()), true);
    assert!(screen.quit_is_guarded());

    screen.drop_quit_notice(AGENT_PLUGIN_ID);
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
) -> (Screen, ScreenReceiver, ServerReceiver, PluginReceiver) {
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
        yinz_bus::pty::ReplacedPane::Suppress,
        None, // run
        None, // completion_tx
        None, // borderless
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
        let focused = tab
            .get_active_pane_id(*client_id)
            .expect("client has a focused pane");
        let pane = tab.get_pane_with_id(focused).expect("focused pane exists");
        assert!(
            pane.selectable(),
            "client is focused on unselectable chrome"
        );
        assert!(tab.has_non_suppressed_pane_with_pid(&focused));
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
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client_id)
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
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client_id)
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
/// `Tab::close_pane_explicitly` destroys the suppressed terminal from inside the tab, so the
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
            yinz_types::internal_ids::TabNumber::new(1),
            crate::ClientId::new(1),
        )
        .unwrap();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_every_client_has_a_tab(&screen);
    assert_eq!(screen.tabs[&TabId::new(2)].position.get(), 0);
    assert_eq!(screen.tabs[&TabId::new(3)].position.get(), 1);
    screen
        .go_to_tab(
            yinz_types::internal_ids::TabNumber::new(2),
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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);
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
        let overlay = PaneId::Plugin(AGENT_PLUGIN_ID);
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
            // Both clients need focus on the overlay before the tiled pane is suppressed.
            tab.new_pane(id, None, None, false, true, placement, None, None)
                .unwrap();
        }
        tab.suppress_pane(hidden, None);
        tab.restore_pending_respawn(hidden, respawn.clone());
        screen.set_quit_notice(AGENT_PLUGIN_ID, Some("working".into()), true);
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
    for mirrored in [false, true] {
        for route in [
            "number", "name", "next", "previous", "history", "left", "right", "deferred",
        ] {
            let source = TabId::new(1);
            let destination = TabId::new(2);
            let client = ClientId::new(1);
            let other = ClientId::new(2);
            let (mut screen, _, _, plugins) =
                screen_with_plugin_senders_and_clients(&[source, destination], &[client, other]);
            screen.session_is_mirrored = mirrored;
            screen
                .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
                .unwrap();
            let id = PaneId::Terminal(TerminalId::new(1));
            let tab = screen.tabs.get_mut(&source).unwrap();
            tab.name = "parked".into();
            tab.handle_pty_bytes(TerminalId::new(1), b"saved parked output".to_vec())
                .unwrap();
            for plugin in [PluginId::new(41), PluginId::new(42)] {
                let bar = PaneId::Plugin(plugin);
                tab.new_pane(
                    bar,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::default(),
                    Some(client),
                    None,
                )
                .unwrap();
                tab.set_pane_selectable(bar, false);
            }
            tab.suppress_pane(id, None);
            screen.settle_tabs().unwrap();
            assert_eq!(screen.active_tab_ids[&client], destination);
            assert!(!screen.tabs[&source].has_selectable_panes());
            assert!(screen.tabs[&source].has_pane_with_pid(&id));
            assert!(requested_tabs(&plugins).is_empty());
            match route {
                "number" => screen
                    .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
                    .unwrap(),
                "name" => assert!(screen.go_to_tab_name("parked".into(), client).unwrap()),
                "next" => screen.switch_tab_next(None, true, client).unwrap(),
                "previous" => screen.switch_tab_prev(None, true, client).unwrap(),
                "history" => screen.toggle_tab(client).unwrap(),
                "deferred" => screen
                    .apply_deferred_tab_switches(&mut HashSet::from([(source, client)]))
                    .unwrap(),
                "left" => screen
                    .switch_tab_prev(Some(Direction::Left), true, client)
                    .unwrap(),
                "right" => screen
                    .switch_tab_next(Some(Direction::Right), true, client)
                    .unwrap(),
                _ => unreachable!(),
            }
            for _ in 0..3 {
                screen.settle_tabs().unwrap();
                assert_eq!(
                    screen.active_tab_ids[&client], source,
                    "{route}, mirrored={mirrored}"
                );
                assert_eq!(screen.tabs[&source].get_active_pane_id(client), Some(id));
                assert!(screen.tabs[&source].are_floating_panes_visible());
            }
            assert_eq!(
                screen.active_tab_ids[&other],
                if mirrored { source } else { destination }
            );
            assert!(screen.tabs[&source]
                .get_pane_with_id(id)
                .unwrap()
                .dump_screen(true, None)
                .contains("saved parked output"));
            assert_every_client_has_a_tab(&screen);
        }
    }
}

#[test]
fn settlement_repairs_floating_focus_after_last_tiled_exit_alone_or_beside_another_tab() {
    for another_tab in [false, true] {
        let source = TabId::new(1);
        let client = ClientId::new(1);
        let tabs = if another_tab {
            vec![source, TabId::new(2)]
        } else {
            vec![source]
        };
        let (mut screen, _, _, plugins) = screen_with_plugin_senders_and_clients(&tabs, &[client]);
        screen
            .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
            .unwrap();
        let tab = screen.tabs.get_mut(&source).unwrap();
        let floating = PaneId::Terminal(TerminalId::new(3));
        tab.new_pane(
            floating,
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(client),
            None,
        )
        .unwrap();
        tab.hide_floating_panes();
        tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
        // Also exercise settlement itself, independently of removal's immediate repair.
        tab.hide_floating_panes();
        screen.settle_tabs().unwrap();
        assert_eq!(screen.active_tab_ids[&client], source);
        assert_eq!(
            screen.tabs[&source].get_active_pane_id(client),
            Some(floating)
        );
        assert!(screen.tabs[&source].are_floating_panes_visible());
        assert_every_client_has_a_tab(&screen);
        assert!(requested_tabs(&plugins).is_empty());
    }
}

#[test]
fn tab_teardown_records_destruction_but_not_held_or_relocated_panes() {
    let source = TabId::new(1);
    let destination = TabId::new(2);
    let client = ClientId::new(1);
    let (mut screen, _, _, _) =
        screen_with_plugin_senders_and_clients(&[source, destination], &[client]);
    let destroyed = PaneId::Terminal(TerminalId::new(1));
    let moved = PaneId::Terminal(TerminalId::new(8));
    let tab = screen.tabs.get_mut(&source).unwrap();
    tab.new_pane(
        moved,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    tab.hold_pane(destroyed, Some(0), false, RunCommand::default());
    tab.suppress_pane(moved, None);
    assert!(screen.destroyed_panes.is_empty());
    screen.close_tab_by_id(source).unwrap();
    assert_eq!(screen.destroyed_panes, HashSet::from([destroyed]));
    assert!(screen.tabs[&destination].has_pane_with_pid(&moved));
    screen.close_pane_explicitly(moved, None);
    assert!(screen.destroyed_panes.contains(&moved));
    assert!(!screen.tabs[&destination].has_pane_with_pid(&moved));
    let discarded = PaneId::Terminal(TerminalId::new(9));
    screen.forget_unplaced_pane(discarded, "destination closed during spawn".into());
    assert!(screen.destroyed_panes.contains(&discarded));
}

#[test]
fn homeless_plugin_drops_its_notice_before_settlement() {
    let (mut screen, _, server, plugins) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);
    screen.close_homeless_pane(PaneId::Plugin(AGENT_PLUGIN_ID));
    assert!(screen
        .destroyed_panes
        .contains(&PaneId::Plugin(AGENT_PLUGIN_ID)));
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
fn explicit_last_tab_close_checks_guards_then_exits_with_hidden_work() {
    for focused in [false, true] {
        let (mut screen, receiver, server, plugins) =
            screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
        screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);
        if focused {
            screen.close_tab(crate::ClientId::new(1)).unwrap();
        } else {
            screen.request_close_tab(TabId::new(1), None).unwrap();
        }
        assert_eq!(launches(&receiver), 1);
        assert!(screen.tabs.contains_key(&TabId::new(1)));
        screen.drop_quit_notice(AGENT_PLUGIN_ID);
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
        screen.settle_tabs().unwrap();
        assert!(screen.tabs.is_empty());
        assert!(requested_tabs(&plugins).is_empty());
        assert!(session_ended(&server));
    }
}

#[test]
fn automatic_replacement_preserves_guarding_plugins_in_every_layer() {
    for layer in 0..5 {
        let (mut screen, receiver, _, plugins) =
            screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[crate::ClientId::new(1)]);
        let id = PaneId::Plugin(AGENT_PLUGIN_ID);
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
        screen.set_quit_notice(AGENT_PLUGIN_ID, Some("working".into()), true);
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
            AGENT_PLUGIN_ID,
            None,
            true,
        ))
        .unwrap();
    let requests = [
        ScreenInstruction::CloseTab(client, None),
        ScreenInstruction::CloseTabWithId(TabId::new(0), None),
        ScreenInstruction::CloseTabWithIndex(yinz_types::internal_ids::TabPosition::new(0)),
    ];
    for request in requests {
        mock.to_screen.send(request).unwrap();
        loop {
            let (instruction, _) = receiver
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            if matches!(instruction, PtyInstruction::FillPluginCwd(..)) {
                break;
            }
        }
    }
    mock.teardown(vec![screen_thread]);
}

#[test]
fn cli_and_ctrl_c_closes_destroy_replacement_chains_and_preserve_input() {
    for route in ["terminal_2", "plugin_42", "terminal_3", "held", "suspended"] {
        let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
        let client = mock.main_client_id;
        let session_metadata = mock.clone_session_metadata();
        let screen_thread = mock.run(None, vec![]);
        let pty_receiver = mock.pty_receiver.take().unwrap();
        let writer_receiver = mock.pty_writer_receiver.take().unwrap();
        let plugin_receiver = mock.plugin_receiver.take().unwrap();
        let pty_log = Watched::new(vec![]);
        let writer_log = Watched::new(vec![]);
        let plugin_log = Watched::new(vec![]);
        let pty_thread = log_actions_in_thread!(pty_log, PtyInstruction::Exit, pty_receiver);
        let writer_thread =
            log_actions_in_thread!(writer_log, PtyWriteInstruction::Exit, writer_receiver);
        let plugin_thread =
            log_actions_in_thread!(plugin_log, PluginInstruction::Exit, plugin_receiver);
        let source = PaneId::Terminal(TerminalId::new(2));
        let middle = PaneId::Plugin(PluginId::new(42));
        let visible = PaneId::Terminal(TerminalId::new(3));
        mock.to_screen
            .send(ScreenInstruction::NewPane(
                source,
                None,
                None,
                None,
                NewPanePlacement::default(),
                false,
                ClientTabIndexOrPaneId::ClientId(client),
                None,
                false,
            ))
            .unwrap();
        for (replacement, replaced) in [(middle, source), (visible, middle)] {
            mock.to_screen
                .send(ScreenInstruction::ReplacePane(
                    replacement,
                    None,
                    None,
                    None,
                    yinz_bus::pty::ReplacedPane::Suppress,
                    ClientTabIndexOrPaneId::PaneId(replaced),
                    None,
                ))
                .unwrap();
        }
        let mut input_completion = None;
        if matches!(route, "held" | "suspended") {
            let stopped = if route == "held" {
                ScreenInstruction::HoldPane(visible, Some(0), RunCommand::default())
            } else {
                ScreenInstruction::SetPaneSuspended(visible, true)
            };
            mock.to_screen.send(stopped).unwrap();
            let (tx, rx) = tokio::sync::oneshot::channel();
            input_completion = Some(rx);
            mock.to_screen
                .send(ScreenInstruction::WriteCharacter(
                    None,
                    vec![3],
                    false,
                    client,
                    Some(crate::NotificationEnd::new(tx)),
                ))
                .unwrap();
        } else {
            send_cli_action_to_server(
                &session_metadata,
                CliAction::ClosePane {
                    pane_id: Some(route.to_owned()),
                },
                ClientId::new(10),
            );
        }
        for id in [source, middle, visible] {
            pty_log.wait_until(
                &format!("{route} tears down {id:?}"),
                yinz_test_support::default_timeout(),
                |log| {
                    log.iter()
                        .any(|instruction| {
                            matches!(instruction,
                    PtyInstruction::ClosePane(closed, _) if *closed == id)
                        })
                        .then_some(())
                },
            );
            plugin_log.wait_until(
                &format!("{route} removes {id:?}"),
                yinz_test_support::default_timeout(),
                |log| {
                    log.iter()
                        .any(|instruction| {
                            matches!(instruction,
                    PluginInstruction::Update(events) if events.iter().any(|(_, _, event)|
                        matches!(event, Event::PaneClosed(closed) if *closed == id)))
                        })
                        .then_some(())
                },
            );
        }
        // Untargeted input proves that the surviving pane has keyboard focus.
        mock.to_screen
            .send(ScreenInstruction::WriteCharacter(
                None,
                b"still focused".to_vec(),
                false,
                client,
                None,
            ))
            .unwrap();
        writer_log.wait_until(
            "input reaches the surviving pane",
            yinz_test_support::default_timeout(),
            |log| {
                log.iter()
                    .any(|instruction| {
                        matches!(instruction, PtyWriteInstruction::Write(bytes, id, _)
                if bytes == b"still focused" && *id == TerminalId::new(0))
                    })
                    .then_some(())
            },
        );
        if let Some(mut completion) = input_completion {
            assert!(completion.try_recv().is_ok(), "Ctrl+C completion was lost");
        }
        mock.teardown(vec![
            pty_thread,
            writer_thread,
            plugin_thread,
            screen_thread,
        ]);
        for id in [source, middle, visible] {
            assert_eq!(
                pty_log
                    .lock()
                    .iter()
                    .filter(|instruction| matches!(instruction,
                PtyInstruction::ClosePane(closed, _) if *closed == id))
                    .count(),
                1
            );
            assert_eq!(
                plugin_log
                    .lock()
                    .iter()
                    .filter_map(|instruction| match instruction {
                        PluginInstruction::Update(events) => Some(events),
                        _ => None,
                    })
                    .flatten()
                    .filter(
                        |(_, _, event)| matches!(event, Event::PaneClosed(closed) if *closed == id)
                    )
                    .count(),
                1
            );
        }
    }
}

#[test]
fn buffered_explicit_and_self_closes_keep_their_order_and_complete_once() {
    for explicit_first in [true, false] {
        for with_client in [true, false] {
            let client = ClientId::new(1);
            let (mut screen, _screen_receiver, _, plugins) =
                screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[client]);
            let (background_tx, background_rx) = channels::unbounded();
            screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background_tx));
            let source = PaneId::Terminal(TerminalId::new(1));
            let replacement = PaneId::Plugin(PluginId::new(42));
            hook_pane_over_a_held_terminal(
                &mut screen,
                TabId::new(1),
                TerminalId::new(1),
                PluginId::new(42),
            );
            screen.set_quit_notice(PluginId::new(42), None, true);
            screen
                .pane_output_activity
                .insert(source, std::time::Instant::now());
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            let mut completion = crate::NotificationEnd::new(tx);
            completion.set_exit_status(23);
            let explicit = ScreenInstruction::ClosePane(
                replacement,
                with_client.then_some(client),
                Some(completion),
                None,
            );
            let close_self = ScreenInstruction::ClosePane(replacement, None, None, None);
            let mut events = if explicit_first {
                vec![explicit, close_self]
            } else {
                vec![close_self, explicit]
            };
            let (duplicate_tx, mut duplicate_rx) = tokio::sync::oneshot::channel();
            events.push(ScreenInstruction::ClosePane(
                replacement,
                None,
                Some(crate::NotificationEnd::new(duplicate_tx)),
                None,
            ));
            events.push(ScreenInstruction::ClosePane(replacement, None, None, None));

            super::super::replay_pending_plugin_events(&mut screen, events).unwrap();

            assert_eq!(rx.try_recv().unwrap().exit_status, Some(23));
            assert!(duplicate_rx.try_recv().is_ok());
            assert!(screen.tab_containing_pane(replacement).is_none());
            assert_eq!(screen.tab_containing_pane(source).is_none(), explicit_first);
            assert_eq!(
                screen.pane_output_activity.contains_key(&source),
                !explicit_first
            );
            assert!(!screen.quit_is_guarded());
            if explicit_first {
                assert!(
                    background_rx.try_iter().any(|(instruction, _)| matches!(
                        instruction,
                        BackgroundJob::RenderToClients
                    )),
                    "inline explicit close must still schedule its repaint"
                );
            }
            let closed: Vec<_> = plugins
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(events) => Some(events),
                    _ => None,
                })
                .flatten()
                .filter_map(|(_, _, event)| match event {
                    Event::PaneClosed(id) => Some(id),
                    _ => None,
                })
                .collect();
            assert_eq!(closed.iter().filter(|id| **id == replacement).count(), 1);
            assert_eq!(
                closed.iter().filter(|id| **id == source).count(),
                usize::from(explicit_first)
            );
            if !explicit_first {
                assert_eq!(
                    screen.get_active_pane_id(&client),
                    Some(source),
                    "close_self restores the source"
                );
            }
        }
    }
}

#[test]
fn explicit_close_before_creation_waits_for_removal_and_completion() {
    for plugin in [false, true] {
        for with_client in [false, true] {
            let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
            let client = mock.main_client_id;
            let screen_thread = mock.run(None, vec![]);
            let server = mock.server_receiver.take().unwrap();
            let processed = || {
                let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
                loop {
                    if matches!(
                        server
                            .recv_timeout(
                                deadline.saturating_duration_since(std::time::Instant::now()),
                            )
                            .unwrap()
                            .0,
                        ServerInstruction::Log(..)
                    ) {
                        break;
                    }
                }
            };
            let plugins = mock.plugin_receiver.take().unwrap();
            let received = Watched::new(vec![]);
            let plugin_thread = log_actions_in_thread!(received, PluginInstruction::Exit, plugins);
            let id = if plugin {
                PaneId::Plugin(crate::PluginId::new(42))
            } else {
                PaneId::Terminal(crate::TerminalId::new(42))
            };
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            let mut completion = crate::NotificationEnd::new(tx);
            completion.set_exit_status(23);
            mock.to_screen
                .send(ScreenInstruction::ClosePane(
                    id,
                    with_client.then_some(client),
                    Some(completion),
                    Some(23),
                ))
                .unwrap();
            // Both requests are replayed together when creation arrives, so the duplicate
            // precedes the close notification queued by the first request.
            let (duplicate_tx, mut duplicate_rx) = tokio::sync::oneshot::channel();
            mock.to_screen
                .send(ScreenInstruction::ClosePane(
                    id,
                    with_client.then_some(client),
                    Some(crate::NotificationEnd::new(duplicate_tx)),
                    None,
                ))
                .unwrap();
            mock.to_screen
                .send(ScreenInstruction::QueryTabNames(client, None))
                .unwrap();
            processed();
            assert!(
                matches!(
                    rx.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ),
                "close completion must wait for creation"
            );
            assert!(matches!(
                duplicate_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            let creation = if plugin {
                ScreenInstruction::AddPlugin(
                    Some(true),
                    false,
                    yinz_bus::pty::ReplacedPane::Suppress,
                    yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                        "file:/path/to/fake/plugin",
                        &None,
                        None,
                        None,
                    )
                    .unwrap(),
                    None,
                    None,
                    crate::PluginId::new(42),
                    None,
                    None,
                    false,
                    None,
                    Some(true),
                    Some(client),
                    None,
                    None,
                )
            } else {
                ScreenInstruction::NewPane(
                    id,
                    None,
                    None,
                    None,
                    NewPanePlacement::default(),
                    false,
                    ClientTabIndexOrPaneId::ClientId(client),
                    None,
                    false,
                )
            };
            mock.to_screen.send(creation).unwrap();
            let completed = crate::route::wait_for_action_completion(rx, "close", false);
            assert_eq!(
                completed.exit_status,
                Some(23),
                "the original completion is retained"
            );
            received.wait_until("buffered close removes its pane", yinz_test_support::default_timeout(), |log| {
                log.iter().any(|instruction| matches!(instruction, PluginInstruction::Update(events)
                    if events.iter().any(|(_, _, event)| matches!(event, Event::PaneClosed(closed) if *closed == id)))).then_some(())
            });
            mock.to_screen
                .send(ScreenInstruction::QueryTabNames(client, None))
                .unwrap();
            processed();
            assert!(
                duplicate_rx.try_recv().is_ok(),
                "queued duplicate must complete before another creation"
            );

            let (tx, mut rx) = tokio::sync::oneshot::channel();
            mock.to_screen
                .send(ScreenInstruction::ClosePane(
                    id,
                    with_client.then_some(client),
                    Some(crate::NotificationEnd::new(tx)),
                    None,
                ))
                .unwrap();
            mock.to_screen
                .send(ScreenInstruction::QueryTabNames(client, None))
                .unwrap();
            processed();
            let duplicate_finished = rx.try_recv().is_ok();
            mock.teardown(vec![plugin_thread, screen_thread]);
            assert!(
                duplicate_finished,
                "closing an already closed pane left completion pending"
            );
            let count = received
                .lock()
                .iter()
                .filter_map(|instruction| match instruction {
                    PluginInstruction::Update(events) => Some(events),
                    _ => None,
                })
                .flatten()
                .filter(|(_, _, event)| matches!(event, Event::PaneClosed(closed) if *closed == id))
                .count();
            assert_eq!(count, 1, "one notification per destroyed pane");
        }
    }
}

#[test]
fn close_notification_releases_requests_buffered_for_a_discarded_pane() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let client = mock.main_client_id;
    let screen_thread = mock.run(None, vec![]);
    let server = mock.server_receiver.take().unwrap();
    let processed = || {
        let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
        loop {
            if matches!(
                server
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .unwrap()
                    .0,
                ServerInstruction::Log(..)
            ) {
                break;
            }
        }
    };
    for id in [
        PaneId::Terminal(TerminalId::new(42)),
        PaneId::Plugin(PluginId::new(42)),
    ] {
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        mock.to_screen
            .send(ScreenInstruction::ClosePane(
                id,
                None,
                Some(crate::NotificationEnd::new(tx)),
                None,
            ))
            .unwrap();
        mock.to_screen
            .send(ScreenInstruction::QueryTabNames(client, None))
            .unwrap();
        processed();
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        mock.to_screen
            .send(ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id: id })
            .unwrap();
        mock.to_screen
            .send(ScreenInstruction::QueryTabNames(client, None))
            .unwrap();
        processed();
        assert!(rx.try_recv().is_ok());
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
            AGENT_PLUGIN_ID,
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
            .recv_timeout(yinz_test_support::default_timeout())
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
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);
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
    let id = PaneId::Plugin(AGENT_PLUGIN_ID);
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
            yinz_types::internal_ids::TabNumber::new(3),
            crate::ClientId::new(1),
        )
        .unwrap();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert!(screen.tabs[&TabId::new(3)].has_pane_with_pid(&id));
    assert!(!screen.tabs[&TabId::new(2)].has_pane_with_pid(&id));
}

#[test]
fn navigating_by_stable_id_recovers_suppressed_terminal_through_screen_thread() {
    let size = Size { cols: 80, rows: 24 };
    let mut mock = MockScreen::new(size);
    let client = mock.main_client_id;
    let screen_thread = mock.run(None, vec![]);
    let renders = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(renders, ServerInstruction::KillSession, receiver);
    let pty = mock.pty_receiver.take().unwrap();
    let writer = mock.pty_writer_receiver.take().unwrap();
    for instruction in [
        ScreenInstruction::PtyBytes(TerminalId::new(0), b"parked transcript".to_vec()),
        ScreenInstruction::NewTab(
            None,
            None,
            Some(TiledPaneLayout::default()),
            vec![],
            None,
            (Some(vec![]), Some(vec![])),
            None,
            false,
            true,
            client,
            None,
        ),
        ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(9), None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(1),
            true,
            client,
            None,
            None,
        ),
        ScreenInstruction::SuppressPane(PaneId::Terminal(TerminalId::new(0)), client),
        ScreenInstruction::PtyBytes(TerminalId::new(9), b"other tab".to_vec()),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    wait_for_settled_render(&renders, size, "hidden work stays hidden", |frame| {
        frame.contains("other tab") && !frame.contains("parked transcript")
    });
    let small_size = Size { cols: 80, rows: 4 };
    for instruction in [
        ScreenInstruction::RecomputeTabSize(client, small_size),
        ScreenInstruction::GoToTabWithId(TabId::new(0), Some(client), None),
        ScreenInstruction::PtyBytes(TerminalId::new(9), b" still active".to_vec()),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    wait_for_settled_render(
        &renders,
        small_size,
        "failed recovery keeps the screen running on the current tab",
        |frame| frame.contains("other tab still active") && !frame.contains("parked transcript"),
    );
    mock.to_screen
        .send(ScreenInstruction::RecomputeTabSize(client, size))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::GoToTabWithId(
            TabId::new(0),
            Some(client),
            None,
        ))
        .unwrap();
    // Output received after navigation is rendered only if post-instruction settlement kept us here.
    mock.to_screen
        .send(ScreenInstruction::PtyBytes(
            TerminalId::new(0),
            b" recovered".to_vec(),
        ))
        .unwrap();
    wait_for_settled_render(
        &renders,
        size,
        "navigation retains the recovered tab",
        |frame| frame.contains("parked transcript recovered") && !frame.contains("other tab"),
    );
    mock.teardown(vec![server_thread, screen_thread]);
    assert!(pty.try_iter().all(|(instruction, _)| !matches!(
        instruction,
        PtyInstruction::ClosePane(..)
            | PtyInstruction::SpawnTerminal(..)
            | PtyInstruction::ReRunCommandInPane(..)
    )));
    assert!(writer
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

#[test]
fn navigation_recovers_parked_work_after_visible_pane_closes() {
    for replace_existing in [false, true] {
        let source = TabId::new(1);
        let client = ClientId::new(1);
        let (mut screen, _, _, _) =
            screen_with_plugin_senders_and_clients(&[source, TabId::new(2)], &[client]);
        screen
            .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
            .unwrap();
        let original = PaneId::Terminal(TerminalId::new(1));
        let added = PaneId::Terminal(TerminalId::new(3));
        let tab = screen.tabs.get_mut(&source).unwrap();
        tab.new_pane(
            added,
            None,
            None,
            !replace_existing,
            false,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
        let (hidden, visible) = if replace_existing {
            let replacement = tab.extract_pane(added, true).unwrap();
            tab.suppress_pane_and_replace_with_other_pane(original, replacement, None);
            (original, added)
        } else {
            (added, original)
        };
        screen.settle_tabs().unwrap();
        assert!(
            screen.tabs[&source]
                .get_pane_info(hidden)
                .unwrap()
                .is_suppressed
        );
        screen
            .tabs
            .get_mut(&source)
            .unwrap()
            .close_pane(visible, false, None);
        screen.settle_tabs().unwrap();
        assert_ne!(screen.active_tab_ids[&client], source);
        screen
            .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
            .unwrap();
        screen.settle_tabs().unwrap();
        assert_eq!(screen.active_tab_ids[&client], source);
        assert_eq!(
            screen.tabs[&source].get_active_pane_id(client),
            Some(hidden)
        );
    }
}

#[test]
fn navigation_with_no_floating_room_preserves_hidden_terminal_for_retry() {
    let source = TabId::new(1);
    let client = ClientId::new(1);
    let hidden = PaneId::Terminal(TerminalId::new(1));
    let (mut screen, _, _, _) =
        screen_with_plugin_senders_and_clients(&[source, TabId::new(2)], &[client]);
    let tab = screen.tabs.get_mut(&source).unwrap();
    tab.suppress_pane(hidden, None);
    tab.resize_whole_tab(Size { cols: 1, rows: 1 }).unwrap();
    screen.settle_tabs().unwrap();
    screen
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
        .unwrap();
    assert_eq!(screen.active_tab_ids[&client], TabId::new(2));
    assert!(
        screen.tabs[&source]
            .get_pane_info(hidden)
            .unwrap()
            .is_suppressed
    );
    screen
        .tabs
        .get_mut(&source)
        .unwrap()
        .resize_whole_tab(Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
        .unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&client], source);
    assert_eq!(
        screen.tabs[&source].get_active_pane_id(client),
        Some(hidden)
    );
}

#[test]
fn navigating_to_the_current_tab_recovers_its_hidden_terminal() {
    let source = TabId::new(1);
    let client = ClientId::new(1);
    let hidden = PaneId::Terminal(TerminalId::new(1));
    let (mut screen, _, _, _) = screen_with_plugin_senders_and_clients(&[source], &[client]);
    screen
        .tabs
        .get_mut(&source)
        .unwrap()
        .suppress_pane(hidden, None);
    screen
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
        .unwrap();
    assert_eq!(
        screen.tabs[&source].get_active_pane_id(client),
        Some(hidden)
    );
    assert!(screen.tabs[&source].are_floating_panes_visible());
}

#[test]
fn explicit_pane_focus_does_not_recover_an_unrequested_terminal() {
    let client = ClientId::new(1);
    let source = TabId::new(1);
    let (mut screen, _, _, _) =
        screen_with_plugin_senders_and_clients(&[source, TabId::new(2)], &[client]);
    let requested = PaneId::Terminal(TerminalId::new(9));
    let other = PaneId::Terminal(TerminalId::new(1));
    let tab = screen.tabs.get_mut(&source).unwrap();
    tab.new_pane(
        requested,
        None,
        None,
        true,
        false,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    tab.suppress_pane(other, None);
    screen
        .focus_pane_with_id(requested, true, false, client)
        .unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&client], source);
    assert_eq!(
        screen.tabs[&source].get_active_pane_id(client),
        Some(requested)
    );
    assert!(
        screen.tabs[&source]
            .get_pane_info(other)
            .unwrap()
            .is_suppressed
    );
}

#[test]
fn moving_a_pane_to_a_hidden_only_tab_does_not_recover_parked_work() {
    let client = ClientId::new(1);
    let source = TabId::new(1);
    let destination = TabId::new(2);
    let (mut screen, _, _, _) =
        screen_with_plugin_senders_and_clients(&[source, destination], &[client]);
    screen
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), client)
        .unwrap();
    let hidden = PaneId::Terminal(TerminalId::new(2));
    screen
        .tabs
        .get_mut(&destination)
        .unwrap()
        .suppress_pane(hidden, None);
    screen
        .break_pane_to_new_tab(Direction::Right, client)
        .unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&client], destination);
    assert!(
        screen.tabs[&destination]
            .get_pane_info(hidden)
            .unwrap()
            .is_suppressed
    );
    assert!(screen.tabs[&destination]
        .has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(1))));
}

#[test]
fn detaching_while_replacement_loads_preserves_confirmation_for_next_client() {
    let client = ClientId::new(1);
    let (mut screen, screen_receiver, server_receiver, plugin_receiver) =
        screen_with_plugin_senders_and_clients(&[TabId::new(1)], &[client]);
    screen.set_quit_notice(AGENT_PLUGIN_ID, None, true);
    close_pane(&mut screen, PaneId::Terminal(TerminalId::new(1)));
    screen.settle_tabs().unwrap();
    let replacement = requested_tabs(&plugin_receiver)[0];
    screen.remove_client(client).unwrap();
    load_replacement_tab(&mut screen, replacement, 2);
    assert!(
        screen.active_tab_ids.is_empty(),
        "layout must not reattach a departed client"
    );
    screen.settle_tabs().unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(launches(&screen_receiver), 0);
    assert!(!session_ended(&server_receiver));
    screen.add_client(CLIENT_2).unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT_2], replacement);
    assert_eq!(launches(&screen_receiver), 1);
    screen.settle_tabs().unwrap();
    assert_eq!(launches(&screen_receiver), 0);
    assert_eq!(screen.tabs.len(), 1);
    assert!(requested_tabs(&plugin_receiver).is_empty());
}
