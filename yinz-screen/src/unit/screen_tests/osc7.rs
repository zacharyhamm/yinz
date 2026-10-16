use super::*;

const CLIENT: ClientId = ClientId::new(1);
const OTHER_CLIENT: ClientId = ClientId::new(2);
const SIZE: Size = Size {
    cols: 121,
    rows: 20,
};

fn screen_with_two_panes() -> (Screen, ServerReceiver) {
    let (mut screen, _, receiver) = create_new_screen_with_capture(SIZE, true, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        CLIENT,
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(CLIENT).unwrap();
    render_osc7(&mut screen, &receiver);
    (screen, receiver)
}

fn emit_osc7(screen: &mut Screen, terminal: u32, uri: &str) {
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(terminal),
            format!("\x1b]7;{uri}\x1b\\").into_bytes(),
        )
        .unwrap();
}

fn render_osc7(screen: &mut Screen, receiver: &ServerReceiver) -> Vec<(ClientId, String)> {
    // These renders run synchronously, so the channel contains every byte of this frame.
    screen.render_to_clients().unwrap();
    let mut reports = vec![];
    for (instruction, _) in receiver.try_iter() {
        if let ServerInstruction::Render(Some(clients)) = instruction {
            for (client, output) in clients {
                for sequence in output.split("\x1b]7;").skip(1) {
                    let (uri, _) = sequence
                        .split_once("\x1b\\")
                        .expect("forwarded OSC 7 must have a string terminator");
                    reports.push((client, uri.to_owned()));
                }
            }
        }
    }
    reports.sort();
    reports
}

#[test]
fn osc7_forwards_only_changes_from_the_focused_pane() {
    let (mut screen, receiver) = screen_with_two_panes();
    emit_osc7(&mut screen, 2, "file://host/unfocused");
    assert!(render_osc7(&mut screen, &receiver).is_empty());

    emit_osc7(&mut screen, 1, "file://host/first");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/first".to_owned())]
    );
    emit_osc7(&mut screen, 1, "file://host/first");
    assert!(render_osc7(&mut screen, &receiver).is_empty());
    emit_osc7(&mut screen, 1, "file://host/second");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/second".to_owned())]
    );
}

#[test]
fn osc7_focus_changes_restate_the_directory_after_a_pane_without_a_report() {
    let (mut screen, receiver) = screen_with_two_panes();
    emit_osc7(&mut screen, 1, "file://host/upper");
    render_osc7(&mut screen, &receiver);

    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .move_focus_down(CLIENT)
        .unwrap();
    assert!(render_osc7(&mut screen, &receiver).is_empty());
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .move_focus_up(CLIENT)
        .unwrap();
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/upper".to_owned())]
    );

    emit_osc7(&mut screen, 2, "file://host/lower");
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .move_focus_down(CLIENT)
        .unwrap();
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/lower".to_owned())]
    );
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .move_focus_up(CLIENT)
        .unwrap();
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/upper".to_owned())]
    );
}

#[test]
fn osc7_tracks_each_clients_focus_and_last_report_separately() {
    let (mut screen, receiver) = screen_with_two_panes();
    screen.set_client_size(OTHER_CLIENT, SIZE).unwrap();
    screen.add_client(OTHER_CLIENT).unwrap();
    screen
        .get_active_tab_mut(OTHER_CLIENT)
        .unwrap()
        .move_focus_down(OTHER_CLIENT)
        .unwrap();
    emit_osc7(&mut screen, 1, "file://host/upper");
    emit_osc7(&mut screen, 2, "file://host/lower");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [
            (CLIENT, "file://host/upper".to_owned()),
            (OTHER_CLIENT, "file://host/lower".to_owned()),
        ]
    );

    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .move_focus_down(CLIENT)
        .unwrap();
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/lower".to_owned())]
    );
}

#[test]
fn osc7_follows_floating_and_overlay_focus_back_to_the_tiled_pane() {
    let (mut screen, receiver) = screen_with_two_panes();
    emit_osc7(&mut screen, 1, "file://host/tiled");
    render_osc7(&mut screen, &receiver);
    for (terminal, overlay, uri) in [
        (3, false, "file://host/floating"),
        (4, true, "file://host/overlay"),
    ] {
        open_floating_pane(&mut screen, TabId::new(0), terminal, "", overlay);
        emit_osc7(&mut screen, terminal, uri);
        assert_eq!(
            render_osc7(&mut screen, &receiver),
            [(CLIENT, uri.to_owned())]
        );
    }
    for (terminal, uri) in [(4, "file://host/floating"), (3, "file://host/tiled")] {
        screen.get_active_tab_mut(CLIENT).unwrap().close_pane(
            PaneId::Terminal(TerminalId::new(terminal)),
            false,
            None,
        );
        assert_eq!(
            render_osc7(&mut screen, &receiver),
            [(CLIENT, uri.to_owned())]
        );
    }
}

#[test]
fn osc7_is_not_forwarded_to_watchers() {
    let (mut screen, receiver) = screen_with_two_panes();
    screen.add_watcher_client(OTHER_CLIENT).unwrap();
    screen.add_client(OTHER_CLIENT).unwrap();
    screen.set_watcher_size(OTHER_CLIENT, SIZE);
    screen.set_followed_client(CLIENT).unwrap();
    emit_osc7(&mut screen, 1, "file://host/focused");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/focused".to_owned())]
    );
    assert!(!screen.last_forwarded_osc7.contains_key(&OTHER_CLIENT));
    assert!(!screen.watcher_clients[&OTHER_CLIENT].should_force_render());
}

#[test]
fn osc7_forgets_a_departed_clients_report_before_its_id_is_reused() {
    let (mut screen, receiver) = screen_with_two_panes();
    screen.set_client_size(OTHER_CLIENT, SIZE).unwrap();
    screen.add_client(OTHER_CLIENT).unwrap();
    emit_osc7(&mut screen, 1, "file://host/focused");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [
            (CLIENT, "file://host/focused".to_owned()),
            (OTHER_CLIENT, "file://host/focused".to_owned()),
        ]
    );
    screen.remove_client(OTHER_CLIENT).unwrap();
    assert!(!screen.last_forwarded_osc7.contains_key(&OTHER_CLIENT));
    screen.set_client_size(OTHER_CLIENT, SIZE).unwrap();
    screen.add_client(OTHER_CLIENT).unwrap();
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(OTHER_CLIENT, "file://host/focused".to_owned())]
    );
}

#[test]
fn osc7_is_repeated_after_a_terminal_reset() {
    let (mut screen, receiver) = screen_with_two_panes();
    emit_osc7(&mut screen, 1, "file://host/focused");
    render_osc7(&mut screen, &receiver);
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"\x1bc".to_vec())
        .unwrap();
    assert!(render_osc7(&mut screen, &receiver).is_empty());
    emit_osc7(&mut screen, 1, "file://host/focused");
    assert_eq!(
        render_osc7(&mut screen, &receiver),
        [(CLIENT, "file://host/focused".to_owned())]
    );
}
