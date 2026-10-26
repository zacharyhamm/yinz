//! Where a spawned agent pane goes - `Screen::agent_pane_target` and the arithmetic under it.
//!
//! The surface is 80x20 and the floor 40x10 throughout, so a tab clears the floor with up to
//! three tiled panes (1600 / 4 = 400) and fails it with four (1600 / 5 = 320). A synchronous
//! `Screen` has no dispatcher for `NewPane`, so these ask the method the handler asks; the
//! last two go through the screen thread to see the whole thing, with the floor set from the
//! config the way a session sets it.

use super::*;
use crate::route::{wait_for_action_completion, NotificationEnd};
use crate::screen::agent_placement::{choose_tab, floor_of, projected_share, Candidate};
use std::collections::BTreeMap;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::pane_size::Viewport;
use {yinz_types::data::OriginatingPlugin, yinz_types::data::AGENT_PANE_CONTEXT};

const SURFACE: Size = Size { cols: 80, rows: 20 };
const FLOOR: Size = Size { cols: 40, rows: 10 };
const CLIENT: ClientId = crate::ClientId::new(1);

fn run(command: &str) -> Run {
    Run::Command(RunCommand {
        command: PathBuf::from(command),
        ..Default::default()
    })
}

/// A command a plugin opened as an agent pane, whatever the program is called.
fn marked_run(command: &str) -> Run {
    Run::Command(RunCommand {
        command: PathBuf::from(command),
        originating_plugin: Some(OriginatingPlugin::new(
            yinz_types::internal_ids::PluginId::new(2),
            crate::ClientId::new(1),
            BTreeMap::from([(AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
        )),
        ..Default::default()
    })
}

fn tiled() -> NewPanePlacement {
    NewPanePlacement::Tiled {
        direction: None,
        borderless: None,
    }
}

fn screen_with_floor() -> Screen {
    let mut screen = create_new_screen(SURFACE, false, false);
    screen.agent_pane_min_size = FLOOR;
    screen
}

/// A tab holding `count` tiled panes, numbered from `first_pid`.
fn tab_with_panes(screen: &mut Screen, tab_id: TabId, first_pid: u32, count: u32) {
    new_tab(screen, first_pid, tab_id);
    for pid in first_pid + 1..first_pid + count {
        add_pane_to_tab(
            screen,
            tab_id,
            PaneId::Terminal(crate::TerminalId::new(pid)),
            false,
        );
    }
}

fn target_of(
    screen: &mut Screen,
    invoked_with: Option<Run>,
    placement: NewPanePlacement,
    target: ClientTabIndexOrPaneId,
) -> (ClientTabIndexOrPaneId, Option<ClientId>, NewPanePlacement) {
    let mut placement = placement;
    let placed = screen.agent_pane_target(invoked_with.as_ref(), &mut placement, false, target);
    (placed.target, placed.focus_after_placing, placement)
}

fn plugin_capture(screen: &mut Screen) -> Receiver<(PluginInstruction, ErrorContext)> {
    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    plugin_receiver
}

/// Every `NewTab` the plugin thread was asked for: its tab id, whether the client follows, and
/// the run instructions its layout is told to leave alone.
fn new_tabs_asked_for(
    receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(TabId, bool, Vec<Option<Run>>)> {
    new_tabs_asked_for_by_client(receiver)
        .into_iter()
        .map(|(tab_id, focus, _, ignored)| (tab_id, focus, ignored))
        .collect()
}

/// As `new_tabs_asked_for`, with the client the tab was made for.
fn new_tabs_asked_for_by_client(
    receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(TabId, bool, ClientId, Vec<Option<Run>>)> {
    receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::NewTab(_, _, Some(layout), _, tab_id, _, _, focus, client, _) => {
                Some((tab_id, focus, client, layout.run_instructions_to_ignore))
            },
            _ => None,
        })
        .collect()
}

#[test]
fn a_share_is_the_viewport_split_among_the_tiled_panes_and_the_newcomer() {
    let viewport = Viewport {
        x: 0,
        y: 1,
        rows: 20,
        cols: 80,
    };
    assert_eq!(projected_share(viewport, 0), 1600);
    assert_eq!(projected_share(viewport, 1), 800);
    assert_eq!(projected_share(viewport, 3), 400);
    assert_eq!(projected_share(viewport, 4), 320);
}

#[test]
fn projected_areas_saturate() {
    assert_eq!(
        projected_share(
            Viewport {
                x: 0,
                y: 0,
                rows: usize::MAX,
                cols: 2,
            },
            0,
        ),
        usize::MAX
    );
    assert_eq!(
        floor_of(Size {
            cols: usize::MAX,
            rows: 2,
        }),
        Some(usize::MAX)
    );
}

#[test]
fn the_roomiest_qualifying_tab_wins_and_a_tie_goes_to_the_lower_position() {
    let candidates = [
        Candidate {
            tab_id: TabId::new(7),
            position: TabPosition::new(2),
            share: 800,
        },
        Candidate {
            tab_id: TabId::new(3),
            position: TabPosition::new(1),
            share: 800,
        },
        Candidate {
            tab_id: TabId::new(5),
            position: TabPosition::new(0),
            share: 533,
        },
    ];
    assert_eq!(choose_tab(&candidates, 400), Some(TabId::new(3)));
    assert_eq!(choose_tab(&candidates, 600), Some(TabId::new(3)));
    assert_eq!(choose_tab(&candidates, 801), None);
    assert_eq!(choose_tab(&[], 1), None);
}

#[test]
fn an_agent_stays_in_the_focused_tab_while_its_share_clears_the_floor() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 3);
    let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
    let (placed, focus, _) = target_of(&mut screen, Some(run("claude")), tiled(), target);
    assert_eq!(placed, target);
    assert_eq!(focus, None);
    assert_eq!(screen.tabs.len(), 1, "no tab was made");
}

#[test]
fn an_agent_moves_to_the_roomiest_other_tab_when_the_focused_tab_is_crowded() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 2);
    tab_with_panes(&mut screen, TabId::new(2), 20, 1);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(run("codex")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(CLIENT),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(2)));
    assert_eq!(
        focus,
        Some(CLIENT),
        "the spawn wanted focus, and a tab index places without a client"
    );
    assert_eq!(screen.tabs.len(), 3, "no tab was made");
}

#[test]
fn a_spawn_that_did_not_want_focus_is_moved_without_it() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 1);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(run("codex")),
        tiled(),
        ClientTabIndexOrPaneId::ClientIdNoFocus(CLIENT),
    );
    assert_eq!(
        placed,
        ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(1))
    );
    assert_eq!(focus, None);
}

#[test]
fn an_agent_opens_a_new_tab_when_every_tab_is_crowded() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 4);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let plugin_receiver = plugin_capture(&mut screen);
    let invoked_with = Some(run("claude"));
    let (placed, focus, _) = target_of(
        &mut screen,
        invoked_with.clone(),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(CLIENT),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(2)));
    assert_eq!(focus, None, "the layout landing moves the client");
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .is_some_and(|tab| tab.is_pending()),
        "the tab exists and waits for its layout"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .is_some_and(|tab| !tab.has_no_connected_clients()),
        "the client is seated in the tab it follows the pane to"
    );
    assert_eq!(
        new_tabs_asked_for(&plugin_receiver),
        vec![(TabId::new(2), true, vec![invoked_with])],
        "one tab, the client following, and a layout that will not add a shell for the pane"
    );
}

#[test]
fn a_tab_made_for_an_unfocused_spawn_does_not_take_the_client_along() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    let plugin_receiver = plugin_capture(&mut screen);
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(marked_run("some-wrapper")),
        tiled(),
        ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(1))),
    );
    assert_eq!(
        placed,
        ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(1))
    );
    assert_eq!(focus, None);
    assert_eq!(
        new_tabs_asked_for(&plugin_receiver)
            .into_iter()
            .map(|(tab_id, follows, _)| (tab_id, follows))
            .collect::<Vec<_>>(),
        vec![(TabId::new(1), false)]
    );
    // The client stays where it was, and only there: `apply_layout` moves a client out of
    // its tab on a focus change alone, so seating it in the new tab as well would leave it
    // connected to both.
    assert!(
        screen
            .tabs
            .get(&TabId::new(1))
            .is_some_and(|tab| tab.has_no_connected_clients()),
        "the new tab has no client in it"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(0))
            .is_some_and(|tab| !tab.has_no_connected_clients()),
        "the client is still in the tab it was looking at"
    );
    assert_eq!(screen.active_tab_ids.get(&CLIENT), Some(&TabId::new(0)));
}

/// `yinz run -- claude` is a client of its own with no screen. The tab it is crowding, and
/// the client who follows the pane, are the attached client's - the way the `ClientId` arm
/// acts for the first attached client when the one named is not.
#[test]
fn a_spawn_from_a_client_with_no_screen_acts_for_the_attached_one() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 1);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let cli_client: ClientId = crate::ClientId::new(42);
    assert!(!screen.active_tab_ids.contains_key(&cli_client));
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(run("claude")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(cli_client),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(1)));
    assert_eq!(
        focus,
        Some(CLIENT),
        "the attached client is brought to the pane"
    );

    // And a tab made for the pane is made for the attached client too.
    tab_with_panes(&mut screen, TabId::new(2), 20, 4);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    add_pane_to_tab(
        &mut screen,
        TabId::new(1),
        PaneId::Terminal(crate::TerminalId::new(11)),
        false,
    );
    add_pane_to_tab(
        &mut screen,
        TabId::new(1),
        PaneId::Terminal(crate::TerminalId::new(12)),
        false,
    );
    add_pane_to_tab(
        &mut screen,
        TabId::new(1),
        PaneId::Terminal(crate::TerminalId::new(13)),
        false,
    );
    let plugin_receiver = plugin_capture(&mut screen);
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(run("claude")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(cli_client),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(3)));
    assert_eq!(focus, None);
    assert_eq!(
        new_tabs_asked_for_by_client(&plugin_receiver)
            .into_iter()
            .map(|(tab_id, follows, client, _)| (tab_id, follows, client))
            .collect::<Vec<_>>(),
        vec![(TabId::new(3), true, CLIENT)]
    );
}

#[test]
fn a_pending_tab_is_not_a_candidate() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    // A tab whose layout has not landed: empty, and not to be trusted about it.
    screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, Some(CLIENT))
        .expect("TEST");
    let _plugin_receiver = plugin_capture(&mut screen);
    let (placed, _, _) = target_of(
        &mut screen,
        Some(run("claude")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(CLIENT),
    );
    assert_eq!(
        placed,
        ClientTabIndexOrPaneId::TabIndex(TabId::new(2)),
        "a tab of its own, not the pending one"
    );
}

#[test]
fn a_fullscreen_tab_is_not_a_candidate() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 2);
    tab_with_panes(&mut screen, TabId::new(2), 20, 3);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(10)));
    assert!(screen.tabs[&TabId::new(1)].is_fullscreen_active());
    screen.go_to_tab(TabNumber::new(0), CLIENT).expect("TEST");
    let (placed, _, _) = target_of(
        &mut screen,
        Some(run("claude")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(CLIENT),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(2)));
}

#[test]
fn a_plain_shell_is_never_moved() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    for invoked_with in [None, Some(run("zsh")), Some(run("git"))] {
        let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
        let (placed, _, _) = target_of(&mut screen, invoked_with, tiled(), target);
        assert_eq!(placed, target);
    }
    assert_eq!(screen.tabs.len(), 1);
}

/// Behind an interpreter only the script names the agent - an argument that happens to
/// look like one must not move the pane.
#[test]
fn an_agent_name_among_an_interpreters_arguments_does_not_move_the_pane() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    for (command, args) in [
        ("python3", &["gemini_test.py", "--api", "gemini"][..]),
        ("uv", &["--directory", "/work/claude", "run", "pytest"]),
    ] {
        let invoked_with = Run::Command(RunCommand {
            command: PathBuf::from(command),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            ..Default::default()
        });
        let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
        let (placed, _, _) = target_of(&mut screen, Some(invoked_with), tiled(), target);
        assert_eq!(placed, target, "{command} {args:?}");
    }
    assert_eq!(screen.tabs.len(), 1);
}

#[test]
fn an_explicit_tab_direction_or_layer_is_honoured() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 1);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let by_tab = ClientTabIndexOrPaneId::TabIndex(TabId::new(0));
    let (placed, _, _) = target_of(&mut screen, Some(run("claude")), tiled(), by_tab);
    assert_eq!(placed, by_tab, "--tab-id names the tab");

    let focused = ClientTabIndexOrPaneId::ClientId(CLIENT);
    for placement in [
        NewPanePlacement::Tiled {
            direction: Some(Direction::Right),
            borderless: None,
        },
        NewPanePlacement::Floating(None),
        NewPanePlacement::InPlace {
            pane_id_to_replace: None,
            close_replaced_pane: false,
            borderless: None,
        },
    ] {
        let (placed, _, kept) =
            target_of(&mut screen, Some(run("claude")), placement.clone(), focused);
        assert_eq!(placed, focused, "{placement:?}");
        assert_eq!(kept, placement);
    }
}

#[test]
fn a_pane_with_no_preference_tiles_where_it_is_sent_unless_the_floating_layer_is_up() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 1);
    screen.go_to_tab(TabNumber::new(1), CLIENT).expect("TEST");
    let no_preference = NewPanePlacement::NoPreference { borderless: None };
    let focused = ClientTabIndexOrPaneId::ClientId(CLIENT);
    let (placed, _, rewritten) = target_of(
        &mut screen,
        Some(run("claude")),
        no_preference.clone(),
        focused,
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(1)));
    assert_eq!(
        rewritten,
        tiled(),
        "no preference in the source tab meant tiled, so the destination is told so"
    );

    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Terminal(crate::TerminalId::new(50)),
        true,
    );
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .show_floating_panes();
    assert!(screen.tabs[&TabId::new(0)].are_floating_panes_visible());
    let (placed, _, kept) = target_of(
        &mut screen,
        Some(run("claude")),
        no_preference.clone(),
        focused,
    );
    assert_eq!(
        placed, focused,
        "with the floating layer up, no preference means a floating pane"
    );
    assert_eq!(kept, no_preference);
}

#[test]
fn floating_panes_do_not_count_toward_the_share() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 3);
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Terminal(crate::TerminalId::new(50)),
        true,
    );
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Terminal(crate::TerminalId::new(51)),
        true,
    );
    let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
    let (placed, _, _) = target_of(&mut screen, Some(run("claude")), tiled(), target);
    assert_eq!(
        placed, target,
        "three tiled panes clear the floor whatever floats over them"
    );
}

#[test]
fn a_zero_floor_disables_the_rule() {
    for floor in [
        Size { rows: 0, cols: 40 },
        Size { rows: 10, cols: 0 },
        Size { rows: 0, cols: 0 },
    ] {
        let mut screen = screen_with_floor();
        screen.agent_pane_min_size = floor;
        tab_with_panes(&mut screen, TabId::new(0), 1, 4);
        let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
        let (placed, _, _) = target_of(&mut screen, Some(run("claude")), tiled(), target);
        assert_eq!(placed, target, "{floor:?}");
        assert_eq!(screen.tabs.len(), 1);
    }
}

#[test]
fn a_pane_a_plugin_marked_as_an_agent_is_placed_like_a_named_agent() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    tab_with_panes(&mut screen, TabId::new(1), 10, 1);
    let near_plugin_pane =
        ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(1)));
    let (placed, focus, _) = target_of(
        &mut screen,
        Some(marked_run("some-wrapper")),
        tiled(),
        near_plugin_pane,
    );
    assert_eq!(
        placed,
        ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(1))
    );
    assert_eq!(focus, None, "the plugin focuses its pane itself");

    let (placed, _, _) = target_of(
        &mut screen,
        Some(run("some-wrapper")),
        tiled(),
        near_plugin_pane,
    );
    assert_eq!(
        placed, near_plugin_pane,
        "unmarked, the wrapper is not an agent"
    );

    let nowhere = ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(99)));
    let (placed, _, _) = target_of(&mut screen, Some(marked_run("claude")), tiled(), nowhere);
    assert_eq!(
        placed, nowhere,
        "a pane no tab holds gives nothing to reason from"
    );
}

#[test]
fn a_suppressed_pane_is_parked_where_it_was_asked_for() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    let target = ClientTabIndexOrPaneId::ClientId(CLIENT);
    let mut placement = tiled();
    let placed =
        screen.agent_pane_target(Some(run("claude")).as_ref(), &mut placement, true, target);
    assert_eq!(placed.target, target);
}

fn new_pane_instruction(
    pid: u32,
    invoked_with: Option<Run>,
    target: ClientTabIndexOrPaneId,
) -> ScreenInstruction {
    ScreenInstruction::NewPane(
        PaneId::Terminal(crate::TerminalId::new(pid)),
        Some("claude".to_string()),
        None,
        invoked_with,
        NewPanePlacement::default(),
        false,
        target,
        None,
        false,
    )
}

/// The screen thread with the floor a session would read from its config: 80x24, which one
/// pane on an 80x20 surface already fails.
fn mock_screen_with_floor(columns: usize, rows: usize) -> MockScreen {
    let mut mock_screen = MockScreen::new(SURFACE);
    mock_screen.config.options.agent_pane_min_columns = Some(columns);
    mock_screen.config.options.agent_pane_min_rows = Some(rows);
    mock_screen
}

#[test]
fn through_the_screen_thread_an_agent_spawn_gets_a_tab_of_its_own_from_the_config() {
    let mut mock_screen = mock_screen_with_floor(80, 24);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    let invoked_with = Some(run("claude"));
    let client = ClientTabIndexOrPaneId::ClientId(mock_screen.main_client_id);
    mock_screen
        .to_screen
        .send(new_pane_instruction(2, invoked_with.clone(), client))
        .unwrap();
    mock_screen.teardown(vec![screen_thread]);
    let made: Vec<_> = new_tabs_asked_for(&plugin_receiver)
        .into_iter()
        .filter(|(tab_id, _, _)| *tab_id == TabId::new(1))
        .collect();
    assert_eq!(made, vec![(TabId::new(1), true, vec![invoked_with])]);
}

#[test]
fn through_the_screen_thread_a_zero_floor_leaves_the_spawn_where_it_was_asked_for() {
    let mut mock_screen = mock_screen_with_floor(0, 24);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    let client = ClientTabIndexOrPaneId::ClientId(mock_screen.main_client_id);
    mock_screen
        .to_screen
        .send(new_pane_instruction(2, Some(run("claude")), client))
        .unwrap();
    mock_screen.teardown(vec![screen_thread]);
    assert!(
        new_tabs_asked_for(&plugin_receiver)
            .iter()
            .all(|(tab_id, _, _)| *tab_id == TabId::new(0)),
        "only the tab the test started with"
    );
}

#[test]
fn through_the_screen_thread_a_pane_opened_near_a_plugin_gets_its_tab_without_the_client() {
    let mut mock_screen = mock_screen_with_floor(80, 24);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(7)]);
    let near_plugin = ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(crate::PluginId::new(7)));
    mock_screen
        .to_screen
        .send(new_pane_instruction(
            2,
            Some(marked_run("codex")),
            near_plugin,
        ))
        .unwrap();
    mock_screen.teardown(vec![screen_thread]);
    let made: Vec<_> = new_tabs_asked_for(&plugin_receiver)
        .into_iter()
        .filter(|(tab_id, _, _)| *tab_id == TabId::new(2))
        .map(|(tab_id, follows, _)| (tab_id, follows))
        .collect();
    assert_eq!(made, vec![(TabId::new(2), false)]);
}

/// A tab made for an agent pane is pending until its layout lands, and the screen knows it
/// without the screen thread having to be told: `has_pending_tabs` reads the tabs themselves,
/// so instructions the thread defers while a tab is pending defer for this tab too.
#[test]
fn a_tab_made_for_an_agent_pane_is_pending_until_its_layout_lands() {
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 4);
    assert!(!screen.has_pending_tabs());
    let _plugin_receiver = plugin_capture(&mut screen);
    let (placed, _, _) = target_of(
        &mut screen,
        Some(run("claude")),
        tiled(),
        ClientTabIndexOrPaneId::ClientId(CLIENT),
    );
    assert_eq!(placed, ClientTabIndexOrPaneId::TabIndex(TabId::new(1)));
    assert!(screen.has_pending_tabs(), "the tab waits for its layout");
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(1),
            true,
            CLIENT,
            None,
        )
        .expect("TEST");
    assert!(!screen.has_pending_tabs(), "the layout landed");
}

#[test]
fn a_failed_layout_override_preserves_pending_state_for_caller_cleanup() {
    use yinz_types::geometry::SplitSize;

    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, Some(CLIENT))
        .expect("TEST");
    assert!(screen.has_pending_tabs());

    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout {
            split_size: Some(SplitSize::Fixed(SURFACE.rows + 1)),
            ..Default::default()
        }],
        ..Default::default()
    };
    // Fixed sizes fail even on the layout applier's retry ignoring percentages.
    for ignore_percentages in [false, true] {
        assert!(layout
            .position_panes_in_space(&(&SURFACE).into(), None, ignore_percentages, true)
            .is_err());
    }

    let tab = screen.tabs.get_mut(&TabId::new(1)).expect("TEST");
    assert!(tab.is_pending());
    tab.override_layout(
        layout,
        vec![],
        None,
        None,
        vec![],
        vec![],
        HashMap::new(),
        false,
        false,
        CLIENT,
        None,
    )
    .expect_err("the screen must receive the failure to report it and clean up spawned panes");
    assert!(tab.is_pending(), "rejected layouts leave the tab unchanged");
    assert!(
        screen.has_pending_tabs(),
        "the screen still owns settlement"
    );
}

/// The plugin that spawned the pane focuses it in the same breath as the spawn response. The
/// focus waits for the tab's layout: its completion is answered only once the layout lands.
#[test]
fn through_the_screen_thread_a_focus_on_the_new_pane_waits_for_the_tab_layout() {
    let mut mock_screen = mock_screen_with_floor(80, 24);
    let _plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(7)]);
    let near_plugin = ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(crate::PluginId::new(7)));
    mock_screen
        .to_screen
        .send(new_pane_instruction(
            2,
            Some(marked_run("codex")),
            near_plugin,
        ))
        .unwrap();
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::FocusPaneWithId(
            PaneId::Terminal(crate::TerminalId::new(2)),
            false,
            false,
            mock_screen.main_client_id,
            Some(NotificationEnd::new(completion_tx)),
        ))
        .unwrap();
    // A query answered after the two above proves they were handled; the focus has not been.
    mock_screen
        .to_screen
        .send(ScreenInstruction::QueryTabNames(
            mock_screen.main_client_id,
            None,
        ))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match server_receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            Ok((ServerInstruction::Log(..), _)) => break,
            Ok(_) => continue,
            Err(error) => panic!("the screen thread did not answer the probe: {error}"),
        }
    }
    assert!(
        completion_rx.try_recv().is_err(),
        "the focus is deferred while the tab made for the pane is pending"
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(2),
            false,
            mock_screen.main_client_id,
            None,
            None,
        ))
        .unwrap();
    let completed = wait_for_action_completion(completion_rx, "focus", false);
    assert_eq!(completed.exit_status, None, "{completed:?}");
    mock_screen.teardown(vec![screen_thread]);
}

fn place_reserved_agent(
    screen: &mut Screen,
    pid: u32,
    destination: yinz_bus::route::AgentTabPlacement,
) -> tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut completion = NotificationEnd::new(tx);
    completion.wait_for_agent_start = true;
    completion.created_tab_id = destination.created.then_some(destination.tab_id);
    let tab_id = destination.tab_id;
    completion.agent_tab = Some(destination);
    screen
        .place_new_pane(
            PaneId::Terminal(crate::TerminalId::new(pid)),
            None,
            None,
            Some(run("claude")),
            tiled(),
            false,
            ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
            Some(completion),
            false,
            &mut Default::default(),
        )
        .unwrap();
    rx
}

#[test]
fn explicit_agent_tab_survives_reordering_but_not_close_and_id_reuse() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    tab_with_panes(&mut screen, TabId::new(1), 10, 4);
    let first = screen
        .resolve_agent_tab(AgentPlacement::Tab(TabId::new(1)), None, CLIENT)
        .unwrap();
    let delayed = first.clone();
    screen.tabs.get_mut(&TabId::new(1)).unwrap().position = TabPosition::new(0);
    let result = place_reserved_agent(&mut screen, 20, first)
        .blocking_recv()
        .unwrap();
    assert_eq!(result.affected_tab_id, Some(TabId::new(1)));
    assert!(result.error_message.is_none());
    assert!(result.created_tab_id.is_none());
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_eq!(screen.get_new_tab_id(), TabId::new(1));
    tab_with_panes(&mut screen, TabId::new(1), 30, 1);
    let result = place_reserved_agent(&mut screen, 21, delayed)
        .blocking_recv()
        .unwrap();
    assert!(result.error_message.unwrap().contains("closed"));
    assert!(result.affected_tab_id.is_none());
    assert!(!screen
        .tabs
        .values()
        .any(|tab| tab.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(21)))));
    assert!(screen
        .resolve_agent_tab(AgentPlacement::Tab(TabId::new(99)), None, CLIENT)
        .is_err());
}

#[test]
fn new_agent_tab_completion_waits_for_layout_and_does_not_change_focus() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let focus = screen.active_tab_ids.clone();
    let target = screen
        .resolve_agent_tab(
            AgentPlacement::NewTab(Some("review".into())),
            Some(run("claude")),
            CLIENT,
        )
        .unwrap();
    let tab_id = target.tab_id;
    assert_eq!(screen.tabs[&tab_id].name, "review");
    let mut completion = place_reserved_agent(&mut screen, 20, target.clone());
    assert!(matches!(
        completion.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        !screen.cleanup_agent_tab(&target),
        "pending work prevents cleanup"
    );
    screen
        .tabs
        .get_mut(&tab_id)
        .unwrap()
        .finish_pending_layout();
    let result = completion.blocking_recv().unwrap();
    assert!(result.error_message.is_none());
    assert_eq!(result.affected_tab_id, Some(tab_id));
    assert_eq!(result.created_tab_id, Some(tab_id));
    assert_eq!(screen.active_tab_ids, focus);
    assert!(
        !screen.cleanup_agent_tab(&target),
        "populated tabs must survive"
    );
}

#[test]
fn closing_pending_agent_destination_fails_all_launches_and_cleanup_cannot_delete_replacement() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let target = screen
        .resolve_agent_tab(AgentPlacement::NewTab(None), Some(run("claude")), CLIENT)
        .unwrap();
    let first = place_reserved_agent(&mut screen, 20, target.clone());
    let second = place_reserved_agent(&mut screen, 21, target.clone());
    screen.close_tab_by_id(target.tab_id).unwrap();
    for completion in [first, second] {
        let result = completion.blocking_recv().unwrap();
        assert!(result.error_message.is_some());
        assert!(result.affected_tab_id.is_none());
    }
    tab_with_panes(&mut screen, target.tab_id, 30, 1);
    assert!(
        screen.cleanup_agent_tab(&target),
        "the launch-owned incarnation is already gone"
    );
    assert!(screen.tabs.contains_key(&target.tab_id));
}

#[test]
fn only_drained_empty_launch_created_tabs_are_cleaned_up() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let target = screen
        .resolve_agent_tab(AgentPlacement::NewTab(None), None, CLIENT)
        .unwrap();
    assert!(!screen.cleanup_agent_tab(&target));
    screen
        .tabs
        .get_mut(&target.tab_id)
        .unwrap()
        .finish_pending_layout();
    assert!(screen.cleanup_agent_tab(&target));
    assert!(!screen.tabs.contains_key(&target.tab_id));
    let existing = screen
        .resolve_agent_tab(AgentPlacement::Tab(TabId::new(0)), None, CLIENT)
        .unwrap();
    assert!(!screen.cleanup_agent_tab(&existing));
}

#[test]
fn late_agent_layout_cannot_land_in_recreated_tab() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let target = screen
        .resolve_agent_tab(AgentPlacement::NewTab(None), Some(run("claude")), CLIENT)
        .unwrap();
    screen.close_tab_by_id(target.tab_id).unwrap();
    tab_with_panes(&mut screen, target.tab_id, 30, 1);
    let before = screen.tabs[&target.tab_id].get_all_pane_ids();
    screen
        .land_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(crate::TerminalId::new(40), None)],
            vec![],
            vec![],
            HashMap::new(),
            target.tab_id,
            false,
            CLIENT,
            Some(NotificationEnd::for_agent_layout(target)),
            None,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(screen.tabs[&TabId::new(1)].get_all_pane_ids(), before);
}

#[test]
fn failed_pending_agent_layout_reports_failure_without_deleting_the_pane() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let target = screen
        .resolve_agent_tab(AgentPlacement::NewTab(None), Some(run("claude")), CLIENT)
        .unwrap();
    let completion = place_reserved_agent(&mut screen, 20, target.clone());
    screen.recover_pending_tab(target.tab_id).unwrap();
    let result = completion.blocking_recv().unwrap();
    assert!(result.error_message.unwrap().contains("layout failed"));
    assert!(result.affected_tab_id.is_none());
    assert!(screen.tabs[&target.tab_id]
        .has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(20))));
    assert!(!screen.cleanup_agent_tab(&target));
}

#[test]
fn empty_new_agent_tab_survives_layout_before_delayed_startup() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let _plugins = plugin_capture(&mut screen);
    let target = screen
        .resolve_agent_tab(
            AgentPlacement::NewTab(Some("review".into())),
            Some(run("claude")),
            CLIENT,
        )
        .unwrap();
    screen
        .land_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            target.tab_id,
            false,
            CLIENT,
            Some(NotificationEnd::for_agent_layout(target.clone())),
            None,
            &mut Default::default(),
        )
        .unwrap();
    screen.settle_tabs().unwrap();
    assert!(screen.tabs.contains_key(&target.tab_id));
    let result = place_reserved_agent(&mut screen, 20, target.clone())
        .blocking_recv()
        .unwrap();
    assert!(result.error_message.is_none(), "{result:?}");
    assert_eq!(result.affected_tab_id, Some(target.tab_id));
    assert_eq!(screen.tabs[&target.tab_id].name, "review");
    assert!(screen.tabs[&target.tab_id].is_user_named());
}

#[test]
fn a_deferred_tab_close_already_rejects_agent_arrivals() {
    use yinz_types::agent::control::AgentPlacement;
    let mut screen = screen_with_floor();
    tab_with_panes(&mut screen, TabId::new(0), 1, 1);
    let target = screen
        .resolve_agent_tab(AgentPlacement::Tab(TabId::new(0)), None, CLIENT)
        .unwrap();
    screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, None)
        .unwrap();
    screen.close_tab_by_id(TabId::new(0)).unwrap();
    assert!(
        screen.tabs.contains_key(&TabId::new(0)),
        "close waits for the other pending layout"
    );
    assert!(screen
        .resolve_agent_tab(AgentPlacement::Tab(TabId::new(0)), None, CLIENT)
        .is_err());
    let result = place_reserved_agent(&mut screen, 20, target)
        .blocking_recv()
        .unwrap();
    assert!(result.error_message.unwrap().contains("closed"));
    assert!(result.affected_tab_id.is_none());
}

#[test]
fn suppressed_agent_launch_completes_in_the_reserved_tab_without_layout_or_focus_changes() {
    use yinz_types::agent::control::AgentPlacement;
    for requested in [None, Some(TabId::new(0)), Some(TabId::new(1))] {
        let mut screen = screen_with_floor();
        tab_with_panes(&mut screen, TabId::new(0), 1, 4);
        tab_with_panes(&mut screen, TabId::new(1), 10, 1);
        let active_tabs = screen.active_tab_ids.clone();
        let destination = screen
            .resolve_agent_tab(AgentPlacement::Suppressed(requested), None, CLIENT)
            .unwrap();
        let tab_id = destination.tab_id;
        assert_eq!(tab_id, requested.unwrap_or(active_tabs[&CLIENT]));
        let tab = &screen.tabs[&tab_id];
        let focus = tab.get_active_pane_id(CLIENT);
        let geometry: Vec<_> = tab
            .get_all_pane_ids()
            .into_iter()
            .map(|id| (id, tab.get_pane_with_id(id).unwrap().current_geom()))
            .collect();
        let pane = PaneId::Terminal(crate::TerminalId::new(20));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut completion = NotificationEnd::new(tx);
        completion.wait_for_agent_start = true;
        completion.agent_tab = Some(destination);
        screen
            .place_new_pane(
                pane,
                None,
                None,
                Some(run("claude")),
                tiled(),
                true,
                ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                Some(completion),
                false,
                &mut Default::default(),
            )
            .unwrap();
        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.affected_pane_id, Some(pane));
        assert_eq!(result.affected_tab_id, Some(tab_id));
        assert!(result.error_message.is_none());
        assert!(result.created_tab_id.is_none());
        assert_eq!(screen.tabs.len(), 2);
        assert_eq!(screen.active_tab_ids, active_tabs);
        let tab = &screen.tabs[&tab_id];
        assert!(tab.get_pane_info(pane).unwrap().is_suppressed);
        assert_eq!(tab.get_active_pane_id(CLIENT), focus);
        for (id, geom) in geometry {
            assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), geom);
        }
    }
}
