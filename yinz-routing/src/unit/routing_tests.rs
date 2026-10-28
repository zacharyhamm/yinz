use super::*;
use yinz_runtime::channels::SenderWithContext;

#[test]
fn actions_without_completion_owners_do_not_wait_for_timeout() {
    for action in [
        Action::Confirm,
        Action::Deny,
        Action::SkipConfirm {
            action: Box::new(Action::NoOp),
        },
        Action::SkipConfirm {
            action: Box::new(Action::Quit),
        },
        Action::ToggleMouseMode,
        Action::ArrangePanesByProject,
        Action::NoOp,
    ] {
        let (plugin_tx, plugin_rx) = yinz_runtime::channels::unbounded();
        let (server_tx, server_rx) = yinz_runtime::channels::unbounded();
        let started = std::time::Instant::now();
        let (should_break, result) = route_action(
            action.clone(),
            ClientId::new(1),
            None,
            None,
            ThreadSenders {
                to_plugin: Some(SenderWithContext::new(plugin_tx)),
                to_server: Some(SenderWithContext::new(server_tx)),
                ..Default::default()
            },
            None,
            None,
            InputMode::Normal,
            None,
            true,
            true,
        )
        .unwrap();
        // Timing is the regression: these local no-op/dispatch paths used to wait a full second.
        assert!(
            started.elapsed() < crate::completion::ACTION_COMPLETION_TIMEOUT / 2,
            "{action:?}"
        );
        assert!(
            !result.unwrap().confirmed,
            "dispatch must not claim downstream completion"
        );
        let quit = matches!(&action, Action::SkipConfirm { action } if **action == Action::Quit);
        assert_eq!(should_break, quit);
        assert_eq!(server_rx.len(), usize::from(quit));
        if quit {
            assert!(matches!(
                server_rx.try_recv().unwrap().0,
                ServerInstruction::ClientExit(_, None)
            ));
        }
        let arrange = action == Action::ArrangePanesByProject;
        assert_eq!(plugin_rx.len(), usize::from(arrange));
        if arrange {
            assert!(matches!(
                plugin_rx.try_recv().unwrap().0,
                PluginInstruction::ArrangePanesByProject
            ));
        }
    }
}

#[test]
fn keybind_pipe_forwards_in_place_intent_and_explicit_targets() {
    use yinz_types::internal_ids::TerminalId;

    let explicit = PaneId::Terminal(TerminalId::new(7));
    for (in_place, pane_id, expected) in [
        (true, None, None),
        (true, Some(explicit), Some(explicit)),
        (false, Some(explicit), None),
        (false, None, None),
    ] {
        let (screen_tx, screen_rx) = yinz_runtime::channels::unbounded();
        let (plugin_tx, plugin_rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_plugin: Some(SenderWithContext::new(plugin_tx)),
            ..Default::default()
        };
        let result = route_action(
            Action::KeybindPipe {
                name: Some("message".into()),
                payload: None,
                args: None,
                plugin: None,
                plugin_id: None,
                configuration: None,
                launch_new: false,
                skip_cache: false,
                floating: None,
                in_place: Some(in_place),
                cwd: None,
                pane_title: None,
            },
            ClientId::new(1),
            None,
            pane_id,
            senders,
            None,
            None,
            InputMode::Normal,
            None,
            true,
            true, // is_client_input
        );
        result.unwrap();
        assert!(screen_rx.try_recv().is_err());
        let (instruction, _) = plugin_rx.try_recv().unwrap();
        let PluginInstruction::KeybindPipe {
            should_open_in_place,
            pane_id_to_replace,
            ..
        } = instruction
        else {
            panic!("unexpected plugin instruction");
        };
        assert_eq!(
            (should_open_in_place, pane_id_to_replace),
            (in_place, expected)
        );
    }
}

#[test]
fn cli_pipe_eof_requires_a_previous_message_on_the_same_connection() {
    let (to_plugin, plugins) = yinz_runtime::channels::unbounded();
    let (to_server, server) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        to_server: Some(SenderWithContext::new(to_server)),
        should_silently_fail: true,
        ..Default::default()
    };
    for focus_plugin in [false, true] {
        let mut seen = HashSet::new();
        for (pipe_id, payload, expected_eof) in [
            ("empty", None, false),
            ("stream", Some("input".to_owned()), false),
            ("stream", Some(String::new()), false),
            ("stream", None, true),
            ("empty", None, true),
        ] {
            route_action(
                Action::CliPipe {
                    focus_plugin,
                    pipe_id: pipe_id.into(),
                    name: Some("test".into()),
                    payload,
                    plugin: None,
                    args: None,
                    configuration: None,
                    floating: None,
                    in_place: None,
                    launch_new: false,
                    skip_cache: false,
                    cwd: None,
                    pane_title: None,
                },
                ClientId::new(1),
                Some(ClientId::new(2)),
                None,
                senders.clone(),
                None,
                Some(&mut seen),
                InputMode::Normal,
                None,
                true,
                true, // is_client_input
            )
            .unwrap();
            let (
                PluginInstruction::CliPipe {
                    is_eof,
                    focus_plugin: forwarded_focus,
                    ..
                },
                _,
            ) = plugins.try_recv().unwrap()
            else {
                panic!("expected a CLI pipe instruction");
            };
            assert_eq!(is_eof, expected_eof);
            assert_eq!(forwarded_focus, focus_plugin);
        }
        assert_eq!(server.try_iter().count(), 2, "associate each pipe once");
    }
}

/// The bug this guards: bound keys become their own screen instructions and never reach
/// `WriteCharacter`, so without a dismissal here a tab switch leaves a stale menu on screen,
/// still offering to act on the pane it was opened over.
#[test]
fn only_keys_bound_to_something_other_than_writing_dismiss_a_context_menu() {
    let write = Action::Write {
        key_with_modifier: None,
        bytes: vec![b'a'],
        is_kitty_keyboard_protocol: false,
    };
    assert!(!actions_dismiss_context_menu(std::slice::from_ref(&write)));
    assert!(!actions_dismiss_context_menu(&[Action::WriteChars {
        chars: "a".to_owned()
    }]));
    assert!(actions_dismiss_context_menu(&[Action::GoToNextTab]));
    assert!(
        actions_dismiss_context_menu(&[write, Action::GoToNextTab]),
        "one non-writing action in the sequence is enough"
    );
}

/// The bug this guards: `bind "Enter" { Copy; SwitchToMode "Normal"; }` set the status bar's
/// "Text copied to..." message with the first action and cleared it with the second's
/// `InputReceived`, so a keyboard selection copied silently while a mouse one did not.
#[test]
fn a_key_whose_batch_copies_leaves_the_clipboard_message_alone() {
    assert!(actions_leave_clipboard_message(&[Action::Copy]));
    assert!(
        actions_leave_clipboard_message(&[
            Action::Copy,
            Action::SwitchToMode {
                input_mode: InputMode::Normal
            },
        ]),
        "the trailing mode switch is part of the keystroke that copied"
    );
    assert!(actions_leave_clipboard_message(&[
        Action::CopyLastCommandOutput,
        Action::SwitchToMode {
            input_mode: InputMode::Locked
        },
    ]));
    assert!(!actions_leave_clipboard_message(&[
        Action::SwitchToMode {
            input_mode: InputMode::Normal
        },
        Action::GoToNextTab,
    ]));
}

/// The other half of the same bug: the batch decision only helps if `route_action` honours
/// it, and the `InputReceived` it would otherwise send is what clears the message.
#[test]
fn a_suppressed_action_tells_the_plugins_nothing_about_the_input() {
    let route = |leaves_clipboard_message| {
        let (tx, rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_plugin: Some(SenderWithContext::new(tx)),
            should_silently_fail: true,
            ..Default::default()
        };
        let _ = route_action(
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
            ClientId::new(1),
            None,
            None,
            senders,
            None,
            None,
            InputMode::Normal,
            None,
            leaves_clipboard_message,
            true, // is_client_input
        );
        let mut events = Vec::new();
        while let Ok((instruction, _ctx)) = rx.try_recv() {
            if let PluginInstruction::Update(updates) = instruction {
                events.extend(updates.into_iter().map(|(_, _, event)| event));
            }
        }
        events
    };
    assert!(
        route(false)
            .iter()
            .any(|event| matches!(event, Event::InputReceived)),
        "an ordinary action still reports the input"
    );
    assert!(
        !route(true)
            .iter()
            .any(|event| matches!(event, Event::InputReceived)),
        "an action belonging to a keystroke that copied stays silent"
    );
}

/// A keybind is routed with no pane id, so its `in_place` can never become a
/// `pane_id_to_replace`; the instruction carries the intent on its own for the host to
/// resolve against the client's focused pane.
#[test]
fn a_keybind_pipe_opened_in_place_carries_the_intent_without_a_pane_id() {
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(tx)),
        should_silently_fail: true,
        ..Default::default()
    };
    let _ = route_action(
        Action::KeybindPipe {
            name: Some("go".to_owned()),
            payload: None,
            args: None,
            plugin: Some("yinz:configuration".to_owned()),
            plugin_id: None,
            configuration: None,
            launch_new: false,
            skip_cache: false,
            floating: Some(false),
            in_place: Some(true),
            cwd: None,
            pane_title: None,
        },
        ClientId::new(1),
        None,
        None,
        senders,
        None,
        None,
        InputMode::Normal,
        None,
        false,
        true, // is_client_input
    );
    let placement = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|(instruction, _ctx)| match instruction {
            PluginInstruction::KeybindPipe {
                should_open_in_place,
                pane_id_to_replace,
                ..
            } => Some((should_open_in_place, pane_id_to_replace)),
            _ => None,
        })
        .expect("the keybind reached the plugin thread");
    assert_eq!(placement, (true, None));
}

#[test]
fn a_blocking_pane_with_an_explicit_target_is_routed_to_that_pane() {
    use yinz_bus::pty::ClientTabIndexOrPaneId;
    use yinz_types::internal_ids::TerminalId;

    let client = ClientId::new(1);
    let caller = PaneId::Terminal(TerminalId::new(1));
    let target = PaneId::Terminal(TerminalId::new(3));
    for (placement, expected) in [
        // The CLI's `--pane-id` conflicts with `--near-current-pane`: the target alone must
        // route, or the pane is placed in the client's tab, where the target may not be.
        (
            NewPanePlacement::InPlace {
                pane_id_to_replace: Some(target),
                close_replaced_pane: false,
                borderless: None,
            },
            ClientTabIndexOrPaneId::PaneId(target),
        ),
        (
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(target),
                borderless: None,
            },
            ClientTabIndexOrPaneId::PaneId(target),
        ),
        // Without one, the originating pane is still used only when asked for.
        (
            NewPanePlacement::InPlace {
                pane_id_to_replace: None,
                close_replaced_pane: false,
                borderless: None,
            },
            ClientTabIndexOrPaneId::ClientId(client),
        ),
    ] {
        let (pty_tx, pty_rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_pty: Some(SenderWithContext::new(pty_tx)),
            ..Default::default()
        };
        let (completion_tx, _completion_rx) = oneshot::channel();
        panes::new_blocking_pane(
            placement,
            None,
            None,
            None,
            false, // near_current_pane
            false, // no_focus
            None,
            &senders,
            &None,
            client,
            Some(caller),
            completion_tx,
        )
        .unwrap();
        let (instruction, _) = pty_rx.try_recv().unwrap();
        let PtyInstruction::SpawnTerminal(_, _, _, _, routing, _, blocking) = instruction else {
            panic!("unexpected pty instruction");
        };
        assert!(blocking);
        assert_eq!(routing, expected);
    }
}

/// Listing panes asks the pty thread about every terminal pane in one request, so a busy pty
/// thread costs one deadline for the whole list rather than one per pane.
#[test]
fn list_panes_asks_the_pty_thread_once_for_every_terminal_pane() {
    use yinz_types::data::{PaneInfo, PaneListEntry};
    use yinz_types::internal_ids::{PluginId, TabId, TabPosition, TerminalId};

    let (screen_tx, screen_rx) = yinz_runtime::channels::unbounded();
    let (pty_tx, pty_rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(screen_tx)),
        to_pty: Some(SenderWithContext::new(pty_tx)),
        ..Default::default()
    };
    let terminals: Vec<PaneId> = [1, 2, 3]
        .map(|id| PaneId::Terminal(TerminalId::new(id)))
        .to_vec();
    let entries: Vec<PaneListEntry> = terminals
        .iter()
        .copied()
        .chain([PaneId::Plugin(PluginId::new(9))])
        .map(|id| PaneListEntry {
            pane_info: PaneInfo {
                id,
                ..Default::default()
            },
            tab_id: TabId::new(0),
            tab_position: TabPosition::new(0),
            tab_name: "tab".to_owned(),
            pane_command: None,
            pane_cwd: None,
        })
        .collect();

    let listing = std::thread::spawn(move || {
        queries::list_panes(
            false,
            false,
            false,
            false,
            false,
            true, // output_json
            &senders,
            None,
            None,
            ClientId::new(1),
        )
    });
    let (
        ScreenInstruction::ListPanes {
            response_channel, ..
        },
        _,
    ) = screen_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
    else {
        panic!("expected a ListPanes request");
    };
    response_channel.send(entries).unwrap();
    let (
        PtyInstruction::GetPanesMetadata {
            pane_ids,
            response_channel,
        },
        _,
    ) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
    else {
        panic!("expected one batched metadata request");
    };
    assert_eq!(pane_ids, terminals);
    response_channel.send(Default::default()).unwrap();
    listing.join().unwrap().unwrap();
    assert!(
        pty_rx.try_recv().is_err(),
        "no per-pane requests should follow the batch"
    );
}
