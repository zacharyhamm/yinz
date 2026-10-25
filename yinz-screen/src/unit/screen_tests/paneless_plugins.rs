//! Plugins without a pane: one that declares none runs headless (`PluginPaneless`), and one the
//! plugin thread unloaded is gone (`PluginUnloaded`). Neither may keep or regain a pane record,
//! and removing a record must never ask the pty thread to close it, since that unloads.

use super::*;
use crate::route::NotificationEnd;
use crate::screen::thread_main::PendingEvents;
use crate::PluginId;
use yinz_bus::pty::ReplacedPane;
use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabLoadState, PaneTabsCommand, PaneTabsLayout};

const SIZE: Size = Size {
    cols: 100,
    rows: 30,
};
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(7);
const LAYOUT_TAB: TabId = TabId::new(1);

type Completion = tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>;

fn run(url: &str) -> RunPluginOrAlias {
    yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, None).unwrap()
}

fn headless_run() -> RunPluginOrAlias {
    run("file:/path/to/headless/plugin")
}

fn completion() -> (NotificationEnd, Completion) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (NotificationEnd::new(tx), rx)
}

fn answer(mut rx: Completion) -> yinz_bus::route::ActionCompletionResult {
    rx.try_recv().expect("the completion must be answered")
}

/// A screen with one tab holding terminal 1, whose pty and plugin instructions land in the
/// returned receivers instead of going nowhere.
struct Harness {
    screen: Screen,
    pty: Receiver<(PtyInstruction, ErrorContext)>,
    plugin: Receiver<(PluginInstruction, ErrorContext)>,
    pending: PendingEvents,
}

impl Harness {
    fn new() -> Self {
        let mut screen = create_new_screen(SIZE, false, false);
        let (to_pty, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        let (to_plugin, plugin): ChannelWithContext<PluginInstruction> = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        new_tab(&mut screen, 1, TabId::new(0));
        Harness {
            screen,
            pty,
            plugin,
            pending: PendingEvents::default(),
        }
    }

    fn pty_instructions(&self) -> Vec<PtyInstruction> {
        self.pty
            .try_iter()
            .map(|(instruction, _)| instruction)
            .collect()
    }

    fn has_pane(&self, plugin: PluginId) -> bool {
        self.screen
            .tab_containing_pane(PaneId::Plugin(plugin))
            .is_some()
    }

    fn paneless(&mut self, plugin: PluginId) {
        self.screen
            .plugin_paneless(plugin, headless_run(), None, &mut self.pending)
            .unwrap();
    }

    fn unloaded(&mut self, plugin: PluginId, intercepts: &mut HashMap<ClientId, PluginId>) {
        self.screen
            .plugin_unloaded(plugin, intercepts, &mut self.pending)
            .unwrap();
    }

    /// Lands a new tab whose layout holds a terminal beside a slot for `plugin`, the way
    /// `ApplyLayout` does.
    fn land_layout_with(&mut self, plugin: PluginId) {
        self.screen
            .new_tab(LAYOUT_TAB, (vec![], vec![]), None, Some(CLIENT))
            .unwrap();
        self.screen
            .land_layout(
                layout_with_plugin_slot(),
                vec![],
                vec![(TerminalId::new(2), None)],
                vec![],
                vec![],
                HashMap::from([(headless_run(), vec![plugin])]),
                LAYOUT_TAB,
                true,
                CLIENT,
                None,
                None,
                &mut self.pending,
            )
            .unwrap();
    }

    /// `AddPlugin` for `plugin` as a floating pane in tab 0, answered through the returned
    /// completion. Returns whether the screen placed (or parked) it.
    fn add_plugin(&mut self, plugin: PluginId) -> (bool, Completion) {
        let (completion, rx) = completion();
        let placed = self
            .screen
            .add_plugin(
                Some(true),
                false,
                ReplacedPane::Suppress,
                headless_run(),
                None,
                Some(TabId::new(0)),
                plugin,
                None,
                None,
                false,
                None,
                Some(false),
                Some(CLIENT),
                Some(completion),
                None,
                &mut self.pending,
            )
            .unwrap();
        (placed, rx)
    }
}

/// A terminal beside a slot for `headless_run()`.
fn layout_with_plugin_slot() -> TiledPaneLayout {
    TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout {
                run: Some(Run::Plugin(headless_run())),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn closes_pty_pane(instructions: &[PtyInstruction], plugin: PluginId) -> bool {
    instructions.iter().any(|instruction| {
        matches!(instruction, PtyInstruction::ClosePane(PaneId::Plugin(id), _) if *id == plugin)
    })
}

#[test]
fn a_paneless_plugin_loses_its_layout_slot_without_a_pty_close() {
    let mut harness = Harness::new();
    harness.land_layout_with(PLUGIN);
    assert!(harness.has_pane(PLUGIN));
    assert!(
        harness
            .screen
            .render_blocker
            .blocking_plugins
            .contains_key(&PLUGIN),
        "a landed layout plugin blocks the first render until it renders"
    );
    harness.pty_instructions();

    harness.paneless(PLUGIN);

    assert!(!harness.has_pane(PLUGIN), "the slot must be removed");
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "a pty close would unload the plugin that runs headless"
    );
    assert!(!harness
        .screen
        .render_blocker
        .blocking_plugins
        .contains_key(&PLUGIN));
    assert!(harness.screen.paneless_plugins.contains_key(&PLUGIN));
    assert!(harness
        .screen
        .tabs
        .get(&LAYOUT_TAB)
        .is_some_and(|tab| tab.has_pane_with_pid(&PaneId::Terminal(TerminalId::new(2)))));
}

#[test]
fn a_layout_landing_after_the_plugin_went_paneless_drops_its_slot() {
    for unloaded in [false, true] {
        let mut harness = Harness::new();
        if unloaded {
            harness.unloaded(PLUGIN, &mut HashMap::new());
        } else {
            harness.paneless(PLUGIN);
        }
        harness.land_layout_with(PLUGIN);

        assert!(!harness.has_pane(PLUGIN), "unloaded: {unloaded}");
        assert!(
            !harness
                .screen
                .render_blocker
                .blocking_plugins
                .contains_key(&PLUGIN),
            "nothing will ever release a blocker registered for it (unloaded: {unloaded})"
        );
        assert!(
            !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
            "unloaded: {unloaded}"
        );
    }
}

#[test]
fn an_override_layout_drops_the_slot_of_a_paneless_plugin() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);

    harness
        .screen
        .complete_override_layout(
            vec![crate::screen::TabOverrideResult {
                tab_index: LAYOUT_TAB,
                tab_name: None,
                tiled_layout: layout_with_plugin_slot(),
                floating_layouts: vec![],
                swap_tiled_layouts: None,
                swap_floating_layouts: None,
                new_terminal_pids: vec![(TerminalId::new(2), None)],
                new_floating_pane_pids: vec![],
                plugin_ids: HashMap::from([(headless_run(), vec![PLUGIN])]),
            }],
            vec![],
            vec![],
            true,
            true,
            CLIENT,
            &mut harness.pending,
        )
        .unwrap();

    assert!(harness
        .screen
        .tabs
        .get(&LAYOUT_TAB)
        .is_some_and(|tab| tab.has_pane_with_pid(&PaneId::Terminal(TerminalId::new(2)))));
    assert!(!harness.has_pane(PLUGIN));
    assert!(!closes_pty_pane(&harness.pty_instructions(), PLUGIN));
}

#[test]
fn output_parked_for_a_paneless_plugin_is_dropped() {
    let mut harness = Harness::new();
    let pane = PaneId::Plugin(PLUGIN);
    harness.pending.waiting_for_pane.insert(
        pane,
        vec![
            ScreenInstruction::PluginBytes(vec![PluginRenderAsset::new(
                PLUGIN,
                CLIENT,
                b"parked".to_vec(),
            )]),
            ScreenInstruction::ShowPluginCursor(PLUGIN, CLIENT, Some((1, 1))),
        ],
    );

    harness.paneless(PLUGIN);

    assert!(!harness.pending.waiting_for_pane.contains_key(&pane));
    assert!(harness.screen.destroyed_panes.contains(&pane));
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "parked output is no request to close the plugin"
    );
}

#[test]
fn closing_a_paneless_plugin_unloads_it_and_answers() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);
    let (completion, rx) = completion();

    harness
        .screen
        .close_pane_from_instruction(
            PaneId::Plugin(PLUGIN),
            Some(CLIENT),
            Some(completion),
            None,
            &mut harness.pending,
        )
        .unwrap();

    assert!(closes_pty_pane(&harness.pty_instructions(), PLUGIN));
    let result = answer(rx);
    assert_eq!(result.exit_status, None);
    assert_eq!(result.error_message, None);
    assert!(
        !harness
            .pending
            .waiting_for_pane
            .contains_key(&PaneId::Plugin(PLUGIN)),
        "the close must not wait for a pane that never comes"
    );

    // The unload is on its way: a launch before `PluginUnloaded` starts a new instance.
    harness
        .screen
        .launch_or_focus_plugin(
            headless_run(),
            true,
            false,
            false,
            false,
            None,
            false,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    assert!(harness
        .pty_instructions()
        .iter()
        .any(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..))));
}

#[test]
fn a_close_waiting_for_the_pane_unloads_a_plugin_that_turns_out_paneless() {
    let mut harness = Harness::new();
    let (completion, rx) = completion();
    harness
        .screen
        .close_pane_from_instruction(
            PaneId::Plugin(PLUGIN),
            Some(CLIENT),
            Some(completion),
            None,
            &mut harness.pending,
        )
        .unwrap();
    assert!(harness
        .pending
        .waiting_for_pane
        .contains_key(&PaneId::Plugin(PLUGIN)));

    harness.paneless(PLUGIN);

    assert!(closes_pty_pane(&harness.pty_instructions(), PLUGIN));
    assert_eq!(answer(rx).error_message, None);
}

#[test]
fn focusing_a_paneless_plugin_says_it_runs_headless() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);
    let (completion, rx) = completion();

    harness
        .screen
        .focus_pane_by_request(
            PaneId::Plugin(PLUGIN),
            false,
            false,
            CLIENT,
            Some(completion),
            &mut harness.pending,
        )
        .unwrap();

    let result = answer(rx);
    assert_eq!(result.exit_status, Some(1));
    assert_eq!(
        result.error_message.as_deref(),
        Some("plugin 7 runs headless: it declares no pane")
    );
}

#[test]
fn an_unloaded_plugin_loses_its_orphan_pane_subscriptions_and_intercepts() {
    let mut harness = Harness::new();
    let other = PluginId::new(8);
    let other_client = ClientId::new(2);
    let (placed, _rx) = harness.add_plugin(PLUGIN);
    assert!(placed && harness.has_pane(PLUGIN));
    for plugin in [PLUGIN, other] {
        harness.screen.background_plugin_subscriptions.insert(
            (plugin, CLIENT),
            HashSet::from([yinz_types::data::EventType::PaneUpdate]),
        );
    }
    let mut intercepts = HashMap::from([(CLIENT, PLUGIN), (other_client, other)]);
    harness.pty_instructions();

    harness.unloaded(PLUGIN, &mut intercepts);

    assert!(!harness.has_pane(PLUGIN), "the orphaned pane must go");
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "the plugin is already unloaded"
    );
    assert!(harness.screen.unloaded_plugins.contains(&PLUGIN));
    assert_eq!(
        harness
            .screen
            .background_plugin_subscriptions
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![(other, CLIENT)]
    );
    assert_eq!(intercepts, HashMap::from([(other_client, other)]));
}

#[test]
fn an_add_plugin_after_the_unload_is_dropped_with_an_error() {
    let mut harness = Harness::new();
    harness.unloaded(PLUGIN, &mut HashMap::new());

    let (placed, rx) = harness.add_plugin(PLUGIN);

    assert!(!placed);
    assert!(!harness.has_pane(PLUGIN));
    let result = answer(rx);
    assert_eq!(result.exit_status, Some(1));
    assert_eq!(
        result.error_message.as_deref(),
        Some("plugin unloaded before its pane was placed")
    );
}

#[test]
fn an_add_plugin_after_paneless_restores_the_pane() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);

    let (placed, rx) = harness.add_plugin(PLUGIN);

    assert!(placed && harness.has_pane(PLUGIN));
    assert!(!harness.screen.paneless_plugins.contains_key(&PLUGIN));
    assert!(!harness
        .screen
        .destroyed_panes
        .contains(&PaneId::Plugin(PLUGIN)));
    assert_eq!(answer(rx).affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
}

#[test]
fn an_add_plugin_parked_for_a_client_is_dropped_when_the_plugin_goes_paneless() {
    let mut harness = Harness::new();
    let (completion, rx) = completion();
    harness
        .pending
        .waiting_for_client
        .push(ScreenInstruction::AddPlugin(
            Some(true),
            false,
            ReplacedPane::Suppress,
            headless_run(),
            None,
            None,
            PLUGIN,
            None,
            None,
            false,
            None,
            None,
            None,
            Some(completion),
            None,
        ));

    harness.paneless(PLUGIN);

    assert!(harness.pending.waiting_for_client.is_empty());
    let result = answer(rx);
    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
    assert_eq!(
        result.stdout_message.as_deref(),
        Some("file:/path/to/headless/plugin runs headless: it declares no pane")
    );
}

#[test]
fn launch_or_focus_answers_with_the_running_headless_instance() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);
    harness.pty_instructions();
    let (completion, rx) = completion();

    harness
        .screen
        .launch_or_focus_plugin(
            headless_run(),
            true,
            false,
            false,
            false,
            None,
            false,
            CLIENT,
            Some(completion),
            None,
        )
        .unwrap();

    assert!(
        !harness
            .pty_instructions()
            .iter()
            .any(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..))),
        "a second instance must not be loaded"
    );
    let result = answer(rx);
    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
    assert_eq!(
        result.stdout_message.as_deref(),
        Some("file:/path/to/headless/plugin is already running headless")
    );
    // With nothing to focus, the instance hears it was asked for again, from this client.
    assert_eq!(relaunches(&harness), [(PLUGIN, CLIENT)]);

    // Another plugin is not deduplicated against it.
    harness
        .screen
        .launch_or_focus_plugin(
            run("file:/path/to/another/plugin"),
            true,
            false,
            false,
            false,
            None,
            false,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    assert!(harness
        .pty_instructions()
        .iter()
        .any(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..))));
}

/// The private pipe messages that told a headless plugin it was launched again.
fn relaunches(harness: &Harness) -> Vec<(PluginId, ClientId)> {
    harness
        .plugin
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::KeybindPipe {
                name,
                plugin_and_client_id,
                ..
            } if name == crate::screen::plugins::RELAUNCH_PIPE => plugin_and_client_id,
            _ => None,
        })
        .collect()
}

fn launch_or_focus(harness: &mut Harness, run: RunPluginOrAlias) -> Completion {
    let (completion, rx) = completion();
    harness
        .screen
        .launch_or_focus_plugin(
            run,
            true,
            false,
            false,
            false,
            None,
            false,
            CLIENT,
            Some(completion),
            None,
        )
        .unwrap();
    rx
}

fn loads_another(instructions: &[PtyInstruction]) -> bool {
    instructions
        .iter()
        .any(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..)))
}

/// A headless launch is answered by the screen once it has recorded the plugin, so the
/// launcher's next action, which reaches the screen on another channel, always finds it.
#[test]
fn a_headless_launch_is_answered_once_the_screen_records_the_plugin() {
    let mut harness = Harness::new();
    let (mut launch, rx) = completion();
    launch.set_affected_pane_id(PaneId::Plugin(PLUGIN));
    harness
        .screen
        .plugin_paneless(PLUGIN, headless_run(), Some(launch), &mut harness.pending)
        .unwrap();
    assert_eq!(answer(rx).affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
    assert!(harness.screen.paneless_plugins.contains_key(&PLUGIN));
    harness.pty_instructions();
    let again = launch_or_focus(&mut harness, headless_run());
    assert!(!loads_another(&harness.pty_instructions()));
    assert_eq!(answer(again).affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
}

/// A git plugin has no pane and no declaration while its repository resolves. A second launch
/// finds it loading rather than starting another, and closing it unloads it.
#[test]
fn a_plugin_resolving_its_repository_is_not_loaded_twice() {
    let mut harness = Harness::new();
    let (mut launch, rx) = completion();
    launch.set_affected_pane_id(PaneId::Plugin(PLUGIN));
    harness
        .screen
        .plugin_resolving(PLUGIN, headless_run(), Some(launch));
    assert_eq!(
        answer(rx).affected_pane_id,
        Some(PaneId::Plugin(PLUGIN)),
        "the launch has the plugin's id before its clone ends"
    );
    harness.pty_instructions();

    let again = launch_or_focus(&mut harness, headless_run());
    assert!(
        !loads_another(&harness.pty_instructions()),
        "a second press must not start a second instance"
    );
    let result = answer(again);
    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
    assert_eq!(
        result.stdout_message.as_deref(),
        Some("file:/path/to/headless/plugin is still loading")
    );
    assert!(relaunches(&harness).is_empty());

    // Closing it by id unloads it.
    let (close, closed) = completion();
    harness
        .screen
        .close_pane_from_instruction(
            PaneId::Plugin(PLUGIN),
            Some(CLIENT),
            Some(close),
            None,
            &mut harness.pending,
        )
        .unwrap();
    assert!(closes_pty_pane(&harness.pty_instructions(), PLUGIN));
    assert_eq!(answer(closed).error_message, None);
    assert!(!harness.screen.resolving_plugins.contains_key(&PLUGIN));

    // Whatever it turns out to be ends the record: a pane, no pane, or a failure.
    for outcome in ["pane", "paneless", "failed"] {
        let plugin = PluginId::new(20);
        let mut harness = Harness::new();
        harness
            .screen
            .plugin_resolving(plugin, headless_run(), None);
        match outcome {
            "pane" => assert!(harness.add_plugin(plugin).0),
            "paneless" => harness.paneless(plugin),
            _ => harness.screen.plugin_failed(plugin),
        }
        assert!(
            !harness.screen.resolving_plugins.contains_key(&plugin),
            "{outcome}"
        );
        harness.pty_instructions();
        launch_or_focus(&mut harness, headless_run());
        assert_eq!(
            loads_another(&harness.pty_instructions()),
            outcome == "failed",
            "{outcome}"
        );
    }
}

/// A headless instance whose load failed or that crashed runs no more: a launch replaces it,
/// unloading it, instead of answering that it is already running.
#[test]
fn a_launch_replaces_a_failed_headless_instance() {
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);
    harness.screen.plugin_failed(PLUGIN);
    harness.pty_instructions();

    let rx = launch_or_focus(&mut harness, headless_run());

    let instructions = harness.pty_instructions();
    assert!(loads_another(&instructions), "{instructions:?}");
    assert!(
        closes_pty_pane(&instructions, PLUGIN),
        "the failed instance is unloaded"
    );
    assert!(relaunches(&harness).is_empty());
    // The launch travels with the new load, which answers it.
    drop(instructions);
    assert_ne!(
        answer(rx).stdout_message.as_deref(),
        Some("file:/path/to/headless/plugin is already running headless")
    );

    // A reload that runs it again counts it as running again.
    let mut harness = Harness::new();
    harness.paneless(PLUGIN);
    harness.screen.plugin_failed(PLUGIN);
    harness.paneless(PLUGIN);
    harness.pty_instructions();
    launch_or_focus(&mut harness, headless_run());
    assert!(!loads_another(&harness.pty_instructions()));
    assert_eq!(relaunches(&harness), [(PLUGIN, CLIENT)]);
}

#[test]
fn a_paneless_pane_tab_child_takes_the_failure_path() {
    let mut harness = Harness::new();
    let descriptor = |label: &str| PaneTabDescriptor {
        label: label.into(),
        location: "file:/path/to/headless/plugin".into(),
        initial_cwd: None,
        configuration: BTreeMap::new(),
    };
    let info = harness
        .screen
        .pane_tabs_command(
            PaneTabsCommand::Mount {
                pane_id: PaneId::Terminal(TerminalId::new(1)),
                layout: PaneTabsLayout {
                    active: 1,
                    tabs: vec![descriptor("Terminal"), descriptor("Headless")],
                },
                existing: 0,
            },
            CLIENT,
        )
        .unwrap()
        .unwrap();
    let generation = harness
        .plugin
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab {
                id,
                index: 1,
                generation,
                ..
            } if id == info.id => Some(generation),
            _ => None,
        })
        .expect("selecting the unloaded slot loads it");
    harness.paneless(PLUGIN);

    harness
        .screen
        .pane_tab_loaded(info.id, 1, generation, Ok((PLUGIN, headless_run())))
        .unwrap();

    assert!(!harness.has_pane(PLUGIN));
    let slot = harness
        .screen
        .tabs
        .get(&TabId::new(0))
        .and_then(|tab| tab.pane_tabs_info(info.id))
        .map(|info| info.tabs[1].clone())
        .unwrap();
    assert_eq!(slot.plugin_id, None);
    assert_eq!(
        slot.state,
        PaneTabLoadState::Failed("file:/path/to/headless/plugin declares no pane".into())
    );
}

/// Through the screen thread: the intercept table lives in its loop, so only a real
/// `PluginUnloaded` can show that the unload ends the plugin's intercept.
#[test]
fn plugin_unloaded_ends_the_plugins_key_intercept() {
    use yinz_types::keys::{BareKey, KeyWithModifier};
    let typed = |key: char| {
        ScreenInstruction::WriteCharacter(
            Some(KeyWithModifier::new(BareKey::Char(key))),
            key.to_string().into_bytes(),
            false,
            CLIENT,
            None,
        )
    };
    let mut mock = MockScreen::new(SIZE);
    let plugin_receiver = mock.plugin_receiver.take().unwrap();
    let screen_thread = mock.run(None, vec![]);
    let received = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(received, PluginInstruction::Exit, plugin_receiver);
    let other = PluginId::new(8);
    for step in [
        ScreenInstruction::InterceptKeyPresses(PLUGIN, CLIENT),
        ScreenInstruction::PluginUnloaded(PLUGIN),
        typed('x'),
        ScreenInstruction::InterceptKeyPresses(other, CLIENT),
        typed('y'),
    ] {
        mock.to_screen.send(step).unwrap();
    }
    let intercepted = |instructions: &[PluginInstruction]| -> Vec<(PluginId, char)> {
        instructions
            .iter()
            .filter_map(|instruction| match instruction {
                PluginInstruction::Update(updates) => Some(updates),
                _ => None,
            })
            .flatten()
            .filter_map(|(plugin, _, event)| match (plugin, event) {
                (Some(plugin), Event::InterceptedKeyPress(key)) => match key.bare_key {
                    BareKey::Char(c) => Some((*plugin, c)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    };
    received.wait_until(
        "the key typed under the later intercept",
        yinz_test_support::default_timeout(),
        |instructions| {
            intercepted(instructions)
                .contains(&(other, 'y'))
                .then_some(())
        },
    );
    mock.teardown(vec![plugin_thread, screen_thread]);

    assert_eq!(intercepted(&received.lock()), vec![(other, 'y')]);
}

/// Through the screen thread, the `background_permissions` way: a headless plugin's permission
/// request is a screen modal and needs no pane of its own.
#[test]
fn a_paneless_plugin_still_gets_its_permission_prompt() {
    use yinz_types::data::{PermissionType, PluginPermission};
    let mut mock = MockScreen::new(SIZE);
    let renders = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let logger = log_actions_in_thread!(renders, ServerInstruction::KillSession, receiver);
    let screen_thread = mock.run(None, vec![]);
    mock.to_screen
        .send(ScreenInstruction::PluginPaneless {
            plugin_id: PLUGIN,
            run: Box::new(headless_run()),
            completion: None,
        })
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::RequestPluginPermissions(
            PLUGIN,
            PluginPermission::new(
                "headless-permission-fixture".into(),
                vec![PermissionType::ReadApplicationState],
            ),
        ))
        .unwrap();

    wait_for_settled_render(
        &renders,
        SIZE,
        "a permission prompt for a plugin without a pane",
        |frame| frame.contains("Plugin permissions"),
    );
    mock.teardown(vec![screen_thread, logger]);
}

/// With no client attached an `AddPlugin` waits for one. If the plugin is unloaded meanwhile, the
/// caller hears so now, not whenever a client happens to attach.
#[test]
fn an_add_plugin_parked_for_a_client_is_answered_when_the_plugin_unloads() {
    let mut harness = Harness::new();
    let (completion, rx) = completion();
    harness
        .pending
        .waiting_for_client
        .push(ScreenInstruction::AddPlugin(
            Some(true),
            false,
            ReplacedPane::Suppress,
            headless_run(),
            None,
            None,
            PLUGIN,
            None,
            None,
            false,
            None,
            None,
            None,
            Some(completion),
            None,
        ));

    harness.unloaded(PLUGIN, &mut HashMap::new());

    assert!(harness.pending.waiting_for_client.is_empty());
    let result = answer(rx);
    assert_eq!(result.exit_status, Some(1));
    assert_eq!(
        result.error_message.as_deref(),
        Some("plugin unloaded before its pane was placed")
    );
}

/// A git plugin's declaration is sent from its repository thread, so `PluginPaneless` can arrive
/// after the plugin thread reported the unload. The gone plugin must not count as running.
#[test]
fn a_paneless_report_after_the_unload_is_ignored() {
    let mut harness = Harness::new();
    harness.unloaded(PLUGIN, &mut HashMap::new());

    harness.paneless(PLUGIN);

    assert!(!harness.screen.paneless_plugins.contains_key(&PLUGIN));
    harness.pty_instructions();
    harness
        .screen
        .launch_or_focus_plugin(
            headless_run(),
            true,
            false,
            false,
            false,
            None,
            false,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    assert!(
        harness
            .pty_instructions()
            .iter()
            .any(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..))),
        "a launch must start a new instance instead of naming the gone one"
    );
    let (completion, rx) = completion();
    harness
        .screen
        .close_pane_from_instruction(
            PaneId::Plugin(PLUGIN),
            Some(CLIENT),
            Some(completion),
            None,
            &mut harness.pending,
        )
        .unwrap();
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "a gone plugin is not unloaded again"
    );
    assert_eq!(answer(rx).error_message, None);
}

/// Closing the shown plugin tab of a pane tabs container keeps its pane as a blank slot that
/// owns no guest, and unloads the plugin. The unload report that follows must leave that blank
/// slot alone: it is the container's pane now, not the plugin's.
#[test]
fn an_unload_after_closing_the_shown_pane_tab_keeps_the_blank_slot() {
    let mut harness = Harness::new();
    let descriptor = |label: &str| PaneTabDescriptor {
        label: label.into(),
        location: "file:/path/to/headless/plugin".into(),
        initial_cwd: None,
        configuration: BTreeMap::new(),
    };
    let info = harness
        .screen
        .pane_tabs_command(
            PaneTabsCommand::Mount {
                pane_id: PaneId::Terminal(TerminalId::new(1)),
                layout: PaneTabsLayout {
                    active: 1,
                    tabs: vec![descriptor("Terminal"), descriptor("Plugin")],
                },
                existing: 0,
            },
            CLIENT,
        )
        .unwrap()
        .unwrap();
    let generation = harness
        .plugin
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab {
                id,
                index: 1,
                generation,
                ..
            } if id == info.id => Some(generation),
            _ => None,
        })
        .expect("selecting the unloaded slot loads it");
    harness
        .screen
        .pane_tab_loaded(info.id, 1, generation, Ok((PLUGIN, headless_run())))
        .unwrap();
    let shown = |harness: &Harness| {
        harness
            .screen
            .tabs
            .get(&TabId::new(0))
            .and_then(|tab| tab.pane_tabs_info(info.id))
            .map(|info| info.pane_id)
    };
    assert_eq!(shown(&harness), Some(PaneId::Plugin(PLUGIN)));
    harness
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .close_pane_tab_child(info.id, 1)
        .unwrap();
    assert!(harness.has_pane(PLUGIN), "the blank slot keeps the pane");

    harness.unloaded(PLUGIN, &mut HashMap::new());

    assert_eq!(
        shown(&harness),
        Some(PaneId::Plugin(PLUGIN)),
        "the container must survive"
    );
    assert!(
        harness.has_pane(PLUGIN),
        "the container's blank slot must not be removed"
    );
}

fn unloads_of(harness: &Harness, plugin: PluginId) -> usize {
    harness
        .plugin
        .try_iter()
        .filter(|(instruction, _)| matches!(instruction, PluginInstruction::Unload(id) if *id == plugin))
        .count()
}

/// Any plugin pane can be mounted into pane tabs (`PaneTabsCommand::Mount`), not only one a
/// layout placed: here, one a launch opened. When it reloads without its declaration it keeps
/// running headless, so it leaves its pane tab without the unload that closing the pane tab
/// sends.
#[test]
fn a_mounted_plugin_pane_that_goes_paneless_leaves_its_pane_tab_and_keeps_running() {
    let mut harness = Harness::new();
    let (placed, _answer) = harness.add_plugin(PLUGIN);
    assert!(placed && harness.has_pane(PLUGIN));
    let descriptor = |label: &str| PaneTabDescriptor {
        label: label.into(),
        location: "file:/path/to/headless/plugin".into(),
        initial_cwd: None,
        configuration: BTreeMap::new(),
    };
    let info = harness
        .screen
        .pane_tabs_command(
            PaneTabsCommand::Mount {
                pane_id: PaneId::Plugin(PLUGIN),
                layout: PaneTabsLayout {
                    active: 0,
                    tabs: vec![descriptor("Plugin"), descriptor("Other")],
                },
                existing: 0,
            },
            CLIENT,
        )
        .unwrap()
        .unwrap();
    unloads_of(&harness, PLUGIN);
    harness.pty_instructions();

    harness.paneless(PLUGIN);

    assert_eq!(
        unloads_of(&harness, PLUGIN),
        0,
        "a headless plugin is not unloaded"
    );
    assert!(!closes_pty_pane(&harness.pty_instructions(), PLUGIN));
    assert!(harness.screen.paneless_plugins.contains_key(&PLUGIN));
    let tabs = harness
        .screen
        .tabs
        .get(&TabId::new(0))
        .and_then(|tab| tab.pane_tabs_info(info.id))
        .expect("the pane tabs survive");
    assert!(tabs
        .tabs
        .iter()
        .all(|slot| slot.pane_id() != Some(PaneId::Plugin(PLUGIN))));
}

/// A reload that declares a pane again takes the plugin out of the background registry: with a
/// pane in a tab it hears pane and tab events as any plugin pane does, and a registration left
/// behind would keep it hearing every other tab's too.
#[test]
fn a_reload_that_declares_a_pane_again_leaves_the_background_registry() {
    let mut harness = Harness::new();
    let other = PluginId::new(8);
    harness.paneless(PLUGIN);
    for plugin in [PLUGIN, other] {
        harness.screen.background_plugin_subscriptions.insert(
            (plugin, CLIENT),
            HashSet::from([yinz_types::data::EventType::PaneUpdate]),
        );
    }

    let (placed, _rx) = harness.add_plugin(PLUGIN);

    assert!(placed && harness.has_pane(PLUGIN));
    assert_eq!(
        harness
            .screen
            .background_plugin_subscriptions
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![(other, CLIENT)],
        "only the reloaded plugin leaves the registry"
    );
}

/// An `AddPlugin` for a plugin that never ran headless leaves its registration alone: a plugin
/// with no tab hears pane and tab events through the registry whether it has a pane or not.
#[test]
fn an_add_plugin_for_a_plugin_that_was_never_headless_keeps_its_registration() {
    let mut harness = Harness::new();
    harness.screen.background_plugin_subscriptions.insert(
        (PLUGIN, CLIENT),
        HashSet::from([yinz_types::data::EventType::PaneUpdate]),
    );

    let (placed, _rx) = harness.add_plugin(PLUGIN);

    assert!(placed);
    assert!(harness
        .screen
        .background_plugin_subscriptions
        .contains_key(&(PLUGIN, CLIENT)));
}

const OWNER: PluginId = PluginId::new(3);

impl Harness {
    /// A left/right multipane in tab 0 of the plugin `OWNER` and `child`, the way a plugin
    /// composes itself with a slot, with the client's keyboard on `child`.
    fn compose_with(&mut self, child: PaneId) -> [PaneId; 2] {
        use yinz_types::multipane::*;
        let children = [PaneId::Plugin(OWNER), child];
        let placement = OverlayPlacement::dismissible(None);
        let tab = self.screen.tabs.get_mut(&TabId::new(0)).unwrap();
        for pane in children {
            tab.new_pane(
                pane,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(placement.clone()),
                Some(CLIENT),
                None,
            )
            .unwrap();
        }
        self.screen
            .multipane_command(
                MultipaneOperation::Create {
                    children,
                    split: MultipaneSplit {
                        direction: MultipaneDirection::LeftRight,
                        ratio: 5000,
                    },
                    placement,
                    focus: Some(1),
                    min_cells: 0,
                },
                CLIENT,
            )
            .unwrap();
        assert_eq!(self.screen.active_pane_id(CLIENT), Some(child));
        self.pty_instructions();
        children
    }

    fn multipane_of(&self, pane: PaneId) -> Option<yinz_types::multipane::MultipaneInfo> {
        self.screen
            .tab_containing_pane(pane)
            .and_then(|tab| tab.multipane_info(pane))
    }
}

/// A slot held for a multipane child that will not have a pane there is given up: it leaves the
/// container, whose owner stays as its only pane and gets the keyboard the slot had. The pty frees
/// the held terminal, and nothing asks it to close a plugin.
#[test]
fn a_given_up_multipane_slot_leaves_its_owner_alone_in_the_container() {
    let mut harness = Harness::new();
    let slot = PaneId::Terminal(TerminalId::new(5));
    let [owner, _] = harness.compose_with(slot);

    harness
        .screen
        .give_up_slot(slot, &mut harness.pending)
        .unwrap();

    assert!(harness.screen.tab_containing_pane(slot).is_none());
    assert!(harness.has_pane(OWNER), "the owner stays");
    assert_eq!(
        harness.multipane_of(owner),
        None,
        "one pane is no multipane"
    );
    assert!(harness
        .screen
        .tabs
        .get(&TabId::new(0))
        .unwrap()
        .pane_is_overlay(owner));
    assert_eq!(harness.screen.active_pane_id(CLIENT), Some(owner));
    assert!(harness.screen.destroyed_panes.contains(&slot));
    let pty = harness.pty_instructions();
    assert!(
        matches!(pty.as_slice(), [PtyInstruction::ClosePane(closed, _)] if *closed == slot),
        "only the held terminal is freed: {pty:?}"
    );
    assert!(!closes_pty_pane(&pty, OWNER));
}

/// A plugin in a multipane slot that reloads without a pane gives the slot up the same way, and
/// keeps running: nothing is sent to the pty, which would unload it.
#[test]
fn a_multipane_child_that_goes_paneless_leaves_its_container_without_a_pty_close() {
    let mut harness = Harness::new();
    let [owner, child] = harness.compose_with(PaneId::Plugin(PLUGIN));

    harness.paneless(PLUGIN);

    assert!(!harness.has_pane(PLUGIN));
    assert!(harness.has_pane(OWNER));
    assert_eq!(harness.multipane_of(owner), None);
    assert_eq!(harness.screen.active_pane_id(CLIENT), Some(owner));
    assert!(harness.screen.destroyed_panes.contains(&child));
    assert!(harness.screen.paneless_plugins.contains_key(&PLUGIN));
    assert!(
        harness.pty_instructions().is_empty(),
        "a headless plugin is not unloaded"
    );
}

/// A slot whose container is gone by the time it is given up closes as any `ClosePane` closes
/// it, freeing what it held.
#[test]
fn a_given_up_slot_outside_a_multipane_closes_as_usual() {
    let mut harness = Harness::new();
    let slot = PaneId::Terminal(TerminalId::new(5));
    // What `multipanes::reserve` holds a slot with: the default shell, held before it starts.
    let held_shell = RunCommand {
        command: PathBuf::from("/bin/sh"),
        hold_on_start: true,
        hold_on_close: true,
        ..Default::default()
    };
    harness
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .new_pane(
            slot,
            None,
            Some(Run::Command(held_shell)),
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(CLIENT),
            None,
        )
        .unwrap();
    harness.pty_instructions();

    harness
        .screen
        .give_up_slot(slot, &mut harness.pending)
        .unwrap();

    assert!(harness.screen.tab_containing_pane(slot).is_none());
    assert!(matches!(
        harness.pty_instructions().as_slice(),
        [PtyInstruction::ClosePane(closed, _)] if *closed == slot
    ));
    // The held shell never ran, so nothing exited: no `pane_exit` hook.
    let hooks: Vec<_> = harness
        .plugin
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RunHook { point, pane_id, .. } => Some((point, pane_id)),
            _ => None,
        })
        .collect();
    assert!(hooks.is_empty(), "{hooks:?}");
}

/// A plugin shown as a pane tab in a multipane slot that goes paneless closes its pane tab, as it
/// does outside a multipane: the pane tabs container keeps its blank slot and its place in the
/// multipane. Taking the member out would leave the pane tabs naming a pane that is gone.
#[test]
fn a_pane_tab_child_in_a_multipane_slot_that_goes_paneless_keeps_the_container() {
    let mut harness = Harness::new();
    let [owner, child] = harness.compose_with(PaneId::Plugin(PLUGIN));
    let descriptor = |label: &str| PaneTabDescriptor {
        label: label.into(),
        location: "file:/path/to/headless/plugin".into(),
        initial_cwd: None,
        configuration: BTreeMap::new(),
    };
    let info = harness
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .mount_pane_tabs(
            child,
            PaneTabsLayout {
                active: 0,
                tabs: vec![descriptor("Plugin"), descriptor("Other")],
            },
            0,
            CLIENT,
        )
        .unwrap();
    harness.pty_instructions();

    harness.paneless(PLUGIN);

    let tab = harness.screen.tabs.get(&TabId::new(0)).unwrap();
    let tabs = tab.pane_tabs_info(info.id).expect("the pane tabs survive");
    assert_eq!(
        tabs.pane_id, child,
        "the blank slot keeps the container's pane"
    );
    assert!(tabs.tabs.iter().all(|slot| slot.pane_id() != Some(child)));
    assert!(tab.has_pane_with_pid(&child));
    assert_eq!(
        harness.multipane_of(owner).map(|info| info.children),
        Some([owner, child]),
        "the container keeps its place in the multipane"
    );
    assert!(harness.screen.paneless_plugins.contains_key(&PLUGIN));
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "a headless plugin is not unloaded"
    );
    assert_eq!(
        unloads_of(&harness, PLUGIN),
        0,
        "nor is it unloaded with its pane tab"
    );
}

/// Answers arriving through the screen thread, each under the label of the request it ends.
struct Answers(Watched<Vec<(String, yinz_bus::route::ActionCompletionResult)>>);

impl Answers {
    fn new() -> Self {
        Answers(Watched::new(vec![]))
    }

    /// A completion whose answer is recorded under `label` once the screen gives it.
    fn completion(&self, label: &str) -> Option<NotificationEnd> {
        let (completion, rx) = completion();
        let answers = self.0.clone();
        let label = label.to_owned();
        std::thread::spawn(move || {
            if let Ok(result) = rx.blocking_recv() {
                answers.lock().push((label, result));
            }
        });
        Some(completion)
    }

    fn wait_for(&self, count: usize) -> Vec<(String, yinz_bus::route::ActionCompletionResult)> {
        self.0.wait_until(
            &format!("{count} answers"),
            yinz_test_support::default_timeout(),
            |answers| (answers.len() >= count).then(|| answers.clone()),
        )
    }
}

/// `yinz action close-pane --pane-id plugin_N` sends `CloseFocusWithPaneId`. For a headless
/// plugin it unloads the plugin and answers, as `ClosePane` does, through the screen thread's
/// dispatch rather than only through `close_pane_from_instruction`.
#[test]
fn closing_a_headless_plugin_by_id_through_the_screen_thread_unloads_it() {
    let mut mock = MockScreen::new(SIZE);
    let pty_receiver = mock.pty_receiver.take().unwrap();
    let pty = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(pty, PtyInstruction::Exit, pty_receiver);
    let screen_thread = mock.run(None, vec![]);
    let answers = Answers::new();
    mock.to_screen
        .send(ScreenInstruction::PluginPaneless {
            plugin_id: PLUGIN,
            run: Box::new(headless_run()),
            completion: None,
        })
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::CloseFocusWithPaneId(
            PaneId::Plugin(PLUGIN),
            answers.completion("close"),
        ))
        .unwrap();

    let answered = answers.wait_for(1);
    pty.wait_until(
        "the headless plugin's unload",
        yinz_test_support::default_timeout(),
        |instructions| closes_pty_pane(instructions, PLUGIN).then_some(()),
    );
    mock.teardown(vec![pty_thread, screen_thread]);
    assert_eq!(answered[0].1.exit_status, None, "{answered:?}");
    assert_eq!(answered[0].1.error_message, None, "{answered:?}");
}

/// A close by id for a pane that never existed says so, instead of succeeding.
#[test]
fn closing_a_pane_id_that_never_existed_is_an_error() {
    let mut mock = MockScreen::new(SIZE);
    let screen_thread = mock.run(None, vec![]);
    let answers = Answers::new();
    for pane_id in [
        PaneId::Plugin(PLUGIN),
        PaneId::Terminal(TerminalId::new(99)),
    ] {
        mock.to_screen
            .send(ScreenInstruction::CloseFocusWithPaneId(
                pane_id,
                answers.completion(&format!("{pane_id:?}")),
            ))
            .unwrap();
    }
    let answered = answers.wait_for(2);
    mock.teardown(vec![screen_thread]);
    for (label, result) in answered {
        assert_eq!(result.exit_status, Some(1), "{label}");
        assert!(
            result
                .error_message
                .as_deref()
                .is_some_and(|error| error.contains("not found")),
            "{label}: {result:?}"
        );
    }
}

/// Every request that needs a headless plugin's pane is refused with the reason: none succeeds
/// as if the pane had been acted on, and none waits for a pane that never comes.
#[test]
fn requests_for_a_headless_plugins_pane_say_it_runs_headless() {
    use yinz_types::geometry::{Direction, FloatingPaneCoordinates, ResizeStrategy};
    let mut mock = MockScreen::new(SIZE);
    let screen_thread = mock.run(None, vec![]);
    let answers = Answers::new();
    let headless = PaneId::Plugin(PLUGIN);
    let terminal = PaneId::Terminal(TerminalId::new(0));
    mock.to_screen
        .send(ScreenInstruction::PluginPaneless {
            plugin_id: PLUGIN,
            run: Box::new(headless_run()),
            completion: None,
        })
        .unwrap();
    let c = |label: &str| answers.completion(label);
    let requests = vec![
        ScreenInstruction::RenamePane(headless, b"name".to_vec(), c("rename")),
        ScreenInstruction::RenamePaneWithPaneId(headless, b"name".to_vec(), c("rename by id")),
        ScreenInstruction::UndoRenamePaneWithPaneId(headless, c("undo rename")),
        ScreenInstruction::TogglePanePinnedWithPaneId(headless, c("pin")),
        ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(headless, c("float")),
        ScreenInstruction::ToggleFullscreenWithPaneId(headless, c("fullscreen")),
        ScreenInstruction::ScrollUpWithPaneId(headless, c("scroll")),
        ScreenInstruction::ResizeWithPaneId(
            headless,
            ResizeStrategy::new(Resize::Increase, None),
            c("resize"),
        ),
        ScreenInstruction::MovePaneWithPaneIdCli(headless, Some(Direction::Left), c("move")),
        ScreenInstruction::EditScrollbackWithPaneId(headless, false, c("edit scrollback")),
        ScreenInstruction::WriteToPaneId(b"x".to_vec(), headless, c("write")),
        ScreenInstruction::TogglePaneBorderless(headless, c("borderless")),
        ScreenInstruction::SetPaneAgent(headless, None, c("agent")),
        ScreenInstruction::StackPanes(vec![terminal, headless], CLIENT, c("stack")),
        ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![headless],
            default_shell: None,
            should_change_focus_to_new_tab: true,
            new_tab_name: None,
            client_id: CLIENT,
            completion_tx: c("break to a new tab"),
        },
        ScreenInstruction::ChangeFloatingPanesCoordinates(
            vec![(headless, FloatingPaneCoordinates::default())],
            c("coordinates"),
        ),
        ScreenInstruction::ReplacePaneWithExistingPane(terminal, headless, false, c("replace")),
    ];
    let count = requests.len();
    for request in requests {
        mock.to_screen.send(request).unwrap();
    }

    let answered = answers.wait_for(count);
    mock.teardown(vec![screen_thread]);
    for (label, result) in answered {
        assert_eq!(result.exit_status, Some(1), "{label}: {result:?}");
        assert_eq!(
            result.error_message.as_deref(),
            Some("plugin 7 runs headless: it declares no pane"),
            "{label}"
        );
    }
}

/// A request parked for a plugin pane still being placed is refused once the plugin turns out
/// to be headless, instead of ending as if it had been done.
#[test]
fn a_request_parked_for_the_pane_is_refused_when_the_plugin_turns_out_headless() {
    let mut harness = Harness::new();
    let (completion, rx) = completion();
    harness
        .pending
        .waiting_for_pane
        .entry(PaneId::Plugin(PLUGIN))
        .or_default()
        .push(ScreenInstruction::SetPaneAgent(
            PaneId::Plugin(PLUGIN),
            None,
            Some(completion),
        ));

    harness.paneless(PLUGIN);

    let result = answer(rx);
    assert_eq!(result.exit_status, Some(1));
    assert_eq!(
        result.error_message.as_deref(),
        Some("plugin 7 runs headless: it declares no pane")
    );
    assert!(
        !closes_pty_pane(&harness.pty_instructions(), PLUGIN),
        "a refused request must not unload the plugin"
    );
}

#[test]
fn successful_reload_refreshes_headless_locations_without_losing_alias_metadata() {
    use yinz_types::input::layout::PluginAlias;

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("v1.scm");
    let second = dir.path().join("v2.scm");
    let current = dir.path().join("current.scm");
    std::fs::write(&first, "").unwrap();
    std::fs::write(&second, "").unwrap();
    std::os::unix::fs::symlink(&first, &current).unwrap();
    let url = format!("file:{}", current.display());
    let original = run(&url).get_run_plugin().unwrap();
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&second, &current).unwrap();
    let resolved = run(&url).get_run_plugin().unwrap();

    for alias in [false, true] {
        let mut harness = Harness::new();
        let recorded = if alias {
            RunPluginOrAlias::Alias(PluginAlias {
                name: "current".into(),
                configuration: Some(original.configuration.clone()),
                initial_cwd: Some(dir.path().to_owned()),
                run_plugin: Some(original.clone()),
            })
        } else {
            RunPluginOrAlias::RunPlugin(original.clone())
        };
        harness
            .screen
            .plugin_paneless(PLUGIN, recorded.clone(), None, &mut harness.pending)
            .unwrap();
        harness
            .screen
            .update_plugin_location(PLUGIN, resolved.location.clone());
        let updated = &harness.screen.paneless_plugins[&PLUGIN];
        let mut expected = original.clone();
        expected.location = resolved.location.clone();
        assert_eq!(updated.get_run_plugin().unwrap(), expected);
        if let (RunPluginOrAlias::Alias(before), RunPluginOrAlias::Alias(after)) =
            (&recorded, updated)
        {
            assert_eq!(before.name, after.name);
            assert_eq!(before.configuration, after.configuration);
            assert_eq!(before.initial_cwd, after.initial_cwd);
        }
        let result = answer(launch_or_focus(&mut harness, recorded));
        assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(PLUGIN)));
        assert!(!loads_another(&harness.pty_instructions()));
        assert_eq!(relaunches(&harness), vec![(PLUGIN, CLIENT)]);

        harness.unloaded(PLUGIN, &mut HashMap::new());
        harness
            .screen
            .update_plugin_location(PLUGIN, resolved.location.clone());
        assert!(!harness.screen.paneless_plugins.contains_key(&PLUGIN));
    }
}
