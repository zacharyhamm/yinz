use super::*;

const CLIENT: ClientId = ClientId::new(1);
const OTHER_CLIENT: ClientId = ClientId::new(2);
const SIZE: Size = Size {
    cols: 121,
    rows: 20,
};

struct PacedScreen {
    screen: Screen,
    to_server: ServerReceiver,
    to_background_jobs: Receiver<(BackgroundJob, ErrorContext)>,
    backlog: RenderBacklogs,
    ready: RenderReadyCallbacks,
    to_screen: Receiver<(ScreenInstruction, ErrorContext)>,
    client_messages: CapturedClientMessages,
}

fn screen_with_backlog() -> PacedScreen {
    let (mut screen, _, to_server) = create_new_screen_with_capture(SIZE, true, true, true, false);
    let fake_os_input = FakeInputOutput::default();
    let backlog = fake_os_input.render_backlog.clone();
    let ready = fake_os_input.render_ready.clone();
    let client_messages = fake_os_input.server_to_client_messages.clone();
    screen.bus.os_input = Some(Box::new(fake_os_input));
    let (to_background_jobs, background_jobs_receiver) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    let (to_screen, screen_receiver) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    new_tab(&mut screen, 1, TabId::new(0));
    PacedScreen {
        screen,
        to_server,
        to_background_jobs: background_jobs_receiver,
        backlog,
        ready,
        to_screen: screen_receiver,
        client_messages,
    }
}

fn backlog(queued_renders: usize) -> RenderBacklog {
    RenderBacklog { queued_renders }
}

fn behind() -> RenderBacklog {
    backlog(1)
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

fn render_retries_requested(receiver: &Receiver<(BackgroundJob, ErrorContext)>) -> usize {
    receiver
        .try_iter()
        .filter(|(job, _)| matches!(job, BackgroundJob::RenderToClients))
        .count()
}

/// What `thread_main` does with each debounced `RenderToClients`.
fn paced_render(screen: &mut Screen) {
    screen.render_to_clients().unwrap();
}

#[test]
fn a_client_with_a_queued_frame_holds_back_rendering() {
    let mut paced = screen_with_backlog();
    assert!(paced.screen.deferred_tabs().is_empty());

    // Writing the only frame it has is keeping up.
    *paced.backlog.lock().unwrap() = vec![(CLIENT, backlog(0))];
    assert!(paced.screen.deferred_tabs().is_empty());

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    assert!(!paced.screen.deferred_tabs().is_empty());
}

#[test]
fn single_ready_client_clears_stale_deferrals() {
    let mut paced = screen_with_backlog();
    assert!(paced.screen.deferred_tabs().is_empty());
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    assert_eq!(
        paced.screen.deferred_tabs(),
        &[TabId::new(0)].into_iter().collect()
    );
    assert!(paced.ready.lock().unwrap().contains_key(&CLIENT));

    // A removed subscriber or tab can leave deferral state until the next pass.
    paced.screen.deferred_subscriber_renders = true;
    paced.screen.deferred_render_tabs.insert(TabId::new(99));
    paced.backlog.lock().unwrap().clear();
    assert!(paced.screen.deferred_tabs().is_empty());
    assert!(paced.screen.deferred_render_tabs.is_empty());
    assert!(!paced.screen.deferred_subscriber_renders);
}

#[test]
fn deferred_panes_deliver_bells_and_directory_updates_once_after_resuming() {
    for visual_bell in [false, true] {
        let mut paced = screen_with_backlog();
        paced.screen.visual_bell = visual_bell;
        paced_render(&mut paced.screen);
        let _ = rendered_bytes(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
        let osc7 = "\x1b]7;file://host/deferred\x1b\\";
        paced
            .screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(1),
                format!("pending-content\x07{osc7}").into_bytes(),
            )
            .unwrap();

        let previous_osc7 = paced.screen.last_forwarded_osc7.clone();
        for _ in 0..2 {
            paced_render(&mut paced.screen);
            assert!(!paced.screen.deferred_render_tabs.is_empty());
            assert!(rendered_by_client(&paced.to_server).is_empty());
            assert_eq!(paced.screen.last_forwarded_osc7, previous_osc7);
            let tab = paced.screen.tabs.get(&TabId::new(0)).unwrap();
            assert!(tab.get_active_pane(CLIENT).unwrap().has_bell());
        }

        paced.backlog.lock().unwrap().clear();
        let ready = paced.ready.lock().unwrap().remove(&CLIENT).unwrap();
        ready();
        assert!(paced.to_screen.try_iter().any(|(instruction, _)| {
            matches!(instruction, ScreenInstruction::RenderWriterReady)
        }));
        paced.screen.resume_deferred_render();
        paced_render(&mut paced.screen);
        let bytes = rendered_bytes(&paced.to_server);
        assert!(bytes.contains("pending-content"), "{bytes:?}");
        assert_eq!(bytes.matches('\x07').count(), 1, "{bytes:?}");
        assert_eq!(bytes.matches(osc7).count(), 1, "{bytes:?}");
        assert_eq!(
            paced.screen.last_forwarded_osc7[&CLIENT].as_deref(),
            Some("file://host/deferred")
        );
        paced_render(&mut paced.screen);
        let bytes = rendered_bytes(&paced.to_server);
        assert!(!bytes.contains('\x07'), "{bytes:?}");
        assert!(!bytes.contains(osc7), "{bytes:?}");
    }
}

#[test]
fn deferred_clients_do_not_receive_another_tabs_bell_or_directory_updates() {
    for visual_bell in [false, true] {
        let mut paced = screen_with_backlog();
        paced.screen.visual_bell = visual_bell;
        paced.screen.add_client(OTHER_CLIENT).unwrap();
        new_tab(&mut paced.screen, 2, TabId::new(1));
        assert_eq!(paced.screen.active_tab_ids[&CLIENT], TabId::new(1));
        assert_eq!(paced.screen.active_tab_ids[&OTHER_CLIENT], TabId::new(0));
        paced_render(&mut paced.screen);
        let _ = rendered_by_client(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
        let previous_osc7 = paced.screen.last_forwarded_osc7.get(&OTHER_CLIENT).cloned();
        for (tab_id, terminal, text) in [(0, 1, "blocked"), (1, 2, "ready")] {
            paced
                .screen
                .tabs
                .get_mut(&TabId::new(tab_id))
                .unwrap()
                .handle_pty_bytes(
                    TerminalId::new(terminal),
                    format!("{text}\x07\x1b]7;file://host/{text}\x1b\\").into_bytes(),
                )
                .unwrap();
        }
        paced_render(&mut paced.screen);
        let frames = rendered_by_client(&paced.to_server);
        assert!(!frames.contains_key(&OTHER_CLIENT), "{frames:?}");
        assert!(frames[&CLIENT].contains('\x07'), "{frames:?}");
        assert!(frames[&CLIENT].contains("\x1b]7;file://host/ready\x1b\\"));
        assert_eq!(
            paced.screen.last_forwarded_osc7.get(&OTHER_CLIENT),
            previous_osc7.as_ref()
        );
        paced.backlog.lock().unwrap().clear();
        paced_render(&mut paced.screen);
        let frames = rendered_by_client(&paced.to_server);
        assert!(frames[&OTHER_CLIENT].contains('\x07'), "{frames:?}");
        assert!(frames[&OTHER_CLIENT].contains("\x1b]7;file://host/blocked\x1b\\"));
    }
}

#[test]
fn queued_frames_hold_back_pending_updates_until_the_writer_is_ready() {
    for kind in ["clients", "watchers", "subscribers"] {
        let mut paced = screen_with_backlog();
        let recipient = match kind {
            "watchers" => {
                paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
                paced.screen.remove_client(CLIENT).unwrap();
                OTHER_CLIENT
            },
            "subscribers" => {
                paced.screen.subscribe_to_pane_renders(
                    OTHER_CLIENT,
                    vec![PaneId::Terminal(TerminalId::new(1))],
                    None,
                    false,
                );
                OTHER_CLIENT
            },
            _ => CLIENT,
        };
        paced_render(&mut paced.screen);
        let _ = rendered_by_client(&paced.to_server);
        paced.client_messages.lock().unwrap().clear();
        for row in 1..=3 {
            *paced.backlog.lock().unwrap() = vec![(CLIENT, backlog(1)), (recipient, backlog(1))];
            paced
                .screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .handle_pty_bytes(
                    TerminalId::new(1),
                    format!("\x1b[{row};1Hpending-{row}").into_bytes(),
                )
                .unwrap();
            paced.screen.render().unwrap();
            let _ = render_retries_requested(&paced.to_background_jobs);
            for _ in 0..2 {
                paced_render(&mut paced.screen);
                assert!(
                    rendered_by_client(&paced.to_server).is_empty(),
                    "{kind}: {row}"
                );
                assert!(
                    paced.client_messages.lock().unwrap().is_empty(),
                    "{kind}: {row}"
                );
                assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);
                assert!(
                    paced.ready.lock().unwrap().contains_key(&recipient),
                    "{kind}"
                );
            }
        }
        paced.backlog.lock().unwrap().clear();
        paced.ready.lock().unwrap().remove(&recipient).unwrap()();
        assert!(paced
            .to_screen
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, ScreenInstruction::RenderWriterReady)));
        paced.screen.resume_deferred_render();
        assert_eq!(render_retries_requested(&paced.to_background_jobs), 1);
        paced_render(&mut paced.screen);
        let contents = if kind == "subscribers" {
            paced.client_messages.lock().unwrap()[&recipient]
                .iter()
                .filter_map(|message| match message {
                    ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
                        Some(viewport.join("\n"))
                    },
                    _ => None,
                })
                .collect::<String>()
        } else {
            rendered_by_client(&paced.to_server)
                .remove(&recipient)
                .unwrap()
        };
        for row in 1..=3 {
            assert!(
                contents.contains(&format!("pending-{row}")),
                "{kind}: {contents:?}"
            );
        }
    }
}

#[test]
fn queued_frames_do_not_bypass_other_blocked_recipients() {
    let slow = ClientId::new(3);
    for kind in ["clients", "watchers", "subscribers"] {
        let mut paced = screen_with_backlog();
        match kind {
            "clients" => {
                paced.screen.add_client(slow).unwrap();
                paced.screen.add_client(OTHER_CLIENT).unwrap();
                paced.screen.remove_client(CLIENT).unwrap();
            },
            "watchers" => {
                paced.screen.add_watcher_client(slow).unwrap();
                paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
                paced.screen.remove_client(CLIENT).unwrap();
            },
            _ => {
                for client in [slow, OTHER_CLIENT] {
                    paced.screen.subscribe_to_pane_renders(
                        client,
                        vec![PaneId::Terminal(TerminalId::new(1))],
                        None,
                        false,
                    );
                }
            },
        }
        paced_render(&mut paced.screen);
        paced.to_server.try_iter().for_each(drop);
        paced.client_messages.lock().unwrap().clear();
        *paced.backlog.lock().unwrap() = vec![
            (CLIENT, behind()),
            (slow, backlog(1)),
            (OTHER_CLIENT, backlog(1)),
        ];
        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), b"pending-healthy-writer".to_vec())
            .unwrap();
        paced.screen.render().unwrap();
        paced_render(&mut paced.screen);
        assert!(paced.client_messages.lock().unwrap().is_empty(), "{kind}");
        assert!(
            !paced.to_server.try_iter().any(|(instruction, _)| {
                matches!(instruction, ServerInstruction::Render(Some(output))
                if output.values().any(|bytes| bytes.contains("pending-healthy-writer")))
            }),
            "{kind}"
        );
        assert!(paced.ready.lock().unwrap().contains_key(&slow), "{kind}");
        assert!(
            paced.ready.lock().unwrap().contains_key(&OTHER_CLIENT),
            "{kind}"
        );

        paced
            .backlog
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != OTHER_CLIENT);
        paced.ready.lock().unwrap().remove(&OTHER_CLIENT).unwrap()();
        assert!(paced
            .to_screen
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, ScreenInstruction::RenderWriterReady)));
        paced_render(&mut paced.screen);
        if kind == "subscribers" {
            assert!(paced.client_messages.lock().unwrap()[&OTHER_CLIENT]
                .iter()
                .any(|message| {
                    matches!(message, ServerToClientMsg::PaneRenderUpdate { viewport, .. }
                    if viewport.iter().any(|line| line.contains("pending-healthy-writer")))
                }));
        } else {
            assert!(paced.to_server.try_iter().any(|(instruction, _)| {
                matches!(instruction, ServerInstruction::Render(Some(output))
                    if output.get(&OTHER_CLIENT).is_some_and(|bytes| bytes.contains("pending-healthy-writer")))
            }), "{kind}");
        }
    }
}

#[test]
fn ready_recipients_do_not_prevent_arming_blocked_recipients() {
    let mut paced = screen_with_backlog();
    for id in 2..10 {
        let client = ClientId::new(id);
        paced.screen.add_watcher_client(client).unwrap();
        paced.backlog.lock().unwrap().push((client, behind()));
    }
    assert!(paced.screen.deferred_tabs().is_empty());
    for id in 2..10 {
        assert!(paced.ready.lock().unwrap().contains_key(&ClientId::new(id)));
    }
}

#[test]
fn watcher_pacing_uses_the_departed_clients_last_tab() {
    let mut paced = screen_with_backlog();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.remove_client(CLIENT).unwrap();
    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
    assert_eq!(
        paced.screen.deferred_tabs(),
        &[TabId::new(1)].into_iter().collect()
    );
    paced.screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_eq!(
        paced.screen.deferred_tabs(),
        &[TabId::new(0)].into_iter().collect()
    );
    paced.screen.remove_watcher_client(OTHER_CLIENT);
    assert!(paced.screen.deferred_tabs().is_empty());
}

#[test]
fn one_slow_client_does_not_hold_back_the_others() {
    let mut paced = screen_with_backlog();
    paced.screen.add_client(OTHER_CLIENT).unwrap();
    // Attaching asks for a frame of its own.
    let _ = render_retries_requested(&paced.to_background_jobs);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    assert!(paced.screen.deferred_tabs().is_empty());
    assert!(paced.screen.deferred_tabs().is_empty());
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
    assert!(!paced.screen.deferred_tabs().is_empty());
}

#[test]
fn a_slow_regular_client_does_not_hold_back_a_healthy_watcher() {
    let mut paced = screen_with_backlog();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];

    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"watcher-still-updates".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let watcher_frame: String = paced
        .to_server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
            _ => None,
        })
        .collect();
    assert!(
        watcher_frame.contains("watcher-still-updates"),
        "{watcher_frame:?}"
    );

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
    assert!(!paced.screen.deferred_tabs().is_empty());
}

#[test]
fn hover_only_changes_wake_a_healthy_watcher() {
    let mut paced = screen_with_backlog();
    let hovered_pane = PaneId::Terminal(TerminalId::new(1));
    add_pane_to_tab(
        &mut paced.screen,
        TabId::new(0),
        PaneId::Terminal(TerminalId::new(2)),
        false,
    );
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"unchanged-hover-content".to_vec())
        .unwrap();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];

    let pane = paced.screen.tabs[&TabId::new(0)]
        .get_pane_with_id(hovered_pane)
        .unwrap();
    let position = Position::new(pane.y() as i32 + 2, pane.x() as u16 + 2);
    // Enter the unfocused pane, then leave the tab. Both only change frame state.
    for position in [position, Position::new(-1, 0)] {
        paced
            .screen
            .handle_mouse_event(MouseEvent::new_buttonless_motion(position), CLIENT);
        paced_render(&mut paced.screen);
        // Rendering is synchronous, so this channel contains the complete frame.
        let frame: String = paced
            .to_server
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
                _ => None,
            })
            .collect();
        assert!(!frame.is_empty());
        assert!(!frame.contains("unchanged-hover-content"), "{frame:?}");
        assert!(!paced.screen.watcher_clients[&OTHER_CLIENT].should_force_render());
        paced
            .screen
            .handle_mouse_event(MouseEvent::new_buttonless_motion(position), CLIENT);
    }
}

#[test]
fn watchers_receive_only_the_lines_that_changed() {
    let mut paced = screen_with_backlog();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(CLIENT).unwrap();
    fn write(paced: &mut PacedScreen, bytes: &[u8]) {
        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), bytes.to_vec())
            .unwrap();
        paced_render(&mut paced.screen);
    }
    write(&mut paced, b"unchanged-first-line\r\n");
    let _ = rendered_bytes(&paced.to_server);

    write(&mut paced, b"newly-written-line");
    let frames: Vec<_> = paced
        .to_server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => Some(output),
            _ => None,
        })
        .collect();
    for client in [CLIENT, OTHER_CLIENT] {
        let frame: String = frames
            .iter()
            .filter_map(|frame| frame.get(&client).cloned())
            .collect();
        assert!(
            frame.contains("newly-written-line"),
            "{client:?}: {frame:?}"
        );
        assert!(
            !frame.contains("unchanged-first-line"),
            "{client:?}: {frame:?}"
        );
    }
}

#[test]
fn watcher_only_sessions_pace_until_the_watcher_drains_or_detaches() {
    let mut paced = screen_with_backlog();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.remove_client(CLIENT).unwrap();
    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
    assert!(!paced.screen.deferred_tabs().is_empty());

    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, backlog(0))];
    assert!(paced.screen.deferred_tabs().is_empty());
    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
    paced.screen.remove_watcher_client(OTHER_CLIENT);
    assert!(paced.screen.deferred_tabs().is_empty());
}

#[test]
fn a_slow_terminal_does_not_hold_back_a_pane_subscriber() {
    let mut paced = screen_with_backlog();
    paced.screen.subscribe_to_pane_renders(
        OTHER_CLIENT,
        vec![PaneId::Terminal(TerminalId::new(1))],
        None,
        false,
    );
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    paced.client_messages.lock().unwrap().clear();
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"subscriber-still-updates".to_vec())
        .unwrap();

    paced.screen.render().unwrap();
    let _ = render_retries_requested(&paced.to_background_jobs);

    paced_render(&mut paced.screen);
    assert_eq!(
        rendered_bytes(&paced.to_server),
        "",
        "snapshots must not enqueue a full terminal frame"
    );
    let messages = paced.client_messages.lock().unwrap();
    assert!(
        messages.get(&OTHER_CLIENT).is_some_and(|messages| {
            messages.iter().any(|message| {
        matches!(message, ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
            if viewport.iter().any(|line| line.contains("subscriber-still-updates")))
    })
        }),
        "{messages:?}"
    );
    drop(messages);
    paced.backlog.lock().unwrap().clear();
    paced_render(&mut paced.screen);
    assert!(
        rendered_bytes(&paced.to_server).contains("subscriber-still-updates"),
        "snapshot delivery must preserve the terminal's pending dirty state"
    );
}

#[test]
fn slow_terminal_and_subscriber_defer_until_the_subscriber_drains() {
    let mut paced = screen_with_backlog();
    paced.screen.subscribe_to_pane_renders(
        OTHER_CLIENT,
        vec![PaneId::Terminal(TerminalId::new(1))],
        None,
        false,
    );
    paced.screen.render_to_clients().unwrap();
    paced.client_messages.lock().unwrap().clear();
    let _ = rendered_bytes(&paced.to_server);
    let _ = render_retries_requested(&paced.to_background_jobs);
    // The subscriber's backlog is a coalesced PaneRenderUpdate, not a Render.
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"deferred-subscriber-update".to_vec())
        .unwrap();

    paced.screen.render().unwrap();
    let _ = render_retries_requested(&paced.to_background_jobs);

    paced_render(&mut paced.screen);
    assert!(paced.client_messages.lock().unwrap().is_empty());
    assert_eq!(rendered_bytes(&paced.to_server), "");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);

    // The attached terminal is still slow, but the subscriber is ready again.
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    paced_render(&mut paced.screen);
    let messages = paced.client_messages.lock().unwrap();
    let subscriber_messages = messages.get(&OTHER_CLIENT).expect("subscriber update");
    assert!(subscriber_messages.iter().any(|message| {
        matches!(message, ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
            if viewport.iter().any(|line| line.contains("deferred-subscriber-update")))
    }), "{subscriber_messages:?}");
    assert_eq!(rendered_bytes(&paced.to_server), "");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);
}

#[test]
fn deferred_subscriber_snapshots_wait_for_new_render_requests() {
    let pane = PaneId::Terminal(TerminalId::new(1));
    for ansi in [false, true] {
        let mut paced = screen_with_backlog();
        paced
            .screen
            .subscribe_to_pane_renders(OTHER_CLIENT, vec![pane], None, ansi);
        paced.screen.render_to_clients().unwrap();
        paced.client_messages.lock().unwrap().clear();
        let _ = rendered_bytes(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
        paced
            .screen
            .handle_pty_bytes(
                TerminalId::new(1),
                b"first-subscriber-update".to_vec(),
                &mut super::super::thread_main::PendingEvents::default(),
            )
            .unwrap();
        paced_render(&mut paced.screen);
        assert!(!paced.client_messages.lock().unwrap().is_empty());
        paced.client_messages.lock().unwrap().clear();

        // Forget the comparison cache: another snapshot query would now send an update even
        // with unchanged pane contents, exposing work that an equality check normally hides.
        paced
            .screen
            .pane_render_subscribers
            .get_mut(&OTHER_CLIENT)
            .unwrap()
            .previous_viewports
            .clear();
        for _ in 0..3 {
            paced_render(&mut paced.screen);
        }
        assert!(paced.client_messages.lock().unwrap().is_empty());
        paced.screen.render().unwrap();
        paced_render(&mut paced.screen);
        assert!(!paced.client_messages.lock().unwrap().is_empty());
        paced.client_messages.lock().unwrap().clear();

        // PTY events schedule directly, bypassing Screen::render(). They must invalidate too.
        paced
            .screen
            .handle_pty_bytes(
                TerminalId::new(1),
                b"\r\nnext-subscriber-update".to_vec(),
                &mut super::super::thread_main::PendingEvents::default(),
            )
            .unwrap();
        paced_render(&mut paced.screen);
        let messages = paced.client_messages.lock().unwrap();
        assert!(messages[&OTHER_CLIENT]
            .iter()
            .any(|message| matches!(message,
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
                if viewport.iter().any(|line| line.contains("next-subscriber-update")))));
        drop(messages);
        assert_eq!(rendered_bytes(&paced.to_server), "");
        paced.backlog.lock().unwrap().clear();
        paced_render(&mut paced.screen);
        let frame = rendered_bytes(&paced.to_server);
        assert!(frame.contains("first-subscriber-update"), "{frame:?}");
        assert!(frame.contains("next-subscriber-update"), "{frame:?}");
    }
}

#[test]
fn an_idle_subscribed_pane_does_not_bypass_a_slow_terminals_backlog() {
    let mut paced = screen_with_backlog();
    paced.screen.subscribe_to_pane_renders(
        OTHER_CLIENT,
        vec![PaneId::Terminal(TerminalId::new(1))],
        None,
        false,
    );
    new_tab(&mut paced.screen, 2, TabId::new(1));
    paced.screen.render_to_clients().unwrap();
    paced.client_messages.lock().unwrap().clear();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(2), b"only-the-slow-tab-changed".to_vec())
        .unwrap();

    paced_render(&mut paced.screen);
    assert_eq!(rendered_bytes(&paced.to_server), "");
    assert!(paced.client_messages.lock().unwrap().is_empty());
    paced.backlog.lock().unwrap().clear();
    paced_render(&mut paced.screen);
    assert!(rendered_bytes(&paced.to_server).contains("only-the-slow-tab-changed"));
}

#[test]
fn deferred_plugin_snapshots_use_the_same_client_across_tabs() {
    let subscriber = ClientId::new(3);
    for ansi in [false, true] {
        let mut paced = screen_with_backlog();
        paced.screen.add_client(OTHER_CLIENT).unwrap();
        new_tab(&mut paced.screen, 2, TabId::new(1));
        let panes = [
            (TabId::new(0), crate::PluginId::new(20), OTHER_CLIENT),
            (TabId::new(1), crate::PluginId::new(21), CLIENT),
        ];
        let (plugin_sender, _plugin_receiver) = channels::unbounded();
        let plugin_sender = SenderWithContext::new(plugin_sender);
        for (tab, plugin, client) in panes {
            assert!(paced.screen.tabs[&tab].has_connected_client(&client));
            paced.screen.tabs.get_mut(&tab).unwrap().senders.to_plugin =
                Some(plugin_sender.clone());
            paced
                .screen
                .tabs
                .get_mut(&tab)
                .unwrap()
                .new_pane(
                    PaneId::Plugin(plugin),
                    None,
                    None,
                    false,
                    false,
                    NewPanePlacement::default(),
                    Some(client),
                    None,
                )
                .unwrap();
            paced
                .screen
                .tabs
                .get_mut(&tab)
                .unwrap()
                .handle_plugin_bytes(plugin, CLIENT, b"initial-plugin-content".to_vec())
                .unwrap();
        }
        paced.screen.subscribe_to_pane_renders(
            subscriber,
            panes
                .iter()
                .map(|(_, plugin, _)| PaneId::Plugin(*plugin))
                .collect(),
            None,
            ansi,
        );
        let messages = paced.client_messages.lock().unwrap();
        let initial = &messages[&subscriber];
        assert_eq!(initial.len(), 2);
        assert!(initial.iter().all(|message| matches!(message,
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: true, .. }
            if viewport.iter().any(|line| line.contains("initial-plugin-content")))));
        drop(messages);
        paced.client_messages.lock().unwrap().clear();
        let _ = rendered_bytes(&paced.to_server);
        for (tab, plugin, _) in panes {
            paced
                .screen
                .tabs
                .get_mut(&tab)
                .unwrap()
                .handle_plugin_bytes(plugin, CLIENT, b"updated-plugin-content".to_vec())
                .unwrap();
        }
        paced.screen.render().unwrap();
        *paced.backlog.lock().unwrap() = vec![
            (CLIENT, behind()),
            (OTHER_CLIENT, behind()),
            (subscriber, backlog(0)),
        ];
        paced_render(&mut paced.screen);
        assert_eq!(rendered_bytes(&paced.to_server), "");
        let messages = paced.client_messages.lock().unwrap();
        let updates = &messages[&subscriber];
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|message| matches!(message,
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
            if viewport.iter().any(|line| line.contains("updated-plugin-content")))));
    }
}

#[test]
fn an_idle_watched_tab_does_not_bypass_a_slow_terminals_backlog() {
    let mut paced = screen_with_backlog();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    // Production also registers watcher clients with the screen. They must not
    // enter the regular-client readiness check through that second membership.
    paced.screen.add_client(OTHER_CLIENT).unwrap();
    // Watchers can retain a departed followed client; without its active-tab entry
    // the renderer uses tab zero, while the attached terminal is on tab one.
    paced.screen.set_followed_client(ClientId::new(99)).unwrap();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(2), b"unwatched-change".to_vec())
        .unwrap();

    paced_render(&mut paced.screen);
    assert!(paced
        .to_server
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, ServerInstruction::Render(_))));

    // A change to the followed tab must still wake the healthy watcher.
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"watched-change".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let frames: String = paced
        .to_server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
            _ => None,
        })
        .collect();
    assert!(frames.contains("watched-change"), "{frames:?}");
}

#[test]
fn caption_changes_wake_a_watcher_and_idle_frames_resume_pacing() {
    for pane_id in [
        PaneId::Terminal(TerminalId::new(2)),
        PaneId::Plugin(crate::PluginId::new(2)),
    ] {
        let mut paced = screen_with_backlog();
        let (plugin_sender, _plugin_receiver) = channels::unbounded();
        let tab = paced.screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
        // The shared fixture hides all pane frames; this test exercises visible titles.
        tab.set_pane_frames(PaneFrameStyle::Full);
        add_pane_to_tab(&mut paced.screen, TabId::new(0), pane_id, false);
        if let PaneId::Plugin(plugin) = pane_id {
            let departed_client = ClientId::new(3);
            paced.screen.add_client(departed_client).unwrap();
            paced
                .screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .handle_plugin_bytes(plugin, departed_client, b"departed-viewer".to_vec())
                .unwrap();
            paced.screen.render_to_clients().unwrap();
            paced.screen.remove_client(departed_client).unwrap();
            // Removing the client force-renders every cached plugin grid. Its
            // now-inactive grid must not keep the watched client's frame ready.
        }
        paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
        paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
        paced.screen.set_followed_client(CLIENT).unwrap();
        paced.screen.render_to_clients().unwrap();
        let _ = rendered_bytes(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];

        paced
            .screen
            .with_pane(pane_id, |pane| pane.set_title("changed-caption".into()))
            .unwrap();
        paced_render(&mut paced.screen);
        let watcher_frame: String = paced
            .to_server
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
                _ => None,
            })
            .collect();
        assert!(
            watcher_frame.contains("changed-caption"),
            "{pane_id:?}: {watcher_frame:?}"
        );
        paced
            .screen
            .with_pane(pane_id, |pane| pane.set_title("changed-caption".into()))
            .unwrap();
        for highlight in [true, false] {
            let set_override = |pane: &mut dyn Pane| {
                if highlight {
                    pane.add_highlight_pane_frame_color_override(
                        Some("highlighted-caption".into()),
                        Some(CLIENT),
                    );
                } else {
                    pane.clear_pane_frame_color_override(Some(CLIENT));
                }
            };
            paced.screen.with_pane(pane_id, set_override).unwrap();
            paced_render(&mut paced.screen);
            let frames = rendered_bytes(&paced.to_server);
            assert!(
                frames.contains(if highlight {
                    "highlighted-caption"
                } else {
                    "changed-caption"
                }),
                "{frames:?}"
            );
            paced.screen.with_pane(pane_id, set_override).unwrap();
        }
    }
}

#[test]
fn watcher_replays_display_clear_created_during_stack_list_rendering() {
    let mut paced = screen_with_backlog();
    *paced.screen.stacked_pane_list.borrow_mut() = false;
    let collapsed = PaneId::Terminal(TerminalId::new(1));
    let tab = paced.screen.get_active_tab_mut(CLIENT).unwrap();
    tab.new_stacked_pane(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        false,
        true,
        Some(collapsed),
        Some(CLIENT),
        None,
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(TerminalId::new(2), b"stack-content".to_vec())
        .unwrap();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced_render(&mut paced.screen);
    let initial = rendered_by_client(&paced.to_server);
    for client in [CLIENT, OTHER_CLIENT] {
        assert!(initial[&client].contains("stack-content"));
    }
    assert!(!paced.screen.tabs[&TabId::new(0)].should_clear_display_before_rendering());
    assert!(!paced.screen.watcher_clients[&OTHER_CLIENT].should_force_render());
    assert!(paced.screen.tabs[&TabId::new(0)]
        .get_pane_with_id(collapsed)
        .unwrap()
        .current_geom()
        .rows
        .is_fixed());

    // The regular render adopts the classic stack and consumes its new clear flag.
    *paced.screen.stacked_pane_list.borrow_mut() = true;
    paced_render(&mut paced.screen);
    assert!(paced.screen.tabs[&TabId::new(0)].pane_is_hidden_stack_list_member(&collapsed));
    let frames = rendered_by_client(&paced.to_server);
    for client in [CLIENT, OTHER_CLIENT] {
        let frame = &frames[&client];
        assert!(frame.contains("\x1b[2J"), "{client}: {frame:?}");
        assert!(frame.contains("stack-content"), "{client}: {frame:?}");
    }
}

#[test]
fn a_clear_on_another_tab_does_not_clear_the_watcher() {
    let mut paced = screen_with_backlog();
    let watched_client = ClientId::new(3);
    paced.screen.add_client(watched_client).unwrap();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    assert_eq!(paced.screen.active_tab_ids[&watched_client], TabId::new(0));
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(watched_client).unwrap();
    paced_render(&mut paced.screen);
    let _ = rendered_by_client(&paced.to_server);

    paced
        .screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .set_should_clear_display_before_rendering();
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"watched-change".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let frames = rendered_by_client(&paced.to_server);
    assert!(frames[&CLIENT].contains("\x1b[2J"));
    for client in [watched_client, OTHER_CLIENT] {
        assert!(!frames[&client].contains("\x1b[2J"), "{frames:?}");
        assert!(frames[&client].contains("watched-change"), "{frames:?}");
    }
}

#[test]
fn collapsed_stack_titles_wake_watchers_without_losing_hidden_content() {
    for stack_list in [false, true] {
        let mut paced = screen_with_backlog();
        *paced.screen.stacked_pane_list.borrow_mut() = stack_list;
        let collapsed = PaneId::Terminal(TerminalId::new(1));
        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .new_stacked_pane(
                PaneId::Terminal(TerminalId::new(2)),
                None,
                None,
                false,
                true,
                Some(collapsed),
                Some(CLIENT),
                None,
                None,
            )
            .unwrap();
        if stack_list {
            assert!(paced.screen.tabs[&TabId::new(0)].pane_is_hidden_stack_list_member(&collapsed));
        } else {
            assert!(paced.screen.tabs[&TabId::new(0)]
                .get_pane_with_id(collapsed)
                .unwrap()
                .current_geom()
                .rows
                .is_fixed());
        }
        paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
        paced.screen.set_followed_client(CLIENT).unwrap();
        paced.screen.render_to_clients().unwrap();
        let _ = rendered_bytes(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];

        paced
            .screen
            .with_pane(collapsed, |pane| pane.set_title("collapsed-caption".into()))
            .unwrap();
        paced_render(&mut paced.screen);
        let watcher_frame: String = paced
            .to_server
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
                _ => None,
            })
            .collect();
        assert!(
            watcher_frame.contains("collapsed-caption"),
            "{watcher_frame:?}"
        );

        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), b"hidden-content-survives".to_vec())
            .unwrap();
        paced_render(&mut paced.screen);
        let unexpected: Vec<_> = paced
            .to_server
            .try_iter()
            .filter(|(instruction, _)| matches!(instruction, ServerInstruction::Render(_)))
            .collect();
        // Stack-list chrome may clear its spacer row on a render pass, but hidden
        // content must remain unconsumed until this member is expanded.
        assert!(
            unexpected.iter().all(|(instruction, _)| match instruction {
                ServerInstruction::Render(Some(output)) => output
                    .values()
                    .all(|frame| !frame.contains("hidden-content-survives")),
                _ => true,
            }),
            "{unexpected:?}"
        );
        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), b"\x1b]2;osc-caption\x07".to_vec())
            .unwrap();
        paced_render(&mut paced.screen);
        assert!(rendered_bytes(&paced.to_server).contains("osc-caption"));
        paced
            .screen
            .with_pane(collapsed, |pane| pane.set_bell_notification(true))
            .unwrap();
        paced_render(&mut paced.screen);
        let _ = rendered_bytes(&paced.to_server);
        paced
            .screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .focus_pane_with_id(collapsed, false, false, CLIENT)
            .unwrap();
        paced_render(&mut paced.screen);
        assert!(rendered_bytes(&paced.to_server).contains("hidden-content-survives"));
    }
}

#[test]
fn fullscreen_hidden_floating_panes_do_not_keep_idle_watchers_ready() {
    let mut paced = screen_with_backlog();
    let floating = PaneId::Terminal(TerminalId::new(2));
    add_pane_to_tab(&mut paced.screen, TabId::new(0), floating, true);
    paced
        .screen
        .with_pane(floating, |pane| pane.set_pinned(true))
        .unwrap();
    let tab = paced.screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.toggle_pane_no_ui_fullscreen(PaneId::Terminal(TerminalId::new(1)));
    assert!(tab.fullscreen_covers_ui());
    assert!(!tab.has_floating_panes_to_render());
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0))];
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(2), b"hidden-floater-change".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    assert!(paced
        .to_server
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, ServerInstruction::Render(_))));

    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .toggle_pane_no_ui_fullscreen(PaneId::Terminal(TerminalId::new(1)));
    paced_render(&mut paced.screen);
    assert!(rendered_bytes(&paced.to_server).contains("hidden-floater-change"));
}

#[test]
fn unavailable_backlog_does_not_panic_or_hold_back_the_screen() {
    let mut paced = screen_with_backlog();
    let backlog = paced.backlog.clone();
    assert!(std::thread::spawn(move || {
        let _guard = backlog.lock().unwrap();
        panic!("poison the backlog query");
    })
    .join()
    .is_err());
    assert!(paced.screen.deferred_tabs().is_empty());
    paced.screen.render_to_clients().unwrap();
}

#[test]
fn a_frame_skipped_while_behind_is_retried_and_carried_by_the_next_render() {
    let mut paced = screen_with_backlog();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    let _ = render_retries_requested(&paced.to_background_jobs);

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    let tab = paced.screen.get_active_tab_mut(CLIENT).unwrap();
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[3;1Hskipped-frame".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    assert_eq!(rendered_bytes(&paced.to_server), "");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);

    // More output arrives before the client drains. The retried frame must carry both.
    let tab = paced.screen.get_active_tab_mut(CLIENT).unwrap();
    tab.handle_pty_bytes(TerminalId::new(1), b"\x1b[5;1Hnext-frame".to_vec())
        .unwrap();
    paced.backlog.lock().unwrap().clear();
    paced_render(&mut paced.screen);
    // These renders run synchronously, so the channel holds every byte of the frame.
    let frame = rendered_bytes(&paced.to_server);
    assert!(frame.contains("skipped-frame"), "{frame:?}");
    assert!(frame.contains("next-frame"), "{frame:?}");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);
}

#[test]
fn writer_notification_resumes_deferred_frames_through_the_debouncer() {
    let mut paced = screen_with_backlog();
    paced_render(&mut paced.screen);
    let _ = rendered_bytes(&paced.to_server);
    let _ = render_retries_requested(&paced.to_background_jobs);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"notification-resumed".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let jobs: Vec<_> = paced
        .to_background_jobs
        .try_iter()
        .map(|(job, _)| job)
        .collect();
    assert!(!jobs.contains(&BackgroundJob::RenderToClients));
    paced_render(&mut paced.screen);
    assert!(paced.to_background_jobs.try_iter().next().is_none());
    assert_eq!(rendered_bytes(&paced.to_server), "");

    let first_ready = paced.ready.lock().unwrap()[&CLIENT].clone();
    paced_render(&mut paced.screen);
    assert!(Arc::ptr_eq(
        &first_ready,
        &paced.ready.lock().unwrap()[&CLIENT]
    ));

    paced.backlog.lock().unwrap().clear();
    let ready = paced.ready.lock().unwrap().remove(&CLIENT).unwrap();
    ready();
    assert!(paced
        .to_screen
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ScreenInstruction::RenderWriterReady)));
    paced.screen.resume_deferred_render();
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 1);
    paced_render(&mut paced.screen);
    assert!(rendered_bytes(&paced.to_server).contains("notification-resumed"));
    assert!(paced.screen.deferred_render_tabs.is_empty());
    let _ = render_retries_requested(&paced.to_background_jobs);
    paced.screen.resume_deferred_render();
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);
}

#[test]
fn disconnect_releases_deferred_work() {
    let mut paced = screen_with_backlog();
    paced_render(&mut paced.screen);
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"pending-frame".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    assert!(!paced.screen.deferred_render_tabs.is_empty());
    let _ = render_retries_requested(&paced.to_background_jobs);
    paced.screen.remove_client(CLIENT).unwrap();
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 1);
    paced_render(&mut paced.screen);
    assert!(paced.screen.deferred_render_tabs.is_empty());
}

#[test]
fn watcher_replays_shared_boundaries_after_a_split() {
    let mut paced = screen_with_backlog();
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced_render(&mut paced.screen);
    let _ = rendered_bytes(&paced.to_server);
    add_pane_to_tab(
        &mut paced.screen,
        TabId::new(0),
        PaneId::Terminal(TerminalId::new(2)),
        false,
    );
    paced_render(&mut paced.screen);
    let frames: String = paced
        .to_server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => output.get(&OTHER_CLIENT).cloned(),
            _ => None,
        })
        .collect();
    assert!(frames.contains('│'), "{frames:?}");
}

#[test]
fn output_on_another_tab_does_not_repaint_watched_contents() {
    let mut paced = screen_with_backlog();
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"unchanged-watched-content".to_vec())
        .unwrap();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_watcher_size(OTHER_CLIENT, SIZE);
    paced.screen.set_followed_client(ClientId::new(99)).unwrap();
    paced_render(&mut paced.screen);
    let _ = rendered_bytes(&paced.to_server);
    paced
        .screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(2), b"other-tab-content".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let frames: Vec<_> = paced
        .to_server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => Some(output),
            _ => None,
        })
        .collect();
    assert!(frames.iter().any(|frame| frame
        .get(&CLIENT)
        .is_some_and(|s| s.contains("other-tab-content"))));
    assert!(frames
        .iter()
        .all(|frame| !frame.contains_key(&OTHER_CLIENT)));
}

#[test]
fn queued_frames_keep_each_tabs_pending_work_deferred() {
    let mut paced = screen_with_backlog();
    paced.screen.add_client(OTHER_CLIENT).unwrap();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
    assert_eq!(
        paced.screen.deferred_tabs(),
        &HashSet::from([TabId::new(0), TabId::new(1)])
    );
}

/// Every client's bytes from the frames sent so far.
fn rendered_by_client(receiver: &ServerReceiver) -> HashMap<ClientId, String> {
    let mut by_client: HashMap<ClientId, String> = HashMap::new();
    for (instruction, _) in receiver.try_iter() {
        if let ServerInstruction::Render(Some(output)) = instruction {
            for (client_id, bytes) in output {
                by_client.entry(client_id).or_default().push_str(&bytes);
            }
        }
    }
    by_client
}

/// The last absolute cursor move in `bytes`, as `(row, column)`.
fn last_goto(bytes: &str) -> Option<(usize, usize)> {
    bytes
        .match_indices("\x1b[")
        .filter_map(|(start, _)| {
            let rest = &bytes[start + 2..];
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == ';'))?;
            if !rest[end..].starts_with('H') {
                return None;
            }
            let (row, column) = rest[..end].split_once(';')?;
            Some((row.parse().ok()?, column.parse().ok()?))
        })
        .last()
}

fn show_toast(screen: &mut Screen) {
    use crate::screen::Notification;
    use yinz_types::animation::Tween;
    use yinz_types::internal_ids::NotificationId;
    let mut toast = Notification::new(
        NotificationId::new(0),
        "build finished".to_owned(),
        None,
        None,
        std::time::Instant::now(),
    );
    toast.reveal = Tween::settled(1.0);
    toast.y_offset = Tween::settled(0.0);
    screen.notifications.push(toast);
}

fn open_menu(screen: &mut Screen, client_id: ClientId) {
    use yinz_types::data::{ContextMenuContext, ContextMenuItem};
    use yinz_types::input::actions::Action;
    let request_id = screen.open_context_menu(
        ContextMenuContext {
            pane_id: PaneId::Terminal(TerminalId::new(1)),
            click: (40, 10),
            ..Default::default()
        },
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem::new("Copy", vec![Action::Copy])],
        Some(crate::PluginId::new(7)),
        client_id,
    );
}

/// A held-back client gets no frame at all, overlays included: toasts, menus and palettes paint
/// only for clients a rendered tab put in the frame, so none can leave the cursor on its last
/// goto. Once the client catches up, the overlay arrives with the pane and the cursor where it
/// belongs.
#[test]
fn overlays_wait_with_a_held_back_tab_and_arrive_with_its_cursor() {
    for overlay in ["toast", "menu", "palette"] {
        let mut paced = screen_with_backlog();
        paced.screen.add_client(OTHER_CLIENT).unwrap();
        paced
            .screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), b"\x1b[4;7H".to_vec())
            .unwrap();
        paced_render(&mut paced.screen);
        let pane_cursor = last_goto(&rendered_bytes(&paced.to_server)).expect("cursor placed");
        let _ = rendered_by_client(&paced.to_server);
        *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
        // Written without moving the cursor the host was last shown.
        paced
            .screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), b"\x1b7held-back\x1b8".to_vec())
            .unwrap();
        match overlay {
            "toast" => show_toast(&mut paced.screen),
            "menu" => open_menu(&mut paced.screen, CLIENT),
            _ => paced.screen.toggle_command_palette(CLIENT),
        }
        for _ in 0..2 {
            paced_render(&mut paced.screen);
            assert!(
                paced.screen.deferred_render_tabs.contains(&TabId::new(0)),
                "{overlay}"
            );
            assert_eq!(
                rendered_by_client(&paced.to_server),
                HashMap::new(),
                "{overlay}"
            );
        }

        paced.backlog.lock().unwrap().clear();
        paced_render(&mut paced.screen);
        let frame = rendered_bytes(&paced.to_server);
        assert!(frame.contains("held-back"), "{overlay}: {frame:?}");
        match overlay {
            "toast" => assert!(frame.contains("build finished"), "{frame:?}"),
            "menu" => assert!(frame.contains("Copy"), "{frame:?}"),
            // The palette places its own cursor, in its input, after everything else.
            _ => assert!(frame.ends_with("\x1b[2 q\x1b[?25h"), "{frame:?}"),
        }
        if overlay != "palette" {
            assert_eq!(last_goto(&frame), Some(pane_cursor), "{overlay}: {frame:?}");
        }
    }
}

/// A held-back client that still owes its host a Kitty delete keeps it for its own next frame,
/// rather than making every pass dirty and sending everyone else empty frames meanwhile.
#[test]
fn deletes_owed_by_a_held_back_client_wait_for_its_next_frame() {
    use crate::output::{HostImageRecord, HostKittyState};
    let mut paced = screen_with_backlog();
    paced.screen.add_client(OTHER_CLIENT).unwrap();
    new_tab(&mut paced.screen, 2, TabId::new(1));
    assert_ne!(
        paced.screen.active_tab_ids[&CLIENT],
        paced.screen.active_tab_ids[&OTHER_CLIENT]
    );
    paced_render(&mut paced.screen);
    let _ = rendered_by_client(&paced.to_server);
    paced
        .screen
        .kitty_host_capabilities
        .borrow_mut()
        .insert(CLIENT, true);
    // An image the host still holds, and the store has since freed.
    let mut host_state = HostKittyState::default();
    host_state.transmitted.insert(
        (1, None),
        HostImageRecord {
            host_image_id: 9,
            raster_revision: 0,
        },
    );
    paced
        .screen
        .client_kitty_host_state
        .borrow_mut()
        .insert(CLIENT, host_state);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    paced_render(&mut paced.screen);
    paced_render(&mut paced.screen);
    assert_eq!(rendered_by_client(&paced.to_server), HashMap::new());

    paced.backlog.lock().unwrap().clear();
    paced_render(&mut paced.screen);
    let frames = rendered_by_client(&paced.to_server);
    assert!(
        frames
            .get(&CLIENT)
            .is_some_and(|frame| frame.contains("\x1b_Ga=d,q=2,d=I,i=9")),
        "{frames:?}"
    );
    assert!(!frames.contains_key(&OTHER_CLIENT), "{frames:?}");
}

/// Bells go to regular clients. A watcher's stream is the watcher pass's: a bell reaching it from
/// the regular pass would be a frame of its own.
#[test]
fn bells_do_not_reach_watchers_through_the_regular_pass() {
    let mut paced = screen_with_backlog();
    let watcher = ClientId::new(3);
    paced.screen.add_watcher_client(watcher).unwrap();
    paced.screen.add_client(watcher).unwrap();
    paced_render(&mut paced.screen);
    let _ = rendered_by_client(&paced.to_server);
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"ring\x07".to_vec())
        .unwrap();
    paced_render(&mut paced.screen);
    let frames = rendered_by_client(&paced.to_server);
    assert!(
        frames
            .get(&CLIENT)
            .is_some_and(|frame| frame.contains('\x07')),
        "{frames:?}"
    );
    assert!(
        !frames
            .get(&watcher)
            .is_some_and(|frame| frame.contains('\x07')),
        "{frames:?}"
    );
}
