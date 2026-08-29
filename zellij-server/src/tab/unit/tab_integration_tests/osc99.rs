use super::*;

#[test]
fn osc99_notification_forwarded_through_tab_to_server_render() {
    // Integration test: OSC 99 sequence emitted by a pane's PTY is parsed by Grid,
    // drained by Tab and handed to the screen, which namespaces it per host.
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    // Feed OSC 99 sequence to pane (pid=1) — BEL terminated
    tab.handle_pty_bytes(1, Vec::from("\x1b]99;i=test1:p=title;Hello World\x07"))
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

    tab.handle_pty_bytes(1, Vec::from("\x1b]99;i=test2:p=title;Build done\x1b\\"))
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
    // When no i= key is present, namespace_notification_id adds i=p<N>.0
    let size = Size { cols: 80, rows: 24 };
    let (mut tab, screen_receiver) = create_new_tab_with_screen_receiver(size, ModeInfo::default());

    tab.handle_pty_bytes(1, Vec::from("\x1b]99;p=title;No ID here\x07"))
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
        1,
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
        1,
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
        1,
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
        1,
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
fn osc99_grid_parses_and_stores_notification() {
    // Direct Grid-level test: feed OSC 99 bytes through vte parser,
    // verify pending_desktop_notifications is populated correctly.
    use crate::panes::grid::Grid;
    use crate::panes::link_handler::LinkHandler;

    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));

    let mut grid = Grid::new(
        24,
        80,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false, // debug
        true,  // arrow_fonts
        true,  // styled_underlines
        true,  // osc8_hyperlinks
        false, // explicitly_disable_kitty_keyboard_protocol
    );

    // Feed OSC 99 through vte parser
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(
        &mut grid,
        b"\x1b]99;i=gridtest:p=title;Grid notification\x07",
    );

    assert_eq!(
        grid.pending_desktop_notifications.len(),
        1,
        "Should have one pending notification"
    );

    let notification = grid.pending_desktop_notifications.first().unwrap();
    let payload = match notification {
        crate::panes::grid::PendingNotification::Osc99 { payload, .. } => payload.clone(),
        other => panic!("Expected an OSC 99 notification, got: {:?}", other),
    };
    assert!(
        payload.contains("i=gridtest"),
        "Payload should contain i=gridtest, got: {:?}",
        payload
    );
}

#[test]
fn osc99_namespace_denormalize_roundtrip() {
    // Test that namespace_notification_id and denormalize_notification_response
    // are inverse operations.
    use crate::panes::grid::namespace_notification_id;
    use crate::screen::denormalize_notification_response;

    let original_metadata = "i=mynotif:p=title:a=report";
    let pane_id: u32 = 42;

    // Namespace — a=report means 'r' flag is set
    let namespaced = namespace_notification_id(original_metadata, pane_id, true);
    assert!(
        namespaced.contains("i=p42r.mynotif"),
        "Should namespace to i=p42r.mynotif (a=report → 'r' flag), got: {:?}",
        namespaced
    );

    // Simulate a response with the namespaced ID
    let response_payload = "i=p42r.mynotif;activated".to_string();
    let result = denormalize_notification_response(response_payload.as_bytes());
    assert!(result.is_some(), "Should successfully denormalize");

    let response = result.unwrap();
    assert_eq!(response.terminal_id, 42, "Should extract pane_id 42");
    assert!(
        response.forward_to_pane,
        "'r' flag in i=p42r.mynotif means the response goes back to the app"
    );
    assert!(
        response.focus_pane,
        "an activation report focuses the originating pane"
    );

    let response_str = String::from_utf8_lossy(&response.bytes);
    assert!(
        response_str.contains("i=mynotif"),
        "Should restore original i=mynotif, got: {:?}",
        response_str
    );
    assert!(
        !response_str.contains("p42"),
        "Should not contain namespaced prefix, got: {:?}",
        response_str
    );
}

#[test]
fn osc99_denormalize_with_no_namespace_returns_none() {
    use crate::screen::denormalize_notification_response;

    // A response without the p<N>. namespace pattern should return None
    let result = denormalize_notification_response(b"i=plain_id;data");
    assert!(
        result.is_none(),
        "Should return None for non-namespaced response"
    );
}

#[test]
fn osc99_namespace_without_identifier_adds_default() {
    use crate::panes::grid::namespace_notification_id;

    // With a=report → 'r' flag
    let metadata = "p=title:a=report";
    let namespaced = namespace_notification_id(metadata, 7, true);
    assert!(
        namespaced.contains("i=p7r.:"),
        "Should add default i=p7r. when no i= present (a=report → 'r' flag), got: {:?}",
        namespaced
    );

    // Without a=report → no 'r' flag
    let metadata = "p=title";
    let namespaced = namespace_notification_id(metadata, 7, false);
    assert!(
        namespaced.contains("i=p7.:"),
        "Should add default i=p7. when no i= and no a=report, got: {:?}",
        namespaced
    );
}

#[test]
fn osc99_namespace_does_not_leave_empty_metadata_entries() {
    use crate::panes::grid::namespace_notification_id;

    let namespaced = namespace_notification_id("", 1, false);
    assert_eq!(
        namespaced, "i=p1.:a=focus,report",
        "an empty metadata section produces no empty key=value entries"
    );
}

#[test]
fn osc99_namespace_is_stable_across_the_escapes_of_one_notification() {
    use crate::panes::grid::namespace_notification_id;

    let notification = namespace_notification_id("i=myid:a=report", 4, true);
    let close = namespace_notification_id("i=myid:p=close", 4, true);
    assert!(
        notification.contains("i=p4r.myid") && close.contains("i=p4r.myid"),
        "a close request addresses the same identifier the notification got, got: {:?} and {:?}",
        notification,
        close
    );
    assert!(
        !close.contains("a="),
        "a close request is not turned into something the host reports on, got: {:?}",
        close
    );
}

#[test]
fn osc99_namespace_ensures_report_action() {
    use crate::panes::grid::namespace_notification_id;

    // a=focus → a=focus,report
    let result = namespace_notification_id("i=test:p=title:a=focus", 1, false);
    assert!(
        result.contains("a=focus,report"),
        "a=focus should be augmented with report, got: {:?}",
        result
    );

    // a=report → unchanged
    let result = namespace_notification_id("i=test:p=title:a=report", 1, true);
    assert!(
        result.contains("a=report"),
        "a=report should be preserved, got: {:?}",
        result
    );
    assert!(
        !result.contains("a=report,report"),
        "Should not duplicate report, got: {:?}",
        result
    );

    // a=focus,report → unchanged
    let result = namespace_notification_id("i=test:p=title:a=focus,report", 1, true);
    assert!(
        result.contains("a=focus,report"),
        "a=focus,report should be preserved, got: {:?}",
        result
    );

    let result = namespace_notification_id("i=test:p=title", 1, false);
    assert!(
        result.contains("a=focus,report"),
        "Missing a= should keep the protocol's default focus action, got: {:?}",
        result
    );
}

#[test]
fn osc99_report_flag_roundtrip() {
    use crate::panes::grid::namespace_notification_id;
    use crate::screen::denormalize_notification_response;

    // App sends a=report → namespaced with 'r' flag → denormalize returns app_wants_report=true
    let namespaced = namespace_notification_id("i=myid:p=title:a=report", 5, true);
    assert!(
        namespaced.contains("i=p5r.myid"),
        "a=report should produce 'r' flag in namespace, got: {:?}",
        namespaced
    );
    let response = "i=p5r.myid;activated".to_string();
    let response = denormalize_notification_response(response.as_bytes()).unwrap();
    assert_eq!(response.terminal_id, 5);
    assert!(
        response.forward_to_pane,
        "Should detect 'r' flag and hand the report to the app"
    );

    // App sends a=focus (no report) → namespaced without 'r' flag → denormalize returns false
    let namespaced = namespace_notification_id("i=myid:p=title:a=focus", 5, false);
    assert!(
        namespaced.contains("i=p5.myid"),
        "a=focus should NOT produce 'r' flag, got: {:?}",
        namespaced
    );
    assert!(
        namespaced.contains("a=focus,report"),
        "a=focus should be augmented with report for the host terminal, got: {:?}",
        namespaced
    );
    let response = "i=p5.myid;activated".to_string();
    let response = denormalize_notification_response(response.as_bytes()).unwrap();
    assert_eq!(response.terminal_id, 5);
    assert!(
        !response.forward_to_pane,
        "No 'r' flag means the app is not handed a report it never asked for"
    );
    assert!(
        response.focus_pane,
        "the pane is still focused when the notification is clicked"
    );
}

#[test]
fn osc99_query_response_is_handed_to_the_pane_without_focusing_it() {
    use crate::panes::grid::namespace_notification_id;
    use crate::screen::denormalize_notification_response;

    let namespaced = namespace_notification_id("i=qid:p=?", 3, false);
    assert_eq!(
        namespaced, "i=p3.qid:p=?",
        "a query is namespaced and otherwise left alone"
    );
    let response = "i=p3.qid:p=?;a=focus,report:p=title,body".to_string();
    let response = denormalize_notification_response(response.as_bytes()).unwrap();
    assert_eq!(response.terminal_id, 3);
    assert!(
        response.forward_to_pane,
        "the app is waiting for the answer to its query"
    );
    assert!(
        !response.focus_pane,
        "answering a query is not the user clicking a notification"
    );
    assert!(
        String::from_utf8_lossy(&response.bytes).contains("i=qid:p=?"),
        "the app's own identifier is restored, got: {:?}",
        String::from_utf8_lossy(&response.bytes)
    );
}

#[test]
fn osc99_close_report_does_not_steal_focus() {
    use crate::screen::denormalize_notification_response;

    let response = denormalize_notification_response(b"i=p3.myid:p=close;").unwrap();
    assert_eq!(response.terminal_id, 3);
    assert!(
        response.forward_to_pane,
        "close events only arrive when the app asked for them with c=1"
    );
    assert!(
        !response.focus_pane,
        "dismissing a notification is not activating it"
    );
}

#[test]
fn osc99_liveness_answer_is_restricted_to_the_asking_pane() {
    use crate::screen::denormalize_notification_response;

    let response =
        denormalize_notification_response(b"i=p3.myid:p=alive;p3.one,p4r.other,p3r.two").unwrap();
    let restored = String::from_utf8_lossy(&response.bytes).to_string();
    assert!(
        restored.contains(";one,two"),
        "the pane's own identifiers are restored, got: {:?}",
        restored
    );
    assert!(
        !restored.contains("other"),
        "identifiers belonging to other panes are not disclosed, got: {:?}",
        restored
    );
}

#[test]
fn osc99_response_to_an_unidentified_notification_uses_the_protocol_default() {
    use crate::screen::denormalize_notification_response;

    let response = denormalize_notification_response(b"i=p3r.;activated").unwrap();
    assert!(
        String::from_utf8_lossy(&response.bytes).contains("i=0"),
        "an app that sent no identifier is answered with i=0, got: {:?}",
        String::from_utf8_lossy(&response.bytes)
    );
}

#[test]
fn hidden_cursor_still_emits_cup_for_host_terminal_positioning() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());

    tab.handle_pty_bytes(1, Vec::from("\u{1b}[10;20H\u{1b}[?25l".as_bytes()))
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
