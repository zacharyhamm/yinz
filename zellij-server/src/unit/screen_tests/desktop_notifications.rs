//! Desktop notifications from panes, rendered per client in the protocol its host speaks.

use super::*;
use crate::notifications::NotificationProtocol;
use crate::panes::grid::PendingNotification;
use zellij_types::input::options::HostNotificationProtocol;

fn collect_forwarded_notifications(server_receiver: &ServerReceiver) -> String {
    let mut output = String::new();
    while let Ok((instruction, _)) = server_receiver.try_recv() {
        if let ServerInstruction::Render(Some(client_map)) = instruction {
            for (_client_id, content) in client_map {
                output.push_str(&content);
            }
        }
    }
    output
}

fn screen_with_a_client_for_notifications(
    host_notification_protocol: HostNotificationProtocol,
    host_terminal_env: BTreeMap<String, String>,
) -> (Screen, ServerReceiver) {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, _tty_stdin_bytes, server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    screen.set_host_notification_protocol(host_notification_protocol);
    screen.set_client_host_terminal_env(crate::ClientId::new(1), host_terminal_env);
    while server_receiver.try_recv().is_ok() {}
    (screen, server_receiver)
}

fn kitty_env() -> BTreeMap<String, String> {
    [("TERM".to_owned(), "xterm-kitty".to_owned())]
        .into_iter()
        .collect()
}

#[test]
fn an_osc_9_notification_is_translated_for_an_osc_99_host() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, kitty_env());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned(),
        }],
        crate::TerminalId::new(1),
    );

    assert!(
        collect_forwarded_notifications(&server_receiver)
            .contains("\u{1b}]99;;the build finished\u{7}"),
        "an OSC 9 notification is re-rendered in the protocol the host speaks"
    );
}

#[test]
fn an_osc_99_notification_reaching_an_osc_9_host_is_translated_down() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, BTreeMap::new());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc99 {
            payload: "i=1;the build finished".to_owned(),
            terminator: "\u{7}".to_owned(),
            wants_report: false,
            display: Some(("the build finished".to_owned(), String::new())),
        }],
        crate::TerminalId::new(1),
    );

    assert!(
        collect_forwarded_notifications(&server_receiver)
            .contains("\u{1b}]9;the build finished\u{7}"),
        "an unrecognized host is spoken to in the legacy protocol"
    );
}

#[test]
fn an_osc_99_request_with_nothing_to_show_is_not_sent_to_an_osc_9_host() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, BTreeMap::new());

    screen.forward_desktop_notifications(
        vec![
            PendingNotification::Osc99 {
                payload: "i=1:d=0;the build".to_owned(),
                terminator: "\u{7}".to_owned(),
                wants_report: false,
                display: None,
            },
            PendingNotification::Osc99 {
                payload: "i=1:p=close;".to_owned(),
                terminator: "\u{7}".to_owned(),
                wants_report: false,
                display: None,
            },
        ],
        crate::TerminalId::new(1),
    );

    assert_eq!(
        collect_forwarded_notifications(&server_receiver),
        "",
        "unfinished chunks and closes have no legacy equivalent to send"
    );
}

#[test]
fn an_osc_99_notification_reaching_an_osc_99_host_keeps_its_namespaced_identifier() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, kitty_env());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc99 {
            payload: "i=myid;the build finished".to_owned(),
            terminator: "\u{7}".to_owned(),
            wants_report: false,
            display: Some(("the build finished".to_owned(), String::new())),
        }],
        crate::TerminalId::new(7),
    );

    let output = collect_forwarded_notifications(&server_receiver);
    assert!(
        output.contains("i=p7.myid") && output.contains("the build finished"),
        "the identifier is namespaced with the pane id, got: {:?}",
        output
    );
}

#[test]
fn the_configured_protocol_overrides_host_detection() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Osc9, kitty_env());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned(),
        }],
        crate::TerminalId::new(1),
    );

    let output = collect_forwarded_notifications(&server_receiver);
    assert!(
        output.contains("\u{1b}]9;the build finished\u{7}") && !output.contains("\u{1b}]99;"),
        "a kitty host configured to osc9 is spoken to in osc9, got: {:?}",
        output
    );
}

#[test]
fn a_host_configured_to_bell_gets_a_bell_instead_of_a_notification() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Bell, kitty_env());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc777 {
            title: "the title".to_owned(),
            body: "the body".to_owned(),
        }],
        crate::TerminalId::new(1),
    );

    let output = collect_forwarded_notifications(&server_receiver);
    assert!(
        output.contains("\u{7}") && !output.contains("\u{1b}]"),
        "a bell is rung instead of a notification being sent, got: {:?}",
        output
    );
}

#[test]
fn a_host_configured_to_off_is_not_sent_anything() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Off, kitty_env());

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned(),
        }],
        crate::TerminalId::new(1),
    );

    assert_eq!(
        collect_forwarded_notifications(&server_receiver),
        "",
        "notifications are suppressed entirely"
    );
}

#[test]
fn changing_the_configured_protocol_at_runtime_re_resolves_connected_clients() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, kitty_env());
    assert_eq!(
        screen.notification_protocol_for_client(&ClientId::new(1)),
        NotificationProtocol::Osc99,
        "the kitty host is detected on connect"
    );

    screen.set_host_notification_protocol(HostNotificationProtocol::Bell);
    assert_eq!(
        screen.notification_protocol_for_client(&ClientId::new(1)),
        NotificationProtocol::Bell,
        "an already connected client picks up the new configuration"
    );

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned(),
        }],
        crate::TerminalId::new(1),
    );
    let output = collect_forwarded_notifications(&server_receiver);
    assert!(
        output.contains("\u{7}") && !output.contains("\u{1b}]"),
        "the reconfigured protocol is the one actually spoken, got: {:?}",
        output
    );

    screen.set_host_notification_protocol(HostNotificationProtocol::Auto);
    assert_eq!(
        screen.notification_protocol_for_client(&ClientId::new(1)),
        NotificationProtocol::Osc99,
        "returning to auto restores detection from the stored host environment"
    );
}

#[test]
fn each_client_is_spoken_to_in_the_protocol_of_its_own_host() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, _tty_stdin_bytes, server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    screen.set_client_size(crate::ClientId::new(2), size);
    screen.add_client(crate::ClientId::new(2)).unwrap();
    screen.set_client_host_terminal_env(crate::ClientId::new(1), kitty_env());
    screen.set_client_host_terminal_env(crate::ClientId::new(2), BTreeMap::new());
    while server_receiver.try_recv().is_ok() {}

    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned(),
        }],
        crate::TerminalId::new(1),
    );

    let output = collect_forwarded_notifications(&server_receiver);
    assert!(
        output.contains("\u{1b}]99;;the build finished\u{7}"),
        "the kitty client gets the kitty protocol, got: {:?}",
        output
    );
    assert!(
        output.contains("\u{1b}]9;the build finished\u{7}"),
        "the other client gets the legacy protocol, got: {:?}",
        output
    );
}

#[test]
fn a_client_that_never_reported_its_host_env_gets_the_configured_protocol() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, _tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.notification_protocol_for_client(&ClientId::new(1)),
        NotificationProtocol::Osc9,
        "an unknown host defaults to the legacy protocol"
    );

    screen.set_host_notification_protocol(HostNotificationProtocol::Off);
    assert_eq!(
        screen.notification_protocol_for_client(&ClientId::new(1)),
        NotificationProtocol::Off,
        "the configured protocol still applies without a reported host env"
    );
}

#[test]
fn a_departing_client_leaves_no_host_notification_state_behind() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, _tty_stdin_bytes, _server_receiver) =
        create_new_screen_with_capture(size, true, true, true, true);
    new_tab(&mut screen, 1, 0);
    screen.set_client_size(crate::ClientId::new(2), size);
    screen.add_client(crate::ClientId::new(2)).unwrap();
    screen.set_client_host_terminal_env(crate::ClientId::new(2), kitty_env());
    assert!(screen
        .client_notification_protocols
        .contains_key(&ClientId::new(2)));
    assert!(screen
        .client_host_terminal_env
        .contains_key(&ClientId::new(2)));

    screen.remove_client(crate::ClientId::new(2)).unwrap();

    assert!(
        !screen
            .client_notification_protocols
            .contains_key(&ClientId::new(2)),
        "the resolved notification protocol is forgotten"
    );
    assert!(
        !screen
            .client_host_terminal_env
            .contains_key(&ClientId::new(2)),
        "the reported host environment is forgotten"
    );
}
