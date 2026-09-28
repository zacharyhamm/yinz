use super::{
    denormalize_notification_response, namespace_notification_id, NotificationProtocol,
    PendingNotification,
};
use std::collections::BTreeMap;
use zellij_types::input::options::HostNotificationProtocol;
use zellij_types::internal_ids::TerminalId;

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_kitty_host_is_detected_from_its_term() {
    assert_eq!(
        NotificationProtocol::from_host_terminal_env(&env(&[("TERM", "xterm-kitty")])),
        NotificationProtocol::Osc99
    );
    // The match is exact, on purpose: `xterm-kitty` is the value kitty itself sets, and a
    // terminal that merely mentions kitty in a terminfo name is not known to render OSC 99,
    // where OSC 9 is the safe fallback. A real kitty behind another TERM still has
    // `KITTY_WINDOW_ID`, which the sibling test covers.
    for term in ["xterm-kitty-direct", "kitty", "xterm-kitty2"] {
        assert_eq!(
            NotificationProtocol::from_host_terminal_env(&env(&[("TERM", term)])),
            NotificationProtocol::Osc9,
            "TERM={term} is not an exact kitty match"
        );
    }
}

#[test]
fn a_kitty_host_is_detected_from_its_window_id() {
    assert_eq!(
        NotificationProtocol::from_host_terminal_env(&env(&[
            ("TERM", "xterm-256color"),
            ("KITTY_WINDOW_ID", "1")
        ])),
        NotificationProtocol::Osc99
    );
}

#[test]
fn an_unrecognized_host_falls_back_to_the_legacy_protocol() {
    assert_eq!(
        NotificationProtocol::from_host_terminal_env(&env(&[("TERM", "xterm-256color")])),
        NotificationProtocol::Osc9
    );
    assert_eq!(
        NotificationProtocol::from_host_terminal_env(&BTreeMap::new()),
        NotificationProtocol::Osc9
    );
}

#[test]
fn detection_is_used_when_the_protocol_is_configured_to_auto() {
    assert_eq!(
        NotificationProtocol::resolve(
            HostNotificationProtocol::Auto,
            &env(&[("TERM", "xterm-kitty")])
        ),
        NotificationProtocol::Osc99
    );
    assert_eq!(
        NotificationProtocol::resolve(
            HostNotificationProtocol::Auto,
            &env(&[("TERM", "xterm-256color")])
        ),
        NotificationProtocol::Osc9
    );
}

#[test]
fn an_explicitly_configured_protocol_overrides_detection() {
    let kitty = env(&[("TERM", "xterm-kitty")]);
    assert_eq!(
        NotificationProtocol::resolve(HostNotificationProtocol::Osc9, &kitty),
        NotificationProtocol::Osc9
    );
    assert_eq!(
        NotificationProtocol::resolve(HostNotificationProtocol::Bell, &kitty),
        NotificationProtocol::Bell
    );
    assert_eq!(
        NotificationProtocol::resolve(HostNotificationProtocol::Off, &kitty),
        NotificationProtocol::Off
    );
    assert_eq!(
        NotificationProtocol::resolve(
            HostNotificationProtocol::Osc99,
            &env(&[("TERM", "xterm-256color")])
        ),
        NotificationProtocol::Osc99
    );
}

#[test]
fn each_protocol_renders_its_own_escape_sequence() {
    assert_eq!(
        NotificationProtocol::Osc99.render("title", "body"),
        Some("\u{1b}]99;;title: body\u{7}".to_owned())
    );
    assert_eq!(
        NotificationProtocol::Osc9.render("title", "body"),
        Some("\u{1b}]9;title: body\u{7}".to_owned())
    );
    assert_eq!(
        NotificationProtocol::Bell.render("title", "body"),
        Some("\u{7}".to_owned())
    );
    assert_eq!(NotificationProtocol::Off.render("title", "body"), None);
}

#[test]
fn a_missing_title_or_body_is_not_padded_with_a_separator() {
    assert_eq!(
        NotificationProtocol::Osc9.render("", "body"),
        Some("\u{1b}]9;body\u{7}".to_owned())
    );
    assert_eq!(
        NotificationProtocol::Osc9.render("title", ""),
        Some("\u{1b}]9;title\u{7}".to_owned())
    );
}

#[test]
fn notification_text_cannot_escape_its_host_terminal_sequence() {
    let title = "build\u{7}\u{1b}]52;c;YWJj\u{1b}\\";
    let body = "finished\u{9c}\u{9b}2J\n✓";
    for (protocol, prefix) in [
        (NotificationProtocol::Osc9, "\u{1b}]9;"),
        (NotificationProtocol::Osc99, "\u{1b}]99;;"),
    ] {
        let output = protocol.render(title, body).unwrap();
        assert_eq!(
            output,
            format!("{prefix}build]52;c;YWJj\\: finished2J✓\u{7}")
        );
        let payload = output
            .strip_prefix(prefix)
            .unwrap()
            .strip_suffix('\u{7}')
            .unwrap();
        assert!(payload.chars().all(|c| !c.is_control()));
    }
}

#[test]
fn a_notification_carries_its_title_and_body_across_protocols() {
    assert_eq!(
        PendingNotification::Osc9 {
            body: "body".to_owned()
        }
        .title_and_body(),
        (String::new(), "body".to_owned())
    );
    assert_eq!(
        PendingNotification::Osc777 {
            title: "title".to_owned(),
            body: "body".to_owned()
        }
        .title_and_body(),
        ("title".to_owned(), "body".to_owned())
    );
    assert_eq!(
        PendingNotification::Osc99 {
            payload: "i=1;the title".to_owned(),
            terminator: "\u{7}".to_owned(),
            wants_report: false,
            display: Some(("the title".to_owned(), "the body".to_owned())),
        }
        .title_and_body(),
        ("the title".to_owned(), "the body".to_owned())
    );
}

// OSC 99 identifiers, namespaced per pane on the way out and restored on the host's answer.

#[test]
fn osc99_namespace_denormalize_roundtrip() {
    // Test that namespace_notification_id and denormalize_notification_response
    // are inverse operations.
    let original_metadata = "i=mynotif:p=title:a=report";
    let pane_id: u32 = 42;

    // Namespace — a=report means 'r' flag is set
    let namespaced = namespace_notification_id(original_metadata, TerminalId::new(pane_id), true);
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
    assert_eq!(
        response.terminal_id,
        TerminalId::new(42),
        "Should extract pane_id 42"
    );
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
    // A response without the p<N>. namespace pattern should return None
    let result = denormalize_notification_response(b"i=plain_id;data");
    assert!(
        result.is_none(),
        "Should return None for non-namespaced response"
    );
}

#[test]
fn osc99_namespace_without_identifier_adds_default() {
    // With a=report → 'r' flag
    let metadata = "p=title:a=report";
    let namespaced = namespace_notification_id(metadata, TerminalId::new(7), true);
    assert!(
        namespaced.contains("i=p7r.:"),
        "Should add default i=p7r. when no i= present (a=report → 'r' flag), got: {:?}",
        namespaced
    );

    // Without a=report → no 'r' flag
    let metadata = "p=title";
    let namespaced = namespace_notification_id(metadata, TerminalId::new(7), false);
    assert!(
        namespaced.contains("i=p7.:"),
        "Should add default i=p7. when no i= and no a=report, got: {:?}",
        namespaced
    );
}

#[test]
fn osc99_namespace_does_not_leave_empty_metadata_entries() {
    let namespaced = namespace_notification_id("", TerminalId::new(1), false);
    assert_eq!(
        namespaced, "i=p1.:a=focus,report",
        "an empty metadata section produces no empty key=value entries"
    );
}

#[test]
fn osc99_namespace_is_stable_across_the_escapes_of_one_notification() {
    let notification = namespace_notification_id("i=myid:a=report", TerminalId::new(4), true);
    let close = namespace_notification_id("i=myid:p=close", TerminalId::new(4), true);
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
    // a=focus → a=focus,report
    let result = namespace_notification_id("i=test:p=title:a=focus", TerminalId::new(1), false);
    assert!(
        result.contains("a=focus,report"),
        "a=focus should be augmented with report, got: {:?}",
        result
    );

    // a=report → unchanged
    let result = namespace_notification_id("i=test:p=title:a=report", TerminalId::new(1), true);
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
    let result =
        namespace_notification_id("i=test:p=title:a=focus,report", TerminalId::new(1), true);
    assert!(
        result.contains("a=focus,report"),
        "a=focus,report should be preserved, got: {:?}",
        result
    );

    let result = namespace_notification_id("i=test:p=title", TerminalId::new(1), false);
    assert!(
        result.contains("a=focus,report"),
        "Missing a= should keep the protocol's default focus action, got: {:?}",
        result
    );
}

#[test]
fn osc99_report_flag_roundtrip() {
    // App sends a=report → namespaced with 'r' flag → denormalize returns app_wants_report=true
    let namespaced = namespace_notification_id("i=myid:p=title:a=report", TerminalId::new(5), true);
    assert!(
        namespaced.contains("i=p5r.myid"),
        "a=report should produce 'r' flag in namespace, got: {:?}",
        namespaced
    );
    let response = "i=p5r.myid;activated".to_string();
    let response = denormalize_notification_response(response.as_bytes()).unwrap();
    assert_eq!(response.terminal_id, TerminalId::new(5));
    assert!(
        response.forward_to_pane,
        "Should detect 'r' flag and hand the report to the app"
    );

    // App sends a=focus (no report) → namespaced without 'r' flag → denormalize returns false
    let namespaced = namespace_notification_id("i=myid:p=title:a=focus", TerminalId::new(5), false);
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
    assert_eq!(response.terminal_id, TerminalId::new(5));
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
    let namespaced = namespace_notification_id("i=qid:p=?", TerminalId::new(3), false);
    assert_eq!(
        namespaced, "i=p3.qid:p=?",
        "a query is namespaced and otherwise left alone"
    );
    let response = "i=p3.qid:p=?;a=focus,report:p=title,body".to_string();
    let response = denormalize_notification_response(response.as_bytes()).unwrap();
    assert_eq!(response.terminal_id, TerminalId::new(3));
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
    let response = denormalize_notification_response(b"i=p3.myid:p=close;").unwrap();
    assert_eq!(response.terminal_id, TerminalId::new(3));
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
    let response = denormalize_notification_response(b"i=p3r.;activated").unwrap();
    assert!(
        String::from_utf8_lossy(&response.bytes).contains("i=0"),
        "an app that sent no identifier is answered with i=0, got: {:?}",
        String::from_utf8_lossy(&response.bytes)
    );
}
