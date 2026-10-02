use super::*;
use std::time::Duration;

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
    client_messages: CapturedClientMessages,
}

fn screen_with_backlog() -> PacedScreen {
    let (mut screen, _, to_server) = create_new_screen_with_capture(SIZE, true, true, true, false);
    let fake_os_input = FakeInputOutput::default();
    let backlog = fake_os_input.render_backlog.clone();
    let client_messages = fake_os_input.server_to_client_messages.clone();
    screen.bus.os_input = Some(Box::new(fake_os_input));
    let (to_background_jobs, background_jobs_receiver) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    new_tab(&mut screen, 1, TabId::new(0));
    PacedScreen {
        screen,
        to_server,
        to_background_jobs: background_jobs_receiver,
        backlog,
        client_messages,
    }
}

fn backlog(queued_renders: usize, in_flight_for: Option<Duration>) -> RenderBacklog {
    RenderBacklog {
        queued_renders,
        in_flight_for,
    }
}

fn behind() -> RenderBacklog {
    backlog(1, Some(Duration::from_millis(50)))
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
    if !screen.defer_render_while_clients_are_behind() {
        screen.render_to_clients().unwrap();
    }
}

#[test]
fn a_client_with_a_queued_frame_holds_back_rendering() {
    let paced = screen_with_backlog();
    assert!(!paced.screen.every_client_is_behind_on_renders());

    // Writing the only frame it has is keeping up.
    *paced.backlog.lock().unwrap() = vec![(CLIENT, backlog(0, Some(Duration::from_secs(1))))];
    assert!(!paced.screen.every_client_is_behind_on_renders());

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind())];
    assert!(paced.screen.every_client_is_behind_on_renders());
}

#[test]
fn a_client_stalled_on_one_write_stops_holding_back_rendering() {
    let paced = screen_with_backlog();
    *paced.backlog.lock().unwrap() = vec![(CLIENT, backlog(3, Some(Duration::from_secs(60))))];
    assert!(!paced.screen.every_client_is_behind_on_renders());
}

#[test]
fn one_slow_client_does_not_hold_back_the_others() {
    let mut paced = screen_with_backlog();
    paced.screen.add_client(OTHER_CLIENT).unwrap();
    // Attaching asks for a frame of its own.
    let _ = render_retries_requested(&paced.to_background_jobs);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0, None))];
    assert!(!paced.screen.every_client_is_behind_on_renders());
    assert!(!paced.screen.defer_render_while_clients_are_behind());
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);

    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, behind())];
    assert!(paced.screen.defer_render_while_clients_are_behind());
}

#[test]
fn a_slow_regular_client_does_not_hold_back_a_healthy_watcher() {
    let mut paced = screen_with_backlog();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.set_followed_client(CLIENT).unwrap();
    paced.screen.render_to_clients().unwrap();
    let _ = rendered_bytes(&paced.to_server);
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0, None))];

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
    assert!(paced.screen.defer_render_while_clients_are_behind());
}

#[test]
fn watcher_only_sessions_pace_until_the_watcher_drains_or_detaches() {
    let mut paced = screen_with_backlog();
    paced.screen.add_watcher_client(OTHER_CLIENT).unwrap();
    paced.screen.remove_client(CLIENT).unwrap();
    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
    assert!(paced.screen.defer_render_while_clients_are_behind());

    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, backlog(0, None))];
    assert!(!paced.screen.defer_render_while_clients_are_behind());
    *paced.backlog.lock().unwrap() = vec![(OTHER_CLIENT, behind())];
    paced.screen.remove_watcher_client(OTHER_CLIENT);
    assert!(!paced.screen.defer_render_while_clients_are_behind());
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
    paced.client_messages.lock().unwrap().clear();
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0, None))];
    paced
        .screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"subscriber-still-updates".to_vec())
        .unwrap();

    paced_render(&mut paced.screen);
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

    paced_render(&mut paced.screen);
    assert!(paced.client_messages.lock().unwrap().is_empty());
    assert_eq!(rendered_bytes(&paced.to_server), "");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 1);

    // The attached terminal is still slow, but the subscriber is ready again.
    *paced.backlog.lock().unwrap() = vec![(CLIENT, behind()), (OTHER_CLIENT, backlog(0, None))];
    paced_render(&mut paced.screen);
    let messages = paced.client_messages.lock().unwrap();
    let subscriber_messages = messages.get(&OTHER_CLIENT).expect("subscriber update");
    assert!(subscriber_messages.iter().any(|message| {
        matches!(message, ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
            if viewport.iter().any(|line| line.contains("deferred-subscriber-update")))
    }), "{subscriber_messages:?}");
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 0);
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
    assert_eq!(render_retries_requested(&paced.to_background_jobs), 1);

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
