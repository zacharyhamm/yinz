#[cfg(test)]
const TERMINAL_0: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(0);

use super::*;
use yinz_test_support::Watched;

#[test]
pub fn nested_guest_announce_gets_announce_ack_with_ancestry() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
            message: guest_announce_message(),
        });
    let announce_ack = received_pty_instructions.wait_until(
        "an announce_ack frame written to the guest pane",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    PtyWriteInstruction::Write(bytes, TERMINAL_0, None) => {
                        decode_nested_frame(bytes)
                    },
                    _ => None,
                })
        },
    );
    received_background_jobs.wait_until(
        "liveness pings started for the guest pane",
        yinz_test_support::default_timeout(),
        |jobs| {
            jobs.iter()
                .any(|job| {
                    matches!(
                        job,
                        BackgroundJob::StartNestedGuestPing(PaneId::Terminal(TERMINAL_0))
                    )
                })
                .then_some(())
        },
    );
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    assert_eq!(
        announce_ack,
        yinz_ipc::nested_session::NestedSessionMessage::AnnounceAck {
            ancestry: vec!["yinz-test".to_owned().into()],
            capabilities: vec![yinz_ipc::nested_session::NestedSessionCapability::NestedControl],
            descend_keys: vec![],
        }
    );
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StartNestedGuestPing(PaneId::Terminal(TERMINAL_0))
    )));
}

#[test]
pub fn nested_guest_announce_in_never_mode_still_completes_handshake() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.nested_session_handling = Some(NestedSessionHandling::Never);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
            message: guest_announce_message(),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let announce_ack = received_pty_instructions
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, TERMINAL_0, None) => decode_nested_frame(bytes),
            _ => None,
        })
        .expect("an announce_ack frame written to the guest pane even in never mode");
    assert!(matches!(
        announce_ack,
        yinz_ipc::nested_session::NestedSessionMessage::AnnounceAck { .. }
    ));
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StartNestedGuestPing(PaneId::Terminal(TERMINAL_0))
    )));
}

#[test]
pub fn nested_guest_bye_stops_liveness_pings() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
            message: guest_announce_message(),
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
            message: yinz_ipc::nested_session::NestedSessionMessage::Bye,
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedGuestPingTick {
            pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let ping_frames_written = received_pty_instructions
        .lock()
        .iter()
        .filter(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, TERMINAL_0, None) => matches!(
                decode_nested_frame(bytes),
                Some(yinz_ipc::nested_session::NestedSessionMessage::Ping)
            ),
            _ => false,
        })
        .count();
    assert_eq!(ping_frames_written, 0);
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StopNestedGuestPing(PaneId::Terminal(TERMINAL_0))
    )));
}

#[test]
fn stacking_a_zoomed_guest_sends_the_exit_frame_before_the_next_instruction() {
    use yinz_ipc::nested_session::NestedSessionMessage;

    let (mut screen, capture) = create_new_screen_with_forward_capture(Size {
        cols: 120,
        rows: 40,
    });
    let tab_id = TabId::new(0);
    let guest = PaneId::Terminal(TERMINAL_0);
    let sibling = PaneId::Terminal(TerminalId::new(1));
    new_tab(&mut screen, 0, tab_id);
    add_pane_to_tab(&mut screen, tab_id, sibling, false);
    screen.handle_nested_session_message_from_pane(guest, guest_announce_message());
    capture.drain_pty_writes();
    let guest_fullscreen_frames = || {
        capture
            .drain_pty_writes()
            .into_iter()
            .filter_map(
                |(bytes, terminal_id)| match (terminal_id, decode_nested_frame(&bytes)) {
                    (TERMINAL_0, Some(NestedSessionMessage::FullscreenState { fullscreen })) => {
                        Some(fullscreen)
                    },
                    _ => None,
                },
            )
            .collect::<Vec<_>>()
    };
    screen.handle_nested_session_message_from_pane(
        guest,
        NestedSessionMessage::ToggleHostFullscreen { fullscreen: true },
    );
    assert_eq!(guest_fullscreen_frames(), [true]);

    assert_eq!(screen.stack_panes(vec![sibling, guest]), Some(guest));
    assert_eq!(guest_fullscreen_frames(), [false]);
}
