use super::*;

#[test]
fn osc99_notification_forwarded_through_tab_to_server_render() {
    // Integration test: OSC 99 sequence emitted by a pane's PTY is parsed by Grid,
    // drained by Tab and handed to the screen, which namespaces it per host.
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    // Feed OSC 99 sequence to pane (pid=1) — BEL terminated
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;i=test1:p=title;Hello World\x07"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    // The forwarded output should contain a namespaced OSC 99 sequence
    assert!(
        output.contains("\x1b]99;"),
        "Render output should contain OSC 99 prefix, got: {:?}",
        output
    );
    // The identifier should be namespaced with pane ID (pid=1)
    assert!(
        output.contains("i=p1.test1"),
        "Identifier should be namespaced as i=p1.test1, got: {:?}",
        output
    );
    // The payload should be preserved
    assert!(
        output.contains("Hello World"),
        "Payload 'Hello World' should be forwarded, got: {:?}",
        output
    );
}

#[test]
fn osc99_notification_st_terminator_forwarded() {
    // Same as above but with ESC \ (ST) terminator
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;i=test2:p=title;Build done\x1b\\"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert!(
        output.contains("i=p1.test2"),
        "Identifier should be namespaced, got: {:?}",
        output
    );
    assert!(
        output.contains("Build done"),
        "Payload should be forwarded, got: {:?}",
        output
    );
}

#[test]
fn osc99_notification_without_identifier_gets_default() {
    // When no i= key is present, namespace_notification_id adds i=p<N>. with an empty id
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;p=title;No ID here\x07"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert!(
        output.contains("i=p1.:"),
        "Missing i= should get default i=p1. (empty original id), got: {:?}",
        output
    );
    assert!(
        output.contains("No ID here"),
        "Payload should be forwarded, got: {:?}",
        output
    );
}

#[test]
fn osc99_multiple_notifications_forwarded() {
    // Multiple OSC 99 sequences in one PTY write should all be forwarded
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;i=n1:p=title;First\x07\x1b]99;i=n2:p=title;Second\x07"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert!(
        output.contains("i=p1.n1"),
        "First notification should be namespaced, got: {:?}",
        output
    );
    assert!(
        output.contains("i=p1.n2"),
        "Second notification should be namespaced, got: {:?}",
        output
    );
    assert!(
        output.contains("First") && output.contains("Second"),
        "Both payloads should be forwarded, got: {:?}",
        output
    );
}

#[test]
fn osc99_chunks_of_one_notification_share_an_identifier() {
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;i=chunked:a=report:d=0;Hello\x07\x1b]99;i=chunked:p=body;World\x07"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert_eq!(
        output.matches("i=p1r.chunked").count(),
        2,
        "both chunks address the same notification, got: {:?}",
        output
    );
}

#[test]
fn osc99_notification_mixed_with_regular_output() {
    // OSC 99 embedded in regular terminal output should be extracted and forwarded
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("hello\x1b]99;i=mid:p=title;Inline notification\x07world"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert!(
        output.contains("i=p1.mid"),
        "Notification should be forwarded, got: {:?}",
        output
    );
    assert!(
        output.contains("Inline notification"),
        "Notification payload should be present, got: {:?}",
        output
    );
}

#[test]
fn osc99_notification_preserves_metadata_keys() {
    // All metadata keys (p=, a=, u=, etc.) should be preserved through forwarding
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\x1b]99;i=rich:p=title:a=report:u=2;Important!\x07"),
    )
    .unwrap();

    let output = collect_forwarded_osc99(&screen_receiver);

    assert!(
        output.contains("i=p1r.rich"),
        "Identifier should be namespaced with 'r' flag (a=report present), got: {:?}",
        output
    );
    assert!(
        output.contains("p=title"),
        "p=title metadata should be preserved, got: {:?}",
        output
    );
    assert!(
        output.contains("a=report"),
        "a=report metadata should be preserved, got: {:?}",
        output
    );
    assert!(
        output.contains("u=2"),
        "u=2 metadata should be preserved, got: {:?}",
        output
    );
}

#[test]
fn hidden_cursor_still_emits_cup_for_host_terminal_positioning() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[10;20H\u{1b}[?25l".as_bytes()),
    )
    .unwrap();

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let serialized = output.serialize().unwrap();
    let client_output = serialized.get(&client_id).unwrap();

    assert!(
        client_output.contains("\u{1b}[?25l"),
        "Expected hide-cursor sequence in output"
    );

    assert!(
        client_output.contains("\u{1b}[11;21H"),
        "Expected CUP sequence for hidden cursor positioning, got: {:?}",
        client_output
    );

    assert!(
        !client_output.contains("\u{1b}[?25h"),
        "Show-cursor sequence must not be present when app has hidden the cursor"
    );
}

#[hegel::test(test_cases = 300)]
fn suppressing_panes_never_loses_one(tc: hegel::TestCase) {
    hegel::stateful::run(SuppressedPaneModel::new(), tc);
}
