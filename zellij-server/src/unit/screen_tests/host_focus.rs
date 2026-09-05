//! Host terminal focus (DECSET 1004) reaching the pane the client is looking at.

use super::*;

fn subscribe_pane_to_focus_events(screen: &mut Screen, client_id: ClientId, terminal_id: u32) {
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .handle_pty_bytes(
            crate::TerminalId(terminal_id),
            Vec::from("\u{1b}[?1004h".as_bytes()),
        )
        .unwrap();
}

fn focus_events_written_to_pane(tty_stdin_bytes: &TtyStdinBytes, terminal_id: u32) -> String {
    tty_stdin_bytes
        .lock()
        .unwrap()
        .get(&TerminalId(terminal_id))
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default()
}

#[test]
fn host_focus_changes_are_forwarded_to_the_active_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    subscribe_pane_to_focus_events(&mut screen, client_id, 1);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(client_id, false);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O",
        "the active pane is told the host lost focus"
    );

    screen.host_terminal_focus_changed(client_id, true);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O\u{1b}[I",
        "the active pane is told the host regained focus"
    );
}

#[test]
fn repeated_host_focus_reports_are_not_forwarded_twice() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    subscribe_pane_to_focus_events(&mut screen, client_id, 1);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(client_id, false);
    screen.host_terminal_focus_changed(client_id, false);
    screen.host_terminal_focus_changed(client_id, true);
    screen.host_terminal_focus_changed(client_id, true);

    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O\u{1b}[I",
        "only transitions are forwarded"
    );
}

#[test]
fn host_focus_is_not_forwarded_to_a_pane_that_did_not_subscribe() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(client_id, false);
    screen.host_terminal_focus_changed(client_id, true);

    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "",
        "a pane that did not enable focus tracking gets nothing"
    );
}

#[test]
fn host_focus_loss_is_withheld_while_another_client_is_still_focused() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let first_client_id = ClientId(1);
    let second_client_id = ClientId(2);
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    screen.set_client_size(second_client_id, size);
    screen.add_client(second_client_id).unwrap();
    subscribe_pane_to_focus_events(&mut screen, first_client_id, 1);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(first_client_id, false);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "",
        "the pane is still focused by the second client"
    );

    screen.host_terminal_focus_changed(second_client_id, false);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O",
        "the pane loses focus once no client is focused on it"
    );

    screen.host_terminal_focus_changed(first_client_id, true);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O\u{1b}[I",
        "the pane regains focus with the first client"
    );

    screen.host_terminal_focus_changed(second_client_id, true);
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "\u{1b}[O\u{1b}[I",
        "the pane is already focused, the second client changes nothing"
    );
}

#[test]
fn host_focus_changes_of_clients_on_different_panes_are_independent() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let first_client_id = ClientId(1);
    let second_client_id = ClientId(2);
    let second_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, false);
    new_tab(&mut screen, 1, 0);
    screen.set_client_size(second_client_id, size);
    screen.add_client(second_client_id).unwrap();
    {
        let active_tab = screen.get_active_tab_mut(first_client_id).unwrap();
        active_tab
            .horizontal_split(second_pane_id, None, first_client_id, None, None)
            .unwrap();
        active_tab.move_focus_up(first_client_id).unwrap();
    }
    screen
        .get_active_tab_mut(second_client_id)
        .unwrap()
        .move_focus_down(second_client_id)
        .unwrap();
    subscribe_pane_to_focus_events(&mut screen, first_client_id, 1);
    subscribe_pane_to_focus_events(&mut screen, first_client_id, 2);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(second_client_id, false);

    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "",
        "the pane of the still-focused client is untouched"
    );
    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 2),
        "\u{1b}[O",
        "only the pane of the unfocused client is told"
    );
}

#[test]
fn a_departing_client_leaves_no_host_focus_state_behind() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, _tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    screen.set_client_size(crate::ClientId(2), size);
    screen.add_client(crate::ClientId(2)).unwrap();
    screen.host_terminal_focus_changed(crate::ClientId(2), false);
    assert!(screen.client_host_focused.contains_key(&ClientId(2)));

    screen.remove_client(crate::ClientId(2)).unwrap();

    assert!(
        !screen.client_host_focused.contains_key(&ClientId(2)),
        "focus state is forgotten"
    );
}

#[test]
fn a_client_whose_host_focus_was_never_reported_counts_as_focused() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    subscribe_pane_to_focus_events(&mut screen, crate::ClientId(1), 1);
    tty_stdin_bytes.lock().unwrap().clear();

    screen.host_terminal_focus_changed(crate::ClientId(1), true);

    assert_eq!(
        focus_events_written_to_pane(&tty_stdin_bytes, 1),
        "",
        "a client is assumed focused until told otherwise, so this is not a transition"
    );
}
