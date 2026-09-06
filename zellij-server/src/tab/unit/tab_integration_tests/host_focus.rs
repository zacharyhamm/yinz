//! Host terminal focus events written to a pane's pty.

use super::*;

#[test]
fn host_focus_events_only_reach_panes_that_asked_for_them() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));

    tab.send_host_focus_event_to_pane(pane_id, false);
    assert!(
        tty_stdin_bytes.lock().unwrap().is_empty(),
        "a pane that did not subscribe to focus events receives nothing"
    );

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.send_host_focus_event_to_pane(pane_id, false);
    tab.send_host_focus_event_to_pane(pane_id, true);

    let written = tty_stdin_bytes
        .lock()
        .unwrap()
        .get(&TerminalId::new(1))
        .map(|bytes| String::from_utf8_lossy(bytes).to_string());
    assert_eq!(written, Some("\u{1b}[O\u{1b}[I".to_owned()));
}

#[test]
fn host_focus_events_are_not_sent_to_plugin_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);

    tab.send_host_focus_event_to_pane(PaneId::Plugin(crate::PluginId::new(1)), false);

    assert!(
        tty_stdin_bytes.lock().unwrap().is_empty(),
        "a plugin pane has no pty to write a focus event to"
    );
}
