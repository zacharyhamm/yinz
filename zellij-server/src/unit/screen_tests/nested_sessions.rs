use super::*;
use zellij_test_support::Watched;

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
            pane_id: PaneId::Terminal(crate::TerminalId(0)),
            message: guest_announce_message(),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let announce_ack = received_pty_instructions
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, TerminalId(0), None) => decode_nested_frame(bytes),
            _ => None,
        })
        .expect("an announce_ack frame written to the guest pane");
    assert_eq!(
        announce_ack,
        zellij_ipc::nested_session::NestedSessionMessage::AnnounceAck {
            ancestry: vec!["zellij-test".to_owned()],
            capabilities: vec![zellij_ipc::nested_session::NestedSessionCapability::NestedControl],
            descend_keys: vec![],
        }
    );
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StartNestedGuestPing(PaneId::Terminal(TerminalId(0)))
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
            pane_id: PaneId::Terminal(crate::TerminalId(0)),
            message: guest_announce_message(),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let announce_ack = received_pty_instructions
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, TerminalId(0), None) => decode_nested_frame(bytes),
            _ => None,
        })
        .expect("an announce_ack frame written to the guest pane even in never mode");
    assert!(matches!(
        announce_ack,
        zellij_ipc::nested_session::NestedSessionMessage::AnnounceAck { .. }
    ));
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StartNestedGuestPing(PaneId::Terminal(TerminalId(0)))
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
            pane_id: PaneId::Terminal(crate::TerminalId(0)),
            message: guest_announce_message(),
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(crate::TerminalId(0)),
            message: zellij_ipc::nested_session::NestedSessionMessage::Bye,
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedGuestPingTick {
            pane_id: PaneId::Terminal(crate::TerminalId(0)),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let ping_frames_written = received_pty_instructions
        .lock()
        .iter()
        .filter(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, TerminalId(0), None) => matches!(
                decode_nested_frame(bytes),
                Some(zellij_ipc::nested_session::NestedSessionMessage::Ping)
            ),
            _ => false,
        })
        .count();
    assert_eq!(ping_frames_written, 0);
    assert!(received_background_jobs.lock().iter().any(|job| matches!(
        job,
        BackgroundJob::StopNestedGuestPing(PaneId::Terminal(TerminalId(0)))
    )));
}
