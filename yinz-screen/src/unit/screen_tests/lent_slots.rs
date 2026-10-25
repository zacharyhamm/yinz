//! A plugin running a command, or another plugin, in a pane's slot: the merge-queue stack of an
//! exited agent behind its `agent_exit` hook, with a held CI gate the hook ran in its own place.
//! The gate only borrows the hook's slot, so closing it however the user likes gives the slot
//! back; and nothing opens in place of a pane that has no slot to give.

use super::*;
use crate::route::NotificationEnd;
use crate::screen::thread_main::PendingEvents;
use yinz_bus::pty::ReplacedPane;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::{PluginId, TabId};

const TAB: TabId = TabId::new(0);
const CLIENT: ClientId = ClientId::new(1);
const AGENT: PaneId = PaneId::Terminal(TerminalId::new(1));
const HOOK: PaneId = PaneId::Plugin(PluginId::new(2));
const GATE: PaneId = PaneId::Terminal(TerminalId::new(3));

struct Captured {
    pty: Receiver<(PtyInstruction, ErrorContext)>,
    plugins: Receiver<(PluginInstruction, ErrorContext)>,
}

fn screen() -> (Screen, Captured) {
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
        false,
    );
    let (to_pty, pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (to_plugin, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    new_tab(&mut screen, 1, TAB);
    (screen, Captured { pty, plugins })
}

fn plugin_run() -> Run {
    Run::Plugin(RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url("file:/hook.scm").unwrap(),
    ))
}

fn held_gate() -> RunCommand {
    RunCommand {
        command: "ci".into(),
        hold_on_close: true,
        ..Default::default()
    }
}

/// An in-place request's completion, and what it will answer.
fn completion() -> (
    Option<NotificationEnd>,
    tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (Some(NotificationEnd::new(tx)), rx)
}

/// The agent exits and its hook opens over it, as `ShowPane::in_place_of_hooked_pane` asks;
/// then the hook runs its gate in its own place, as `OpenCommandPaneInPlaceOfPlugin` asks.
fn merge_queue_stack(screen: &mut Screen) {
    let tab = screen.tabs.get_mut(&TAB).unwrap();
    tab.hold_pane(AGENT, Some(0), false, RunCommand::default());
    assert!(screen.replace_pane(
        HOOK,
        None,
        Some(plugin_run()),
        None,
        ReplacedPane::Suppress,
        ClientTabIndexOrPaneId::PaneId(AGENT),
        &mut None,
    ));
    let (mut done, _) = completion();
    assert!(screen.replace_pane(
        GATE,
        Some(held_gate()),
        None,
        None,
        ReplacedPane::Lend,
        ClientTabIndexOrPaneId::PaneId(HOOK),
        &mut done,
    ));
    let tab = screen.tabs.get_mut(&TAB).unwrap();
    tab.hold_pane(GATE, Some(1), false, held_gate());
    tab.focus_pane_with_id(GATE, false, false, CLIENT).unwrap();
}

fn suppressed(screen: &Screen, id: PaneId) -> Option<bool> {
    screen.get_pane_info(id).map(|info| info.is_suppressed)
}

fn closed_by_pty(captured: &Captured) -> Vec<PaneId> {
    captured
        .pty
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ClosePane(id, _) => Some(id),
            _ => None,
        })
        .collect()
}

#[test]
fn ctrl_c_on_the_held_gate_leaves_the_hook_in_the_slot_over_its_suppressed_agent() {
    let (mut screen, captured) = screen();
    merge_queue_stack(&mut screen);
    let (to_screen, from_tab) = channels::unbounded();
    screen.tabs.get_mut(&TAB).unwrap().senders.to_screen = Some(SenderWithContext::new(to_screen));
    closed_by_pty(&captured);

    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .write_to_active_terminal(&None, vec![3], false, CLIENT, None)
        .unwrap();
    let Some((closing, done)) =
        from_tab
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                ScreenInstruction::CloseFocusWithPaneId(id, done) => Some((id, done)),
                _ => None,
            })
    else {
        panic!("Ctrl-C on a held pane did not ask to close it");
    };
    assert_eq!(closing, GATE);
    // What `CloseFocusWithPaneId` does on the screen thread.
    screen.close_pane_explicitly(closing, done);

    assert_eq!(suppressed(&screen, GATE), None, "the gate is closed");
    assert_eq!(suppressed(&screen, HOOK), Some(false), "the hook is back");
    assert_eq!(
        suppressed(&screen, AGENT),
        Some(true),
        "the agent is still behind it"
    );
    let tab = screen.tabs.get(&TAB).unwrap();
    assert_eq!(tab.stands_in_for_a_pane(HOOK), Some(AGENT));
    assert_eq!(closed_by_pty(&captured), vec![GATE]);
    assert!(!screen.destroyed_panes.contains(&HOOK));
    assert!(!screen.destroyed_panes.contains(&AGENT));
}

#[test]
fn opening_in_place_of_a_suppressed_or_missing_pane_closes_the_new_pane_and_says_why() {
    for (new_pane, target, why) in [
        (GATE, AGENT, "suppressed"),
        (GATE, PaneId::Terminal(TerminalId::new(99)), "not found"),
        // The plugin side: a hook opening over an agent that went into hiding meanwhile.
        (PaneId::Plugin(PluginId::new(7)), AGENT, "suppressed"),
    ] {
        let (mut screen, captured) = screen();
        let tab = screen.tabs.get_mut(&TAB).unwrap();
        tab.suppress_pane_and_replace_with_pid(
            AGENT,
            HOOK,
            ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .unwrap();
        closed_by_pty(&captured);
        let (mut done, answer) = completion();
        let run = matches!(new_pane, PaneId::Plugin(_)).then(plugin_run);
        assert!(!screen.replace_pane(
            new_pane,
            None,
            run,
            None,
            ReplacedPane::Lend,
            ClientTabIndexOrPaneId::PaneId(target),
            &mut done,
        ));
        drop(done);
        let answer = answer.blocking_recv().unwrap();
        assert!(
            answer.error_message.as_deref().unwrap().contains(why),
            "{answer:?}"
        );
        assert_eq!(answer.affected_pane_id, None);
        // Closing a plugin pane on the pty thread unloads the plugin: nothing is orphaned.
        assert_eq!(closed_by_pty(&captured), vec![new_pane]);
        assert_eq!(suppressed(&screen, new_pane), None);
        assert_eq!(suppressed(&screen, AGENT), Some(true));
        assert_eq!(suppressed(&screen, HOOK), Some(false));
    }
}

#[test]
fn a_plugin_opened_in_place_of_a_pane_id_is_checked_before_loading_and_lent_the_slot() {
    let cover = PaneId::Plugin(PluginId::new(9));
    // Refused before the plugin thread loads anything.
    for (target, why) in [
        (AGENT, "suppressed"),
        (PaneId::Terminal(TerminalId::new(99)), "not found"),
    ] {
        let (mut screen, captured) = screen();
        merge_queue_stack(&mut screen);
        let (done, answer) = completion();
        screen.open_plugin_pane_in_place_of_pane_id(
            target,
            "file:/hook.scm".into(),
            BTreeMap::new(),
            PathBuf::from("/"),
            CLIENT,
            done.unwrap(),
        );
        let answer = answer.blocking_recv().unwrap();
        assert!(
            answer.error_message.as_deref().unwrap().contains(why),
            "{answer:?}"
        );
        assert!(captured
            .plugins
            .try_iter()
            .all(|(instruction, _)| !matches!(
                instruction,
                PluginInstruction::OpenPluginPaneInPlaceOfPaneId { .. }
            )));
    }

    // A visible pane: the plugin thread is asked to load into this tab, and the pane it adds
    // covers the target without taking it - closing the cover explicitly brings it back.
    let (mut screen, captured) = screen();
    let (done, _answer) = completion();
    screen.open_plugin_pane_in_place_of_pane_id(
        AGENT,
        "file:/hook.scm".into(),
        BTreeMap::from([("job".into(), "42".into())]),
        PathBuf::from("/"),
        CLIENT,
        done.unwrap(),
    );
    let forwarded = captured
        .plugins
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::OpenPluginPaneInPlaceOfPaneId {
                pane_id,
                tab_id,
                configuration,
                completion,
                ..
            } => Some((pane_id, tab_id, configuration, completion)),
            _ => None,
        });
    let Some((pane_id, tab_id, configuration, completion)) = forwarded else {
        panic!("the plugin thread was not asked to load the cover");
    };
    assert_eq!((pane_id, tab_id), (AGENT, TAB));
    assert_eq!(configuration["job"], "42");
    // What the plugin thread answers with once it has loaded the plugin.
    let mut pending = PendingEvents::default();
    screen
        .add_plugin(
            Some(false),
            true,
            ReplacedPane::Lend,
            RunPluginOrAlias::RunPlugin(
                yinz_runtime::plugin_location::run_plugin_from_url("file:/hook.scm").unwrap(),
            ),
            None,
            Some(TAB),
            PluginId::new(9),
            Some(AGENT),
            None,
            false,
            None,
            Some(false),
            Some(CLIENT),
            Some(completion),
            None,
            &mut pending,
        )
        .unwrap();
    assert_eq!(suppressed(&screen, cover), Some(false));
    assert_eq!(suppressed(&screen, AGENT), Some(true));
    closed_by_pty(&captured);
    screen.close_pane_explicitly(cover, None);
    assert_eq!(
        suppressed(&screen, AGENT),
        Some(false),
        "the agent survives"
    );
    assert_eq!(closed_by_pty(&captured), vec![cover]);
}

#[test]
fn replacing_with_an_existing_pane_refuses_a_target_without_a_slot_and_moves_nothing() {
    let (mut screen, captured) = screen();
    merge_queue_stack(&mut screen);
    let elsewhere = PaneId::Terminal(TerminalId::new(5));
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .vertical_split(elsewhere, None, None, CLIENT, None, None)
        .unwrap();
    closed_by_pty(&captured);
    for (target, existing, why) in [
        (AGENT, elsewhere, "suppressed"),
        (HOOK, elsewhere, "suppressed"),
        (GATE, GATE, "itself"),
        (
            PaneId::Terminal(TerminalId::new(99)),
            elsewhere,
            "not found",
        ),
    ] {
        let (done, answer) = completion();
        let refused = screen
            .replace_pane_with_existing_pane(target, existing, false, done)
            .unwrap_err();
        assert!(format!("{refused:#}").contains(why), "{refused:#}");
        assert!(answer
            .blocking_recv()
            .unwrap()
            .error_message
            .unwrap()
            .contains(why));
        assert_eq!(suppressed(&screen, elsewhere), Some(false));
        assert_eq!(suppressed(&screen, GATE), Some(false));
        assert_eq!(suppressed(&screen, HOOK), Some(true));
        assert_eq!(suppressed(&screen, AGENT), Some(true));
    }
    assert!(closed_by_pty(&captured).is_empty());

    // The hook taking its slot back from an exited gate still works, and the agent stays
    // behind the hook.
    screen
        .replace_pane_with_existing_pane(GATE, HOOK, false, None)
        .unwrap();
    assert_eq!(suppressed(&screen, GATE), None);
    assert_eq!(suppressed(&screen, HOOK), Some(false));
    assert_eq!(suppressed(&screen, AGENT), Some(true));
    assert_eq!(
        screen.tabs.get(&TAB).unwrap().stands_in_for_a_pane(HOOK),
        Some(AGENT)
    );
    assert_eq!(closed_by_pty(&captured), vec![GATE]);
}

#[test]
fn a_new_in_place_pane_whose_target_holds_no_slot_is_refused_without_stopping_the_screen() {
    let mut mock_screen = MockScreen::new(Size {
        cols: 121,
        rows: 20,
    });
    let screen_thread = mock_screen.run(None, vec![]);
    let (done, answer) = completion();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewPane(
            PaneId::Terminal(TerminalId::new(5)),
            None,
            None,
            None,
            NewPanePlacement::InPlace {
                pane_id_to_replace: Some(PaneId::Terminal(TerminalId::new(99))),
                close_replaced_pane: false,
                borderless: None,
            },
            false,
            // As `new-pane --in-place --tab-id`: the client path already logs and carries on.
            ClientTabIndexOrPaneId::TabIndex(TAB),
            done,
            false,
        ))
        .unwrap();
    let answer = answer
        .blocking_recv()
        .expect("the screen answers the refused placement");
    assert!(answer.error_message.is_some(), "{answer:?}");
    // A screen thread that returned the refusal as an error panics here.
    mock_screen.teardown(vec![screen_thread]);
}
