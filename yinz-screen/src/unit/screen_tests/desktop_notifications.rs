//! Desktop notifications from panes, rendered per client in the protocol its host speaks.

use super::*;
use crate::notifications::NotificationProtocol;
use crate::panes::grid::PendingNotification;
use yinz_types::agent::report::{AgentReport, AgentReportState};
use yinz_types::agent::{
    hook_outcome, AgentEvent, AgentSource, AgentState, HookEvent, HookOutcome,
};
use yinz_types::input::options::HostNotificationProtocol;
use yinz_types::internal_ids::TabId;

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
    new_tab(&mut screen, 1, TabId::new(0));
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
fn encoded_notification_text_cannot_inject_a_host_clipboard_command() {
    let (mut screen, server_receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Osc9, BTreeMap::new());
    let pane = screen
        .get_active_tab_mut(ClientId::new(1))
        .unwrap()
        .get_active_pane_mut(ClientId::new(1))
        .unwrap();
    // Decodes to "done", BEL, an OSC 52 clipboard write, and BEL.
    pane.handle_pty_bytes(b"\x1b]99;e=1;ZG9uZQcbXTUyO2M7WVdKagc=\x07".to_vec());
    let notifications = pane.drain_desktop_notifications();
    assert_eq!(notifications.len(), 1);
    screen.forward_desktop_notifications(notifications, TerminalId::new(1));

    let output = collect_forwarded_notifications(&server_receiver);
    assert!(output.contains("\x1b]9;done]52;c;YWJj\x07"), "{output:?}");
    assert!(!output.contains("\x1b]52;"), "{output:?}");
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
    new_tab(&mut screen, 1, TabId::new(0));
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
    new_tab(&mut screen, 1, TabId::new(0));

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
    new_tab(&mut screen, 1, TabId::new(0));
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

const PLAN_PROMPT: &str = "Plan mode prompt: Implement this plan?";

fn agent_events(receiver: &Receiver<(PluginInstruction, ErrorContext)>) -> Vec<AgentEvent> {
    receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(events) => events
                .into_iter()
                .filter_map(|(_, _, event)| {
                    if let Event::AgentEvent(event) = event {
                        Some(event)
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn codex_plan_notifications_preserve_metadata_and_report_once() {
    for bytes in [
        format!("\x1b]9;{PLAN_PROMPT}\x07"),
        format!("\x1b]777;notify;{PLAN_PROMPT};Codex\x07"),
        format!("\x1b]99;i=plan:d=0;Codex\x07\x1b]99;i=plan:p=body;{PLAN_PROMPT}\x07"),
    ] {
        let (mut screen, server_receiver) =
            screen_with_a_client_for_notifications(HostNotificationProtocol::Osc9, BTreeMap::new());
        let terminal = TerminalId::new(7);
        let pane_id = PaneId::Terminal(terminal);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .new_pane(
                pane_id,
                None,
                Some(Run::Command(RunCommand {
                    command: PathBuf::from("codex"),
                    cwd: Some(PathBuf::from("/pane")),
                    ..Default::default()
                })),
                false,
                true,
                NewPanePlacement::default(),
                Some(ClientId::new(1)),
                None,
            )
            .unwrap();
        let hook = HookEvent {
            hook_event_name: "Stop".into(),
            session_id: "conversation".into(),
            cwd: Some("/conversation/worktree".into()),
            turn_id: Some("completed-turn".into()),
            ..Default::default()
        };
        let HookOutcome::Reported(mut agent) = hook_outcome("codex", &hook) else {
            panic!()
        };
        agent.run_id = Some("run".into());
        crate::screen::thread_main::apply_queued_agent_report(
            &mut screen,
            &AgentReport {
                report_id: "stop".into(),
                pane_id,
                state: AgentReportState::Set(agent),
                event: Some(AgentEvent::from_hook(
                    "codex",
                    terminal,
                    &hook,
                    serde_json::to_string(&hook).unwrap(),
                )),
                trace: false,
                process_id: ProcessId::new(42),
                kind: "codex".into(),
            },
            std::time::Instant::now(),
        )
        .unwrap();
        let (sender, receiver) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        let pane = screen
            .get_active_tab_mut(ClientId::new(1))
            .unwrap()
            .get_active_pane_mut(ClientId::new(1))
            .unwrap();
        assert_eq!(pane.pid(), pane_id);
        pane.handle_pty_bytes(bytes.into_bytes());
        let notifications = pane.drain_desktop_notifications();
        assert!(!notifications.is_empty());
        // Repeated prompts still forward, but only the first changes Idle to Waiting.
        screen.forward_desktop_notifications(notifications.clone(), terminal);
        screen.forward_desktop_notifications(notifications, terminal);
        let agent = screen.get_pane_info(pane_id).unwrap().agent.unwrap();
        assert!(agent.is_waiting_on_a_person());
        assert_eq!(agent.waiting_on.as_deref(), Some("approve the plan"));
        assert_eq!(agent.session_id.as_deref(), Some("conversation"));
        assert_eq!(agent.cwd.as_deref(), Some("/conversation/worktree"));
        assert_eq!(agent.run_id.as_deref(), Some("run"));
        let events = agent_events(&receiver);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].hook, "Elicitation");
        let hook: HookEvent = serde_json::from_str(&events[0].raw).unwrap();
        assert_eq!(hook.session_id, "conversation");
        assert_eq!(hook.cwd, agent.cwd);
        assert_eq!(hook.turn_id, None);
        assert!(collect_forwarded_notifications(&server_receiver).contains(PLAN_PROMPT));
    }
}

#[test]
fn codex_plan_notifications_ignore_unrelated_or_incomplete_text() {
    for notification in [
        PendingNotification::Osc9 {
            body: "Implement this plan?".into(),
        },
        PendingNotification::Osc9 {
            body: format!("prefix {PLAN_PROMPT}"),
        },
        PendingNotification::Osc777 {
            title: "Plan mode prompt:".into(),
            body: "Implement this plan?".into(),
        },
        PendingNotification::Osc99 {
            payload: format!("i=plan:d=0;{PLAN_PROMPT}"),
            terminator: "\x07".into(),
            wants_report: false,
            display: None,
        },
    ] {
        let (mut screen, _) =
            screen_with_a_client_for_notifications(HostNotificationProtocol::Osc9, BTreeMap::new());
        let pane = PaneId::Terminal(TerminalId::new(1));
        let HookOutcome::Reported(agent) = hook_outcome(
            "codex",
            &HookEvent {
                hook_event_name: "Stop".into(),
                session_id: "conversation".into(),
                ..Default::default()
            },
        ) else {
            panic!()
        };
        crate::screen::apply_pane_agent_report(&mut screen, pane, Some(agent.clone())).unwrap();
        let (sender, receiver) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        screen.forward_desktop_notifications(vec![notification], TerminalId::new(1));
        assert_eq!(screen.get_pane_info(pane).unwrap().agent, Some(agent));
        assert!(agent_events(&receiver).is_empty());
    }
}

#[test]
fn codex_plan_notifications_require_an_idle_reported_conversation() {
    for case in [
        "busy",
        "exited",
        "waiting",
        "missing-id",
        "empty-id",
        "claude",
        "spawned",
        "registered",
        "no-agent",
    ] {
        let (mut screen, _) =
            screen_with_a_client_for_notifications(HostNotificationProtocol::Osc9, BTreeMap::new());
        let pane = PaneId::Terminal(TerminalId::new(1));
        let HookOutcome::Reported(mut agent) = hook_outcome(
            "codex",
            &HookEvent {
                hook_event_name: "Stop".into(),
                session_id: "conversation".into(),
                ..Default::default()
            },
        ) else {
            panic!()
        };
        match case {
            "busy" => agent.state = AgentState::Busy,
            "exited" => agent.state = AgentState::Exited,
            "waiting" => agent.state = AgentState::Waiting,
            "missing-id" => agent.session_id = None,
            "empty-id" => agent.session_id = Some(String::new()),
            "claude" => agent.kind = "claude".into(),
            "spawned" => agent.source = AgentSource::Spawned,
            "registered" => agent.source = AgentSource::Registered,
            _ => {},
        }
        let agent = (case != "no-agent").then_some(agent);
        crate::screen::apply_pane_agent_report(&mut screen, pane, agent.clone()).unwrap();
        let (sender, receiver) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        screen.forward_desktop_notifications(
            vec![PendingNotification::Osc9 {
                body: PLAN_PROMPT.into(),
            }],
            TerminalId::new(1),
        );
        assert_eq!(screen.get_pane_info(pane).unwrap().agent, agent, "{case}");
        assert!(agent_events(&receiver).is_empty(), "{case}");
    }
}

#[test]
fn notifications_omit_recipients_with_no_terminal_output() {
    let (mut screen, receiver) =
        screen_with_a_client_for_notifications(HostNotificationProtocol::Auto, kitty_env());
    let quiet = ClientId::new(2);
    screen.add_client(quiet).unwrap();
    screen
        .client_notification_protocols
        .insert(quiet, NotificationProtocol::Off);
    screen.forward_desktop_notifications(
        vec![PendingNotification::Osc9 {
            body: "finished".into(),
        }],
        TerminalId::new(1),
    );
    let frames: Vec<_> = receiver
        .try_iter()
        .filter_map(|(instruction, _)| {
            if let ServerInstruction::Render(Some(frames)) = instruction {
                Some(frames)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 1);
    assert!(frames[0][&ClientId::new(1)].contains("finished"));
}
