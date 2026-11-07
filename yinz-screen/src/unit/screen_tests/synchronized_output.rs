use super::*;

#[test]
fn a_synchronized_frame_that_never_ends_is_shown_once_it_lapses() {
    let size = Size { cols: 80, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        b"COMPLETED-FRAME".to_vec(),
    ));
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "completed frame",
        |frame| frame.contains("COMPLETED-FRAME"),
    );
    // The application begins a frame and dies: no end marker and no further output. Only the
    // lapse of the lock, on the screen thread's own deadline, can show what it drew.
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        b"\x1b[?2026h\x1b[2J\x1b[HNEVER-ENDED".to_vec(),
    ));
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "lapsed synchronized frame",
        |frame| frame.contains("NEVER-ENDED") && !frame.contains("COMPLETED-FRAME"),
    );
    mock_screen.teardown(vec![server_thread, screen_thread]);
}
