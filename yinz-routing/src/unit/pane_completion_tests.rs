use super::*;
use crossbeam::channel::{unbounded, RecvTimeoutError};
use yinz_runtime::channels::SenderWithContext;
use yinz_types::internal_ids::TerminalId;

#[test]
fn start_or_reload_plugin_preserves_the_requesting_client() {
    let client = ClientId::new(2);
    let (screen_tx, screen_rx) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(screen_tx)),
        should_silently_fail: true,
        ..Default::default()
    };
    let screen = std::thread::spawn(move || {
        let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let ScreenInstruction::StartOrReloadPluginPane(_, _, requested_client, Some(completion)) =
            instruction
        else {
            panic!("expected a start-or-reload request");
        };
        assert_eq!(requested_client, client);
        drop(completion);
    });
    let (_, result) = route_action(
        Action::StartOrReloadPlugin {
            plugin: Default::default(),
        },
        client,
        None,
        None,
        senders,
        None,
        None,
        InputMode::Normal,
        None,
        false,
        false,
    )
    .unwrap();
    screen.join().unwrap();
    assert!(result.unwrap().error_message.is_none());
}

fn creation_actions() -> Vec<Action> {
    let mut actions = vec![
        Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
        },
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        },
        Action::NewFloatingPane {
            command: None,
            pane_name: None,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        Action::NewInPlacePane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane: false,
            tab_id: None,
        },
        Action::NewStackedPane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        Action::NewTiledPluginPane {
            plugin: Default::default(),
            pane_name: None,
            skip_cache: false,
            cwd: None,
            no_focus: false,
            tab_id: None,
        },
        Action::NewFloatingPluginPane {
            plugin: Default::default(),
            pane_name: None,
            skip_cache: false,
            cwd: None,
            placement: NewPanePlacement::Floating(None),
            no_focus: false,
            tab_id: None,
        },
        Action::NewInPlacePluginPane {
            plugin: Default::default(),
            pane_name: None,
            skip_cache: false,
            close_replaced_pane: false,
            no_focus: false,
            tab_id: None,
        },
    ];
    for (floating, in_place) in [(false, false), (true, false), (false, true)] {
        actions.push(Action::EditFile {
            payload: Default::default(),
            direction: None,
            floating,
            in_place,
            close_replaced_pane: false,
            start_suppressed: false,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        });
    }
    actions
}

fn create_pane(
    action: Action,
    pane_id: Option<PaneId>,
    senders: ThreadSenders,
) -> ActionCompletionResult {
    route_action(
        action,
        ClientId::new(1),
        None,
        pane_id,
        senders,
        None,
        None,
        InputMode::Normal,
        None,
        false,
        false,
    )
    .unwrap()
    .1
    .unwrap()
}

#[test]
fn pane_creation_waits_for_placement_past_the_action_timeout() {
    let pane_id = PaneId::Terminal(TerminalId::new(42));
    let mut routes = vec![];
    for action in creation_actions() {
        let name = action.to_string();
        let (pty_tx, pty_rx) = unbounded();
        let (screen_tx, screen_rx) = unbounded();
        let (result_tx, result_rx) = unbounded();
        let route = std::thread::spawn(move || {
            let result = create_pane(
                action,
                Some(pane_id),
                ThreadSenders {
                    to_pty: Some(SenderWithContext::new(pty_tx)),
                    to_screen: Some(SenderWithContext::new(screen_tx)),
                    should_silently_fail: true,
                    ..Default::default()
                },
            );
            result_tx.send(result).unwrap();
        });
        let completion = crossbeam::select! {
            recv(pty_rx) -> instruction => match instruction.unwrap().0 {
                PtyInstruction::SpawnTerminal(_, _, _, _, _, Some(completion), blocking) => {
                    assert!(!blocking, "ordinary creation must not wait for command exit");
                    completion
                },
                PtyInstruction::SpawnInPlaceTerminal(_, _, _, _, Some(completion)) => completion,
                _ => panic!("unexpected pty instruction"),
            },
            recv(screen_rx) -> instruction => match instruction.unwrap().0 {
                ScreenInstruction::NewTiledPluginPane(_, _, _, _, _, Some(completion), _, _)
                | ScreenInstruction::NewFloatingPluginPane(_, _, _, _, _, _, Some(completion), _, _)
                | ScreenInstruction::NewInPlacePluginPane(_, _, _, _, _, _, Some(completion), _, _) => completion,
                _ => panic!("unexpected screen instruction"),
            },
            default(Duration::from_secs(10)) => panic!("{name} was not dispatched"),
        };
        routes.push((name, completion, result_rx, route));
    }
    // Keep every completion alive beyond the old deadline, without a sleep.
    assert!(matches!(
        routes[0]
            .2
            .recv_timeout(ACTION_COMPLETION_TIMEOUT + Duration::from_millis(500)),
        Err(RecvTimeoutError::Timeout)
    ));
    for (index, (name, mut completion, result_rx, route)) in routes.into_iter().enumerate() {
        assert!(
            result_rx.try_recv().is_err(),
            "{name} returned before placement"
        );
        if index % 2 == 0 {
            completion.set_affected_pane_id(pane_id);
        } else {
            completion.set_error_message("placement failed".into());
        }
        drop(completion);
        let result = result_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        if index % 2 == 0 {
            assert_eq!(result.affected_pane_id, Some(pane_id), "{name}");
            assert_eq!(result.error_message, None, "{name}");
        } else {
            assert_eq!(
                result.error_message.as_deref(),
                Some("placement failed"),
                "{name}"
            );
        }
        route.join().unwrap();
    }
}

#[test]
fn pane_creation_rejects_empty_and_cancelled_completion() {
    for action in creation_actions() {
        let name = action.to_string();
        // Dropped instructions produce empty completion; an in-place plugin without an
        // originating pane cancels its channel without constructing a NotificationEnd.
        let result = create_pane(
            action,
            None,
            ThreadSenders {
                should_silently_fail: true,
                ..Default::default()
            },
        );
        assert!(
            result
                .error_message
                .as_deref()
                .is_some_and(|error| error.contains("without a pane ID")),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn palette_completion_requires_an_acknowledgement() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    drop(sender);
    assert!(!wait_for_action_completion(receiver, "test", false).confirmed);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    drop(NotificationEnd::new(sender));
    assert!(wait_for_action_completion(receiver, "test", false).confirmed);
}
