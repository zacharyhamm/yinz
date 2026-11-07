use super::*;
use crate::os_input_output::ServerConnection;
use crate::test_support::FakeClientOsApi;
use yinz_runtime::channels;
use yinz_types::internal_ids::SessionName;

#[test]
fn configured_mouse_mode_preserves_manual_toggles_until_configuration_changes() {
    for configured in [None, Some(true), Some(false)] {
        let (client_tx, _client_rx) = channels::unbounded();
        let (_input_tx, input_rx) = channels::unbounded();
        let reannounce = NestedReannounce::spawn(
            Box::new(FakeClientOsApi::default()),
            SessionName::from("mouse-mode-test"),
        );
        reannounce.stop();
        let mut handler = InputHandler::new(
            Box::new(FakeClientOsApi::default()),
            CommandIsExecuting::new(),
            Options {
                mouse_mode: configured,
                ..Default::default()
            },
            SenderWithContext::new(client_tx),
            input_rx,
            input_control_channel().1,
            reannounce,
        );
        let enabled = configured.unwrap_or(true);
        handler.mouse_mode_active = enabled;
        handler.dispatch_action(Action::ToggleMouseMode, None);
        assert_eq!(handler.mouse_mode_active, !enabled);
        for _ in 0..2 {
            handler.set_configured_mouse_mode(enabled);
            assert_eq!(handler.mouse_mode_active, !enabled);
        }
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.options.mouse_mode, Some(!enabled));
        handler.dispatch_action(Action::ToggleMouseMode, None);
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.mouse_mode_active, enabled);
        handler.set_configured_mouse_mode(enabled);
        handler.set_configured_mouse_mode(!enabled);
        assert_eq!(handler.mouse_mode_active, !enabled);
    }
}

#[test]
fn text_area_reports_carry_the_grid_observed_by_the_reporting_client() {
    use yinz_ipc::ipc::PixelDimensions;
    use yinz_types::pane_size::{Size, SizeInPixels};
    let os_api = FakeClientOsApi::default();
    let (client_tx, _client_rx) = channels::unbounded();
    let (_input_tx, input_rx) = channels::unbounded();
    let reannounce = NestedReannounce::spawn(
        Box::new(os_api.clone()),
        SessionName::from("pixel-report-test"),
    );
    reannounce.stop();
    let mut handler = InputHandler::new(
        Box::new(os_api.clone()),
        CommandIsExecuting::new(),
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        input_control_channel().1,
        reannounce,
    );
    let area = SizeInPixels {
        width: 800,
        height: 480,
    };
    handler.handle_stdin_ansi_instruction(AnsiStdinInstruction::PixelDimensions(PixelDimensions {
        text_area_size: Some(area),
        ..Default::default()
    }));
    assert_eq!(
        os_api.sent_messages(),
        vec![ClientToServerMsg::TerminalPixelDimensions {
            pixel_dimensions: PixelDimensions {
                text_area_size: Some(area),
                text_area_grid_size: Some(Size { cols: 80, rows: 24 }),
                character_cell_size: None,
            }
        }]
    );
}

#[test]
fn offline_and_stale_keyboard_paste_and_mouse_input_are_discarded_after_replacement() {
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new(
        "host".into(),
        command.clone(),
        &Default::default(),
        &Default::default(),
    );
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    let key = KeyWithModifier::new(yinz_types::keys::BareKey::Char('x'));
    for stamp in [(0, true), (0, false)] {
        for instruction in [
            InputInstruction::KeyWithModifierEvent(key.clone(), b"x".to_vec(), false),
            InputInstruction::KeyEvent(InputEvent::Paste("OFFLINE".into()), Vec::new()),
            InputInstruction::KeyEvent(
                InputEvent::Mouse(TermwizMouseEvent {
                    x: 10,
                    y: 10,
                    mouse_buttons: MouseButtons::LEFT,
                    modifiers: Modifiers::NONE,
                }),
                Vec::new(),
            ),
        ] {
            input_tx
                .send(InputInstruction::FromGeneration(
                    stamp,
                    Box::new(instruction),
                ))
                .unwrap();
        }
    }
    input_tx
        .send(InputInstruction::FromGeneration(
            (1, true),
            Box::new(InputInstruction::KeyEvent(
                InputEvent::Paste("CURRENT".into()),
                Vec::new(),
            )),
        ))
        .unwrap();
    input_tx.send(InputInstruction::Exit).unwrap();
    recovery.control.lost(0, "EOF".into());
    recovery.control.replacing();
    recovery.control.repainted();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "input-recovery".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        input_control_channel().1,
        nested,
    )
    .handle_input();
    assert_eq!(
        os.sent_messages(),
        vec![ClientToServerMsg::Paste("CURRENT".into())]
    );
}

#[test]
fn offline_focus_changes_survive_recovery_and_queued_generation_changes() {
    use crate::remote_recovery::Phase;

    for phase in [Phase::Reconnecting, Phase::Repainting, Phase::Connected] {
        for (focused, final_focus) in [
            (false, InputInstruction::HostTerminalFocusChanged(false)),
            (
                false,
                InputInstruction::KeyEvent(InputEvent::FocusLost, Vec::new()),
            ),
            (true, InputInstruction::HostTerminalFocusChanged(true)),
            (
                true,
                InputInstruction::KeyEvent(InputEvent::FocusGained, Vec::new()),
            ),
        ] {
            let os = FakeClientOsApi::new();
            let command = CommandIsExecuting::new();
            let recovery = crate::remote_recovery::Recovery::new(
                "host".into(),
                command.clone(),
                &Default::default(),
                &Default::default(),
            );
            os.set_remote_recovery(Some(recovery.control.clone()))
                .unwrap();
            os.send_to_server(ClientToServerMsg::HostTerminalFocusChanged { focused: true });
            recovery.control.lost(0, "EOF".into());
            if phase != Phase::Reconnecting {
                recovery.control.replacing();
            }
            if phase == Phase::Connected {
                recovery.control.repainted();
            }
            let (client_tx, _client_rx) = channels::unbounded();
            let (input_tx, input_rx) = channels::unbounded();
            let input_tx = SenderWithContext::new(input_tx);
            // Both focus representations describe this terminal even when queued
            // before a generation change or read during the outage.
            for (stamp, instruction) in [
                ((0, true), InputInstruction::HostTerminalFocusChanged(false)),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::FocusGained, Vec::new()),
                ),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::FocusLost, Vec::new()),
                ),
                ((0, false), final_focus),
                (
                    (0, false),
                    InputInstruction::KeyEvent(InputEvent::Paste("OFFLINE".into()), Vec::new()),
                ),
            ] {
                input_tx
                    .send(InputInstruction::FromGeneration(
                        stamp,
                        Box::new(instruction),
                    ))
                    .unwrap();
            }
            input_tx.send(InputInstruction::Exit).unwrap();
            let nested = NestedReannounce::spawn(Box::new(os.clone()), "focus-recovery".into());
            nested.stop();
            InputHandler::new(
                Box::new(os.clone()),
                command,
                Options::default(),
                SenderWithContext::new(client_tx),
                input_rx,
                input_control_channel().1,
                nested,
            )
            .handle_input();
            let before_replay = os.sent_messages();
            assert!(!before_replay
                .iter()
                .any(|msg| matches!(msg, ClientToServerMsg::Paste(_))));
            if phase == Phase::Reconnecting {
                assert_eq!(before_replay.len(), 1, "offline focus must only be cached");
                recovery.control.replacing();
            }
            recovery.control.replay(&os);
            let replayed: Vec<_> = os.sent_messages()[before_replay.len()..]
                .iter()
                .filter_map(|msg| match msg {
                    ClientToServerMsg::HostTerminalFocusChanged { focused } => Some(*focused),
                    _ => None,
                })
                .collect();
            assert_eq!(replayed, vec![focused], "{phase:?}");
        }
    }
}

#[test]
fn fresh_keyboard_paste_and_ctrl_c_after_recovery_reach_the_healthy_server() {
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new(
        "host".into(),
        command.clone(),
        &Default::default(),
        &Default::default(),
    );
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    recovery.control.lost(0, "outage".into());
    recovery.control.replacing();
    recovery.control.repainted();
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    let key = KeyWithModifier::new(yinz_types::keys::BareKey::Char('x'));
    let ctrl_c = KeyWithModifier::new(yinz_types::keys::BareKey::Char('c')).with_ctrl_modifier();
    for instruction in [
        InputInstruction::KeyWithModifierEvent(key.clone(), b"x".to_vec(), false),
        InputInstruction::KeyEvent(InputEvent::Paste("fresh".into()), Vec::new()),
        InputInstruction::KeyWithModifierEvent(ctrl_c.clone(), vec![3], false),
    ] {
        input_tx
            .send(InputInstruction::FromGeneration(
                (1, true),
                Box::new(instruction),
            ))
            .unwrap();
    }
    input_tx.send(InputInstruction::Exit).unwrap();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "fresh-input".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        input_control_channel().1,
        nested,
    )
    .handle_input();
    assert!(!recovery.interrupted());
    assert_eq!(
        os.sent_messages(),
        vec![
            ClientToServerMsg::Key {
                key,
                raw_bytes: b"x".to_vec(),
                is_kitty_keyboard_protocol: false
            },
            ClientToServerMsg::Paste("fresh".into()),
            ClientToServerMsg::Key {
                key: ctrl_c,
                raw_bytes: vec![3],
                is_kitty_keyboard_protocol: false
            },
        ]
    );
}

#[test]
fn ctrl_c_during_recovery_cancels_locally_and_is_not_sent_to_the_server() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let os = FakeClientOsApi::new();
    let command = CommandIsExecuting::new();
    let recovery = crate::remote_recovery::Recovery::new(
        "host".into(),
        command.clone(),
        &Default::default(),
        &Default::default(),
    );
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    recovery.control.lost(0, "EOF".into());
    let (client_tx, _client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    input_tx
        .send(InputInstruction::FromGeneration(
            (0, false),
            Box::new(InputInstruction::KeyWithModifierEvent(
                KeyWithModifier::new(yinz_types::keys::BareKey::Char('c')).with_ctrl_modifier(),
                b"\x1b[99;5u".to_vec(),
                true,
            )),
        ))
        .unwrap();
    input_tx.send(InputInstruction::Exit).unwrap();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "input-cancel".into());
    nested.stop();
    InputHandler::new(
        Box::new(os.clone()),
        command,
        Options::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        input_control_channel().1,
        nested,
    )
    .handle_input();
    assert!(recovery.interrupted());
    assert!(os.sent_messages().is_empty());
}

#[test]
fn screensaver_consumes_wake_paste_and_forwards_the_following_input() {
    let os = FakeClientOsApi::new();
    let mut saver =
        crate::screensaver::Screensaver::new(&Default::default(), &Default::default()).unwrap();
    saver
        .tick(
            &os,
            None,
            std::time::Instant::now() + std::time::Duration::from_secs(61),
            true,
        )
        .unwrap();
    assert!(saver.active());
    let (client_tx, client_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    let input_tx = SenderWithContext::new(input_tx);
    for text in ["wake only", "forward this"] {
        input_tx
            .send(InputInstruction::KeyEvent(
                InputEvent::Paste(text.into()),
                vec![],
            ))
            .unwrap();
    }
    input_tx.send(InputInstruction::Exit).unwrap();
    let reannounce =
        NestedReannounce::spawn(Box::new(os.clone()), SessionName::from("screensaver-input"));
    reannounce.stop();
    let mut handler = InputHandler::new(
        Box::new(os.clone()),
        CommandIsExecuting::new(),
        Default::default(),
        SenderWithContext::new(client_tx),
        input_rx,
        input_control_channel().1,
        reannounce,
    );
    handler.activity = Some(saver.activity.clone());
    handler.handle_input();
    assert_eq!(
        os.sent_messages(),
        vec![ClientToServerMsg::Paste("forward this".into())]
    );
    assert!(client_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ClientInstruction::UserActivity)));
}

#[test]
fn queued_control_comes_before_queued_input_and_a_closed_control_channel_is_left_behind() {
    let (control_tx, control_rx) = channels::unbounded();
    let (input_tx, input_rx) = channels::unbounded();
    input_tx.send("input 1").unwrap();
    control_tx.send("control 1").unwrap();
    control_tx.send("control 2").unwrap();
    input_tx.send("input 2").unwrap();
    let mut control = Some(control_rx);
    let mut received = Vec::new();
    for _ in 0..3 {
        received.push(recv_prioritized(&mut control, &input_rx).unwrap());
    }
    assert_eq!(received, ["control 1", "control 2", "input 1"]);
    drop(control_tx);
    assert_eq!(recv_prioritized(&mut control, &input_rx), Ok("input 2"));
    assert!(control.is_none());
    // Wakes for input that arrives while it waits, rather than only for control.
    let late_input = std::thread::spawn(move || input_tx.send("input 3").unwrap());
    let (_control_tx, control_rx) = channels::unbounded::<&str>();
    let mut control = Some(control_rx);
    assert_eq!(recv_prioritized(&mut control, &input_rx), Ok("input 3"));
    late_input.join().unwrap();
}

/// The client loop and the input thread each wait for room in the other's queue. With both
/// queues full and the input thread making no progress, the client loop must still get through
/// mouse-mode changes and a stale forward's completion, and the input thread must deliver them
/// once it drains.
#[test]
fn client_loop_never_waits_on_a_stalled_input_thread_and_its_messages_still_arrive() {
    use crate::client_loop::ClientLoop;
    use crate::render_budget::RenderBudget;
    use crate::stdin_ansi_parser::{ForwardKind, StdinAnsiParser};
    use crate::terminal_escapes::ClientTeardown;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};
    use yinz_ipc::nested_session::{encode_payload, NestedSessionMessage};
    use yinz_runtime::channels::Shutdown;

    // More than any bound the input side used to have, so none could absorb them all.
    const MOUSE_CHANGES: usize = 60;
    const INPUT_CAPACITY: usize = 50;
    let os = FakeClientOsApi::default();
    let client_shutdown = Shutdown::default();
    let (client_tx, client_rx) = channels::bounded(MOUSE_CHANGES + 2);
    let client_tx = client_shutdown.sender(client_tx);
    for change in 0..MOUSE_CHANGES {
        client_tx
            .send(ClientInstruction::SetMouseMode(change % 2 == 1))
            .unwrap();
    }
    client_tx
        .send(ClientInstruction::ForwardQueryToHost {
            token: 2,
            query_bytes: b"\x1b]11;?\x1b\\".to_vec(),
            resolve_async: false,
        })
        .unwrap();
    client_tx
        .send(ClientInstruction::Exit(ExitReason::Normal))
        .unwrap();
    assert!(client_rx.is_full());

    // Each ping makes the input thread wait for room in the client loop's queue.
    let (input_tx, input_rx) = channels::bounded(INPUT_CAPACITY);
    let input_tx = SenderWithContext::new(input_tx);
    for _ in 0..INPUT_CAPACITY {
        input_tx
            .send(InputInstruction::NestedSessionFrameFromHost(
                encode_payload(&NestedSessionMessage::Ping),
            ))
            .unwrap();
    }
    assert!(input_rx.is_full());

    // The server gave up on token 1 and dispatched token 2 while its slot was still open.
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    parser.lock().unwrap().open(ForwardKind::Barrier, 1);
    let (control_tx, control_rx) = input_control_channel();
    let (finished, completion) = mpsc::channel();
    let client_loop = std::thread::spawn({
        let os = os.clone();
        let parser = parser.clone();
        move || {
            let nested = NestedReannounce::spawn(
                Box::new(FakeClientOsApi::default()),
                SessionName::from("stalled-input"),
            );
            nested.stop();
            let budget = Arc::new(RenderBudget::new(100));
            let outcome = ClientLoop {
                signal_quit_supported: Default::default(),
                recovery: &mut None,
                send_client_instructions: None,
                sessions: &mut crate::session_switch::Sessions::new(
                    None,
                    "test".into(),
                    &Default::default(),
                    &mut Default::default(),
                ),
                remote_host: None,
                os_input: &os,
                command_is_executing: &mut CommandIsExecuting::new(),
                stdin_ansi_parser: &parser,
                send_input_control: &control_tx,
                nested_reannounce: &nested,
                teardown: ClientTeardown::new(24, false),
                synchronised_output: None,
                render_budget: &budget,
                startup_started: Instant::now(),
                splash: None,
                screensaver: None,
            }
            .run(&client_rx);
            finished.send((outcome.exit_status, client_rx)).unwrap();
        }
    });
    let (exit_status, client_rx) = completion
        .recv_timeout(Duration::from_secs(30))
        .expect("the client loop must not wait for the stalled input thread");
    client_loop.join().unwrap();
    assert_eq!(exit_status, 0);

    let reannounce = NestedReannounce::spawn(
        Box::new(FakeClientOsApi::default()),
        SessionName::from("stalled-input-drain"),
    );
    reannounce.stop();
    let input_thread = std::thread::spawn({
        let os = os.clone();
        move || {
            input_loop(
                Box::new(os),
                Options::default(),
                CommandIsExecuting::new(),
                client_tx,
                input_rx,
                control_rx,
                reannounce,
                None,
            )
        }
    });
    let mut expected_modes = vec![true];
    expected_modes.extend((0..MOUSE_CHANGES).map(|change| change % 2 == 1));
    os.mouse_modes.wait_until_described(
        "every configured mouse-mode change to reach the terminal",
        Duration::from_secs(30),
        |modes| (modes == &expected_modes).then_some(()),
        |modes| format!("{modes:?}"),
    );
    // Token 1 was closed out by the client loop; token 2's slot by its deadline on the
    // forward-timeout runtime. Both travel the control channel.
    let forwarded = |messages: &Vec<ClientToServerMsg>| -> Vec<u32> {
        messages
            .iter()
            .filter_map(|message| match message {
                ClientToServerMsg::ForwardedReplyFromHost { token, .. } => Some(*token),
                _ => None,
            })
            .collect()
    };
    os.sent_to_server.wait_until_described(
        "both forwards' completions to reach the server, in dispatch order",
        Duration::from_secs(30),
        |messages| (forwarded(messages) == [1, 2]).then_some(()),
        |messages| format!("{:?}", forwarded(messages)),
    );
    for _ in 0..INPUT_CAPACITY {
        let (instruction, _) = client_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("every ping is answered once the input thread drains");
        assert!(matches!(
            instruction,
            ClientInstruction::EmitNestedSessionFrame(_)
        ));
    }
    input_tx.send(InputInstruction::Exit).unwrap();
    input_thread.join().unwrap();
}
