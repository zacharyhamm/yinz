use super::*;
use hegel::generators;
use std::collections::BTreeMap;
use tokio::sync::oneshot;
use yinz_bus::route::ActionCompletionResult;
use yinz_types::input::layout::{RunPlugin, RunPluginLocation};

const CLIENT: ClientId = ClientId::new(1);

fn run(n: u32) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::File(PathBuf::from(format!("/plugins/p{n}.scm"))),
        ..Default::default()
    })
}

fn location(n: u32) -> String {
    run(n).location_string()
}

fn completion() -> (NotificationEnd, oneshot::Receiver<ActionCompletionResult>) {
    let (tx, rx) = oneshot::channel();
    (NotificationEnd::new(tx), rx)
}

fn show(n: u32, client_id: Option<ClientId>, reserved_slot: Option<PaneId>) -> ShowPane {
    ShowPane {
        should_float: Some(true),
        in_place: false,
        replaced: ReplacedPane::Close,
        run: run(n),
        title: Some(format!("title {n}")),
        tab: None,
        pane_id_to_replace: None,
        cwd: None,
        coordinates: None,
        should_focus: Some(true),
        client_id,
        completion: None,
        overlay: None,
        reserved_slot,
    }
}

fn show_request(
    n: u32,
    client_id: Option<ClientId>,
    reserved_slot: Option<PaneId>,
) -> (PaneRequest, oneshot::Receiver<ActionCompletionResult>) {
    let (end, rx) = completion();
    let mut show = show(n, client_id, reserved_slot);
    show.completion = Some(end);
    (PaneRequest::Show(Box::new(show)), rx)
}

fn parked(n: u32) -> PaneRequest {
    PaneRequest::Parked {
        run: run(n),
        cwd: Some(PathBuf::from("/cwd")),
        client_id: CLIENT,
    }
}

fn placed(n: u32) -> PaneRequest {
    PaneRequest::Placed { run: run(n) }
}

fn answered(rx: &mut oneshot::Receiver<ActionCompletionResult>) -> ActionCompletionResult {
    rx.try_recv().expect("the completion was answered")
}

/// A one-word description of each effect, for comparing whole effect lists.
fn kinds(effects: &[PaneEffect]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            PaneEffect::Screen(ScreenInstruction::AddPlugin(
                should_float,
                in_place,
                _,
                _,
                _,
                _,
                id,
                _,
                _,
                start_suppressed,
                _,
                should_focus,
                _,
                completion,
                _,
            )) => format!(
                "AddPlugin({id}, float={should_float:?}, in_place={in_place}, \
                 suppressed={start_suppressed}, focus={should_focus:?}, completion={})",
                completion.is_some()
            ),
            PaneEffect::Screen(ScreenInstruction::PluginPaneless {
                plugin_id,
                completion,
                ..
            }) => match completion {
                Some(_) => format!("PluginPaneless({plugin_id}, answers)"),
                None => format!("PluginPaneless({plugin_id})"),
            },
            PaneEffect::Screen(ScreenInstruction::PluginResolving {
                plugin_id,
                completion,
                ..
            }) => match completion {
                Some(_) => format!("PluginResolving({plugin_id}, answers)"),
                None => format!("PluginResolving({plugin_id})"),
            },
            PaneEffect::Screen(ScreenInstruction::PluginFailed(id)) => {
                format!("PluginFailed({id})")
            },
            PaneEffect::Screen(ScreenInstruction::PluginUnloaded(id)) => {
                format!("PluginUnloaded({id})")
            },
            PaneEffect::Screen(ScreenInstruction::GiveUpSlot(pane_id)) => {
                format!("GiveUpSlot({pane_id:?})")
            },
            PaneEffect::Screen(ScreenInstruction::PluginNotify { text, .. }) => {
                format!("Toast({text})")
            },
            PaneEffect::Screen(ScreenInstruction::UpdatePluginLoadingStage(id, _)) => {
                format!("UpdatePluginLoadingStage({id})")
            },
            PaneEffect::Screen(other) => format!("Screen({other:?})"),
            PaneEffect::Background(BackgroundJob::AnimatePluginLoading(id)) => {
                format!("Animate({id})")
            },
            PaneEffect::Background(BackgroundJob::StopPluginLoadingAnimation(id)) => {
                format!("StopAnimation({id})")
            },
            PaneEffect::Background(other) => format!("Background({other:?})"),
            PaneEffect::Unload(id) => format!("Unload({id})"),
        })
        .collect()
}

const P1: PluginId = PluginId::new(1);

#[test]
fn plugin_panes_declared_show_adds_the_requested_pane() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 10, request);
    assert_eq!(ledger.surface(P1), None);
    assert_eq!(ledger.presence(P1), None);
    let (prepared, effects) = ledger.declare(P1, 10, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=Some(true), in_place=false, suppressed=false, focus=Some(true), \
             completion=true)",
            "Animate(1)",
        ]
    );
    // The screen answers the completion once it places the pane.
    assert!(rx.try_recv().is_err());
    drop(effects);
    let result = answered(&mut rx);
    assert_eq!(result.error_message, None);
    assert_eq!(ledger.surface(P1), Some(Surface::Pane));
    assert_eq!(ledger.presence(P1), Some(true));
    assert_eq!(ledger.scanned(P1), Some(true));
    assert!(!ledger.background(P1));
}

#[test]
fn plugin_panes_undeclared_show_runs_headless_and_says_so() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 10, request);
    let (prepared, effects) = ledger.declare(P1, 10, false);
    assert_eq!(prepared, Prepared::Proceed(Surface::Headless));
    // The screen answers the launch once it has recorded the plugin as headless: a second
    // launch-or-focus must find it running headless.
    assert_eq!(kinds(&effects), ["PluginPaneless(1, answers)"]);
    assert!(rx.try_recv().is_err());
    drop(effects);
    let result = answered(&mut rx);
    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(P1)));
    assert_eq!(
        result.stdout_message,
        Some(format!(
            "{} runs headless: it declares no pane",
            location(1)
        ))
    );
    assert_eq!(result.error_message, None);
    assert_eq!(ledger.surface(P1), Some(Surface::Headless));
    assert_eq!(ledger.presence(P1), Some(false));
    assert_eq!(ledger.scanned(P1), Some(false));
}

#[test]
fn plugin_panes_undeclared_slot_child_closes_its_slot_and_unloads() {
    let slot = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(5));
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), Some(slot));
    ledger.request(P1, 10, request);
    let (prepared, effects) = ledger.declare(P1, 10, false);
    assert_eq!(prepared, Prepared::Abort);
    assert_eq!(
        kinds(&effects),
        [
            "PluginPaneless(1)".to_owned(),
            format!("GiveUpSlot({slot:?})"),
            format!(
                "Toast({} declares no pane; its pane was removed)",
                location(1)
            ),
            "Unload(1)".to_owned(),
        ]
    );
    drop(effects);
    assert_eq!(
        answered(&mut rx).error_message,
        Some(format!("{} declares no pane", location(1)))
    );
}

#[test]
fn plugin_panes_parked_requests_park_only_declared_plugins() {
    let mut ledger = PaneLedger::default();
    ledger.request(P1, 1, parked(1));
    assert!(ledger.background(P1));
    let (prepared, effects) = ledger.declare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=None, in_place=false, suppressed=true, focus=None, \
             completion=false)",
            "Animate(1)",
        ]
    );

    let p2 = PluginId::new(2);
    ledger.request(p2, 2, parked(2));
    let (prepared, effects) = ledger.declare(p2, 2, false);
    assert_eq!(prepared, Prepared::Proceed(Surface::Headless));
    assert_eq!(kinds(&effects), ["PluginPaneless(2)"]);
    assert!(ledger.background(p2));
}

#[test]
fn plugin_panes_placed_plugins_animate_their_slot_or_give_it_up_once_per_location() {
    let mut ledger = PaneLedger::default();
    ledger.request(P1, 1, placed(1));
    let (prepared, effects) = ledger.declare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(kinds(&effects), ["Animate(1)"]);

    // The same undeclared location placed into two tabs explains itself once.
    for (id, toast) in [(2, true), (3, false)] {
        let plugin_id = PluginId::new(id);
        ledger.request(plugin_id, 1, placed(9));
        let (prepared, effects) = ledger.declare(plugin_id, 1, false);
        assert_eq!(prepared, Prepared::Abort);
        let mut expected = vec![format!("PluginPaneless({id})")];
        if toast {
            expected.push(format!(
                "Toast({} declares no pane; its layout pane was removed)",
                location(9)
            ));
        }
        expected.push(format!("Unload({id})"));
        assert_eq!(kinds(&effects), expected);
    }
}

#[test]
fn plugin_panes_a_stale_load_changes_nothing() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, None, None);
    ledger.request(P1, 2, request);
    let (prepared, effects) = ledger.declare(P1, 1, false);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert!(effects.is_empty());
    assert!(ledger.fail(P1, 1, "late").is_empty());
    assert!(rx.try_recv().is_err());
    assert_eq!(ledger.surface(P1), None);
    // An unknown plugin is the loader's to cancel.
    assert_eq!(
        ledger.declare(PluginId::new(9), 1, true).0,
        Prepared::Proceed(Surface::Pane)
    );
}

#[test]
fn plugin_panes_failures_before_the_declaration_create_no_pane() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    let effects = ledger.fail(P1, 1, "no such file\nmore detail");
    assert_eq!(
        kinds(&effects),
        [format!(
            "Toast({} failed to load: no such file (yinz plugins log plugin_1))",
            location(1)
        )]
    );
    assert_eq!(
        answered(&mut rx).error_message.as_deref(),
        Some("no such file\nmore detail")
    );
    // The request is consumed: a declaration that arrives anyway does not revive it.
    assert_eq!(ledger.declare(P1, 1, true), (Prepared::Abort, vec![]));

    // Without a client there is nobody to toast.
    let p2 = PluginId::new(2);
    let (request, mut rx) = show_request(2, None, None);
    ledger.request(p2, 1, request);
    assert!(ledger.fail(p2, 1, "bad").is_empty());
    assert_eq!(answered(&mut rx).error_message.as_deref(), Some("bad"));

    let p3 = PluginId::new(3);
    ledger.request(p3, 1, placed(3));
    assert_eq!(
        kinds(&ledger.fail(p3, 1, "bad")),
        ["StopAnimation(3)", "UpdatePluginLoadingStage(3)"]
    );

    let p4 = PluginId::new(4);
    ledger.request(p4, 1, parked(4));
    assert!(ledger.fail(p4, 1, "bad").is_empty());
}

#[test]
fn plugin_panes_repeated_early_failures_update_the_preserved_slot() {
    let mut ledger = PaneLedger::default();
    ledger.request(P1, 1, placed(1));
    for (load_id, error) in [(1, "first syntax error"), (2, "second syntax error")] {
        if load_id > 1 {
            ledger.reload(P1, load_id);
            assert!(ledger.fail(P1, load_id - 1, "stale error").is_empty());
        }
        let effects = ledger.fail(P1, load_id, error);
        assert_eq!(
            kinds(&effects),
            ["StopAnimation(1)", "UpdatePluginLoadingStage(1)"]
        );
        assert!(effects.iter().any(|effect| matches!(effect,
            PaneEffect::Screen(ScreenInstruction::UpdatePluginLoadingStage(id, indication))
                if *id == P1 && indication.is_error() && indication.to_string().contains(error))));
        assert_eq!(ledger.declare(P1, load_id, true), (Prepared::Abort, vec![]));
    }

    ledger.reload(P1, 3);
    assert!(ledger.fail(P1, 2, "stale error").is_empty());
    let (prepared, effects) = ledger.redeclare(P1, 3, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(kinds(&effects), ["Animate(1)"]);
    assert_eq!(ledger.presence(P1), Some(true));
}

/// A git plugin's declaration waits for its repository. The screen hears at once that it is
/// loading, so a second launch finds it, and the launch is answered with its id at once rather
/// than after the clone. What it then turns out to be reaches the screen without a completion,
/// and a failure before that tells the screen it stopped.
#[test]
fn plugin_panes_a_plugin_resolving_its_repository_is_announced_and_answered_at_once() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    let effects = ledger.resolving(P1, 1);
    assert_eq!(kinds(&effects), ["PluginResolving(1, answers)"]);
    assert!(rx.try_recv().is_err(), "the screen answers it");
    drop(effects);
    let result = answered(&mut rx);
    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(P1)));
    assert_eq!(result.error_message, None);
    assert_eq!(ledger.surface(P1), None);
    let (prepared, effects) = ledger.declare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=Some(true), in_place=false, suppressed=false, focus=Some(true), \
             completion=false)",
            "Animate(1)",
        ]
    );

    let p2 = PluginId::new(2);
    let (request, _rx) = show_request(2, Some(CLIENT), None);
    ledger.request(p2, 1, request);
    ledger.resolving(p2, 1);
    assert_eq!(
        kinds(&ledger.declare(p2, 1, false).1),
        ["PluginPaneless(2)"]
    );

    let p3 = PluginId::new(3);
    let (request, _rx) = show_request(3, Some(CLIENT), None);
    ledger.request(p3, 1, request);
    ledger.resolving(p3, 1);
    assert_eq!(
        kinds(&ledger.fail(p3, 1, "bad")),
        [
            "PluginFailed(3)".to_owned(),
            format!(
                "Toast({} failed to load: bad (yinz plugins log plugin_3))",
                location(3)
            ),
        ]
    );

    // A background start is announced too; a stale load and a layout slot are not.
    let p4 = PluginId::new(4);
    ledger.request(p4, 1, parked(4));
    assert!(ledger.resolving(p4, 2).is_empty());
    assert_eq!(kinds(&ledger.resolving(p4, 1)), ["PluginResolving(4)"]);
    let p5 = PluginId::new(5);
    ledger.request(p5, 1, placed(5));
    assert!(ledger.resolving(p5, 1).is_empty());
}

/// Effects compare by their description: enough to tell which message goes where.
impl PartialEq for PaneEffect {
    fn eq(&self, other: &Self) -> bool {
        kinds(std::slice::from_ref(self)) == kinds(std::slice::from_ref(other))
    }
}

#[test]
fn plugin_panes_failures_after_the_declaration_go_where_the_plugin_is_seen() {
    let mut ledger = PaneLedger::default();
    ledger.request(P1, 1, parked(1));
    ledger.declare(P1, 1, true);
    assert_eq!(
        kinds(&ledger.failure_effects(P1, "boom", Failure::Load { load_id: Some(1) }, true)),
        ["StopAnimation(1)", "UpdatePluginLoadingStage(1)"]
    );
    // A crash paints over the running pane, which has no animation to stop.
    assert_eq!(
        kinds(&ledger.failure_effects(
            P1,
            "boom",
            Failure::Crash {
                generation: Some(1)
            },
            true,
        )),
        ["UpdatePluginLoadingStage(1)"]
    );

    let p2 = PluginId::new(2);
    let (request, _rx) = show_request(2, Some(CLIENT), None);
    ledger.request(p2, 1, request);
    ledger.declare(p2, 1, false);
    let toast = format!(
        "Toast({} failed to load: boom (yinz plugins log plugin_2))",
        location(2)
    );
    // It runs no more, so the screen stops counting it as running: a launch starts another.
    assert_eq!(
        kinds(&ledger.failure_effects(p2, "boom", Failure::Load { load_id: None }, true)),
        ["PluginFailed(2)".to_owned(), toast]
    );
    assert!(ledger
        .failure_effects(p2, "boom", Failure::Load { load_id: None }, false)
        .is_empty());
    // A crash of one client's instance, while another's runs on: toasted, still running.
    assert_eq!(
        kinds(&ledger.failure_effects(
            p2,
            "boom\nstack",
            Failure::Crash {
                generation: Some(3)
            },
            false,
        )),
        [format!(
            "Toast({} crashed: boom (yinz plugins log plugin_2))",
            location(2)
        )]
    );
    assert!(ledger
        .failure_effects(
            p2,
            "boom",
            Failure::Crash {
                generation: Some(3)
            },
            false,
        )
        .is_empty());

    // A background plugin is reported only by its status, but the screen still hears it stopped.
    let p3 = PluginId::new(3);
    ledger.request(p3, 1, parked(3));
    ledger.declare(p3, 1, false);
    assert_eq!(
        kinds(&ledger.failure_effects(p3, "boom", Failure::Load { load_id: None }, true)),
        ["PluginFailed(3)"]
    );
}

#[test]
fn plugin_panes_retire_answers_a_pending_request_and_tells_the_screen() {
    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    assert_eq!(kinds(&ledger.retire(P1, false)), ["PluginUnloaded(1)"]);
    assert_eq!(
        answered(&mut rx).error_message,
        Some("plugin 1 was unloaded before it loaded".to_owned())
    );
    assert_eq!(ledger.surface(P1), None);
    assert!(!ledger.background(P1));

    // At shutdown the screen keeps its panes for resurrection.
    let p2 = PluginId::new(2);
    ledger.request(p2, 1, parked(2));
    ledger.declare(p2, 1, true);
    assert!(ledger.retire(p2, true).is_empty());
    assert_eq!(ledger.surface(p2), None);
}

#[test]
fn plugin_panes_redeclare_moves_a_plugin_between_pane_and_headless() {
    let mut ledger = PaneLedger::default();
    let (request, _rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    ledger.declare(P1, 1, true);
    assert_eq!(
        ledger.redeclare(P1, 1, true),
        (Prepared::Proceed(Surface::Pane), vec![])
    );
    assert_eq!(
        ledger.redeclare(P1, 1, false),
        (
            Prepared::Proceed(Surface::Headless),
            vec![paneless(P1, &run(1), None)]
        )
    );
    assert_eq!(ledger.surface(P1), Some(Surface::Headless));
    // Still headless: the screen hears it again, as it runs again and so is not failed.
    assert_eq!(
        ledger.redeclare(P1, 1, false),
        (
            Prepared::Proceed(Surface::Headless),
            vec![paneless(P1, &run(1), None)]
        )
    );
    // Back to a pane: floating and unfocused, never the original placement or completion.
    let (prepared, effects) = ledger.redeclare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=Some(true), in_place=false, suppressed=false, \
             focus=Some(false), completion=false)",
            "Animate(1)",
        ]
    );

    let p2 = PluginId::new(2);
    ledger.request(p2, 1, parked(2));
    ledger.declare(p2, 1, false);
    assert_eq!(
        kinds(&ledger.redeclare(p2, 1, true).1),
        [
            "AddPlugin(2, float=None, in_place=false, suppressed=true, focus=None, \
             completion=false)",
            "Animate(2)",
        ]
    );

    let p3 = PluginId::new(3);
    ledger.request(p3, 1, placed(3));
    ledger.declare(p3, 1, true);
    let (prepared, effects) = ledger.redeclare(p3, 1, false);
    assert_eq!(prepared, Prepared::Abort);
    assert_eq!(
        kinds(&effects),
        [
            "PluginPaneless(3)".to_owned(),
            format!(
                "Toast({} declares no pane; its layout pane was removed)",
                location(3)
            ),
            "Unload(3)".to_owned(),
        ]
    );
}

#[test]
fn plugin_panes_a_reload_after_an_early_failure_starts_over() {
    // A request that failed before its declaration was known left no pane: the reload that
    // follows a fix decides afresh instead of aborting as the failed load did.
    let mut ledger = PaneLedger::default();
    let (request, _rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    ledger.fail(P1, 1, "declared more than once");
    let (prepared, effects) = ledger.redeclare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=Some(true), in_place=false, suppressed=false, \
             focus=Some(false), completion=false)",
            "Animate(1)",
        ]
    );
    assert_eq!(ledger.surface(P1), Some(Surface::Pane));

    let p2 = PluginId::new(2);
    ledger.request(p2, 1, parked(2));
    ledger.fail(p2, 1, "bad");
    assert_eq!(
        ledger.redeclare(p2, 1, false),
        (
            Prepared::Proceed(Surface::Headless),
            vec![paneless(p2, &run(2), None)]
        )
    );
    assert_eq!(ledger.surface(p2), Some(Surface::Headless));

    // A placed plugin's slot still shows the failure: declared, it only animates again.
    let p3 = PluginId::new(3);
    ledger.request(p3, 1, placed(3));
    ledger.fail(p3, 1, "bad");
    assert_eq!(
        ledger.redeclare(p3, 1, true),
        (
            Prepared::Proceed(Surface::Pane),
            vec![PaneEffect::Background(BackgroundJob::AnimatePluginLoading(
                p3
            ))]
        )
    );
    let p4 = PluginId::new(4);
    ledger.request(p4, 1, placed(4));
    ledger.fail(p4, 1, "bad");
    let (prepared, effects) = ledger.redeclare(p4, 1, false);
    assert_eq!(prepared, Prepared::Abort);
    assert_eq!(
        kinds(&effects),
        [
            "PluginPaneless(4)".to_owned(),
            format!(
                "Toast({} declares no pane; its layout pane was removed)",
                location(4)
            ),
            "Unload(4)".to_owned(),
        ]
    );
}

#[test]
fn plugin_panes_only_the_latest_reload_decides_the_pane() {
    // Two reloads of a git plugin prepare on separate repository threads, and the source changed
    // between them: the later one finds no declaration, and finishes first.
    let mut ledger = PaneLedger::default();
    let (request, _rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    ledger.declare(P1, 1, true);
    ledger.reload(P1, 2);
    ledger.reload(P1, 3);
    assert_eq!(
        ledger.redeclare(P1, 3, false),
        (
            Prepared::Proceed(Surface::Headless),
            vec![paneless(P1, &run(1), None)]
        )
    );
    // The superseded reload's declaration arrives late, and changes nothing: the load that runs
    // is checked against the declaration it was decided by.
    assert_eq!(
        ledger.redeclare(P1, 2, true),
        (Prepared::Proceed(Surface::Headless), vec![])
    );
    assert_eq!(ledger.surface(P1), Some(Surface::Headless));
    assert_eq!(ledger.scanned(P1), Some(false));
    // Nor does a failure of the superseded load.
    assert!(ledger.fail(P1, 2, "late").is_empty());

    // A reload of a plugin whose first load is still preparing supersedes that load too.
    let p2 = PluginId::new(2);
    let (request, mut rx) = show_request(2, Some(CLIENT), None);
    ledger.request(p2, 4, request);
    ledger.reload(p2, 5);
    assert_eq!(
        ledger.declare(p2, 4, true),
        (Prepared::Proceed(Surface::Pane), vec![])
    );
    assert_eq!(ledger.surface(p2), None);
    assert!(rx.try_recv().is_err());
    let (prepared, effects) = ledger.redeclare(p2, 5, false);
    assert_eq!(prepared, Prepared::Proceed(Surface::Headless));
    assert_eq!(kinds(&effects), ["PluginPaneless(2, answers)"]);
    drop(effects);
    assert_eq!(answered(&mut rx).affected_pane_id, Some(PaneId::Plugin(p2)));
}

#[test]
fn plugin_panes_a_crash_never_silences_a_load_failure_with_the_same_number() {
    // Load ids and generations are separate counters: a crash of generation 5 and a failure of
    // load 5 are two failures, and each is toasted.
    let mut ledger = PaneLedger::default();
    let (request, _rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    ledger.declare(P1, 1, false);
    let crash = Failure::Crash {
        generation: Some(5),
    };
    let load = Failure::Load { load_id: Some(5) };
    assert_eq!(
        kinds(&ledger.failure_effects(P1, "boom", crash, false)),
        [format!(
            "Toast({} crashed: boom (yinz plugins log plugin_1))",
            location(1)
        )]
    );
    assert_eq!(
        kinds(&ledger.failure_effects(P1, "boom", load, false)),
        [format!(
            "Toast({} failed to load: boom (yinz plugins log plugin_1))",
            location(1)
        )]
    );
    assert!(ledger.failure_effects(P1, "boom", load, false).is_empty());
    assert_eq!(
        kinds(&ledger.failure_effects(P1, "boom", crash, false)).len(),
        1
    );
}

#[test]
fn plugin_panes_a_reload_after_an_early_failure_opens_the_pane_where_it_was_asked_for() {
    // Nothing happened in the placement asked for, so the recovered pane goes there - unfocused,
    // since the reload, not the user, brings it up.
    let mut ledger = PaneLedger::default();
    let mut tiled = show(1, Some(CLIENT), None);
    tiled.should_float = Some(false);
    tiled.tab = Some(TabId::new(3));
    let (end, mut rx) = completion();
    tiled.completion = Some(end);
    ledger.request(P1, 1, PaneRequest::Show(Box::new(tiled)));
    ledger.fail(P1, 1, "no such file");
    assert!(answered(&mut rx).error_message.is_some());
    let (prepared, effects) = ledger.redeclare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    let Some(PaneEffect::Screen(ScreenInstruction::AddPlugin(
        should_float,
        in_place,
        _,
        _,
        title,
        tab,
        ..,
        should_focus,
        _,
        completion,
        _,
    ))) = effects.first()
    else {
        panic!("expected AddPlugin, got {:?}", kinds(&effects));
    };
    assert_eq!(*should_float, Some(false));
    assert!(!*in_place);
    assert_eq!(title.as_deref(), Some("title 1"));
    assert_eq!(*tab, Some(TabId::new(3)));
    assert_eq!(*should_focus, Some(false));
    assert!(completion.is_none());

    // One in place of another pane is not replayed: that pane may be anything by now.
    let p2 = PluginId::new(2);
    let mut in_place = show(2, Some(CLIENT), None);
    in_place.in_place = true;
    in_place.should_float = Some(false);
    in_place.pane_id_to_replace = Some(PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(4),
    ));
    ledger.request(p2, 1, PaneRequest::Show(Box::new(in_place)));
    ledger.fail(p2, 1, "no such file");
    assert_eq!(
        kinds(&ledger.redeclare(p2, 1, true).1),
        [
            "AddPlugin(2, float=Some(true), in_place=false, suppressed=false, \
             focus=Some(false), completion=false)",
            "Animate(2)",
        ]
    );
}

#[test]
fn plugin_panes_errored_launches_exit_with_status_one() {
    let (request, mut rx) = show_request(1, None, None);
    request.reject("Plugins must have a client id");
    let result = answered(&mut rx);
    assert_eq!(result.exit_status, Some(1));
    assert_eq!(
        result.error_message.as_deref(),
        Some("Plugins must have a client id")
    );

    let mut ledger = PaneLedger::default();
    let (request, mut rx) = show_request(1, Some(CLIENT), None);
    ledger.request(P1, 1, request);
    drop(ledger.fail(P1, 1, "bad"));
    assert_eq!(answered(&mut rx).exit_status, Some(1));

    let p2 = PluginId::new(2);
    let (request, mut rx) = show_request(2, Some(CLIENT), None);
    ledger.request(p2, 1, request);
    drop(ledger.retire(p2, false));
    assert_eq!(answered(&mut rx).exit_status, Some(1));
}

#[test]
fn plugin_panes_hook_payload_matches_the_replace_pane_hook() {
    let pane = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4));
    let mut ledger = PaneLedger::default();
    let show = ShowPane::in_place_of_hooked_pane(
        run(1),
        pane,
        Some("t".to_owned()),
        Some(PathBuf::from("/cwd")),
        CLIENT,
    );
    ledger.request(P1, 1, PaneRequest::Show(Box::new(show)));
    let (_, effects) = ledger.declare(P1, 1, true);
    let Some(PaneEffect::Screen(ScreenInstruction::AddPlugin(
        should_float,
        in_place,
        replaced,
        _,
        title,
        tab,
        id,
        pane_id_to_replace,
        cwd,
        start_suppressed,
        coordinates,
        should_focus,
        client_id,
        completion,
        overlay,
    ))) = effects.first()
    else {
        panic!("expected AddPlugin, got {:?}", kinds(&effects));
    };
    assert_eq!(*should_float, Some(false));
    assert!(*in_place);
    assert_eq!(*replaced, ReplacedPane::Suppress);
    assert_eq!(title.as_deref(), Some("t"));
    assert_eq!(*tab, None);
    assert_eq!(*id, P1);
    assert_eq!(*pane_id_to_replace, Some(pane));
    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/cwd")));
    assert!(!*start_suppressed);
    assert_eq!(*coordinates, None);
    assert_eq!(*should_focus, Some(false));
    assert_eq!(*client_id, Some(CLIENT));
    assert!(completion.is_none());
    assert_eq!(*overlay, None);
}

#[test]
fn plugin_panes_slot_children_float_unfocused_in_place_of_their_slot() {
    let slot = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4));
    let mut ledger = PaneLedger::default();
    let show = ShowPane::into_slot(run(1), slot, CLIENT, PathBuf::from("/cwd"));
    assert_eq!(show.reserved_slot, Some(slot));
    assert_eq!(show.pane_id_to_replace, Some(slot));
    assert_eq!(show.replaced, ReplacedPane::Close);
    ledger.request(P1, 1, PaneRequest::Show(Box::new(show)));
    let (prepared, effects) = ledger.declare(P1, 1, true);
    assert_eq!(prepared, Prepared::Proceed(Surface::Pane));
    assert_eq!(
        kinds(&effects),
        [
            "AddPlugin(1, float=Some(true), in_place=true, suppressed=false, focus=Some(false), \
             completion=false)",
            "Animate(1)",
        ]
    );

    // A slot child whose load fails, or that is unloaded before it loads, gives the slot back.
    let p2 = PluginId::new(2);
    ledger.request(
        p2,
        1,
        PaneRequest::Show(Box::new(ShowPane::into_slot(
            run(2),
            slot,
            CLIENT,
            PathBuf::from("/cwd"),
        ))),
    );
    assert_eq!(
        kinds(&ledger.fail(p2, 1, "bad")),
        [
            format!("GiveUpSlot({slot:?})"),
            format!(
                "Toast({} failed to load: bad (yinz plugins log plugin_2))",
                location(2)
            ),
        ]
    );
    let p3 = PluginId::new(3);
    ledger.request(
        p3,
        1,
        PaneRequest::Show(Box::new(ShowPane::into_slot(
            run(3),
            slot,
            CLIENT,
            PathBuf::from("/cwd"),
        ))),
    );
    assert_eq!(
        kinds(&ledger.retire(p3, false)),
        [
            "PluginUnloaded(3)".to_owned(),
            format!("GiveUpSlot({slot:?})")
        ]
    );
}

#[test]
fn plugin_panes_effects_are_sent_in_order_to_their_threads() {
    let (to_screen, screen_rx) = yinz_runtime::channels::unbounded();
    let (to_plugin, plugin_rx) = yinz_runtime::channels::unbounded();
    let (to_background, background_rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(to_screen)),
        to_plugin: Some(yinz_runtime::channels::SenderWithContext::new(to_plugin)),
        to_background_jobs: Some(yinz_runtime::channels::SenderWithContext::new(
            to_background,
        )),
        ..Default::default()
    };
    let panes: PluginPanes = Arc::new(Mutex::new(PaneLedger::default()));
    {
        let mut ledger = panes.lock().expect("ledger lock");
        ledger.request(P1, 1, placed(1));
        let effects = ledger.declare(P1, 1, false).1;
        send_effects(&senders, effects);
    }
    let screen: Vec<_> = screen_rx.try_iter().map(|(i, _)| i).collect();
    assert!(
        matches!(
            screen.as_slice(),
            [
                ScreenInstruction::PluginPaneless { plugin_id: P1, .. },
                ScreenInstruction::PluginNotify { plugin_id: P1, .. },
            ]
        ),
        "{screen:?}"
    );
    let plugin: Vec<_> = plugin_rx.try_iter().map(|(i, _)| i).collect();
    assert!(
        matches!(plugin.as_slice(), [PluginInstruction::Unload(P1)]),
        "{plugin:?}"
    );

    {
        let mut ledger = panes.lock().expect("ledger lock");
        ledger.request(PluginId::new(2), 2, placed(2));
        let effects = ledger.declare(PluginId::new(2), 2, true).1;
        send_effects(&senders, effects);
    }
    let background: Vec<_> = background_rx.try_iter().map(|(i, _)| i).collect();
    assert_eq!(
        background,
        [BackgroundJob::AnimatePluginLoading(PluginId::new(2))]
    );
}

// Property tests over random sequences of ledger calls.

/// What the screen holds for each plugin, driven only by the effects the ledger returns.
#[derive(Default)]
struct ScreenModel {
    pane_records: HashSet<PluginId>,
}

impl ScreenModel {
    fn apply(&mut self, effects: &[PaneEffect]) {
        for effect in effects {
            match effect {
                PaneEffect::Screen(ScreenInstruction::AddPlugin(_, _, _, _, _, _, id, ..)) => {
                    self.pane_records.insert(*id);
                },
                PaneEffect::Screen(ScreenInstruction::PluginPaneless { plugin_id, .. }) => {
                    self.pane_records.remove(plugin_id);
                },
                PaneEffect::Screen(ScreenInstruction::PluginUnloaded(id)) => {
                    self.pane_records.remove(id);
                },
                _ => {},
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Show { client: bool, slot: bool },
    Parked,
    Placed,
}

#[derive(Debug)]
enum Op {
    Request(Kind),
    Declare {
        pick: usize,
        stale: bool,
        scanned: bool,
    },
    /// A reload begins: the plugin gets a new latest load.
    Reload {
        pick: usize,
    },
    Redeclare {
        pick: usize,
        stale: bool,
        scanned: bool,
    },
    Fail {
        pick: usize,
        stale: bool,
    },
    FailureEffects {
        pick: usize,
        generation: Option<u8>,
        crash: bool,
        stopped: bool,
    },
    /// The first load waits for its repository.
    Resolving {
        pick: usize,
        stale: bool,
    },
    Retire {
        pick: usize,
        shutting_down: bool,
    },
}

fn draw_op(tc: &hegel::TestCase) -> Op {
    let pick = || tc.draw(generators::integers::<usize>().max_value(7));
    match tc.draw(generators::integers::<u8>().max_value(7)) {
        0 => Op::Request(match tc.draw(generators::integers::<u8>().max_value(2)) {
            0 => Kind::Show {
                client: tc.draw(generators::booleans()),
                slot: tc.draw(generators::booleans()),
            },
            1 => Kind::Parked,
            _ => Kind::Placed,
        }),
        1 => Op::Declare {
            pick: pick(),
            stale: tc.draw(generators::booleans()),
            scanned: tc.draw(generators::booleans()),
        },
        2 => Op::Redeclare {
            pick: pick(),
            stale: tc.draw(generators::booleans()),
            scanned: tc.draw(generators::booleans()),
        },
        6 => Op::Reload { pick: pick() },
        3 => Op::Fail {
            pick: pick(),
            stale: tc.draw(generators::booleans()),
        },
        4 => Op::FailureEffects {
            pick: pick(),
            generation: tc.draw(generators::optional(
                generators::integers::<u8>().max_value(2),
            )),
            crash: tc.draw(generators::booleans()),
            stopped: tc.draw(generators::booleans()),
        },
        7 => Op::Resolving {
            pick: pick(),
            stale: tc.draw(generators::booleans()),
        },
        _ => Op::Retire {
            pick: pick(),
            shutting_down: tc.draw(generators::booleans()),
        },
    }
}

struct Run {
    ledger: PaneLedger,
    screen: ScreenModel,
    /// Every plugin id requested so far (ids are never reused, as in the bridge), with its load.
    plugins: Vec<(PluginId, u64)>,
    completions: BTreeMap<PluginId, oneshot::Receiver<ActionCompletionResult>>,
    next_load_id: u64,
}

impl Run {
    fn new() -> Self {
        Self {
            ledger: PaneLedger::default(),
            screen: ScreenModel::default(),
            plugins: vec![],
            completions: BTreeMap::new(),
            next_load_id: 1,
        }
    }

    fn picked(&self, pick: usize) -> Option<(PluginId, u64)> {
        (!self.plugins.is_empty()).then(|| self.plugins[pick % self.plugins.len()])
    }

    /// Run one call against the ledger and return its effects (still undelivered) together with
    /// the plugin it was about and the declaration it carried, if any.
    fn step(&mut self, op: &Op) -> Option<(PluginId, Option<bool>, Vec<PaneEffect>)> {
        match *op {
            Op::Request(kind) => {
                let plugin_id = PluginId::new(self.plugins.len() as u32 + 1);
                let load_id = self.next_load_id;
                self.next_load_id += 1;
                let n = plugin_id.get() % 3;
                let request = match kind {
                    Kind::Show { client, slot } => {
                        let slot = slot.then_some(PaneId::Terminal(
                            yinz_types::internal_ids::TerminalId::new(100 + plugin_id.get()),
                        ));
                        let (request, rx) = show_request(n, client.then_some(CLIENT), slot);
                        self.completions.insert(plugin_id, rx);
                        request
                    },
                    Kind::Parked => parked(n),
                    Kind::Placed => {
                        // The screen already holds the slot a placed plugin loads into.
                        self.screen.pane_records.insert(plugin_id);
                        placed(n)
                    },
                };
                self.ledger.request(plugin_id, load_id, request);
                self.plugins.push((plugin_id, load_id));
                Some((plugin_id, None, vec![]))
            },
            Op::Declare {
                pick,
                stale,
                scanned,
            } => {
                let (plugin_id, load_id) = self.picked(pick)?;
                let load_id = if stale { load_id + 1000 } else { load_id };
                let (prepared, effects) = self.ledger.declare(plugin_id, load_id, scanned);
                // An abort either unloads the plugin, or follows a failure that already answered
                // the load and so needs nothing more.
                if prepared == Prepared::Abort {
                    let unloads = effects
                        .iter()
                        .any(|e| matches!(e, PaneEffect::Unload(id) if *id == plugin_id));
                    assert_eq!(unloads, !effects.is_empty(), "{effects:?}");
                    assert!(!scanned || effects.is_empty(), "{effects:?}");
                }
                Some((plugin_id, Some(scanned), effects))
            },
            Op::Reload { pick } => {
                let (plugin_id, _) = self.picked(pick)?;
                let load_id = self.next_load_id;
                self.next_load_id += 1;
                self.ledger.reload(plugin_id, load_id);
                for (id, load) in &mut self.plugins {
                    if *id == plugin_id {
                        *load = load_id;
                    }
                }
                Some((plugin_id, None, vec![]))
            },
            Op::Redeclare {
                pick,
                stale,
                scanned,
            } => {
                let (plugin_id, load_id) = self.picked(pick)?;
                if stale {
                    // A superseded reload decides nothing, whatever it read.
                    let surface = self.ledger.surface(plugin_id);
                    let scanned_before = self.ledger.scanned(plugin_id);
                    let (prepared, effects) =
                        self.ledger.redeclare(plugin_id, load_id + 1000, scanned);
                    assert!(effects.is_empty(), "{effects:?}");
                    assert_ne!(prepared, Prepared::Abort);
                    assert_eq!(self.ledger.surface(plugin_id), surface);
                    assert_eq!(self.ledger.scanned(plugin_id), scanned_before);
                    return Some((plugin_id, Some(scanned), effects));
                }
                let (prepared, effects) = self.ledger.redeclare(plugin_id, load_id, scanned);
                // A reload is a fresh attempt: it aborts only to unload a plugin that has no
                // place, never silently, which would leave the reload's loading state behind.
                if prepared == Prepared::Abort {
                    assert!(
                        effects
                            .iter()
                            .any(|e| matches!(e, PaneEffect::Unload(id) if *id == plugin_id)),
                        "redeclare aborted without unloading: {effects:?}"
                    );
                }
                Some((plugin_id, Some(scanned), effects))
            },
            Op::Fail { pick, stale } => {
                let (plugin_id, load_id) = self.picked(pick)?;
                let load_id = if stale { load_id + 1000 } else { load_id };
                let effects = self.ledger.fail(plugin_id, load_id, "failed");
                Some((plugin_id, None, effects))
            },
            Op::Resolving { pick, stale } => {
                let (plugin_id, load_id) = self.picked(pick)?;
                let load_id = if stale { load_id + 1000 } else { load_id };
                let effects = self.ledger.resolving(plugin_id, load_id);
                Some((plugin_id, None, effects))
            },
            Op::FailureEffects {
                pick,
                generation,
                crash,
                stopped,
            } => {
                let (plugin_id, _) = self.picked(pick)?;
                let attempt = generation.map(u64::from);
                let failure = if crash {
                    Failure::Crash {
                        generation: attempt,
                    }
                } else {
                    Failure::Load { load_id: attempt }
                };
                let effects = self
                    .ledger
                    .failure_effects(plugin_id, "failed", failure, stopped);
                Some((plugin_id, None, effects))
            },
            Op::Retire {
                pick,
                shutting_down,
            } => {
                let (plugin_id, _) = self.picked(pick)?;
                let effects = self.ledger.retire(plugin_id, shutting_down);
                let unloaded = effects.iter().any(
                    |e| matches!(e, PaneEffect::Screen(ScreenInstruction::PluginUnloaded(id)) if *id == plugin_id),
                );
                // I3: an unload always tells the screen, except at shutdown.
                assert_eq!(unloaded, !shutting_down, "retire(_, {shutting_down})");
                if shutting_down {
                    // The screen keeps what it holds to serialize it; the plugin is gone.
                    self.screen.pane_records.remove(&plugin_id);
                }
                Some((plugin_id, None, effects))
            },
        }
    }

    fn is_owned(&self, plugin_id: PluginId) -> bool {
        self.ledger
            .entries
            .get(&plugin_id)
            .is_some_and(|entry| matches!(entry.state, PaneState::Owned { .. }))
    }

    fn is_headless(&self, plugin_id: PluginId) -> bool {
        self.ledger
            .entries
            .get(&plugin_id)
            .is_some_and(|entry| matches!(entry.state, PaneState::Headless { .. }))
    }
}

fn draw_ops(tc: &hegel::TestCase) -> Vec<Op> {
    let count = tc.draw(generators::integers::<usize>().max_value(60));
    (0..count).map(|_| draw_op(tc)).collect()
}

/// I1: an `AddPlugin` for a plugin comes only from declaring it with a pane.
#[hegel::test(test_cases = 300)]
fn plugin_panes_add_plugin_comes_only_from_a_declared_pane(tc: hegel::TestCase) {
    let mut run = Run::new();
    for op in draw_ops(&tc) {
        let Some((plugin_id, declared, effects)) = run.step(&op) else {
            continue;
        };
        for effect in &effects {
            if let PaneEffect::Screen(ScreenInstruction::AddPlugin(_, _, _, _, _, _, id, ..)) =
                effect
            {
                assert_eq!(*id, plugin_id, "{op:?} added another plugin's pane");
                assert!(
                    matches!(op, Op::Declare { .. } | Op::Redeclare { .. }),
                    "{op:?} added a pane"
                );
                assert_eq!(
                    declared,
                    Some(true),
                    "{op:?} added an undeclared plugin's pane"
                );
                assert!(
                    run.is_owned(plugin_id),
                    "{op:?} added a pane it does not own"
                );
            }
        }
    }
}

/// Whether the plugin's `Show` request still holds its completion: its declaration is unknown,
/// and the screen has not been told it is resolving its repository.
fn awaits_its_answer(ledger: &PaneLedger, plugin_id: PluginId) -> bool {
    ledger.entries.get(&plugin_id).is_some_and(|entry| {
        matches!(
            &entry.state,
            PaneState::Pending {
                request: Some(PaneRequest::Show(show)),
                ..
            } if show.completion.is_some()
        )
    })
}

/// I2: every `Show` request's completion is answered exactly once, and only when the request is
/// resolved: declared, failed, retired, or announced as resolving its repository.
#[hegel::test(test_cases = 300)]
fn plugin_panes_every_show_completion_is_answered_once(tc: hegel::TestCase) {
    let mut run = Run::new();
    for op in draw_ops(&tc) {
        let pending_before: HashSet<PluginId> = run
            .plugins
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| awaits_its_answer(&run.ledger, *id))
            .collect();
        // Effects are delivered (dropped) at once, as the screen would answer them.
        let _ = run.step(&op);
        for (plugin_id, rx) in run.completions.iter_mut() {
            let still_pending = awaits_its_answer(&run.ledger, *plugin_id);
            if still_pending {
                assert!(
                    matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
                    "{op:?} answered the pending request of {plugin_id}"
                );
            } else if pending_before.contains(plugin_id) {
                let result = rx.try_recv().unwrap_or_else(|e| {
                    panic!("{op:?} resolved {plugin_id} without answering it: {e:?}")
                });
                let headless = run.ledger.surface(*plugin_id) == Some(Surface::Headless);
                let resolving = matches!(op, Op::Resolving { .. });
                if (headless || resolving) && result.error_message.is_none() {
                    assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(*plugin_id)));
                }
            }
        }
    }
    // Whatever is still pending is answered when its plugin is unloaded.
    let ids: Vec<PluginId> = run.plugins.iter().map(|(id, _)| *id).collect();
    for plugin_id in ids {
        drop(run.ledger.retire(plugin_id, false));
    }
    for (plugin_id, rx) in run.completions.iter_mut() {
        match rx.try_recv() {
            // Answered here, or earlier and already taken above.
            Ok(_) | Err(oneshot::error::TryRecvError::Closed) => {},
            Err(oneshot::error::TryRecvError::Empty) => {
                panic!("the completion of {plugin_id} was never answered")
            },
        }
    }
}

/// I3 is asserted inside every `Retire` step; this drives enough of them.
#[hegel::test(test_cases = 200)]
fn plugin_panes_retire_always_tells_the_screen_unless_shutting_down(tc: hegel::TestCase) {
    let mut run = Run::new();
    for op in draw_ops(&tc) {
        let _ = run.step(&op);
    }
    let shutting_down = tc.draw(generators::booleans());
    let ids: Vec<PluginId> = run.plugins.iter().map(|(id, _)| *id).collect();
    for plugin_id in ids {
        let effects = run.ledger.retire(plugin_id, shutting_down);
        let unloaded = effects.iter().any(|e| {
            matches!(e, PaneEffect::Screen(ScreenInstruction::PluginUnloaded(id)) if *id == plugin_id)
        });
        assert_eq!(unloaded, !shutting_down);
        assert_eq!(run.ledger.surface(plugin_id), None);
    }
}

/// I4, and the invariant behind the whole ledger: a plugin's surface is `Pane` exactly when it
/// owns a pane, the screen holds a pane record for every plugin that owns one, and never one for
/// a plugin that runs headless.
#[hegel::test(test_cases = 300)]
fn plugin_panes_surface_matches_ownership_and_the_screen(tc: hegel::TestCase) {
    let mut run = Run::new();
    for op in draw_ops(&tc) {
        if let Some((_, _, effects)) = run.step(&op) {
            run.screen.apply(&effects);
        }
        for (plugin_id, _) in &run.plugins {
            let owned = run.is_owned(*plugin_id);
            assert_eq!(
                run.ledger.surface(*plugin_id) == Some(Surface::Pane),
                owned,
                "after {op:?}: surface of {plugin_id}"
            );
            assert_eq!(
                run.ledger.presence(*plugin_id) == Some(true),
                owned,
                "after {op:?}: presence of {plugin_id}"
            );
            if owned {
                assert!(
                    run.screen.pane_records.contains(plugin_id),
                    "after {op:?}: {plugin_id} owns a pane the screen does not hold"
                );
            }
            if run.is_headless(*plugin_id) {
                assert!(
                    !run.screen.pane_records.contains(plugin_id),
                    "after {op:?}: headless {plugin_id} has a pane record"
                );
            }
        }
    }
}

#[test]
fn plugin_panes_notifications_name_the_cause_of_a_failure() {
    assert_eq!(summary("no such file"), "no such file");
    assert_eq!(
        summary("failed to start\n\nCaused by:\n    0: reading the source\n    1: no such file"),
        "no such file"
    );
    assert_eq!(
        summary("failed to start\n\nCaused by:\n    (yinz-declare-pane) must appear"),
        "(yinz-declare-pane) must appear"
    );
    // A crash report, as painted into a pane, around a Steel diagnostic.
    assert_eq!(
        summary(
            "Failed to apply event: note: \n\r   ┌─ :51:8\n\r51 │ (%async-native-command\n\r   │  \
             ^^^^^^^^ yinz command failed: 'HideSelf' needs the plugin's own pane\n\r\n\r"
        ),
        "yinz command failed: 'HideSelf' needs the plugin's own pane"
    );
    // A `^` in the text of a message is not a diagnostic's carets.
    assert_eq!(
        summary(
            "failed to start\n\nCaused by:\n    0: compiling a pattern\n    1: invalid regex ^(a"
        ),
        "invalid regex ^(a"
    );
}
