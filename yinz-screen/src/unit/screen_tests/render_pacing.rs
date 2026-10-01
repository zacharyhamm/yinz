use super::*;
use std::time::Duration;

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size {
    cols: 121,
    rows: 20,
};

fn screen_with_backlog() -> (Screen, ServerReceiver, RenderBacklogs) {
    let (mut screen, _, receiver) = create_new_screen_with_capture(SIZE, true, true, true, false);
    let fake_os_input = FakeInputOutput::default();
    let backlog = fake_os_input.render_backlog.clone();
    screen.bus.os_input = Some(Box::new(fake_os_input));
    new_tab(&mut screen, 1, TabId::new(0));
    (screen, receiver, backlog)
}

fn backlog(queued_renders: usize, in_flight_for: Option<Duration>) -> RenderBacklog {
    RenderBacklog {
        queued_renders,
        in_flight_for,
    }
}

fn rendered_bytes(receiver: &ServerReceiver) -> String {
    receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => output.get(&CLIENT).cloned(),
            _ => None,
        })
        .collect()
}

#[test]
fn a_client_with_a_queued_frame_holds_back_rendering() {
    let (screen, _receiver, client_backlog) = screen_with_backlog();
    assert!(!screen.a_client_is_behind_on_renders());

    // Writing the only frame it has is keeping up.
    *client_backlog.lock().unwrap() = vec![(CLIENT, backlog(0, Some(Duration::from_secs(1))))];
    assert!(!screen.a_client_is_behind_on_renders());

    *client_backlog.lock().unwrap() = vec![(CLIENT, backlog(1, Some(Duration::from_millis(50))))];
    assert!(screen.a_client_is_behind_on_renders());
}

#[test]
fn a_client_stalled_on_one_write_stops_holding_back_rendering() {
    let (screen, _receiver, client_backlog) = screen_with_backlog();
    *client_backlog.lock().unwrap() = vec![
        (CLIENT, backlog(3, Some(Duration::from_secs(60)))),
        (ClientId::new(2), backlog(0, None)),
    ];
    assert!(!screen.a_client_is_behind_on_renders());
}

#[test]
fn a_skipped_frame_is_carried_by_the_next_render() {
    let (mut screen, receiver, _) = screen_with_backlog();
    screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&receiver);

    // Two batches of output arrive while the client is behind, so no frame is rendered
    // between them. The next frame must carry both.
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[3;1Hskipped-frame".to_vec())
        .unwrap();
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[5;1Hnext-frame".to_vec())
        .unwrap();
    // These renders run synchronously, so the channel holds every byte of the frame.
    screen.render_to_clients().unwrap();
    let frame = rendered_bytes(&receiver);
    assert!(frame.contains("skipped-frame"), "{frame:?}");
    assert!(frame.contains("next-frame"), "{frame:?}");
}
