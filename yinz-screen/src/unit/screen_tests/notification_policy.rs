use super::*;
use std::time::{Duration, Instant};
use yinz_bus::plugins::PluginInstanceId;
use yinz_host_protocol::notifications::PendingNotification;
use yinz_types::input::options::HostNotificationProtocol;
use yinz_types::internal_ids::PluginId;
use yinz_types::notifications::{NotificationDecision, NotificationDelivery, NotificationSource};

const POLICY: PluginId = PluginId::new(42);

fn policy_screen() -> (Screen, ServerReceiver, PluginReceiver, PluginInstanceId) {
    let (mut screen, _, server) = create_new_screen_with_capture(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    screen.set_host_notification_protocol(HostNotificationProtocol::Osc99);
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen.set_notification_policy(Some(POLICY), None).unwrap();
    while server.try_recv().is_ok() {}
    (screen, server, rx, PluginInstanceId::default())
}

fn events(receiver: &PluginReceiver) -> Vec<(Option<PluginId>, Event)> {
    let mut events = vec![];
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            events.extend(
                updates
                    .into_iter()
                    .map(|(plugin, _, event)| (plugin, event)),
            );
        }
    }
    events
}

fn desktop_output(receiver: &ServerReceiver) -> String {
    let mut output = String::new();
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let ServerInstruction::Render(Some(clients)) = instruction {
            for content in clients.into_values() {
                output.push_str(&content);
            }
        }
    }
    output
}

fn decide(
    screen: &mut Screen,
    id: u64,
    instance: PluginInstanceId,
    decision: NotificationDecision,
) {
    screen.bind_notification_request(NotificationId::new(id), POLICY, instance);
    screen
        .resolve_notification(NotificationId::new(id), decision, POLICY, instance)
        .unwrap();
}

fn osc(payload: &str, display: Option<(&str, &str)>) -> PendingNotification {
    PendingNotification::Osc99 {
        payload: payload.into(),
        terminator: "\x1b\\".into(),
        wants_report: true,
        display: display.map(|(title, body)| (title.into(), body.into())),
    }
}

#[test]
fn latest_activation_uses_committed_history_including_suppressed_entries() {
    let (mut screen, _server, _plugins, instance) = policy_screen();
    let client_id = ClientId::new(1);
    let first = PaneId::Terminal(TerminalId::new(1));
    let second = PaneId::Terminal(TerminalId::new(2));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .receive_toast("first".into(), Some(first), None, None, None)
        .unwrap();
    decide(&mut screen, 0, instance, NotificationDecision::Suppress);
    screen
        .receive_toast("pending".into(), Some(second), None, None, None)
        .unwrap();

    screen.activate_last_notification(client_id, Instant::now());
    assert_eq!(screen.get_active_pane_id(&client_id), Some(first));
    decide(&mut screen, 1, instance, NotificationDecision::Suppress);
    assert!(screen.notifications.is_empty());
    screen.activate_last_notification(client_id, Instant::now());
    assert_eq!(screen.get_active_pane_id(&client_id), Some(second));
}

#[test]
fn arrivals_are_observable_and_resolutions_commit_in_order_without_blocking_rendering() {
    let (mut screen, server, plugins, instance) = policy_screen();
    screen
        .receive_toast("first".into(), None, None, None, None)
        .unwrap();
    screen
        .receive_toast("second".into(), None, None, None, None)
        .unwrap();
    let arrivals = events(&plugins);
    assert!(
        matches!(&arrivals[0], (None, Event::NotificationReceived(n)) if n.id == NotificationId::new(0) && n.body == "first")
    );
    assert!(
        matches!(&arrivals[1], (Some(POLICY), Event::NotificationRequested(n)) if n.id == NotificationId::new(0))
    );
    decide(&mut screen, 1, instance, NotificationDecision::Suppress);
    assert!(screen.notification_history.is_empty());
    screen
        .get_active_tab_mut(ClientId::new(1))
        .unwrap()
        .get_active_pane_mut(ClientId::new(1))
        .unwrap()
        .handle_pty_bytes(b"screen remains responsive".to_vec());
    screen.render_to_clients().unwrap();
    assert!(
        desktop_output(&server).contains("screen remains responsive"),
        "screen continues rendering while the first decision is pending"
    );
    decide(
        &mut screen,
        0,
        instance,
        NotificationDecision::Replace {
            title: Some("rewritten".into()),
            body: None,
            delivery: Some(NotificationDelivery::Both),
        },
    );
    assert_eq!(
        screen
            .notification_history
            .iter()
            .map(|r| r.info.message.as_str())
            .collect::<Vec<_>>(),
        ["rewritten: first", "second"]
    );
    assert_eq!(screen.notifications.len(), 1);
    let output = desktop_output(&server);
    assert!(
        output.contains("i=yinz-notification-0")
            && output.contains("rewritten")
            && output.contains("first")
    );
    let resolved = events(&plugins)
        .into_iter()
        .filter_map(|(_, e)| {
            if let Event::NotificationResolved(n) = e {
                Some(n)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].original.body, "first");
    assert!(!resolved[0].suppressed);
    assert!(resolved[1].suppressed);
}

#[test]
fn forged_stale_duplicate_and_expired_replies_cannot_decide_notifications() {
    let (mut screen, _, _, instance) = policy_screen();
    screen
        .receive_toast("original".into(), None, None, None, None)
        .unwrap();
    screen.bind_notification_request(NotificationId::new(0), POLICY, instance);
    screen
        .resolve_notification(
            NotificationId::new(0),
            NotificationDecision::Suppress,
            PluginId::new(99),
            instance,
        )
        .unwrap();
    screen
        .resolve_notification(
            NotificationId::new(0),
            NotificationDecision::Suppress,
            POLICY,
            PluginInstanceId::default(),
        )
        .unwrap();
    assert!(screen.notification_history.is_empty());
    screen
        .expire_notification_requests(Instant::now() + Duration::from_secs(2))
        .unwrap();
    decide(&mut screen, 0, instance, NotificationDecision::Suppress);
    assert_eq!(screen.notification_history.len(), 1);
    let outcome = screen.notification_history[0]
        .info
        .outcome
        .as_ref()
        .unwrap();
    assert_eq!(outcome.fallback_reason.as_deref(), Some("policy_timeout"));
    assert!(!outcome.suppressed);
}

#[test]
fn all_delivery_choices_and_policy_origin_bypass_keep_metadata() {
    for delivery in [
        NotificationDelivery::Toast,
        NotificationDelivery::Desktop,
        NotificationDelivery::Both,
    ] {
        let (mut screen, server, plugins, instance) = policy_screen();
        screen
            .receive_toast(
                "original".into(),
                Some(PaneId::Terminal(TerminalId::new(1))),
                None,
                Some(PluginId::new(7)),
                None,
            )
            .unwrap();
        decide(
            &mut screen,
            0,
            instance,
            NotificationDecision::Replace {
                title: None,
                body: Some("final".into()),
                delivery: Some(delivery),
            },
        );
        assert_eq!(
            screen.notifications.len(),
            usize::from(delivery != NotificationDelivery::Desktop)
        );
        assert_eq!(
            desktop_output(&server).contains("i=yinz-notification-0"),
            delivery != NotificationDelivery::Toast
        );
        let info = &screen.notification_history[0].info;
        assert_eq!(
            info.outcome.as_ref().unwrap().original.plugin_id,
            Some(PluginId::new(7))
        );
        assert_eq!(
            info.outcome.as_ref().unwrap().original.source,
            NotificationSource::Toast
        );
        events(&plugins);
        screen
            .receive_toast("policy's own toast".into(), None, None, Some(POLICY), None)
            .unwrap();
        assert_eq!(screen.notification_history.len(), 2);
        assert!(!events(&plugins)
            .iter()
            .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
    }
}

#[test]
fn osc99_chunks_are_retained_until_resolution_and_passed_through_unchanged() {
    let (mut screen, server, plugins, instance) = policy_screen();
    screen
        .receive_desktop_notifications(
            vec![osc("i=a:a=report:d=0:e=1;dGl0bGU=", None)],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(events(&plugins).is_empty());
    assert!(!desktop_output(&server).contains("\x1b]99;"));
    screen
        .receive_desktop_notifications(
            vec![osc("i=a:p=body:d=1;body", Some(("title", "body")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(!desktop_output(&server).contains("\x1b]99;"));
    decide(&mut screen, 0, instance, NotificationDecision::Pass);
    let output = desktop_output(&server);
    assert!(
        output.contains("\x1b]99;i=p1r.a:a=report:d=0:e=1;dGl0bGU=\x1b\\"),
        "{output:?}"
    );
    assert!(
        output.contains("\x1b]99;i=p1r.a:p=body:d=1:a=focus,report;body\x1b\\"),
        "{output:?}"
    );
    assert_eq!(screen.notification_history[0].info.message, "title: body");
}

#[test]
fn suppression_and_close_prevent_every_display_chunk_from_reaching_the_host() {
    for close in [false, true] {
        let (mut screen, server, _, instance) = policy_screen();
        screen
            .receive_desktop_notifications(
                vec![
                    osc("i=a:d=0;secret", None),
                    osc("i=a:d=1:p=body;body", Some(("secret", "body"))),
                ],
                TerminalId::new(1),
            )
            .unwrap();
        if close {
            screen
                .receive_desktop_notifications(vec![osc("i=a:p=close;", None)], TerminalId::new(1))
                .unwrap();
        }
        decide(
            &mut screen,
            0,
            instance,
            if close {
                NotificationDecision::Pass
            } else {
                NotificationDecision::Suppress
            },
        );
        let output = desktop_output(&server);
        assert!(
            !output.contains("secret") && !output.contains(";body"),
            "{output:?}"
        );
        assert_eq!(screen.notification_history.len(), 1);
        assert!(
            screen.notification_history[0]
                .info
                .outcome
                .as_ref()
                .unwrap()
                .suppressed
        );
    }
}

#[test]
fn osc99_rewrite_preserves_application_metadata_and_sanitizes_controls() {
    let (mut screen, server, _, instance) = policy_screen();
    screen
        .receive_desktop_notifications(
            vec![
                osc("i=a:a=report:u=2:d=0:e=1;dGl0bGU=", None),
                osc("i=a:p=body:d=1;body", Some(("title", "body"))),
            ],
            TerminalId::new(1),
        )
        .unwrap();
    decide(
        &mut screen,
        0,
        instance,
        NotificationDecision::Replace {
            title: Some("safe\x07\x1b]52".into()),
            body: Some("new body".into()),
            delivery: None,
        },
    );
    let output = desktop_output(&server);
    assert!(
        output.contains("i=p1r.a")
            && output.contains("u=2")
            && output.contains("a=report")
            && output.contains("new body")
    );
    assert!(!output.contains("e=1") && !output.contains("\x1b]52"));
}

#[test]
fn overflow_falls_back_for_the_entire_assembly_and_records_why() {
    let (mut screen, server, _, _) = policy_screen();
    let huge = format!("i=a:d=0;{}", "x".repeat(65536));
    screen
        .receive_desktop_notifications(vec![osc(&huge, None)], TerminalId::new(1))
        .unwrap();
    assert!(desktop_output(&server).contains(&"x".repeat(65536)));
    screen
        .receive_desktop_notifications(
            vec![osc("i=a:p=body;tail", Some(("x", "tail")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(desktop_output(&server).contains("tail"));
    assert_eq!(
        screen.notification_history[0]
            .info
            .outcome
            .as_ref()
            .unwrap()
            .fallback_reason
            .as_deref(),
        Some("assembly_overflow")
    );
}

#[test]
fn detached_notifications_are_recorded_without_replaying_toasts_on_attach() {
    let (mut screen, server, _, instance) = policy_screen();
    screen.connected_clients.borrow_mut().clear();
    screen
        .receive_toast("detached".into(), None, None, None, None)
        .unwrap();
    decide(&mut screen, 0, instance, NotificationDecision::Pass);
    assert_eq!(screen.notification_history.len(), 1);
    assert!(screen.notifications.is_empty());
    assert!(desktop_output(&server).is_empty());
    screen
        .connected_clients
        .borrow_mut()
        .insert(ClientId::new(1));
    screen.render_to_clients().unwrap();
    assert!(screen.notifications.is_empty());
}

#[test]
fn reload_failure_and_reconfiguration_fall_back_and_reject_the_old_instance() {
    let (mut screen, _, _, old) = policy_screen();
    for (id, reason, disable) in [(0, "policy_reloaded", false), (1, "policy_crashed", true)] {
        screen
            .receive_toast("original".into(), None, None, None, None)
            .unwrap();
        screen.bind_notification_request(NotificationId::new(id), POLICY, old);
        screen
            .notification_policy_unavailable(POLICY, reason.into(), disable)
            .unwrap();
        decide(&mut screen, id, old, NotificationDecision::Suppress);
        assert_eq!(
            screen.notification_history[id as usize]
                .info
                .outcome
                .as_ref()
                .unwrap()
                .fallback_reason
                .as_deref(),
            Some(reason)
        );
    }
    screen
        .notification_policy_unavailable(POLICY, "policy_reloaded".into(), false)
        .unwrap();
    screen
        .receive_toast("after reload".into(), None, None, None, None)
        .unwrap();
    decide(
        &mut screen,
        2,
        PluginInstanceId::default(),
        NotificationDecision::Pass,
    );
    assert_eq!(screen.notification_history.len(), 3);
    screen
        .receive_toast("reconfigure".into(), None, None, None, None)
        .unwrap();
    screen.set_notification_policy(None, None).unwrap();
    assert_eq!(
        screen.notification_history[3]
            .info
            .outcome
            .as_ref()
            .unwrap()
            .fallback_reason
            .as_deref(),
        Some("policy_reconfigured")
    );
}

#[test]
fn pending_overflow_and_oversized_replacements_deliver_originals() {
    let (mut screen, _, _, instance) = policy_screen();
    for index in 0..257 {
        screen
            .receive_toast(format!("original-{index}"), None, None, None, None)
            .unwrap();
    }
    assert_eq!(screen.notification_history.len(), 257);
    assert!(screen.notification_history.iter().all(|record| record
        .info
        .outcome
        .as_ref()
        .unwrap()
        .fallback_reason
        .as_deref()
        == Some("pending_overflow")));
    screen
        .receive_toast("small".into(), None, None, None, None)
        .unwrap();
    decide(
        &mut screen,
        257,
        instance,
        NotificationDecision::Replace {
            title: None,
            body: Some("x".repeat(65537)),
            delivery: None,
        },
    );
    let info = &screen.notification_history[257].info;
    assert_eq!(info.message, "small");
    assert_eq!(
        info.outcome.as_ref().unwrap().fallback_reason.as_deref(),
        Some("replacement_too_large")
    );
}

#[test]
fn completion_waiters_and_synthesized_desktop_clicks_preserve_the_plugin_callback() {
    let (mut screen, _, plugins, instance) = policy_screen();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let click = crate::screen::NotificationClick {
        plugin_id: PluginId::new(7),
        client_id: ClientId::new(1),
        message: "clicked".into(),
        payload: "context".into(),
    };
    screen
        .receive_toast(
            "body".into(),
            None,
            Some(click),
            Some(PluginId::new(7)),
            Some(crate::route::NotificationEnd::new(tx)),
        )
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "completion stays pending while the policy works"
    );
    decide(
        &mut screen,
        0,
        instance,
        NotificationDecision::Replace {
            title: None,
            body: None,
            delivery: Some(NotificationDelivery::Desktop),
        },
    );
    assert!(
        rx.try_recv().is_ok(),
        "resolution releases the action waiter"
    );
    events(&plugins);
    assert!(screen.activate_policy_desktop(b"i=yinz-notification-0;", ClientId::new(1)));
    assert!(plugins.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::NotificationClicked { plugin_id, message, payload, .. } if plugin_id == PluginId::new(7) && message == "clicked" && payload == "context")));
}

#[test]
fn interleaved_osc99_ids_updates_and_control_queries_remain_independent() {
    let (mut screen, server, plugins, instance) = policy_screen();
    screen
        .receive_desktop_notifications(
            vec![
                osc("i=a:d=0;title-a", None),
                osc("i=b:d=0;title-b", None),
                osc("i=q:p=?;", None),
                osc("i=alive:p=alive;", None),
            ],
            TerminalId::new(1),
        )
        .unwrap();
    let controls = desktop_output(&server);
    assert!(controls.contains("p=?") && controls.contains("p=alive"));
    assert!(!controls.contains("title-a") && !controls.contains("title-b"));
    assert!(events(&plugins).is_empty());
    screen
        .receive_desktop_notifications(
            vec![
                osc("i=b:p=body;body-b", Some(("title-b", "body-b"))),
                osc("i=a:p=body;body-a", Some(("title-a", "body-a"))),
            ],
            TerminalId::new(1),
        )
        .unwrap();
    decide(&mut screen, 1, instance, NotificationDecision::Pass);
    decide(&mut screen, 0, instance, NotificationDecision::Suppress);
    let output = desktop_output(&server);
    assert!(output.contains("title-a") && output.contains("body-a"));
    assert!(!output.contains("title-b") && !output.contains("body-b"));
    screen
        .receive_desktop_notifications(
            vec![osc("i=a;updated", Some(("updated", "")))],
            TerminalId::new(1),
        )
        .unwrap();
    decide(&mut screen, 2, instance, NotificationDecision::Pass);
    assert_eq!(screen.notification_history.len(), 3);
    assert_eq!(
        screen.notification_history[0].info.message,
        "title-b: body-b"
    );
    assert_eq!(
        screen.notification_history[1].info.message,
        "title-a: body-a"
    );
    assert_eq!(screen.notification_history[2].info.message, "updated");
}

#[test]
fn osc99_base64_and_read_boundaries_arrive_as_one_complete_notification() {
    let (mut screen, server, plugins, instance) = policy_screen();
    let bytes = b"\x1b]99;i=a:e=1:d=0;dGl0bGU=\x07\x1b]99;i=b:d=0;other\x07\x1b]99;i=a:p=body:e=1;Ym9keQ==\x07";
    for read in bytes.chunks(3) {
        let notifications = {
            let pane = screen
                .get_active_tab_mut(ClientId::new(1))
                .unwrap()
                .get_active_pane_mut(ClientId::new(1))
                .unwrap();
            pane.handle_pty_bytes(read.to_vec());
            pane.drain_desktop_notifications()
        };
        screen
            .receive_desktop_notifications(notifications, TerminalId::new(1))
            .unwrap();
    }
    let events = events(&plugins);
    let arrivals = events
        .iter()
        .filter_map(|(_, event)| {
            if let Event::NotificationReceived(arrival) = event {
                Some(arrival)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(arrivals.len(), 1);
    assert_eq!(
        (arrivals[0].title.as_str(), arrivals[0].body.as_str()),
        ("title", "body")
    );
    assert!(!desktop_output(&server).contains("\x1b]99;"));
    decide(&mut screen, 0, instance, NotificationDecision::Suppress);
    assert!(!desktop_output(&server).contains("\x1b]99;"));
}

#[test]
fn missing_policy_loads_and_unbound_hooks_use_immediate_original_delivery() {
    for reason in [None, Some("policy_load_failed".to_owned())] {
        let (mut screen, _, _, _) = policy_screen();
        screen
            .set_notification_policy(None, reason.clone())
            .unwrap();
        screen
            .receive_toast("original".into(), None, None, None, None)
            .unwrap();
        assert_eq!(screen.notification_history.len(), 1);
        let result = screen.notification_history[0]
            .info
            .outcome
            .as_ref()
            .unwrap();
        assert_eq!(result.fallback_reason, reason);
        assert!(!result.suppressed);
    }
}

#[test]
fn unavailable_osc99_assemblies_stay_bypassed_after_policy_enablement_and_reload() {
    for reason in [None, Some("policy_load_failed"), Some("policy_crashed")] {
        let (mut screen, server, plugins, instance) = policy_screen();
        if reason == Some("policy_crashed") {
            screen
                .notification_policy_unavailable(POLICY, reason.unwrap().into(), true)
                .unwrap();
        } else {
            screen
                .set_notification_policy(None, reason.map(str::to_owned))
                .unwrap();
        }
        screen
            .receive_desktop_notifications(vec![osc("i=a:d=0;title", None)], TerminalId::new(1))
            .unwrap();
        let mut output = desktop_output(&server);
        assert_eq!(
            output.matches("i=p1r.a:d=0:a=focus,report;title").count(),
            1
        );
        assert!(events(&plugins).is_empty());
        if reason == Some("policy_crashed") {
            screen
                .notification_policy_unavailable(POLICY, "policy_reloaded".into(), false)
                .unwrap();
        } else {
            screen.set_notification_policy(Some(POLICY), None).unwrap();
        }
        screen
            .receive_desktop_notifications(
                vec![osc("i=a:p=body:d=1;tail", Some(("title", "tail")))],
                TerminalId::new(1),
            )
            .unwrap();
        output.push_str(&desktop_output(&server));
        assert_eq!(
            output.matches("i=p1r.a:d=0:a=focus,report;title").count(),
            1
        );
        assert_eq!(
            output
                .matches("i=p1r.a:p=body:d=1:a=focus,report;tail")
                .count(),
            1
        );
        let observed = events(&plugins);
        assert_eq!(
            observed
                .iter()
                .filter(|(_, event)| matches!(event, Event::NotificationReceived(_)))
                .count(),
            1
        );
        assert!(!observed
            .iter()
            .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
        assert_eq!(screen.notification_history.len(), 1);
        let outcome = screen.notification_history[0]
            .info
            .outcome
            .as_ref()
            .unwrap();
        assert_eq!(
            outcome.fallback_reason.as_deref(),
            Some(reason.unwrap_or("policy_unavailable"))
        );
        assert!(!outcome.suppressed);

        // Completion removes the bypass marker, so the application can reuse its ID.
        screen
            .receive_desktop_notifications(
                vec![osc("i=a;fresh", Some(("fresh", "")))],
                TerminalId::new(1),
            )
            .unwrap();
        assert!(!desktop_output(&server).contains(";fresh"));
        assert!(events(&plugins).iter().any(|(plugin, event)| {
            *plugin == Some(POLICY) && matches!(event, Event::NotificationRequested(_))
        }));
        decide(&mut screen, 1, instance, NotificationDecision::Pass);
        assert!(desktop_output(&server).contains(";fresh"));
        assert_eq!(screen.notification_history.len(), 2);
        assert!(screen.notification_history[1]
            .info
            .outcome
            .as_ref()
            .unwrap()
            .fallback_reason
            .is_none());
    }
}

#[test]
fn completing_the_oldest_bypassed_osc99_id_at_the_tracking_limit_keeps_its_marker() {
    for available in [true, false] {
        let (mut screen, server, plugins, _) = policy_screen();
        if !available {
            screen.set_notification_policy(None, None).unwrap();
        }
        for id in 0..512 {
            screen
                .receive_desktop_notifications(
                    vec![osc(&format!("i={id}:d=0;chunk"), None)],
                    TerminalId::new(1),
                )
                .unwrap();
        }
        let mut output = desktop_output(&server);
        assert_eq!(
            output.matches("i=p1r.0:d=0:a=focus,report;chunk").count(),
            1
        );
        if !available {
            screen.set_notification_policy(Some(POLICY), None).unwrap();
        }
        screen
            .receive_desktop_notifications(
                vec![osc("i=0:p=body;tail", Some(("chunk", "tail")))],
                TerminalId::new(1),
            )
            .unwrap();
        output.push_str(&desktop_output(&server));
        assert_eq!(
            output.matches("i=p1r.0:d=0:a=focus,report;chunk").count(),
            1
        );
        assert_eq!(
            output.matches("i=p1r.0:p=body:a=focus,report;tail").count(),
            1
        );
        assert!(!events(&plugins)
            .iter()
            .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
        assert_eq!(screen.notification_history.len(), 1);
        assert_eq!(
            screen.notification_history[0]
                .info
                .outcome
                .as_ref()
                .unwrap()
                .fallback_reason
                .as_deref(),
            Some(if available {
                "assembly_overflow"
            } else {
                "policy_unavailable"
            })
        );
    }
}

#[test]
fn unavailable_osc99_tracking_evicts_the_oldest_marker_only_for_new_ids() {
    let (mut screen, _, plugins, _) = policy_screen();
    screen.set_notification_policy(None, None).unwrap();
    for id in 0..512 {
        screen
            .receive_desktop_notifications(
                vec![osc(&format!("i={id}:d=0;chunk"), None)],
                TerminalId::new(1),
            )
            .unwrap();
    }
    // Touching the oldest ID must preserve its insertion sequence.
    screen
        .receive_desktop_notifications(
            vec![osc("i=0:d=0;more", None), osc("i=512:d=0;new", None)],
            TerminalId::new(1),
        )
        .unwrap();
    screen.set_notification_policy(Some(POLICY), None).unwrap();
    screen
        .receive_desktop_notifications(
            vec![osc("i=1;retained", Some(("retained", "")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(!events(&plugins)
        .iter()
        .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
    screen
        .receive_desktop_notifications(
            vec![osc("i=0;evicted", Some(("evicted", "")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(events(&plugins)
        .iter()
        .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
}

#[test]
fn tracking_overflow_bypasses_remaining_chunks_and_pane_cleanup_releases_assemblies() {
    let (mut screen, server, plugins, _) = policy_screen();
    for id in 0..257 {
        screen
            .receive_desktop_notifications(
                vec![osc(&format!("i={id}:d=0;chunk"), None)],
                TerminalId::new(1),
            )
            .unwrap();
    }
    assert!(desktop_output(&server).contains("i=p1r.0"));
    screen
        .receive_desktop_notifications(
            vec![osc("i=0:p=body;tail", Some(("chunk", "tail")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(!events(&plugins)
        .iter()
        .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
    assert_eq!(
        screen.notification_history[0]
            .info
            .outcome
            .as_ref()
            .unwrap()
            .fallback_reason
            .as_deref(),
        Some("assembly_overflow")
    );
    screen.cleanup_closed_pane(PaneId::Terminal(TerminalId::new(1)));
    // Reusing this fake pane id must start a new assembly with no retained raw chunks.
    screen
        .receive_desktop_notifications(
            vec![osc("i=1;fresh", Some(("fresh", "")))],
            TerminalId::new(1),
        )
        .unwrap();
    assert!(events(&plugins)
        .iter()
        .any(|(_, event)| matches!(event, Event::NotificationRequested(_))));
}

#[test]
fn osc99_rewriting_keeps_non_text_chunks() {
    let (mut screen, server, _, instance) = policy_screen();
    screen
        .receive_desktop_notifications(
            vec![
                osc("i=a:d=0;title", None),
                osc("i=a:p=icon:e=1:d=1;opaque-icon", Some(("title", ""))),
            ],
            TerminalId::new(1),
        )
        .unwrap();
    decide(
        &mut screen,
        0,
        instance,
        NotificationDecision::Replace {
            title: Some("new".into()),
            body: None,
            delivery: None,
        },
    );
    let output = desktop_output(&server);
    assert!(output.contains("p=icon:e=1:d=0") && output.contains("opaque-icon"));
    assert!(!output.contains("p=icon:e=1:d=1"));
    assert!(output.contains("p=title:d=0:e=0") && output.contains(";new\x1b\\"));
}

#[test]
fn a_new_suppressed_or_desktop_only_target_removes_the_older_toasts_hint() {
    for decision in [
        NotificationDecision::Suppress,
        NotificationDecision::Replace {
            title: None,
            body: None,
            delivery: Some(NotificationDelivery::Desktop),
        },
    ] {
        let (mut screen, _, _, instance) = policy_screen();
        let client = ClientId::new(1);
        let mode = screen
            .mode_info
            .entry(client)
            .or_insert_with(|| screen.default_mode_info.clone());
        mode.keybinds = Config::from_default_assets()
            .unwrap()
            .keybinds
            .to_keybinds_vec();
        screen
            .receive_toast("older".into(), None, None, None, None)
            .unwrap();
        decide(&mut screen, 0, instance, NotificationDecision::Pass);
        let viewport = screen.notification_browser_viewport(client).unwrap();
        let now = Instant::now() + Duration::from_secs(1);
        assert!(screen.notification_layout(viewport, now, client)[0]
            .footer
            .is_some());
        screen
            .receive_toast("new target".into(), None, None, None, None)
            .unwrap();
        decide(&mut screen, 1, instance, decision);
        assert_eq!(
            screen.notification_history.last().unwrap().info.id,
            NotificationId::new(1)
        );
        let layouts = screen.notification_layout(viewport, now, client);
        assert_eq!(layouts.len(), 1);
        assert!(layouts[0].footer.is_none());
    }
}
