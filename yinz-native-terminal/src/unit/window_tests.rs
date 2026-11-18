use super::*;
use crate::emulation::selection::Selection;
use yinz_ipc::ipc::ExitReason;

#[test]
fn a_render_frame_is_applied_after_a_recoverable_error() {
    use yinz_types::structured_render::{FrameBuilder, WireCell};

    let (sender, messages) = mpsc::sync_channel(2);
    sender
        .send(ServerToClientMsg::LogError {
            lines: vec!["recoverable server error".into()],
        })
        .unwrap();
    let mut frame = FrameBuilder::new(8, 2, 1);
    frame.push_row(
        0,
        0,
        &[WireCell {
            ch: 'x' as u32,
            ..WireCell::BLANK
        }],
    );
    sender
        .send(ServerToClientMsg::RenderFrame {
            frame: frame.finish(),
        })
        .unwrap();

    assert!(receive_message(Some(&messages)).unwrap().is_none());
    let Some(ServerToClientMsg::RenderFrame { frame }) = receive_message(Some(&messages)).unwrap()
    else {
        panic!("expected the next render frame");
    };
    let mut state = TerminalState::new(2, 8);
    assert_eq!(state.apply_frame(&frame).unwrap().seq, 1);
    assert_eq!(state.screen().cell(0, 0).character(), 'x');
}

#[test]
fn recoverable_errors_allow_subsequent_messages() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::LogError {
            lines: vec!["plugin failed to load".into(), "layout rejected".into()],
        })
        .unwrap();
    sender
        .send(ServerToClientMsg::SetMouseMode { enabled: true })
        .unwrap();
    assert!(receive_message(Some(&messages)).unwrap().is_none());
    assert!(matches!(
        receive_message(Some(&messages)).unwrap(),
        Some(ServerToClientMsg::SetMouseMode { enabled: true })
    ));
    assert!(receive_message(Some(&messages)).unwrap().is_none());
}

#[test]
fn explicit_error_exit_remains_fatal() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Error("connection failed".into()),
        })
        .unwrap();
    let error = receive_message(Some(&messages)).unwrap_err();
    assert_eq!(error.to_string(), "connection failed");
}

#[test]
fn disconnected_connection_remains_fatal() {
    let (sender, messages) = mpsc::channel();
    drop(sender);
    let error = receive_message(Some(&messages)).unwrap_err();
    assert!(error.to_string().contains("closed unexpectedly"));
}

#[test]
fn normal_exit_is_forwarded_to_the_window() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        })
        .unwrap();
    assert!(matches!(
        receive_message(Some(&messages)).unwrap(),
        Some(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal
        })
    ));
}

#[cfg(unix)]
fn switch_peer() -> (
    AttachedSender,
    yinz_ipc::ipc::IpcReceiverWithContext<ClientToServerMsg>,
) {
    use interprocess::os::unix::uds_local_socket::Stream;
    let (local, peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let local = yinz_ipc::ipc::IpcSenderWithContext::new(Stream::from(local).into());
    let peer = yinz_ipc::ipc::IpcReceiverWithContext::new(Stream::from(peer).into());
    peer.set_recv_timeout(Some(Duration::from_secs(3))).unwrap();
    (AttachedSender(Arc::new(Mutex::new(local))), peer)
}

#[cfg(unix)]
fn switch_app(opts: CliArgs) -> App {
    let args = TerminalArgs {
        session_name: Some("source".into()),
        rows: Some(2),
        cols: Some(18),
        ..Default::default()
    };
    let mut config = Config::default();
    config.terminal.system_fonts = Some(false);
    let mut app = App::new(
        args,
        opts,
        (config, Options::default()),
        Arc::new(|_| true),
        Clipboard::in_memory,
    )
    .unwrap();
    crate::testing::painter::Painter::apply(&mut app.state, |p| p.text(0, 0, "source content"));
    app.selection.start(0, 0);
    app.selection.extend(0, 5);
    app.connection_geometry.received_frame = true;
    app
}

#[cfg(unix)]
fn stage_test_switch(
    app: &mut App,
    id: u64,
    now: Instant,
    opened: Result<(String, AttachedSender)>,
) -> mpsc::Sender<ServerToClientMsg> {
    let (opening_tx, opening) = mpsc::channel();
    opening_tx.send(opened).unwrap();
    let (tx, messages) = mpsc::channel();
    app.pending_switch = Some(PendingSwitch {
        id,
        deadline: now + FIRST_FRAME_TIMEOUT,
        destination: "destination".into(),
        opening: Some(opening),
        sender: None,
        messages,
        mouse_mode: true,
        reload: false,
    });
    tx
}

#[cfg(unix)]
fn destination_frame() -> ServerToClientMsg {
    use crate::testing::painter::sixel_chunk;
    use yinz_types::structured_render::{FrameBuilder, WireCell};
    let mut frame = FrameBuilder::new(18, 2, 9);
    frame.set_full_repaint(true);
    frame.push_row(
        0,
        0,
        &[WireCell {
            ch: 'D' as u32,
            ..WireCell::BLANK
        }],
    );
    frame.extend_graphics(&[sixel_chunk(
        0,
        1,
        8,
        6,
        "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\",
    )]);
    ServerToClientMsg::RenderFrame {
        frame: frame.finish(),
    }
}

#[cfg(unix)]
fn begin_test_switch(
    app: &mut App,
    name: Option<&str>,
    now: Instant,
) -> (
    mpsc::Sender<Result<(String, AttachedSender)>>,
    mpsc::Sender<ServerToClientMsg>,
) {
    let (opened, opening) = mpsc::channel();
    let (events, messages) = mpsc::channel();
    app.begin_switch(
        SessionConnection {
            name: name.map(Into::into),
            ..Default::default()
        },
        |_, id, _, destination| {
            Ok(PendingSwitch {
                id,
                deadline: now + FIRST_FRAME_TIMEOUT,
                destination,
                opening: Some(opening),
                sender: None,
                messages,
                mouse_mode: true,
                reload: false,
            })
        },
    );
    (opened, events)
}

#[cfg(unix)]
#[test]
fn native_switch_pending_notice_tracks_request_and_resolution() {
    let mut app = switch_app(CliArgs::default());
    let now = Instant::now();
    let next_draw = now + app.pacer.interval();
    let target = (768, 384);
    let assert_overlay = |app: &mut App, text| {
        let expected = notice_scene(Notice::Failure(text), &mut app.cache, target, 0);
        assert_eq!(app.switch_overlay(target, 0), Some(expected));
    };
    app.pacer.drawn(now);
    assert!(app.switch_overlay(target, 0).is_none());
    let (opened, _events) = begin_test_switch(&mut app, None, now);
    assert_eq!(app.pacer.decide(now), Decision::At(next_draw));
    assert_overlay(
        &mut app,
        "Esc cancels\nOpening: a new session\nInput: source",
    );

    app.pacer.drawn(next_draw);
    app.begin_switch(
        SessionConnection {
            name: Some("repeated request".into()),
            ..Default::default()
        },
        |_, _, _, _| panic!("a second request must not start another switch"),
    );
    assert_eq!(app.next_connection_id, 2);
    assert_eq!(app.pacer.decide(next_draw), Decision::Idle);
    assert_overlay(
        &mut app,
        "Esc cancels\nOpening: a new session\nInput: source",
    );

    let (destination, _peer) = switch_peer();
    opened.send(Ok(("resolved".into(), destination))).unwrap();
    app.receive_switch(1, next_draw).unwrap();
    let resolved_draw = next_draw + app.pacer.interval();
    assert_eq!(app.pacer.decide(next_draw), Decision::At(resolved_draw));
    assert_overlay(&mut app, "Esc cancels\nOpening: resolved\nInput: source");

    app.pacer.drawn(resolved_draw);
    let escape = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::empty());
    app.key_pressed(&escape, false).unwrap();
    assert!(app.switch_overlay(target, 0).is_none());
    assert_eq!(
        app.pacer.decide(resolved_draw),
        Decision::At(resolved_draw + app.pacer.interval())
    );
    let (_opened, _events) = begin_test_switch(&mut app, Some("named"), now);
    assert_overlay(&mut app, "Esc cancels\nOpening: named\nInput: source");
}

#[cfg(unix)]
#[test]
fn native_switch_pending_escape_cancels_without_detaching_source() {
    use winit::event::Ime;
    use yinz_types::data::{BareKey, KeyModifier};

    let mut app = switch_app(CliArgs::default());
    let (source, mut source_peer) = switch_peer();
    app.sender = Some(source);
    app.focused = false;
    let reported_geometry = Geometry::calculate(768, 384, 0, app.cache.metrics());
    assert!(reported_geometry.is_some());
    app.connection_geometry.reported = reported_geometry;
    let geometry = app.state.geometry().clone();
    let now = Instant::now();
    let (opened, events) = begin_test_switch(&mut app, Some("destination"), now);
    let (destination, mut destination_peer) = switch_peer();
    opened
        .send(Ok(("destination".into(), destination)))
        .unwrap();
    app.receive_switch(1, now).unwrap();
    events.send(destination_frame()).unwrap();

    let modified = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::SHIFT);
    app.key_pressed(&modified, false).unwrap();
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::Key { key, .. }, _))
            if key.bare_key == BareKey::Esc && key.key_modifiers.contains(&KeyModifier::Shift)
    ));
    assert!(app.pending_switch.is_some());
    app.selection.start(0, 0);
    app.selection.extend(0, 5);
    let selection = app.selection.range();
    let copied = app.selection.text(app.state.screen()).unwrap();
    app.composition
        .ime(Ime::Preedit("editing".into(), Some((0, 7))));
    let composition = app.composition.shown();

    let escape = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::empty());
    app.key_pressed(&escape, false).unwrap();
    app.key_pressed(&escape, true).unwrap();
    app.receive_switch(1, now).unwrap();
    assert!(app.pending_switch.is_none());
    assert!(app.switch_notice.is_none());
    assert!(events.send(destination_frame()).is_err());
    assert!(matches!(
        destination_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(destination_peer.recv_client_msg().is_eof());

    assert_eq!(app.connection_id, 0);
    assert_eq!(app.args.session_name.as_deref(), Some("source"));
    assert_eq!(app.connection_geometry.reported, reported_geometry);
    assert_eq!(app.state.geometry(), &geometry);
    assert_eq!(app.state.screen().cell(0, 0).character(), 's');
    assert!(app.connection_geometry.received_frame);
    assert!(!app.focused);
    assert_eq!(app.selection.range(), selection);
    assert_eq!(app.composition.shown(), composition);
    app.composition.ime(Ime::Disabled);
    let copy = keyboard::Press::new(Key::Character("c".into()), ModifiersState::SUPER);
    app.key_pressed(&copy, false).unwrap();
    assert_eq!(app.get_clipboard().as_deref(), Some(copied.as_str()));
    assert_eq!(app.selection.range(), selection);

    let key = keyboard::Press::new(Key::Character("x".into()), ModifiersState::empty());
    app.key_pressed(&key, false).unwrap();
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::Key { key, .. }, _)) if key.bare_key == BareKey::Char('x')
    ));
    app.key_pressed(&escape, false).unwrap();
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::Key { key, .. }, _))
            if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty()
    ));
    drop(app);
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(source_peer.recv_client_msg().is_eof());
}

#[cfg(unix)]
#[test]
fn native_switch_pending_escape_discards_late_open_and_allows_retry() {
    use yinz_types::data::BareKey;

    let mut app = switch_app(CliArgs::default());
    let (source, mut source_peer) = switch_peer();
    app.sender = Some(source);
    let now = Instant::now();
    let (old_opened, old_events) = begin_test_switch(&mut app, Some("canceled"), now);
    let escape = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::empty());
    app.key_pressed(&escape, false).unwrap();

    let (opened, events) = begin_test_switch(&mut app, Some("retry"), now);
    app.key_pressed(&escape, true).unwrap();
    assert_eq!(app.pending_switch.as_ref().unwrap().id, 2);
    let (late, mut late_peer) = switch_peer();
    let rejected = old_opened.send(Ok(("canceled".into(), late))).unwrap_err();
    drop(rejected);
    assert!(old_events.send(destination_frame()).is_err());
    assert!(matches!(
        late_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(late_peer.recv_client_msg().is_eof());

    let (retry, mut retry_peer) = switch_peer();
    opened.send(Ok(("retry".into(), retry))).unwrap();
    app.receive_switch(1, now + FIRST_FRAME_TIMEOUT).unwrap();
    app.receive_switch(2, now).unwrap();
    events.send(destination_frame()).unwrap();
    app.receive_switch(1, now).unwrap();
    assert_eq!(app.connection_id, 0);
    app.receive_switch(2, now).unwrap();
    app.receive_switch(1, now).unwrap();
    assert_eq!(app.connection_id, 2);
    assert_eq!(app.args.session_name.as_deref(), Some("retry"));
    assert!(app.pending_switch.is_none());
    assert!(app.switch_overlay((768, 384), 0).is_none());
    assert_eq!(app.state.screen().cell(0, 0).character(), 'D');
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(source_peer.recv_client_msg().is_eof());
    assert!(matches!(
        retry_peer.recv_client_msg().msg(),
        Some((
            ClientToServerMsg::HostTerminalFocusChanged { focused: true },
            _
        ))
    ));
    assert!(matches!(
        retry_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::RenderFrameAck { seq: 9 }, _))
    ));
    app.key_pressed(&escape, false).unwrap();
    assert!(matches!(
        retry_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::Key { key, .. }, _))
            if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty()
    ));
    drop(app);
    assert!(matches!(
        retry_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(retry_peer.recv_client_msg().is_eof());
}

#[cfg(unix)]
#[test]
fn native_switch_refusal_retains_source_and_supports_retry() {
    use std::fs;
    use yinz_types::{
        data::{LayoutInfo, SessionConnection},
        internal_ids::TabNumber,
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    fs::write(&main, "terminal { system_fonts false; }").unwrap();
    let layout = root.join("selected.kdl");
    fs::write(&layout, "layout { pane; }").unwrap();
    let swap = layout.with_extension("swap.kdl");
    fs::create_dir(&swap).unwrap();
    let opts = CliArgs {
        config: Some(main),
        config_dir: Some(root),
        ..Default::default()
    };
    let target = SessionConnection {
        name: Some("destination".into()),
        tab_position: Some(TabNumber::new(2)),
        pane_id: Some(yinz_types::pane_id::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(3),
        )),
        layout: Some(LayoutInfo::File(
            layout.display().to_string(),
            Default::default(),
        )),
        ..Default::default()
    };
    let mut app = switch_app(opts);
    let (source, mut source_peer) = switch_peer();
    app.sender = Some(source);
    let saved_selection = app.selection.range();
    let copied = app.selection.text(app.state.screen()).unwrap();
    let prepare = || {
        session::prepare_connection(
            &app.args,
            &app.opts,
            Size {
                cols: app.state.size().cols,
                rows: app.state.size().rows,
            },
            Some(&target),
            &[("destination".into(), Duration::ZERO)],
        )
    };
    let error = prepare().err().expect("directory swap is refused");
    let now = Instant::now();
    let _events = stage_test_switch(&mut app, 1, now, Err(error));
    app.receive_switch(1, now).unwrap();
    assert!(app.pending_switch.is_none());
    let notice = app.switch_notice.as_deref().unwrap();
    for part in [
        "destination",
        swap.to_str().unwrap(),
        "not a regular file",
        "retry",
        "Esc",
    ] {
        assert!(notice.contains(part), "{notice}");
    }
    assert_eq!(app.args.session_name.as_deref(), Some("source"));
    assert_eq!(app.selection.range(), saved_selection);
    assert_eq!(
        app.selection.text(app.state.screen()).as_deref(),
        Some(copied.as_str())
    );
    let escape = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::empty());
    app.key_pressed(&escape, false).unwrap();
    assert!(app.switch_notice.is_none());
    assert_eq!(app.selection.range(), saved_selection);
    app.type_text("source input".into()).unwrap();
    assert!(
        matches!(source_peer.recv_client_msg().msg(), Some((ClientToServerMsg::Text(text), _)) if text == "source input")
    );

    fs::remove_dir(swap).unwrap();
    let prepared = session::prepare_connection(
        &app.args,
        &app.opts,
        Size {
            cols: app.state.size().cols,
            rows: app.state.size().rows,
        },
        Some(&target),
        &[("destination".into(), Duration::ZERO)],
    )
    .unwrap();
    assert!(matches!(
        prepared.opening,
        ClientToServerMsg::AttachClient { tab_number_to_focus, pane_to_focus, .. }
        if tab_number_to_focus == target.tab_position && pane_to_focus == target.pane_id
    ));
    let (destination, mut destination_peer) = switch_peer();
    let events = stage_test_switch(&mut app, 2, now, Ok(("destination".into(), destination)));
    app.receive_switch(2, now).unwrap();
    events.send(destination_frame()).unwrap();
    app.receive_switch(2, now).unwrap();
    assert_eq!(app.connection_id, 2);
    assert_eq!(app.args.session_name.as_deref(), Some("destination"));
    assert!(app.selection.range().is_none());
    assert_eq!(app.state.screen().cell(0, 0).character(), 'D');
    assert!(!app.state.sixels().is_empty());
    assert!(matches!(
        source_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(source_peer.recv_client_msg().is_eof());
    assert!(matches!(
        destination_peer.recv_client_msg().msg(),
        Some((
            ClientToServerMsg::HostTerminalFocusChanged { focused: true },
            _
        ))
    ));
    assert!(matches!(
        destination_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::RenderFrameAck { seq: 9 }, _))
    ));
}

#[cfg(unix)]
#[test]
fn native_switch_target_failure_keeps_source_until_retry() {
    let mut app = switch_app(CliArgs::default());
    let (source, mut source_peer) = switch_peer();
    app.sender = Some(source);
    let selection = app.selection.range();
    let now = Instant::now();
    let (destination, mut destination_peer) = switch_peer();
    let events = stage_test_switch(&mut app, 1, now, Ok(("destination".into(), destination)));
    app.receive_switch(1, now).unwrap();
    events.send(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Error("failed to load native session configuration and layout: /fixture/selected.kdl: not a regular file".into()),
    }).unwrap();
    app.receive_switch(1, now).unwrap();
    assert!(app
        .switch_notice
        .as_deref()
        .unwrap()
        .contains("/fixture/selected.kdl"));
    assert_eq!(app.selection.range(), selection);
    assert_eq!(app.state.screen().cell(0, 0).character(), 's');
    assert!(matches!(
        destination_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(destination_peer.recv_client_msg().is_eof());
    app.type_text("still attached".into()).unwrap();
    assert!(
        matches!(source_peer.recv_client_msg().msg(), Some((ClientToServerMsg::Text(text), _)) if text == "still attached")
    );

    let (retry, _) = switch_peer();
    let events = stage_test_switch(&mut app, 2, now, Ok(("destination".into(), retry)));
    app.receive_switch(2, now).unwrap();
    drop(events);
    app.receive_switch(2, now).unwrap();
    assert!(app
        .switch_notice
        .as_deref()
        .unwrap()
        .contains("closed unexpectedly"));
    assert_eq!(app.connection_id, 0);
}

#[cfg(unix)]
#[test]
fn native_switch_pending_cleanup_and_stale_events() {
    let mut app = switch_app(CliArgs::default());
    let now = Instant::now();
    let (source, mut source_peer) = switch_peer();
    app.sender = Some(source);
    let (pending, mut pending_peer) = switch_peer();
    let stale = stage_test_switch(&mut app, 1, now, Ok(("destination".into(), pending)));
    app.receive_switch(1, now).unwrap();
    app.receive_switch(1, now + FIRST_FRAME_TIMEOUT).unwrap();
    assert!(app.switch_notice.as_deref().unwrap().contains("timed out"));
    assert!(stale.send(destination_frame()).is_err());
    assert!(matches!(
        pending_peer.recv_client_msg().msg(),
        Some((ClientToServerMsg::ClientExited, _))
    ));
    assert!(pending_peer.recv_client_msg().is_eof());
    assert_eq!(app.connection_id, 0);

    let (pending, mut pending_peer) = switch_peer();
    let events = stage_test_switch(&mut app, 2, now, Ok(("destination".into(), pending)));
    app.receive_switch(2, now).unwrap();
    events.send(destination_frame()).unwrap();
    app.receive_switch(1, now).unwrap();
    assert_eq!(app.connection_id, 0);
    assert!(app.pending_switch.is_some());
    // Dropping the window while the destination is still pending retires both peers.
    drop(app);
    for peer in [&mut pending_peer, &mut source_peer] {
        assert!(matches!(
            peer.recv_client_msg().msg(),
            Some((ClientToServerMsg::ClientExited, _))
        ));
        assert!(peer.recv_client_msg().is_eof());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn option_as_alt_preserves_each_side_and_defaults_to_none() {
    use winit::platform::macos::OptionAsAlt as Native;
    use yinz_types::input::native_terminal::OptionAsAlt;
    for (setting, expected) in [
        (OptionAsAlt::None, Native::None),
        (OptionAsAlt::Left, Native::OnlyLeft),
        (OptionAsAlt::Right, Native::OnlyRight),
        (OptionAsAlt::Both, Native::Both),
    ] {
        assert_eq!(native_option_as_alt(setting), expected);
    }
    assert_eq!(
        native_option_as_alt(
            TerminalConfig::default()
                .macos_option_as_alt
                .unwrap_or_default()
        ),
        Native::None
    );
}

#[test]
fn fractional_scroll_accumulates_in_cell_units() {
    use winit::dpi::PhysicalPosition;
    let mut scroll = ScrollAccumulator::default();
    for _ in 0..19 {
        assert_eq!(
            scroll.steps(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 1.0)),
                10.0,
                20.0
            ),
            (0, 0)
        );
    }
    assert_eq!(
        scroll.steps(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 1.0)),
            10.0,
            20.0
        ),
        (0, 1)
    );
}

#[test]
fn fractional_scroll_retains_remainders_and_cancels_direction_changes() {
    let mut scroll = ScrollAccumulator::default();
    let mut steps = |x, y| scroll.steps(MouseScrollDelta::LineDelta(x, y), 10.0, 20.0);
    assert_eq!(steps(0.75, -0.5), (0, 0));
    assert_eq!(steps(-0.5, -0.75), (0, -1));
    assert_eq!(steps(1.0, 0.5), (1, 0));
    assert_eq!(steps(0.75, 0.75), (1, 1));
}

#[test]
fn large_scrolls_are_capped_without_deferring_whole_steps() {
    let mut scroll = ScrollAccumulator::default();
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(100.5, -200.75), 10.0, 20.0),
        (0, -20)
    );
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(0.0, 0.0), 10.0, 20.0),
        (0, 0)
    );
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(0.5, -0.25), 10.0, 20.0),
        (1, -1)
    );
}

#[test]
fn failed_link_opening_allows_subsequent_opening() {
    links::launch::forget_opened();
    open_link("vscode://example");
    links::launch::fail_next();
    open_link("https://example.com/fails");
    open_link("https://example.com/works");
    let opened = links::launch::opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].1, "https://example.com/works");
}

fn embedded_settings() -> TerminalConfig {
    TerminalConfig {
        system_fonts: Some(false),
        ..Default::default()
    }
}

#[test]
fn four_faces_weight_defaults_and_blank_resets_reach_production_options() {
    let settings = TerminalConfig {
        font_family: Some("main".into()),
        font_bold: Some("bold".into()),
        font_italic: Some("italic".into()),
        font_bold_italic: Some("both".into()),
        font_weight: Some(600),
        ..Default::default()
    };
    let options = font_options(&settings, 1.0);
    assert_eq!(options.family.as_deref(), Some("main"));
    assert_eq!(options.bold_family.as_deref(), Some("bold"));
    assert_eq!(options.italic_family.as_deref(), Some("italic"));
    assert_eq!(options.bold_italic_family.as_deref(), Some("both"));
    assert_eq!(options.weight, 600);
    let blank = TerminalConfig {
        font_family: Some(" ".into()),
        font_bold: Some("".into()),
        font_italic: Some("\t".into()),
        font_bold_italic: Some("  ".into()),
        ..Default::default()
    };
    assert_eq!(
        font_options(&blank, 1.0),
        font_options(&TerminalConfig::default(), 1.0)
    );
    let parse = |tag: &str| crate::text::font::FontFeature::parse(tag).unwrap();
    let listed = TerminalConfig {
        font_features: Some(vec![
            "ss01".into(),
            "zero".into(),
            "-calt".into(),
            "+dlig".into(),
            "-calt".into(),
        ]),
        ..Default::default()
    };
    // List order is kept and duplicates are passed through, as upstream does.
    assert_eq!(
        font_options(&listed, 1.0).features,
        vec![
            parse("ss01"),
            parse("zero"),
            parse("-calt"),
            parse("+dlig"),
            parse("-calt")
        ]
    );
    for scale in [1.0, 1.5] {
        assert_eq!(
            effective_font_options(&listed, 1.25, scale).features,
            font_options(&listed, 1.0).features
        );
    }
    for none in [None, Some(Vec::new())] {
        let settings = TerminalConfig {
            font_features: none,
            ..Default::default()
        };
        assert!(font_options(&settings, 1.0).features.is_empty());
    }
}

#[test]
fn effective_font_options_bound_only_final_physical_size() {
    for (points, zoom, scale, expected) in [(3.0, 0.25, 1.0, 4.0), (384.0, 4.0, 4.0, 512.0)] {
        let settings = TerminalConfig {
            font_size: Some(points),
            ..embedded_settings()
        };
        let options = effective_font_options(&settings, zoom, scale);
        assert_eq!(options.size, expected);
        assert!(options.stack(1.0).is_ok());
        assert_eq!(settings.font_size, Some(points));
    }
    let settings = TerminalConfig {
        font_size: Some(384.0),
        ..embedded_settings()
    };
    assert_eq!(
        effective_font_options(&settings, 4.0, 2.0),
        effective_font_options(&settings, 4.0, 3.0)
    );
}

#[test]
fn font_application_preserves_warmed_cache_for_equivalent_and_paint_only_settings() {
    let settings = embedded_settings();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = crate::testing::painter::Painter::state(2, 16, |painter| {
        painter.text(0, 0, "Ag -> fi");
        painter.cluster(1, 0, "e\u{301}", 1);
    });
    let selection = Selection::at(0, 0).extended_to(0, 1);
    retained.set_selection(Some(selection));
    retained.refresh(
        SceneInputs {
            state: &state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        &mut cache,
    );
    assert!(cache.entries() > 0);
    assert!(cache.shaped_runs() > 0);
    assert!(cache.cluster_entries() > 0);
    let atlas = cache.atlases().mask.stamp();
    let generation = cache.generation();
    for settings in [
        settings.clone(),
        TerminalConfig {
            font_size: Some(12.0),
            font_weight: Some(400),
            padding: Some(10.0),
            opacity: Some(0.5),
            font_family: Some(" ".into()),
            ..settings
        },
    ] {
        let requested = effective_font_options(&settings, 1.0, 1.0);
        let replacement = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(replacement, FontUpdate::Unchanged));
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested,
            replacement,
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(cache.atlases().mask.stamp(), atlas);
        assert_eq!(selection.text(state.screen()), "Ag");
    }
}

#[test]
fn font_application_changes_each_font_input_and_failure_keeps_working_cache() {
    let settings = embedded_settings();
    let base = effective_font_options(&settings, 1.0, 1.0);
    let mut variants = Vec::new();
    for field in [
        "regular", "bold", "italic", "both", "weight", "system", "size",
    ] {
        let mut changed = settings.clone();
        match field {
            "regular" => changed.font_family = Some("Iosevka Term".into()),
            "bold" => changed.font_bold = Some("Iosevka Term".into()),
            "italic" => changed.font_italic = Some("Iosevka Term".into()),
            "both" => changed.font_bold_italic = Some("Iosevka Term".into()),
            "weight" => changed.font_weight = Some(700),
            "system" => changed.system_fonts = Some(true),
            _ => changed.font_size = Some(18.0),
        }
        variants.push(effective_font_options(&changed, 1.0, 1.0));
    }
    variants.push(effective_font_options(&settings, 1.2, 1.0));
    variants.push(effective_font_options(&settings, 1.0, 2.0));
    for requested in variants {
        let mut active = base.clone();
        let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
        let generation = cache.generation();
        let mut retained = RetainedScene::new();
        let replacement = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(replacement, FontUpdate::Replacement(_)));
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            replacement,
        );
        assert_ne!(cache.generation(), generation);
        assert_eq!(active, requested);
        let invalid = FontOptions {
            size: f32::NAN,
            ..requested.clone()
        };
        let generation = cache.generation();
        assert!(prepare_fonts(&active, &invalid).is_err());
        assert_eq!(cache.generation(), generation);
        assert_eq!(active, requested);
    }
}

#[test]
fn geometry_reports_cell_changes_even_with_same_grid_and_forces_new_connections() {
    let metrics = effective_font_options(&embedded_settings(), 1.0, 1.0)
        .stack(1.0)
        .unwrap()
        .metrics();
    let first = Geometry::calculate(80, 40, 0, metrics).unwrap();
    assert_eq!(
        first.messages(None).len(),
        2,
        "initial, reconnect and explicit queries have no snapshot"
    );
    assert!(first.messages(Some(first)).is_empty());
    let changed_cell = Geometry {
        cell: SizeInPixels {
            width: first.cell.width + 1,
            ..first.cell
        },
        ..first
    };
    assert!(matches!(
        &changed_cell.messages(Some(first))[..],
        [ClientToServerMsg::TerminalPixelDimensions { .. }]
    ));
    let padded = Geometry::calculate(80, 40, 10, metrics).unwrap();
    assert_eq!(padded.messages(Some(first)).len(), 2);
    assert!(Geometry::calculate(0, 40, 0, metrics).is_none());
    assert!(Geometry::calculate(80, 0, 0, metrics).is_none());
    assert_eq!(
        Geometry::calculate(1, 1, 256, metrics).unwrap().grid,
        Size { rows: 1, cols: 1 }
    );
}

#[test]
fn paint_only_reload_refreshes_retained_ink_without_replacing_fonts() {
    let mut settings = embedded_settings();
    let mut paints = Paints::default();
    let active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = crate::testing::painter::Painter::state(1, 8, |painter| painter.text(0, 0, "Ag"));
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache, paints: &Paints| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache, &paints);
    let generation = cache.generation();
    let before = retained.flatten();
    let current = settings.clone();
    let next_paints = Paints {
        foreground: [12, 34, 56],
        ..paints
    };
    apply_settings(
        &mut settings,
        &mut paints,
        &mut retained,
        current.clone(),
        next_paints,
    );
    refresh(&mut retained, &mut cache, &paints);
    assert!(retained.rebuilt_everything());
    assert_ne!(retained.flatten(), before);
    assert_eq!(cache.generation(), generation);
    assert!(matches!(
        prepare_fonts(&active, &effective_font_options(&settings, 1.0, 1.0)).unwrap(),
        FontUpdate::Unchanged
    ));
    apply_settings(
        &mut settings,
        &mut paints,
        &mut retained,
        current,
        next_paints,
    );
    refresh(&mut retained, &mut cache, &paints);
    assert!(
        !retained.rebuilt_everything(),
        "identical reload invalidated retained ink"
    );
}

#[test]
fn host_pixel_reply_uses_current_window_grid_while_rendered_content_catches_up() {
    let metrics = crate::text::font::CellMetrics {
        width: 21,
        height: 62,
        baseline: 45,
        underline_top: 48,
        strikeout_top: 32,
        stroke: 1,
    };
    let current = Geometry {
        cell: SizeInPixels {
            width: 21,
            height: 62,
        },
        grid: Size {
            rows: 43,
            cols: 119,
        },
    };
    let old_frame_grid = Size {
        rows: 57,
        cols: 132,
    };
    let geometry = host_query_geometry(Some(current), old_frame_grid, metrics);
    assert_eq!(
        crate::host_reply::answer(b"\x1b[14t", geometry, &Paints::default(), None),
        b"\x1b[4;2666;2499t"
    );
    assert_eq!(
        crate::host_reply::answer(b"\x1b[16t", geometry, &Paints::default(), None),
        b"\x1b[6;62;21t"
    );
    let initial = host_query_geometry(None, old_frame_grid, metrics);
    assert_eq!(initial.rows, 57);
    assert_eq!(initial.cols, 132);
}

#[test]
fn font_reload_zoom_and_scale_geometry_share_selection_frame_and_copy_transition() {
    use crate::testing::painter::Painter;
    let original_settings = embedded_settings();
    let initial = effective_font_options(&original_settings, 1.0, 1.0);
    let initial_metrics = initial.stack(1.0).unwrap().metrics();
    let width = initial_metrics.width * 80;
    let height = initial_metrics.height * 8;
    let first = Geometry::calculate(width, height, 0, initial_metrics).unwrap();
    let changed = TerminalConfig {
        font_family: Some("Noto Sans Mono CJK SC".into()),
        system_fonts: Some(true),
        ..original_settings.clone()
    };
    for requested in [
        effective_font_options(&changed, 1.0, 1.0),
        effective_font_options(&original_settings, 1.5, 1.0),
        effective_font_options(&original_settings, 1.0, 1.5),
    ] {
        let replacement = if requested.family.is_some() {
            // A regular-family reload uses deterministic bundled discovery.
            let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
            let discovery = crate::text::discovery::Discovery::synthetic(&directory).unwrap();
            GlyphCache::new(
                crate::text::font::FontStack::build_with(&requested, Some(discovery)).unwrap(),
            )
        } else {
            let FontUpdate::Replacement(cache) = prepare_fonts(&initial, &requested).unwrap()
            else {
                panic!("expected font replacement");
            };
            *cache
        };
        let geometry = Geometry::calculate(width, height, 0, replacement.metrics()).unwrap();
        assert!(geometry.grid.cols < 70);
        let text = format!("{}ABCD end", "x".repeat(70));
        let mut state = Painter::state(first.grid.rows, first.grid.cols, |p| p.text(0, 0, &text));
        let mut selection = LocalSelection::default();
        selection.start(0, 70);
        selection.extend(0, 73);
        // This is the same geometry message/capture hook used by App after
        // successful preparation for reload, keyboard zoom and monitor scale.
        for message in geometry.messages(Some(first)) {
            if let ClientToServerMsg::TerminalResize { new_size } = message {
                selection.resizing(&state, new_size.rows, new_size.cols);
            }
        }
        let frame = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(geometry.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        selection.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
        assert_eq!(
            selection.range(),
            Some(
                Selection::at(70 / geometry.grid.cols, 70 % geometry.grid.cols)
                    .extended_to(73 / geometry.grid.cols, 73 % geometry.grid.cols)
            )
        );
        let update = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            for (offset, ch) in "WXYZ".char_indices() {
                let index = 70 + offset;
                p.text(
                    index / geometry.grid.cols,
                    index % geometry.grid.cols,
                    &ch.to_string(),
                );
            }
        });
        selection.apply_frame(&mut state, &update).unwrap();
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));

        // Start a different selection after the same production resize hook.
        // Its row/column must change for each actual font-derived grid.
        let text = format!(
            "{}ABCD{}WXYZ{}",
            "x".repeat(70),
            "y".repeat(16),
            "z".repeat(46)
        );
        let old_frame = Painter::frame(first.grid.rows, first.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(first.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        let mut state = TerminalState::new(first.grid.rows, first.grid.cols);
        state.apply_frame(&old_frame).unwrap();
        let mut selection = LocalSelection::default();
        selection.start(0, 70);
        selection.extend(0, 73);
        for message in geometry.messages(Some(first)) {
            if let ClientToServerMsg::TerminalResize { new_size } = message {
                selection.resizing(&state, new_size.rows, new_size.cols);
            }
        }
        let old_range = Selection::at(90 / first.grid.cols, 90 % first.grid.cols)
            .extended_to(93 / first.grid.cols, 93 % first.grid.cols);
        selection.start(old_range.anchor.0, old_range.anchor.1);
        selection.extend(old_range.head.0, old_range.head.1);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        selection.apply_frame(&mut state, &old_frame).unwrap();
        let frame = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(geometry.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        selection.apply_frame(&mut state, &frame).unwrap();
        let expected = Selection::at(90 / geometry.grid.cols, 90 % geometry.grid.cols)
            .extended_to(93 / geometry.grid.cols, 93 % geometry.grid.cols);
        assert_ne!(expected, old_range);
        assert_ne!(old_range.text(state.screen()), "WXYZ");
        assert_eq!(expected.text(state.screen()), "WXYZ");
        assert_eq!(selection.range(), Some(expected));
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        // Mirror App's session-switch clear/state reset before the next frame.
        selection.clear();
        state = TerminalState::new(geometry.grid.rows, geometry.grid.cols);
        selection.apply_frame(&mut state, &old_frame).unwrap();
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()), None);
    }
    // Failed font preparation never reaches the geometry/capture hook.
    let invalid = FontOptions {
        size: f32::NAN,
        ..initial.clone()
    };
    assert!(prepare_fonts(&initial, &invalid).is_err());
    let mut state = Painter::state(3, 80, |p| p.text(0, 0, "old!"));
    let mut selection = LocalSelection::default();
    selection.start(0, 0);
    selection.extend(0, 3);
    selection
        .apply_frame(&mut state, &Painter::frame(3, 80, |p| p.text(0, 0, "new!")))
        .unwrap();
    assert_eq!(selection.text(state.screen()).as_deref(), Some("new!"));
}

#[test]
fn ligature_reload_from_files_preserves_warmed_caches_cells_selection_and_geometry() {
    use crate::testing::painter::Painter;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.kdl");
    let drop_ins = directory.path().join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(directory.path().to_owned()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut settings = load();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = Painter::state(3, 16, |p| {
        p.text(0, 0, "a->b =>");
        p.cluster(1, 0, "e\u{301}", 1);
        p.wide(1, 3, '😀');
        p.wide(2, 0, '中');
        p.cursor(0, 10, crate::emulation::screen_buffer::CursorShape::Beam);
    });
    let mut selection = LocalSelection::default();
    selection.start(0, 1);
    selection.extend(0, 2);
    retained.set_selection(selection.range());
    let range = selection.range();
    let cursor = state.cursor_position();
    let cells: Vec<_> = (0..3)
        .flat_map(|row| (0..16).map(move |col| (row, col)))
        .map(|(row, col)| state.screen().cell(row, col))
        .collect();
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache);
    let joined = retained.flatten();
    let original_geometry = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
    let generation = cache.generation();
    let font = cache.primary_of(crate::text::font::FaceStyle::Regular);
    let joined_ids = cache.shape(font, "->");
    let mut previous = true;
    for (name, enabled) in [
        ("ligatures", true),
        ("font_ligatures", true),
        ("ligatures", false),
        ("font_ligatures", false),
        ("font_ligatures", true),
        ("ligatures", false),
        ("ligatures", true),
    ] {
        std::fs::write(
            drop_ins.join("20-ligatures.kdl"),
            format!("terminal {{ {name} {enabled}; }}"),
        )
        .unwrap();
        let next = load();
        let requested = effective_font_options(&next, 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert_eq!(
            matches!(update, FontUpdate::ShapingOnly),
            previous != enabled
        );
        let entries = cache.entries();
        let clusters = cache.cluster_entries();
        let shaped_clusters = cache.shaped_clusters();
        let atlas = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        if previous != enabled {
            assert_eq!(cache.shaped_runs(), 0);
        }
        assert_eq!(cache.shaped_clusters(), shaped_clusters);
        assert_eq!(cache.entries(), entries);
        assert_eq!(cache.cluster_entries(), clusters);
        assert_eq!(
            (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
            atlas
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(
            cache.primary_of(crate::text::font::FaceStyle::Regular),
            font
        );
        assert!(Geometry::calculate(400, 200, 0, cache.metrics())
            .unwrap()
            .messages(Some(original_geometry))
            .is_empty());
        refresh(&mut retained, &mut cache);
        if previous != enabled {
            assert!(retained.rebuilt_everything());
        }
        if enabled {
            assert_eq!(retained.flatten(), joined);
        } else {
            assert_ne!(retained.flatten(), joined);
        }
        let glyphs = cache.shape(font, "->");
        // Disabled production scenes use nominal glyphs; direct shape() remains
        // available for explicit feature callers and is bypassed by the scene.
        if enabled {
            assert_eq!(glyphs, joined_ids);
        }
        assert_eq!(selection.range(), range);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
        assert_eq!(state.cursor_position(), cursor);
        for (index, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(index / 16, index % 16), *cell);
        }
        assert_eq!(active, effective_font_options(&next, 1.0, 1.0));
        settings = next;
        previous = enabled;
    }
    // Invalid main configuration aborts loading before fonts/settings commit.
    let before = retained.flatten();
    let atlas = cache.atlases().mask.stamp();
    std::fs::write(&path, "terminal { ligatures \"false\"; }").unwrap();
    assert!(yinz_cli::setup::Setup::config_without_layout(&opts).is_err());
    assert_eq!(settings.font_ligatures, Some(true));
    let invalid = FontOptions {
        size: f32::NAN,
        ligatures: false,
        ..active.clone()
    };
    assert!(prepare_fonts(&active, &invalid).is_err());
    assert_eq!(cache.atlases().mask.stamp(), atlas);
    assert_eq!(cache.generation(), generation);
    assert_eq!(retained.flatten(), before);
    assert_eq!(selection.range(), range);
    assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
    let mixed = FontOptions {
        size: active.size + 2.0,
        ligatures: false,
        ..active.clone()
    };
    assert!(matches!(
        prepare_fonts(&active, &mixed).unwrap(),
        FontUpdate::Replacement(_)
    ));
    let features = FontOptions {
        features: vec![crate::text::font::FontFeature::parse("zero").unwrap()],
        ligatures: false,
        ..active.clone()
    };
    assert!(matches!(
        prepare_fonts(&active, &features).unwrap(),
        FontUpdate::ShapingOnly
    ));
}

#[test]
fn font_feature_reload_from_files_reshapes_runs_and_preserves_caches_cells_selection_and_geometry()
{
    use crate::testing::painter::Painter;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.kdl");
    let drop_ins = directory.path().join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(directory.path().to_owned()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut active = effective_font_options(&load(), 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = Painter::state(3, 16, |p| {
        p.text(0, 0, "a->b =>");
        p.cluster(1, 0, "e\u{301}", 1);
        p.wide(1, 3, '😀');
        p.wide(2, 0, '中');
        p.cursor(0, 10, crate::emulation::screen_buffer::CursorShape::Beam);
    });
    let mut selection = LocalSelection::default();
    selection.start(0, 1);
    selection.extend(0, 2);
    retained.set_selection(selection.range());
    let range = selection.range();
    let cursor = state.cursor_position();
    let cells: Vec<_> = (0..3)
        .flat_map(|row| (0..16).map(move |col| (row, col)))
        .map(|(row, col)| state.screen().cell(row, col))
        .collect();
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    // Preserved atlas entries can pack differently from a fresh cache. Compare
    // concrete placed geometry; GPU tests compare actual ink.
    let normalize = |mut scene: crate::render::scene::test_helpers::Scene| {
        for glyph in scene.glyphs.iter_mut().chain(scene.color_glyphs.iter_mut()) {
            glyph.entry.x = 0;
            glyph.entry.y = 0;
        }
        scene
    };
    let fresh_scene = |options: &FontOptions| {
        let mut fresh = GlyphCache::new(options.stack(1.0).unwrap());
        let mut scene = RetainedScene::new();
        scene.set_selection(range);
        refresh(&mut scene, &mut fresh);
        normalize(scene.flatten())
    };
    let check_content = |selection: &LocalSelection| {
        assert_eq!(selection.range(), range);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
        assert_eq!(state.cursor_position(), cursor);
        for (index, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(index / 16, index % 16), *cell);
        }
    };
    refresh(&mut retained, &mut cache);
    let joined = normalize(retained.flatten());
    let unjoined = fresh_scene(&FontOptions {
        ligatures: false,
        ..active.clone()
    });
    assert_ne!(joined, unjoined);
    let original_geometry = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
    let generation = cache.generation();
    let font = cache.primary_of(crate::text::font::FaceStyle::Regular);
    for (text, is_joined, shapes) in [
        ("font_features \"-calt\";", false, true),
        ("font_features \"zero\";", true, true),
        ("ligatures false; font_features \"+calt\";", true, true),
        ("ligatures false; font_features \"zero\";", false, true),
        ("ligatures false; font_features;", false, false),
        ("font_features \"calt\";", true, true),
    ] {
        std::fs::write(
            drop_ins.join("20-features.kdl"),
            format!("terminal {{ {text} }}"),
        )
        .unwrap();
        let requested = effective_font_options(&load(), 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::ShapingOnly), "{text}");
        cache.shape(font, "->");
        let entries = cache.entries();
        let clusters = cache.cluster_entries();
        let shaped_clusters = cache.shaped_clusters();
        let atlas = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        assert_eq!(active, requested);
        assert_eq!(cache.shaped_runs(), 0, "{text}");
        assert_eq!(cache.shapes_runs(), shapes, "{text}");
        assert_eq!(cache.shaped_clusters(), shaped_clusters);
        assert_eq!(cache.entries(), entries);
        assert_eq!(cache.cluster_entries(), clusters);
        assert_eq!(
            (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
            atlas
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(
            cache.primary_of(crate::text::font::FaceStyle::Regular),
            font
        );
        assert!(Geometry::calculate(400, 200, 0, cache.metrics())
            .unwrap()
            .messages(Some(original_geometry))
            .is_empty());
        refresh(&mut retained, &mut cache);
        assert!(retained.rebuilt_everything(), "{text}");
        let live = normalize(retained.flatten());
        assert_eq!(live, fresh_scene(&requested), "{text}");
        assert_eq!(live == joined, is_joined, "{text}");
        if !is_joined {
            assert_eq!(live, unjoined, "{text}");
        }
        check_content(&selection);
    }
    // A combined metric change follows the metric path and still reshapes runs.
    // line_height keeps the cell width, so runs survive update_cell itself.
    for (text, joined_after) in [
        ("line_height 1.25; font_features \"-calt\";", false),
        ("line_height 1.5; font_features \"+calt\";", true),
        ("cell_width 1.25; font_features \"-calt\";", false),
        (
            "cell_width 1.25; line_height 1.5; font_features \"zero\";",
            true,
        ),
    ] {
        std::fs::write(
            drop_ins.join("20-features.kdl"),
            format!("terminal {{ {text} }}"),
        )
        .unwrap();
        let requested = effective_font_options(&load(), 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::CellOnly), "{text}");
        cache.shape(font, "->");
        assert!(cache.shaped_runs() > 0);
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        assert_eq!(active, requested);
        assert_eq!(cache.shaped_runs(), 0, "{text}");
        refresh(&mut retained, &mut cache);
        assert!(retained.rebuilt_everything(), "{text}");
        let live = normalize(retained.flatten());
        let fresh = fresh_scene(&requested);
        assert_eq!(live, fresh, "{text}");
        let unjoined = fresh_scene(&FontOptions {
            features: vec![crate::text::font::FontFeature::parse("-calt").unwrap()],
            ..requested.clone()
        });
        assert_eq!(live != unjoined, joined_after, "{text}");
        check_content(&selection);
    }
    // A family or size change still replaces affected font resources.
    std::fs::write(
        drop_ins.join("20-features.kdl"),
        "terminal { font_size 14; font_features \"-calt\"; }",
    )
    .unwrap();
    let requested = effective_font_options(&load(), 1.0, 1.0);
    let update = prepare_fonts(&active, &requested).unwrap();
    assert!(matches!(update, FontUpdate::Replacement(_)));
    apply_fonts(
        &mut active,
        &mut cache,
        &mut retained,
        requested.clone(),
        update,
    );
    refresh(&mut retained, &mut cache);
    assert_eq!(normalize(retained.flatten()), fresh_scene(&requested));
    check_content(&selection);
    let working = load();
    // Invalid drop-ins are skipped whole; working settings are kept.
    std::fs::write(
        drop_ins.join("30-invalid.kdl"),
        "terminal { font_size 20; font_features \"ss1\"; }",
    )
    .unwrap();
    assert_eq!(load(), working);
    assert!(matches!(
        prepare_fonts(&active, &effective_font_options(&load(), 1.0, 1.0)).unwrap(),
        FontUpdate::Unchanged
    ));
    // Invalid main configuration aborts loading before fonts/settings commit.
    std::fs::write(&path, "terminal { font_features 4; }").unwrap();
    assert!(yinz_cli::setup::Setup::config_without_layout(&opts).is_err());
    assert_eq!(active, effective_font_options(&working, 1.0, 1.0));
    check_content(&selection);
}

#[test]
fn cell_metrics_fractional_dpi_zoom_rounding_grid_edges_and_hit_testing() {
    for (points, zoom) in [(12.0, 1.0), (11.3, 1.15), (13.25, 0.85)] {
        for scale in [1.0, 1.25, 1.5, 2.0, 1.0] {
            let settings = TerminalConfig {
                font_size: Some(points),
                ..embedded_settings()
            };
            let natural = effective_font_options(&settings, zoom, scale)
                .stack(1.0)
                .unwrap()
                .metrics();
            for width in [0.5, 1.0, 1.375, 3.0] {
                for height in [0.5, 1.0, 1.275, 3.0] {
                    let settings = TerminalConfig {
                        cell_width: Some(width),
                        line_height: Some(height),
                        ..settings.clone()
                    };
                    let options = effective_font_options(&settings, zoom, scale);
                    assert_eq!(options.cell.cell_width, width);
                    assert_eq!(options.cell.line_height, height);
                    let metrics = options.stack(1.0).unwrap().metrics();
                    let w = (natural.width as f32 * width).round().max(1.0) as u32;
                    let h = (natural.height as f32 * height).round().max(1.0) as u32;
                    assert_eq!((metrics.width, metrics.height), (w, h));
                    let padding = (3.0 * scale).round() as u32;
                    let exact = Geometry::calculate(
                        17 * w + 2 * padding,
                        7 * h + 2 * padding,
                        padding,
                        metrics,
                    )
                    .unwrap();
                    assert_eq!(exact.grid, Size { cols: 17, rows: 7 });
                    let below = Geometry::calculate(
                        17 * w + 2 * padding - 1,
                        7 * h + 2 * padding - 1,
                        padding,
                        metrics,
                    )
                    .unwrap();
                    assert_eq!(below.grid, Size { cols: 16, rows: 6 });
                    assert_eq!(
                        Geometry::calculate(1, 1, padding, metrics).unwrap().grid,
                        Size { cols: 1, rows: 1 }
                    );
                    assert!(Geometry::calculate(0, 20, padding, metrics).is_none());
                    assert!(Geometry::calculate(20, 0, padding, metrics).is_none());
                    let size = crate::emulation::screen_buffer::TermSize { cols: 17, rows: 7 };
                    let edge = (
                        padding as f64 + 3.0 * w as f64,
                        padding as f64 + 2.0 * h as f64,
                    );
                    assert_eq!(pointer_cell(edge, padding, metrics, size), (2, 3));
                    assert_eq!(
                        pointer_cell((edge.0 - 0.01, edge.1 - 0.01), padding, metrics, size),
                        (1, 2)
                    );
                    assert_eq!(pointer_cell((-10.0, -10.0), padding, metrics, size), (0, 0));
                    assert_eq!(pointer_cell((1e6, 1e6), padding, metrics, size), (6, 16));
                    let tiny = Geometry::calculate(1, 1, padding, metrics).unwrap();
                    let old_tiny = Geometry::calculate(1, 1, padding, natural).unwrap();
                    if tiny.cell != old_tiny.cell {
                        assert!(matches!(
                            &tiny.messages(Some(old_tiny))[..],
                            [ClientToServerMsg::TerminalPixelDimensions { .. }]
                        ));
                    }
                    let messages = exact.messages(None);
                    let ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } =
                        messages[0]
                    else {
                        panic!("pixel report first")
                    };
                    assert_eq!(
                        pixel_dimensions.character_cell_size,
                        Some(SizeInPixels {
                            width: w as usize,
                            height: h as usize
                        })
                    );
                    assert_eq!(
                        pixel_dimensions.text_area_size,
                        Some(SizeInPixels {
                            width: 17 * w as usize,
                            height: 7 * h as usize
                        })
                    );
                    assert_eq!(pixel_dimensions.text_area_grid_size, Some(exact.grid));
                    assert!(
                        matches!(messages[1], ClientToServerMsg::TerminalResize { new_size } if new_size == exact.grid)
                    );
                    assert!(exact.messages(Some(exact)).is_empty());
                    // Render frames may still contain the old grid; replies use reported geometry.
                    let geometry =
                        host_query_geometry(Some(exact), Size { cols: 80, rows: 24 }, metrics);
                    assert_eq!(
                        crate::host_reply::answer(b"\x1b[16t", geometry, &Paints::default(), None),
                        format!("\x1b[6;{h};{w}t").as_bytes()
                    );
                    assert_eq!(
                        crate::host_reply::answer(b"\x1b[14t", geometry, &Paints::default(), None),
                        format!("\x1b[4;{};{}t", 7 * h, 17 * w).as_bytes()
                    );
                }
            }
        }
    }
}

#[test]
fn baseline_offset_and_cell_metrics_file_reload_uses_metric_path_and_preserves_content_copy_and_rounding_noops(
) {
    use crate::emulation::selection::LocalSelection;
    use crate::testing::painter::Painter;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut active = effective_font_options(&load(), 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let mut state = Painter::state(2, 16, |p| {
        p.text(0, 0, "Ag -> fi");
        use yinz_types::structured_render::{ATTR_BOLD, ATTR_ITALIC};
        p.styled(0, 8, "Bi", |cell| cell.attrs |= ATTR_BOLD);
        p.styled(0, 10, "It", |cell| cell.attrs |= ATTR_ITALIC);
        p.styled(0, 12, "BI", |cell| cell.attrs |= ATTR_BOLD | ATTR_ITALIC);
        p.wide(1, 0, '中');
        p.wide(1, 2, '😀');
        p.cluster(1, 4, "👩‍💻", 2);
        p.cluster(1, 6, "e\u{301}", 1);
    });
    let mut selection = LocalSelection::default();
    selection.start(1, 0);
    selection.extend(1, 6);
    let expected = selection.text(state.screen());
    let range = selection.range();
    let cells: Vec<_> = (0..32)
        .map(|i| state.screen().cell(i / 16, i % 16))
        .collect();
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        )
    };
    retained.set_selection(range);
    refresh(&mut retained, &mut cache);
    for (width, height, ligatures, offset) in [
        (1.0, 1.0, true, 2),
        (1.0, 1.0, true, -2),
        (1.0, 1.0, true, 64),
        (1.0, 1.0, true, 63),
        (1.0, 1.0, true, -64),
        (1.0, 1.0, false, -63),
        (1.0, 1.0, true, 0),
        (1.5, 1.0, true, 2),
        (1.5, 1.25, true, -2),
        (0.5, 0.5, false, 2),
        (3.0, 3.0, true, -2),
        (1.0, 1.0, true, 0),
        (1.001, 1.001, true, 0),
        (1.002, 1.002, false, 0),
    ] {
        std::fs::write(&path, format!("terminal {{ system_fonts false; cell_width {width}; line_height {height}; ligatures {ligatures}; baseline_offset {offset}; }}")).unwrap();
        let requested = effective_font_options(&load(), 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::CellOnly));
        let offset_only = active.cell.cell_width == requested.cell.cell_width
            && active.cell.line_height == requested.cell.line_height;
        let old_metrics = cache.metrics();
        let generation = cache.generation();
        let atlas = cache.atlases().mask.stamp();
        let clusters = cache.shaped_clusters();
        let old = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        assert_eq!(cache.shaped_clusters(), clusters);
        let new = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
        if (width > 1.0 && width < 1.01) || offset == 63 || offset == -63 {
            assert_eq!(cache.generation(), generation);
            assert_eq!(cache.atlases().mask.stamp(), atlas);
            assert!(new.messages(Some(old)).is_empty());
        } else {
            assert_ne!(cache.generation(), generation);
            assert_ne!(cache.atlases().mask.stamp().identity, atlas.identity);
        }
        if offset_only {
            assert_eq!(
                (cache.metrics().width, cache.metrics().height),
                (old_metrics.width, old_metrics.height)
            );
            assert!(new.messages(Some(old)).is_empty());
        }
        refresh(&mut retained, &mut cache);
        let mut fresh = GlyphCache::new(requested.stack(1.0).unwrap());
        let mut fresh_scene = RetainedScene::new();
        fresh_scene.set_selection(range);
        refresh(&mut fresh_scene, &mut fresh);
        // Preserved atlas entries can have different packing after a ligature
        // toggle. Compare concrete placed geometry; GPU tests compare actual ink.
        let normalize = |mut scene: crate::render::scene::test_helpers::Scene| {
            for glyph in scene.glyphs.iter_mut().chain(scene.color_glyphs.iter_mut()) {
                glyph.entry.x = 0;
                glyph.entry.y = 0;
            }
            scene
        };
        assert_eq!(
            normalize(retained.flatten()),
            normalize(fresh_scene.flatten())
        );
        assert_eq!(selection.range(), range);
        assert_eq!(selection.text(state.screen()), expected);
        for (i, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(i / 16, i % 16), *cell);
        }
    }
    // A font/zoom/monitor-scale change still replaces affected font resources.
    for (zoom, scale) in [(1.2, 1.0), (1.2, 1.25), (1.2, 1.5), (1.2, 2.0), (1.0, 1.0)] {
        let requested = effective_font_options(&load(), zoom, scale);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::Replacement(_)));
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        assert_eq!(selection.text(state.screen()), expected);
    }
    state.set_cell_size(cache.metrics().width, cache.metrics().height);
    let generation = cache.generation();
    std::fs::write(&path, "terminal { baseline_offset 65; }").unwrap();
    assert!(yinz_cli::setup::Setup::config_without_layout(&opts).is_err());
    assert_eq!(cache.generation(), generation);
    std::fs::write(
        &path,
        "terminal { system_fonts false; baseline_offset -2; }",
    )
    .unwrap();
    let requested = effective_font_options(&load(), 1.0, 1.0);
    let update = prepare_fonts(&active, &requested).unwrap();
    apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
    assert_eq!(active.cell.baseline_offset, -2.0);
    assert_eq!(selection.text(state.screen()), expected);
    let dropins = dir.path().join("config.d");
    std::fs::create_dir_all(&dropins).unwrap();
    for offset in [2, -2, 0] {
        std::fs::write(
            dropins.join("10-baseline.kdl"),
            format!("terminal {{ baseline_offset {offset}; }}"),
        )
        .unwrap();
        let requested = effective_font_options(&load(), 1.0, 1.0);
        assert_eq!(requested.cell.baseline_offset, offset as f32);
        let update = prepare_fonts(&active, &requested).unwrap();
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        assert_eq!(selection.text(state.screen()), expected);
    }
}

#[test]
fn cell_metrics_emulation_graphics_and_wide_hit_cells_use_the_measured_cache() {
    use crate::testing::painter::{residency, Painter, Place};
    let mut state = TerminalState::new(4, 12);
    for (width, height) in [(1.0, 1.0), (1.5, 1.25), (0.5, 0.5), (1.0, 1.0)] {
        let settings = TerminalConfig {
            cell_width: Some(width),
            line_height: Some(height),
            ..embedded_settings()
        };
        let cache = GlyphCache::new(
            effective_font_options(&settings, 1.0, 1.0)
                .stack(1.0)
                .unwrap(),
        );
        let metrics = cache.metrics();
        state.set_cell_size(metrics.width, metrics.height);
        Painter::apply(&mut state, |p| {
            p.wide(1, 2, '中');
            p.graphics(&[
                residency(7, 2, 2, [1, 2, 3, 255]),
                Place::of(7, 2, 2).at(2, 3).record(),
            ]);
        });
        let images = crate::render::scene::images_of(&state, metrics);
        assert_eq!(images.len(), 1);
        assert_eq!(
            (images[0].x, images[0].y),
            ((3 * metrics.width) as i32, (2 * metrics.height) as i32)
        );
        let pad = 5;
        let first = pointer_cell(
            (
                (pad + 2 * metrics.width) as f64,
                (pad + metrics.height) as f64,
            ),
            pad,
            metrics,
            state.size(),
        );
        let spacer = pointer_cell(
            (
                (pad + 4 * metrics.width) as f64 - 0.01,
                (pad + 2 * metrics.height) as f64 - 0.01,
            ),
            pad,
            metrics,
            state.size(),
        );
        assert_eq!(first, (1, 2));
        assert_eq!(spacer, (1, 3));
        let selection = Selection::at(first.0, first.1).extended_to(spacer.0, spacer.1);
        assert_eq!(selection.text(state.screen()), "中");
        // Sixel cell changes clear old chunks; identical metrics preserve them.
        let frame = Painter::frame(4, 12, |p| {
            p.graphics(&[crate::testing::painter::sixel_chunk(
                2,
                3,
                2,
                6,
                "\x1bPq#1;2;100;0;0!2~\x1b\\",
            )])
        });
        state.apply_frame(&frame).unwrap();
        assert!(!state.sixels().is_empty());
        state.set_cell_size(metrics.width, metrics.height);
        assert!(!state.sixels().is_empty());
        state.set_cell_size(metrics.width + 1, metrics.height);
        assert!(state.sixels().is_empty());
    }
}

#[test]
fn baseline_offset_production_options_scale_round_clamp_and_keep_geometry() {
    for (zoom, scale) in [
        (1.0, 1.0),
        (1.15, 1.25),
        (0.85, 1.5),
        (1.2, 2.0),
        (1.0, 1.0),
    ] {
        for (width, height) in [(1.0, 1.0), (1.375, 1.275), (0.5, 0.5), (3.0, 3.0)] {
            let settings = TerminalConfig {
                cell_width: Some(width),
                line_height: Some(height),
                ..embedded_settings()
            };
            let zero = effective_font_options(&settings, zoom, scale)
                .stack(1.0)
                .unwrap()
                .metrics();
            let natural = effective_font_options(&embedded_settings(), zoom, scale)
                .stack(1.0)
                .unwrap()
                .metrics();
            let geometry = Geometry::calculate(400, 200, 4, zero).unwrap();
            for offset in [-64, -2, 0, 2, 64] {
                let options = effective_font_options(
                    &TerminalConfig {
                        baseline_offset: Some(offset),
                        ..settings.clone()
                    },
                    zoom,
                    scale,
                );
                let physical = offset as f32 * zoom * scale as f32;
                assert_eq!(options.cell.baseline_offset, physical);
                assert_eq!(
                    (options.cell.cell_width, options.cell.line_height),
                    (width, height)
                );
                let metrics = options.stack(1.0).unwrap().metrics();
                let expected = (natural.baseline as i32
                    + (zero.height as i32 - natural.height as i32).div_euclid(2)
                    + physical.round() as i32)
                    .clamp(1, zero.height as i32) as u32;
                assert_eq!(metrics.baseline, expected, "{offset}, {zoom}, {scale}");
                assert_eq!((metrics.width, metrics.height), (zero.width, zero.height));
                let changed = Geometry::calculate(400, 200, 4, metrics).unwrap();
                assert!(changed.messages(Some(geometry)).is_empty());
                let size = crate::emulation::screen_buffer::TermSize { cols: 16, rows: 8 };
                assert_eq!(
                    pointer_cell((35.0, 42.0), 4, metrics, size),
                    pointer_cell((35.0, 42.0), 4, zero, size)
                );
                for query in [b"\x1b[16t".as_slice(), b"\x1b[14t"] {
                    assert_eq!(
                        crate::host_reply::answer(
                            query,
                            host_query_geometry(Some(changed), changed.grid, metrics),
                            &Paints::default(),
                            None
                        ),
                        crate::host_reply::answer(
                            query,
                            host_query_geometry(Some(geometry), geometry.grid, zero),
                            &Paints::default(),
                            None
                        )
                    );
                }
            }
        }
    }
}

fn baseline_offset_equivalent_file_reloads(initial: Option<i32>, changes: &[Option<i32>]) {
    use crate::testing::painter::Painter;
    use yinz_types::structured_render::{ATTR_BOLD, ATTR_ITALIC};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    let dropins = dir.path().join("config.d");
    std::fs::create_dir(&dropins).unwrap();
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let save = |offset: Option<i32>| {
        let setting = offset
            .map(|value| format!("baseline_offset {value};"))
            .unwrap_or_default();
        std::fs::write(
            dropins.join("10-baseline.kdl"),
            format!("terminal {{ {setting} }}"),
        )
        .unwrap();
    };
    let opts = CliArgs {
        config: Some(path),
        config_dir: Some(dir.path().to_owned()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    save(initial);
    let mut settings = load();
    let mut paints = Paints::default();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = Painter::state(5, 24, |p| {
        for (row, attrs) in [0, ATTR_BOLD, ATTR_ITALIC, ATTR_BOLD | ATTR_ITALIC]
            .into_iter()
            .enumerate()
        {
            p.styled(row, 0, "Ag -> fi", |cell| cell.attrs |= attrs);
        }
        p.wide(4, 0, '中');
        p.wide(4, 3, '😀');
        p.cluster(4, 6, "e\u{301}", 1);
        p.cluster(4, 8, "👩‍💻", 2);
    });
    let selection = Selection::at(0, 3).extended_to(0, 4);
    let copied = selection.text(state.screen());
    let cursor = state.cursor_position();
    let cells: Vec<_> = (0..5)
        .flat_map(|row| (0..24).map(move |col| (row, col)))
        .map(|(row, col)| state.screen().cell(row, col))
        .collect();
    retained.set_selection(Some(selection));
    let refresh = |retained: &mut RetainedScene,
                   cache: &mut GlyphCache,
                   settings: &TerminalConfig,
                   paints: &Paints| {
        retained.set_transparency(crate::render::scene::Transparency {
            opacity: settings.opacity.unwrap_or(1.0),
            mode: yinz_types::input::native_terminal::OpacityMode::Background,
        });
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions {
                    shape: settings
                        .cursor_style
                        .map(|_| crate::emulation::screen_buffer::CursorShape::Underline),
                    blink: settings.cursor_blink,
                    hollow: false,
                },
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache, &settings, &paints);
    let original = retained.flatten();
    let metrics = cache.metrics();
    let geometry = Geometry::calculate(400, 200, 0, metrics).unwrap();
    let resources = (
        cache.generation(),
        cache.atlases().mask.stamp(),
        cache.atlases().color.stamp(),
    );
    for &offset in changes {
        // A synchronous refresh has no scheduler/blink damage. Carry one known
        // pending row through the same save as a separate positive control.
        for damaged in [false, true] {
            if damaged {
                retained.mark_row(2);
            }
            save(offset);
            let next = load();
            assert_eq!(next.baseline_offset, offset);
            let requested = effective_font_options(&next, 1.0, 1.0);
            assert_eq!(requested.stack(1.0).unwrap().metrics(), metrics);
            let update = prepare_fonts(&active, &requested).unwrap();
            apply_fonts(
                &mut active,
                &mut cache,
                &mut retained,
                requested.clone(),
                update,
            );
            let next_paints = paints;
            apply_settings(
                &mut settings,
                &mut paints,
                &mut retained,
                next.clone(),
                next_paints,
            );
            assert_eq!(settings, next);
            assert_eq!(active, requested);
            refresh(&mut retained, &mut cache, &settings, &paints);
            assert!(!retained.rebuilt_everything(), "{initial:?} -> {offset:?}");
            assert_eq!(
                retained.rebuilt_rows(),
                if damaged { &[2][..] } else { &[] }
            );
            assert_eq!(retained.flatten(), original);
            assert_eq!(
                (
                    cache.generation(),
                    cache.atlases().mask.stamp(),
                    cache.atlases().color.stamp()
                ),
                resources,
                "metrics, placement and style fitting must all remain equivalent"
            );
            assert!(Geometry::calculate(400, 200, 0, cache.metrics())
                .unwrap()
                .messages(Some(geometry))
                .is_empty());
            assert_eq!(selection.text(state.screen()), copied);
            assert_eq!(state.cursor_position(), cursor);
            for (index, cell) in cells.iter().enumerate() {
                assert_eq!(state.screen().cell(index / 24, index % 24), *cell);
            }
        }
    }
    // Effective baseline changes still refresh text; clamped baseline plus a
    // shaping change must redraw even when the regular metrics stay identical.
    for fragment in [
        "baseline_offset 2;",
        "baseline_offset -2;",
        "baseline_offset 0;",
        "baseline_offset 64;",
        "baseline_offset 63; ligatures false; font_features \"-calt\";",
    ] {
        std::fs::write(
            dropins.join("10-baseline.kdl"),
            format!("terminal {{ {fragment} }}"),
        )
        .unwrap();
        let next = load();
        let requested = effective_font_options(&next, 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        let next_paints = paints;
        apply_settings(&mut settings, &mut paints, &mut retained, next, next_paints);
        refresh(&mut retained, &mut cache, &settings, &paints);
        assert!(retained.rebuilt_everything(), "{fragment}");
    }
    // Keep the baseline equivalent while changing each unrelated visual input.
    // The cursor/opacity inputs use the same values passed by the window draw.
    for field in ["paints", "cursor", "opacity", "padding"] {
        let mut next = settings.clone();
        next.baseline_offset = Some(if settings.baseline_offset == Some(63) {
            64
        } else {
            63
        });
        let mut next_paints = paints;
        match field {
            "paints" => next_paints.foreground = [12, 34, 56],
            "cursor" => {
                next.cursor_style = Some(yinz_types::input::native_terminal::CursorStyle::Underline)
            },
            "opacity" => next.opacity = Some(0.5),
            "padding" => next.padding = Some(8.0),
            _ => unreachable!(),
        }
        let requested = effective_font_options(&next, 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        let generation = cache.generation();
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        apply_settings(
            &mut settings,
            &mut paints,
            &mut retained,
            next.clone(),
            next_paints,
        );
        refresh(&mut retained, &mut cache, &settings, &paints);
        assert!(retained.rebuilt_everything(), "{field}");
        assert_eq!(settings, next);
        assert_eq!(paints, next_paints);
        assert_eq!(cache.generation(), generation);
    }
}

#[test]
fn baseline_offset_omitted_zero_file_reload_preserves_clean_and_damaged_rows() {
    baseline_offset_equivalent_file_reloads(None, &[None, Some(0), Some(0), None]);
}

#[test]
fn baseline_offset_clamped_file_reload_preserves_clean_and_damaged_rows() {
    for sign in [-1, 1] {
        baseline_offset_equivalent_file_reloads(
            Some(sign * 64),
            &[Some(sign * 63), Some(sign * 64)],
        );
    }
}

#[test]
fn baseline_offset_saved_file_reload_retains_inline_images_through_full_apply() {
    use crate::testing::painter::{residency, sixel_chunk, Painter, Place};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    let dropins = dir.path().join("config.d");
    std::fs::create_dir(&dropins).unwrap();
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path),
        config_dir: Some(dir.path().to_owned()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut settings = load();
    let mut paints = Paints::default();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let state = Painter::state(5, 24, |p| {
        use yinz_types::structured_render::{ATTR_BOLD, ATTR_ITALIC};
        for (row, attrs) in [0, ATTR_BOLD, ATTR_ITALIC, ATTR_BOLD | ATTR_ITALIC]
            .into_iter()
            .enumerate()
        {
            p.styled(row, 0, "Ag -> fi", |cell| cell.attrs |= attrs);
        }
        p.wide(4, 0, '中');
        p.wide(4, 3, '😀');
        p.cluster(4, 6, "e\u{301}", 1);
        p.cluster(4, 8, "👩‍💻", 2);
        p.graphics(&[
            residency(1, 12, 8, [0, 255, 0, 255]),
            Place::of(1, 12, 8).at(1, 20).record(),
            sixel_chunk(3, 20, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ]);
    });
    let selection = Selection::at(0, 0).extended_to(0, 4);
    let copied = selection.text(state.screen());
    let cursor = state.cursor_position();
    let cells: Vec<_> = (0..5)
        .flat_map(|row| (0..24).map(move |col| (row, col)))
        .map(|(r, c)| state.screen().cell(r, c))
        .collect();
    let mut retained = RetainedScene::new();
    retained.set_selection(Some(selection));
    let refresh = |scene: &mut RetainedScene, cache: &mut GlyphCache| {
        scene.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        )
    };
    refresh(&mut retained, &mut cache);
    let original = retained.flatten();
    assert_eq!(original.images.len(), 2);
    let geometry = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
    let baseline = cache.metrics().baseline;
    for offset in [2, -2, 0] {
        std::fs::write(
            dropins.join("10-baseline.kdl"),
            format!("terminal {{ baseline_offset {offset}; }}"),
        )
        .unwrap();
        let next = load();
        let requested = effective_font_options(&next, 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        let generation = cache.generation();
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        let next_paints = paints;
        apply_settings(
            &mut settings,
            &mut paints,
            &mut retained,
            next.clone(),
            next_paints,
        );
        refresh(&mut retained, &mut cache);
        assert!(retained.rebuilt_everything());
        assert_ne!(cache.generation(), generation);
        assert!(!retained.replaced_images());
        assert_eq!(retained.images(), original.images);
        assert_eq!(retained.all_images(), original.images);
        assert_eq!(retained.resident_images(), original.resident_images);
        assert_eq!(cache.metrics().baseline as i32, baseline as i32 + offset);
        assert!(Geometry::calculate(400, 200, 0, cache.metrics())
            .unwrap()
            .messages(Some(geometry))
            .is_empty());
        assert_eq!(settings, next);
        assert_eq!(selection.text(state.screen()), copied);
        assert_eq!(state.cursor_position(), cursor);
        for (i, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(i / 24, i % 24), *cell);
        }
        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(selection));
        let mut fresh_cache = GlyphCache::new(requested.stack(1.0).unwrap());
        refresh(&mut fresh, &mut fresh_cache);
        assert_eq!(retained.flatten(), fresh.flatten());
        if offset == 0 {
            assert_eq!(retained.flatten(), original);
        } else {
            assert_ne!(retained.flatten(), original);
        }
    }
}

#[test]
fn native_startup_dimensions_seed_budget() {
    use yinz_types::structured_render::{decode, FrameBuilder, WireCell};

    for (cols, rows, expected) in [
        (None, None, Some((120, 40))),
        (Some(120), Some(40), Some((120, 40))),
        (Some(4096), Some(1), Some((4096, 1))),
        (Some(1024), Some(1024), Some((1024, 1024))),
        (Some(1024), Some(1025), None),
        (Some(2048), Some(2048), Some((1024, 1024))),
        (Some(4096), Some(4096), Some((1024, 1024))),
    ] {
        let args = TerminalArgs {
            cols,
            rows,
            ..Default::default()
        };
        let raw = args.opening_size();
        assert_eq!(raw.cols, usize::from(cols.unwrap_or(120)));
        assert_eq!(raw.rows, usize::from(rows.unwrap_or(40)));
        let seed = graphical_opening_size(&args);
        assert!((1..=raw.cols).contains(&seed.cols));
        assert!((1..=raw.rows).contains(&seed.rows));
        assert!(seed.cols * seed.rows <= STARTUP_CELL_LIMIT);
        if let Some(expected) = expected {
            assert_eq!((seed.cols, seed.rows), expected);
        }
    }

    let args = TerminalArgs {
        cols: Some(2048),
        rows: Some(2048),
        ..Default::default()
    };
    let seed = graphical_opening_size(&args);
    let state = TerminalState::new(seed.rows, seed.cols);
    for new_session in [true, false] {
        let message = yinz_client::native_first_message(
            &CliArgs::default(),
            &Default::default(),
            &Default::default(),
            None,
            None,
            seed,
            new_session,
            true,
            None,
        );
        let assets = match message {
            ClientToServerMsg::FirstClientConnected { cli_assets } if new_session => cli_assets,
            ClientToServerMsg::AttachClient { cli_assets, .. } if !new_session => cli_assets,
            other => panic!("unexpected native handshake: {other:?}"),
        };
        assert_eq!(
            assets.terminal_window_size,
            Size {
                cols: state.size().cols,
                rows: state.size().rows
            }
        );
    }

    let mut frame = FrameBuilder::new(seed.cols as u16, seed.rows as u16, 1);
    let cells = vec![
        WireCell {
            ch: 'x' as u32,
            ..WireCell::BLANK
        };
        seed.cols
    ];
    for row in 0..seed.rows {
        frame.push_row(row as u16, 0, &cells);
    }
    let bytes = frame.finish();
    let view = decode(&bytes).unwrap();
    assert_eq!(usize::from(view.header().cols), seed.cols);
    assert_eq!(usize::from(view.header().rows), seed.rows);
    assert_eq!(
        view.rows().map(|(_, cells)| cells.len()).sum::<usize>(),
        STARTUP_CELL_LIMIT
    );
    assert!(bytes.len() < yinz_ipc::MAX_IPC_FRAME_LEN);
    assert!(bytes.capacity() < yinz_ipc::MAX_IPC_FRAME_LEN);
}

#[test]
fn native_startup_dimensions_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native-terminal-window.json");
    let mut settings = TerminalConfig {
        initial_columns: Some(100),
        initial_rows: Some(30),
        ..embedded_settings()
    };
    let args = TerminalArgs::default();
    assert_eq!(startup_mode(&args, &settings), StartupMode::Windowed);
    settings.startup_mode = Some(StartupMode::Remember);
    assert_eq!(startup_mode(&args, &settings), StartupMode::Remember);
    let extent = |args: &TerminalArgs, settings: &TerminalConfig| {
        let metrics = effective_font_options(settings, 1.0, 1.0)
            .stack(1.0)
            .unwrap()
            .metrics();
        let remembered = remembered_size(startup_mode(args, settings), &path);
        StartupSize::resolve(args, settings, remembered).extent(
            metrics,
            4,
            PhysicalSize::new(STARTUP_AXIS_LIMIT, STARTUP_AXIS_LIMIT),
        )
    };
    let cell_extent = |columns, rows| {
        let metrics = effective_font_options(&settings, 1.0, 1.0)
            .stack(1.0)
            .unwrap()
            .metrics();
        PhysicalSize::new(metrics.width * columns + 8, metrics.height * rows + 8)
    };
    assert_eq!(extent(&args, &settings), cell_extent(100, 30));
    for bad in ["{", "[800]", "[99,721]", "[800,16385]"] {
        std::fs::write(&path, bad).unwrap();
        assert_eq!(extent(&args, &settings), cell_extent(100, 30), "{bad}");
    }
    std::fs::write(&path, "[801,721]").unwrap();
    assert_eq!(extent(&args, &settings), PhysicalSize::new(801, 721));
    let windowed = TerminalArgs {
        startup_mode: Some(StartupMode::Windowed),
        ..args.clone()
    };
    assert_eq!(extent(&windowed, &settings), cell_extent(100, 30));
    let columns = TerminalArgs {
        cols: Some(120),
        ..args.clone()
    };
    assert_eq!(
        extent(&columns, &settings),
        PhysicalSize::new(cell_extent(120, 40).width, 721)
    );
    let rows = TerminalArgs {
        rows: Some(40),
        ..args.clone()
    };
    assert_eq!(
        extent(&rows, &settings),
        PhysicalSize::new(801, cell_extent(120, 40).height)
    );
    let both = TerminalArgs {
        cols: Some(120),
        ..rows
    };
    assert_eq!(extent(&both, &settings), cell_extent(120, 40));
    for (columns, rows, grid) in [
        (None, None, (120, 40)),
        (Some(100), None, (100, 40)),
        (None, Some(30), (120, 30)),
    ] {
        let config = TerminalConfig {
            initial_columns: columns,
            initial_rows: rows,
            ..settings.clone()
        };
        assert_eq!(extent(&windowed, &config), cell_extent(grid.0, grid.1));
    }
    assert_eq!(
        args.opening_size(),
        Size {
            cols: 120,
            rows: 40
        }
    );
}

#[test]
fn native_startup_dimensions_geometry() {
    let args = TerminalArgs::default();
    let settings = TerminalConfig {
        initial_columns: Some(100),
        initial_rows: Some(30),
        padding: Some(3.5),
        cell_width: Some(1.375),
        line_height: Some(1.275),
        ..embedded_settings()
    };
    let target = StartupSize::resolve(&args, &settings, None);
    let expected_grid = Size {
        cols: 100,
        rows: 30,
    };
    let monitor = PhysicalSize::new(4500, 2800);
    for scale in [1.0, 1.25, 2.0] {
        let cache = GlyphCache::new(
            effective_font_options(&settings, 1.0, scale)
                .stack(1.0)
                .unwrap(),
        );
        let metrics = cache.metrics();
        let padding = scaled_padding(&settings, scale);
        let extent = target.extent(metrics, padding, monitor);
        assert_eq!(
            extent,
            PhysicalSize::new(
                100 * metrics.width + 2 * padding,
                30 * metrics.height + 2 * padding
            )
        );
        assert_eq!(
            Geometry::calculate(extent.width, extent.height, padding, metrics)
                .unwrap()
                .grid,
            expected_grid
        );
        let smaller =
            PhysicalSize::new(extent.width - metrics.width, extent.height - metrics.height);
        let ceiling = correction_bounds(smaller, extent, monitor);
        let corrected = target.extent(metrics, padding, ceiling);
        assert_eq!(corrected, smaller);
        assert_eq!(
            Geometry::calculate(corrected.width, corrected.height, padding, metrics)
                .unwrap()
                .grid,
            Size { cols: 99, rows: 29 }
        );
    }
    assert!(!client_sized(StartupMode::Fullscreen));
    assert!(!client_sized(StartupMode::Maximized));
    assert!(client_sized(StartupMode::Remember));
    assert!(client_sized(StartupMode::Windowed));
}

#[test]
fn native_startup_dimensions_remember_budget() {
    let metrics = crate::text::font::CellMetrics {
        width: 2,
        height: 3,
        baseline: 2,
        underline_top: 2,
        strikeout_top: 1,
        stroke: 1,
    };
    let boundary = PhysicalSize::new(STARTUP_AXIS_LIMIT, STARTUP_AXIS_LIMIT);
    let raw = PhysicalSize::new(7680, 4320);
    let raw_grid = Geometry::grid_size(raw.width, raw.height, 0, metrics);
    assert!(raw.width as usize * raw.height as usize <= STARTUP_PIXEL_LIMIT);
    assert!(raw_grid.cols * raw_grid.rows > STARTUP_CELL_LIMIT);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native-terminal-window.json");
    std::fs::write(&path, "[7680,4320]").unwrap();
    let remembered = remembered_size(StartupMode::Remember, &path);
    assert_eq!(remembered, Some((7680, 4320)));
    let settings = embedded_settings();
    let args = TerminalArgs::default();
    let target = StartupSize::resolve(&args, &settings, remembered);
    let large_metrics = crate::text::font::CellMetrics {
        width: 6,
        height: 6,
        ..metrics
    };
    let estimate = target.extent(large_metrics, 0, boundary);
    assert_eq!(estimate, raw);
    assert!(check_startup_extent(
        estimate,
        Geometry::grid_size(estimate.width, estimate.height, 0, large_metrics)
    )
    .is_ok());

    let safe = |extent: PhysicalSize<u32>, padding| {
        let geometry = Geometry::calculate(extent.width, extent.height, padding, metrics).unwrap();
        check_startup_extent(extent, geometry.grid).unwrap();
        geometry
    };
    let corrected = target.extent(metrics, 0, boundary);
    assert!(corrected.width < raw.width && corrected.height < raw.height);
    let geometry = safe(corrected, 0);
    assert_eq!(geometry.messages(None).len(), 2);
    let rounded = wayland_correction(&target, corrected, corrected, 0, metrics, 1.5)
        .unwrap()
        .to_physical(1.5);
    assert!(rounded.width <= corrected.width && rounded.height <= corrected.height);
    safe(rounded, 0);

    let mixed = TerminalArgs {
        cols: Some(4096),
        ..args.clone()
    };
    let mixed = StartupSize::resolve(&mixed, &settings, remembered).extent(metrics, 8, boundary);
    assert!(mixed.width < 4096 * metrics.width + 16);
    safe(mixed, 8);
    let residual =
        StartupSize::resolve(&args, &settings, Some((801, 721))).extent(metrics, 8, boundary);
    assert_eq!(residual, PhysicalSize::new(801, 721));
    safe(residual, 8);

    let insufficient = PhysicalSize::new(raw.width - 1, raw.height);
    for extent in [raw, insufficient] {
        assert!(check_startup_extent(
            extent,
            Geometry::grid_size(extent.width, extent.height, 0, metrics)
        )
        .is_err());
    }
}

#[test]
fn native_startup_dimensions_resize_order() {
    let metrics = effective_font_options(&embedded_settings(), 1.0, 1.0)
        .stack(1.0)
        .unwrap()
        .metrics();
    let first = PhysicalSize::new(100 * metrics.width, 30 * metrics.height);
    let geometry = Geometry::calculate(first.width, first.height, 0, metrics).unwrap();
    let mut connection = ConnectionGeometry::new(
        Size {
            cols: 120,
            rows: 40,
        },
        Instant::now(),
    );
    assert!(connection.messages(geometry).is_empty());
    connection.received_frame = true;
    let messages = connection.messages(geometry);
    assert!(matches!(messages.as_slice(),
        [ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions }, ClientToServerMsg::TerminalResize { new_size }]
            if pixel_dimensions.text_area_grid_size == Some(geometry.grid) && *new_size == geometry.grid
    ));
    assert!(geometry.messages(Some(geometry)).is_empty());
    assert!(Geometry::calculate(0, first.height, 0, metrics).is_none());
    let queued = PhysicalSize::new(first.width - metrics.width, first.height);
    let resize = Geometry::calculate(queued.width, queued.height, 0, metrics).unwrap();
    connection.reported = Some(geometry);
    assert!(matches!(connection.messages(resize).as_slice(),
        [ClientToServerMsg::TerminalPixelDimensions { .. }, ClientToServerMsg::TerminalResize { new_size }]
            if *new_size == Size { cols: 99, rows: 30 }
    ));
}

#[test]
fn native_startup_dimensions_save_without_resize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    std::fs::write(
        &path,
        "terminal { system_fonts false; padding 4; initial_columns 80; initial_rows 24; }",
    )
    .unwrap();
    let opts = CliArgs {
        config: Some(path),
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut settings = load();
    assert_eq!(
        (settings.initial_columns, settings.initial_rows),
        (Some(80), Some(24))
    );
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let mut paints = Paints::default();
    let state = crate::testing::painter::Painter::state(4, 24, |painter| {
        painter.text(0, 0, "startup marker")
    });
    let selection = Selection::at(0, 0).extended_to(0, 6);
    let before_copy = selection.text(state.screen());
    retained.set_selection(Some(selection));
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache, paints: &Paints| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache, &paints);
    let before_scene = retained.flatten();
    let generation = cache.generation();
    let metrics = cache.metrics();
    let grant = PhysicalSize::new(800, 600);
    let current = Geometry::calculate(
        grant.width,
        grant.height,
        scaled_padding(&settings, 1.0),
        metrics,
    )
    .unwrap();

    yinz_config::config::Config::write_managed_fragment(
        &dir.path().join("config.d"),
        "terminal { initial_columns 100; initial_rows 30; }",
    )
    .unwrap();
    let next = load();
    let requested = effective_font_options(&next, 1.0, 1.0);
    let update = prepare_fonts(&active, &requested).unwrap();
    assert!(matches!(update, FontUpdate::Unchanged));
    apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
    let next_paints = paints;
    apply_settings(&mut settings, &mut paints, &mut retained, next, next_paints);
    refresh(&mut retained, &mut cache, &paints);
    assert!(!retained.rebuilt_everything());
    assert_eq!(retained.flatten(), before_scene);
    assert_eq!(cache.generation(), generation);
    assert_eq!(selection.text(state.screen()), before_copy);
    let padding = scaled_padding(&settings, 1.0);
    let open_geometry =
        Geometry::calculate(grant.width, grant.height, padding, cache.metrics()).unwrap();
    assert!(open_geometry.messages(Some(current)).is_empty());
    let new_extent = StartupSize::resolve(&TerminalArgs::default(), &settings, None).extent(
        metrics,
        padding,
        PhysicalSize::new(4000, 3000),
    );
    assert_eq!(
        Geometry::calculate(new_extent.width, new_extent.height, padding, metrics)
            .unwrap()
            .grid,
        Size {
            cols: 100,
            rows: 30
        }
    );
}

#[test]
fn native_startup_x11_placement_settles_before_geometry() {
    let now = Instant::now();
    let settings = TerminalConfig {
        cell_width: Some(0.55),
        padding: Some(4.0),
        ..embedded_settings()
    };
    let metrics = |scale| {
        effective_font_options(&settings, 1.0, scale)
            .stack(1.0)
            .unwrap()
            .metrics()
    };
    let target = StartupSize::resolve(&TerminalArgs::default(), &settings, None);
    let monitor = PhysicalSize::new(2560, 1440);
    let hint = target.extent(metrics(1.0), 4, monitor);
    assert_eq!(hint.width, 488);
    let mut opening = OpeningGeometry::new(
        WindowBackend::X11,
        StartupCorrection {
            target,
            hint: WindowSize::Physical(hint),
            wayland_ceiling: None,
        },
        now,
    );
    assert!(!opening.placement_ready(now, true));
    let placed = now + OPENING_QUIET - Duration::from_millis(1);
    opening.placement_signal(placed); // Moved, followed by the DPI event.
    assert!(!opening.placement_ready(now + OPENING_QUIET, true));
    let settled = opening.wake_at();
    assert!(opening.placement_ready(settled, true));
    let OpeningPhase::Placement { correction, .. } = &opening.phase else {
        panic!("placement consumed early")
    };
    let (_, expected) = correction
        .request(hint, monitor, 2.0, 8, metrics(2.0))
        .unwrap();
    // The writer leaves the physical extent at 488 during the DPI callback.
    // The measured request uses 9px cells rather than Winit's ratio width of 976.
    assert_eq!(expected.width, 1096);
    opening.phase = OpeningPhase::Grant {
        expected,
        deadline: settled + OPENING_GRANT_LIMIT,
    };
    assert!(!opening.grant_settled(settled, hint));
    assert!(opening.grant_settled(settled, expected));
    assert!(opening.grant_settled(settled + OPENING_GRANT_LIMIT, hint));
}

#[test]
fn native_startup_validates_late_opening_grants() {
    let now = Instant::now();
    let safe = PhysicalSize::new(800, 600);
    let late = PhysicalSize::new(7680, 4320);
    let metrics = crate::text::font::CellMetrics {
        width: 2,
        height: 3,
        baseline: 2,
        underline_top: 2,
        strikeout_top: 1,
        stroke: 1,
    };
    let measure =
        |size: PhysicalSize<u32>| Geometry::calculate(size.width, size.height, 0, metrics).unwrap();
    let opening = OpeningGeometry {
        backend: WindowBackend::X11,
        phase: OpeningPhase::Grant {
            expected: late,
            deadline: now + OPENING_GRANT_LIMIT,
        },
    };
    let mut connection = ConnectionGeometry::new(
        Size {
            cols: 120,
            rows: 40,
        },
        now,
    );
    let safe_geometry = measure(safe);
    opening.validate(safe, safe_geometry).unwrap();
    connection.received_frame = true;
    assert_eq!(connection.messages(safe_geometry).len(), 2);
    connection.reported = Some(safe_geometry);
    assert!(!opening.grant_settled(now, safe));
    assert!(opening.grant_settled(now, late));
    assert!(opening.validate(late, measure(late)).is_err());
    assert!(opening.grant_settled(opening.wake_at(), safe));
    opening.validate(safe, safe_geometry).unwrap();
    assert_eq!(connection.messages(measure(late)).len(), 2);
    assert!(!client_sized(StartupMode::Fullscreen));
    assert!(!client_sized(StartupMode::Maximized));
}

#[test]
fn native_startup_wayland_configure_fit() {
    let monitor = PhysicalSize::new(3840, 2160);
    let mut settings = TerminalConfig {
        cell_width: Some(0.55),
        padding: Some(4.0),
        ..embedded_settings()
    };
    let metrics = |settings: &TerminalConfig, scale| {
        effective_font_options(settings, 1.0, scale)
            .stack(1.0)
            .unwrap()
            .metrics()
    };
    let target = StartupSize::resolve(&TerminalArgs::default(), &settings, None);
    let mut correction = StartupCorrection {
        target,
        hint: WindowSize::Logical(LogicalSize::new(488.0, 808.0)),
        wayland_ceiling: None,
    };
    correction.observe_wayland_grant(PhysicalSize::new(488, 808), 1.0);
    let actual = PhysicalSize::new(976, 1616);
    let request = correction.request(actual, monitor, 2.0, 8, metrics(&settings, 2.0));
    assert!(request.is_none_or(|(_, size)| size.width <= actual.width));
    correction.observe_wayland_grant(PhysicalSize::new(960, 1616), 2.0);
    assert_eq!(correction.wayland_ceiling.unwrap().width, 480);

    settings.initial_columns = Some(100);
    settings.cell_width = Some(1.2);
    let target = StartupSize::resolve(&TerminalArgs::default(), &settings, None);
    let scale = 1.75;
    let metrics = metrics(&settings, scale);
    assert_eq!(metrics.width, 17);
    for (width, expected) in [(1715, (980, 100)), (1713, (970, 99))] {
        let bounds = PhysicalSize::new(width, 1800);
        let fitted = target.extent(metrics, 7, bounds);
        let logical = wayland_correction(&target, fitted, bounds, 7, metrics, scale).unwrap();
        let actual: PhysicalSize<u32> = logical.to_physical(scale);
        assert!(actual.width <= width);
        assert_eq!(
            Geometry::grid_size(actual.width, actual.height, 7, metrics).cols,
            expected.1
        );
        assert_eq!(logical.width, expected.0);
    }
    let cached = StartupSize {
        width: StartupAxis::Pixels(1714),
        ..target
    };
    let fitted = cached.extent(metrics, 7, monitor);
    let logical = wayland_correction(&cached, fitted, monitor, 7, metrics, scale).unwrap();
    assert_eq!(logical.width, 979); // Equally distant physical pixels favor the smaller grant.
}

#[test]
fn native_startup_switch_uses_measured_geometry() {
    let now = Instant::now();
    let seed = Size {
        cols: 120,
        rows: 40,
    };
    let geometry = |cols| Geometry {
        cell: SizeInPixels {
            width: 8,
            height: 20,
        },
        grid: Size { cols, rows: 30 },
    };
    let mut opening = OpeningGeometry::new(
        WindowBackend::Wayland,
        StartupCorrection {
            target: StartupSize::resolve(&TerminalArgs::default(), &embedded_settings(), None),
            hint: WindowSize::Logical(LogicalSize::new(800.0, 600.0)),
            wayland_ceiling: Some(LogicalSize::new(800, 600)),
        },
        now,
    );
    assert!(!opening.placement_ready(now, false));
    let mut previous = ConnectionGeometry::new(seed, now);
    assert_eq!(previous.switch_grid(None), seed);
    previous.received_frame = true;
    previous.reported = Some(geometry(80));
    assert_eq!(previous.switch_grid(None), geometry(80).grid);
    let live = geometry(100);
    let grid = previous.switch_grid(Some(live));
    assert_eq!(grid, live.grid);
    // The same choice is handed to both the new-session handshake and local state.
    let ClientToServerMsg::FirstClientConnected { cli_assets } = yinz_client::native_first_message(
        &CliArgs::default(),
        &Default::default(),
        &Default::default(),
        None,
        None,
        grid,
        true,
        true,
        None,
    ) else {
        panic!("expected create handshake")
    };
    let state = TerminalState::new(grid.rows, grid.cols);
    assert_eq!(
        cli_assets.terminal_window_size,
        Size {
            rows: state.size().rows,
            cols: state.size().cols
        }
    );

    let mut next = ConnectionGeometry::new(grid, now);
    let resized_during_switch = geometry(101);
    assert!(next.messages(resized_during_switch).is_empty());
    assert!(!opening.placement_ready(now, false));
    assert!(opening.placement_ready(now, true));
    assert!(opening.placement_ready(opening.deadline(), false));
    let expected = PhysicalSize::new(800, 600);
    opening.phase = OpeningPhase::Grant {
        expected,
        deadline: now + OPENING_GRANT_LIMIT,
    };
    next.received_frame = true; // Only the destination's first applied and acknowledged frame opens this gate.
    assert_eq!(next.messages(resized_during_switch).len(), 2);
    next.reported = Some(resized_during_switch);
    assert!(next.messages(resized_during_switch).is_empty());
    assert!(opening.grant_settled(now, expected));
}

#[test]
fn native_opening_layout_error_is_fatal() {
    use yinz_ipc::ipc::ExitReason;
    let (tx, rx) = mpsc::channel();
    tx.send(ServerToClientMsg::LogError {
        lines: vec!["recoverable layout failure".into()],
    })
    .unwrap();
    assert!(receive_message(Some(&rx)).unwrap().is_none());
    tx.send(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    })
    .unwrap();
    assert!(matches!(
        receive_message(Some(&rx)).unwrap(),
        Some(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal
        })
    ));
    tx.send(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Error(
            "Initial layout failed at 120 columns by 40 rows: Not enough room for panes".into(),
        ),
    })
    .unwrap();
    let reason = receive_message(Some(&rx)).unwrap_err().to_string();
    assert!(reason.contains("Not enough room for panes") && reason.contains("--rows or --cols"));
}

#[test]
fn native_reload_invalid_startup_axis_retains_settings() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.kdl");
    let opts = CliArgs {
        config: Some(base.clone()),
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };
    std::fs::write(
        &base,
        "terminal { initial_columns 80; initial_rows 24; padding 4; }",
    )
    .unwrap();
    let mut live = load_native_config(&opts).unwrap().0.terminal;
    let original = live.clone();
    std::fs::write(
        &base,
        "// café\r\nterminal {\r\n    initial_rows 1001\r\n}\r\n",
    )
    .unwrap();
    match load_native_config(&opts) {
        Ok((config, _)) => live = config.terminal,
        Err(error) => {
            let error = error.to_string();
            assert!(error.contains(&format!("{}:3:", base.display())), "{error}");
            assert!(
                error.contains("initial_rows") && error.contains("between 2 and 1000"),
                "{error}"
            );
        },
    }
    assert_eq!(live, original);
    std::fs::write(&base, "terminal { initial_columns 80; initial_rows 24; }").unwrap();
    let drop_ins = dir.path().join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(
        drop_ins.join("90-invalid.kdl"),
        "terminal { initial_columns 81; initial_rows 1001; }",
    )
    .unwrap();
    let (config, _) = load_native_config(&opts).unwrap();
    assert_eq!(
        (
            config.terminal.initial_columns,
            config.terminal.initial_rows
        ),
        (Some(80), Some(24))
    );
}

#[cfg(unix)]
#[test]
fn underline_metrics_native_reload_rejects_nonregular_main_and_recovers() {
    use crate::testing::painter::Painter;
    use std::{
        fs,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    use yinz_types::structured_render::{UNDERLINE_SHIFT, UNDERLINE_STRAIGHT};

    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    let config_dir = dir.path().join("shared");
    let drop_ins = config_dir.join("config.d");
    fs::create_dir_all(private.join("config.d")).unwrap();
    fs::create_dir_all(&drop_ins).unwrap();
    fs::create_dir(config_dir.join("layouts")).unwrap();
    let main = private.join("config.kdl");
    let regular = private.join("regular.kdl");
    fs::write(
        &regular,
        "terminal { system_fonts false; underline_offset -2; underline_thickness 1; }",
    )
    .unwrap();
    symlink(&regular, &main).unwrap();
    fs::write(
        private.join("config.d/20-private.kdl"),
        "terminal { underline_offset 50; }",
    )
    .unwrap();
    fs::write(
        config_dir.join("layouts/default.kdl"),
        "layout { pane; }\nterminal { underline_thickness 50; }",
    )
    .unwrap();
    let valid_overlay = drop_ins.join("10-shared.kdl");
    let write_overlay = |thickness| {
        fs::write(
            &valid_overlay,
            format!("terminal {{ underline_thickness {thickness}; }}"),
        )
        .unwrap();
    };
    write_overlay(2);
    fs::write(
        drop_ins.join("20-invalid.kdl"),
        "terminal { underline_offset 65; underline_thickness 51; }",
    )
    .unwrap();
    let opts = CliArgs {
        config: Some(main.clone()),
        config_dir: Some(config_dir),
        ..Default::default()
    };
    let (config, options) = load_native_config(&opts).unwrap();
    let mut paints = paints_for(&config, &options);
    let mut settings = config.terminal;
    assert_eq!(settings.underline_offset, Some(-2));
    assert_eq!(settings.underline_thickness, Some(2));
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = Painter::state(2, 8, |p| {
        p.styled(0, 0, "a", |cell| {
            cell.attrs |= UNDERLINE_STRAIGHT << UNDERLINE_SHIFT
        });
    });
    let mut selection = LocalSelection::default();
    selection.start(0, 0);
    selection.extend(0, 1);
    retained.set_selection(selection.range());
    let copy = selection.text(state.screen());
    assert!(copy.as_deref().is_some_and(|text| text.starts_with('a')));
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache, paints: &Paints| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache, &paints);
    let warm = retained.flatten();
    let initial_settings = settings.clone();
    let initial_paints = paints;
    let initial_fonts = active.clone();
    let generation = cache.generation();
    retained.mark_row(1);

    let pipe = private.join("pipe");
    let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for use_symlink in [false, true] {
        fs::remove_file(&main).unwrap();
        if use_symlink {
            symlink(&pipe, &main).unwrap();
        } else {
            fs::hard_link(&pipe, &main).unwrap();
        }
        write_overlay(3);
        let Err(error) = PreparedReload::load(&opts, &active, 1.0, 1.0) else {
            panic!("nonregular main was accepted");
        };
        let diagnostic = format!("{error:#}");
        for expected in [
            main.to_str().unwrap(),
            "not a regular file",
            "Restore a readable regular file",
        ] {
            assert!(diagnostic.contains(expected), "{diagnostic}");
        }
        assert_eq!(settings, initial_settings);
        assert_eq!(paints, initial_paints);
        assert_eq!(active, initial_fonts);
        assert_eq!(cache.generation(), generation);
        assert_eq!(retained.flatten().rects, warm.rects);
        assert_eq!(selection.text(state.screen()), copy);
    }

    fs::remove_file(&main).unwrap();
    symlink(&regular, &main).unwrap();
    PreparedReload::load(&opts, &active, 1.0, 1.0)
        .unwrap()
        .apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
    refresh(&mut retained, &mut cache, &paints);
    assert_eq!(settings.underline_offset, Some(-2));
    assert_eq!(settings.underline_thickness, Some(3));
    assert_ne!(retained.flatten().rects, warm.rects);
    assert!(!retained.rebuilt_everything());
    assert_eq!(retained.rebuilt_rows(), &[1]);
    assert_eq!(retained.rebuilt_solid_rows(), &[0, 1]);
    assert_eq!(cache.generation(), generation);
    assert_eq!(selection.text(state.screen()), copy);
}

#[test]
fn underline_metrics_saved_reload_scales_once_and_keeps_cells_and_copy() {
    use crate::emulation::screen_buffer::CursorShape;
    use crate::input::composition::Preedit;
    use crate::input::links::LinkRun;
    use crate::testing::painter::Painter;
    use crate::text::font::CellMetrics;
    use yinz_types::structured_render::{
        WireColor, ATTR_STRIKE, UNDERLINE_CURLY, UNDERLINE_DASHED, UNDERLINE_DOTTED,
        UNDERLINE_DOUBLE, UNDERLINE_SHIFT, UNDERLINE_STRAIGHT,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    let base = "system_fonts false; font_size 36; line_height 1.5;";
    let write = |settings: &str| {
        std::fs::write(&path, format!("terminal {{ {base} {settings} }}")).unwrap();
    };
    write("");
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let (config, options) = load_native_config(&opts).unwrap();
    let mut paints = paints_for(&config, &options);
    let mut settings = config.terminal;
    let (zoom, scale) = (1.1, 1.5);
    let mut active = effective_font_options(&settings, zoom, scale);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let underline_color = [230, 15, 71];
    let state = Painter::state(2, 12, |p| {
        for (col, style) in [
            UNDERLINE_STRAIGHT,
            UNDERLINE_DOUBLE,
            UNDERLINE_DOTTED,
            UNDERLINE_DASHED,
            UNDERLINE_CURLY,
        ]
        .into_iter()
        .enumerate()
        {
            p.styled(0, col, "a", |cell| {
                cell.attrs |= style << UNDERLINE_SHIFT;
                cell.underline_color =
                    WireColor::Rgb(underline_color[0], underline_color[1], underline_color[2])
                        .pack();
            });
        }
        p.wide_styled(0, 6, '😀', |cell| {
            cell.attrs |= UNDERLINE_STRAIGHT << UNDERLINE_SHIFT
        });
        p.styled(0, 9, "S", |cell| cell.attrs |= ATTR_STRIKE);
        p.links(&[(1, "https://example.com")]);
        p.linked(1, 0, "link", 1);
        p.cursor(1, 6, CursorShape::Underline);
    });
    let mut selection = LocalSelection::default();
    selection.start(0, 5);
    selection.extend(0, 7);
    let range = selection.range();
    retained.set_selection(range);
    let expected_copy = selection.text(state.screen());
    assert!(expected_copy
        .as_deref()
        .is_some_and(|text| text.contains('😀')));
    let hover = LinkRun {
        id: 1,
        uri: "https://example.com".into(),
        spans: vec![(1, 0, 3)],
    };
    let refresh = |retained: &mut RetainedScene,
                   cache: &mut GlyphCache,
                   paints: &Paints,
                   preedit: Option<&Preedit>| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions::default(),
                hovered_link: Some(&hover),
                preedit,
            },
            cache,
        )
    };
    refresh(&mut retained, &mut cache, &paints, None);
    let zero = cache.metrics();
    let zero_stroke = cache.underline_stroke();
    assert!(
        zero_stroke > 2,
        "fixture must exercise unclamped negative thickness"
    );
    let geometry = Geometry::calculate(940, 440, 4, zero).unwrap();
    let unadjusted = retained.flatten();
    let generation = cache.generation();
    let stamps = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
    let cursor_x = (zero.width * 6) as i32;
    let strike_x = (zero.width * 9) as i32;
    let rects_at = |scene: &RetainedScene, row: usize, x| {
        scene.rows()[row]
            .rects
            .iter()
            .filter(|rect| rect.x == x)
            .copied()
            .collect::<Vec<_>>()
    };
    let cursor_rects = rects_at(&retained, 1, cursor_x);
    let strike_rects = rects_at(&retained, 0, strike_x);
    assert!(cursor_rects.iter().any(|rect| rect.height == zero.stroke));
    assert!(strike_rects.iter().any(|rect| rect.height == zero.stroke));
    for (offset, thickness, delta, stroke_delta) in [
        (-4_i32, -1, -7, -2),
        (4, 1, 7, 2),
        (64, 64, 106, 106),
        (-64, -64, -106, -106),
    ] {
        write(&format!(
            "underline_offset {offset}; underline_thickness {thickness};"
        ));
        let reload = PreparedReload::load(&opts, &active, zoom, scale).unwrap();
        reload.apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
        assert_eq!(
            (settings.underline_offset, settings.underline_thickness),
            (Some(offset), Some(thickness))
        );
        let metrics = cache.metrics();
        let expected_stroke =
            (zero_stroke as i32 + stroke_delta).clamp(1, zero.height as i32) as u32;
        let expected_top = (zero.underline_top as i32 + delta)
            .clamp(0, (zero.height - expected_stroke) as i32) as u32;
        if offset.abs() == 4 {
            assert_eq!(
                expected_top as i32,
                zero.underline_top as i32 + delta,
                "fixture must leave room for movement"
            );
            assert_eq!(expected_stroke as i32, zero_stroke as i32 + stroke_delta);
        }
        assert_eq!(
            (metrics.underline_top, cache.underline_stroke()),
            (expected_top, expected_stroke)
        );
        assert_eq!(
            CellMetrics {
                underline_top: zero.underline_top,
                ..metrics
            },
            zero
        );
        let changed = Geometry::calculate(940, 440, 4, metrics).unwrap();
        assert_eq!(changed, geometry);
        assert!(changed.messages(Some(geometry)).is_empty());
        assert_eq!(
            pointer_cell((52.0, 87.0), 4, zero, state.size()),
            pointer_cell((52.0, 87.0), 4, metrics, state.size())
        );
        for query in [b"\x1b[16t".as_slice(), b"\x1b[14t"] {
            let reply = |geometry, metrics| {
                crate::host_reply::answer(
                    query,
                    host_query_geometry(Some(geometry), changed.grid, metrics),
                    &paints,
                    None,
                )
            };
            assert_eq!(reply(geometry, zero), reply(changed, metrics));
        }

        refresh(&mut retained, &mut cache, &paints, None);
        assert!(!retained.rebuilt_everything());
        assert!(retained.rebuilt_rows().is_empty());
        assert_eq!(retained.rebuilt_solid_rows(), &[0, 1]);
        let rects = &retained.rows()[0].rects;
        for col in 0..5 {
            let left = (col * metrics.width) as i32;
            let strokes: Vec<_> = rects
                .iter()
                .filter(|rect| {
                    rect.color == underline_color
                        && rect.x >= left
                        && rect.x < left + metrics.width as i32
                })
                .collect();
            assert!(!strokes.is_empty(), "underline style at column {col}");
            for rect in &strokes {
                assert_eq!(rect.height, expected_stroke);
                assert!(rect.y >= 0 && rect.y + rect.height as i32 <= metrics.height as i32);
            }
            if col == 0 {
                assert_eq!(
                    (strokes[0].y, strokes[0].width),
                    (expected_top as i32, metrics.width)
                );
            }
        }
        assert!(rects.iter().any(|rect| rect.x == (6 * metrics.width) as i32
            && rect.height == expected_stroke
            && rect.y == expected_top as i32));
        assert!(retained.rows()[1].rects.iter().any(|rect| rect.x == 0
            && rect.y == (metrics.height + expected_top) as i32
            && rect.height == expected_stroke));
        assert_eq!(rects_at(&retained, 1, cursor_x), cursor_rects);
        assert_eq!(rects_at(&retained, 0, strike_x), strike_rects);
        let current = retained.flatten();
        assert_eq!(current.glyphs, unadjusted.glyphs);
        assert_eq!(current.color_glyphs, unadjusted.color_glyphs);
        assert_eq!(selection.text(state.screen()), expected_copy);
        assert_eq!(cache.generation(), generation);
        assert_eq!(
            (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
            stamps
        );
    }

    let clamped = retained.flatten().rects;
    retained.mark_row(1);
    write("underline_offset -64; underline_thickness -63;");
    PreparedReload::load(&opts, &active, zoom, scale)
        .unwrap()
        .apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
    refresh(&mut retained, &mut cache, &paints, None);
    assert_eq!(retained.rebuilt_rows(), &[1]);
    assert_eq!(retained.rebuilt_solid_rows(), &[1]);
    assert_eq!(retained.flatten().rects, clamped);

    let live_settings = settings.clone();
    let live_options = active.clone();
    let live_rects = retained.flatten().rects;
    write("underline_offset 65;");
    let Err(error) = PreparedReload::load(&opts, &active, zoom, scale) else {
        panic!("invalid underline offset was accepted");
    };
    let diagnostic = format!("{error:#}");
    for expected in [
        path.to_str().unwrap(),
        "underline_offset",
        "between -64 and 64",
    ] {
        assert!(diagnostic.contains(expected), "{diagnostic}");
    }
    assert_eq!(settings, live_settings);
    assert_eq!(active, live_options);
    assert_eq!(retained.flatten().rects, live_rects);

    let overlays = dir.path().join("config.d");
    write("underline_offset -2; underline_thickness 2;");
    yinz_config::config::Config::write_managed_fragment(
        &overlays,
        "terminal { underline_offset 0; underline_thickness 0; }",
    )
    .unwrap();
    PreparedReload::load(&opts, &active, zoom, scale)
        .unwrap()
        .apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
    refresh(&mut retained, &mut cache, &paints, None);
    assert_eq!(cache.metrics(), zero);
    assert_eq!(cache.underline_stroke(), zero_stroke);
    assert_eq!(retained.flatten().rects, unadjusted.rects);
    // Explicit zero and omission are equivalent, with unrelated damage already queued.
    write("");
    std::fs::remove_dir_all(&overlays).unwrap();
    retained.mark_row(1);
    PreparedReload::load(&opts, &active, zoom, scale)
        .unwrap()
        .apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
    refresh(&mut retained, &mut cache, &paints, None);
    assert_eq!(retained.rebuilt_rows(), &[1]);
    assert_eq!(retained.rebuilt_solid_rows(), &[1]);
    assert_eq!(
        (settings.underline_offset, settings.underline_thickness),
        (None, None)
    );

    write("underline_offset -2; underline_thickness 1; baseline_offset 2; cell_width 1.5; ligatures false;");
    PreparedReload::load(&opts, &active, zoom, scale)
        .unwrap()
        .apply(
            &mut active,
            &mut cache,
            &mut retained,
            &mut settings,
            &mut paints,
        );
    let adjusted = cache.metrics();
    assert_ne!(cache.generation(), generation);
    assert!(!cache.shapes_runs());
    assert_ne!(adjusted.baseline, zero.baseline);
    let changed = Geometry::calculate(940, 440, 4, adjusted).unwrap();
    assert!(changed
        .messages(Some(geometry))
        .iter()
        .any(|message| matches!(message, ClientToServerMsg::TerminalResize { .. })));

    let composed = Preedit::new("ni", Some((2, 2)));
    refresh(&mut retained, &mut cache, &paints, Some(&composed));
    assert!(retained.rebuilt_everything());
    assert!(retained.rows()[1]
        .rects
        .iter()
        .any(|rect| rect.x == (adjusted.width * 6) as i32
            && rect.y == (adjusted.height + adjusted.underline_top) as i32
            && rect.height == cache.underline_stroke()));
    assert_eq!(selection.text(state.screen()), expected_copy);
}

#[test]
fn underline_metrics_pending_redraw_waits_for_a_drawable_before_pacing() {
    let mut pacer = Pacer::new(None);
    let now = Instant::now();
    let due = now + pacer.interval();
    let usable = Some((320, 200));
    pacer.drawn(now);
    pacer.schedule();
    assert_eq!(redraw_decision(&pacer, usable, now), Decision::At(due));
    for missing in [None, Some((0, 200)), Some((320, 0))] {
        assert_eq!(redraw_decision(&pacer, missing, now), Decision::Idle);
        assert_eq!(redraw_decision(&pacer, missing, due), Decision::Idle);
        pacer.schedule();
        assert_eq!(redraw_decision(&pacer, missing, due), Decision::Idle);
    }
    // Restoration schedules through the same pacer as geometry refresh.
    assert_eq!(redraw_decision(&pacer, usable, due), Decision::Now);
    pacer.schedule();
    pacer.drawn(due);
    assert_eq!(redraw_decision(&pacer, usable, due), Decision::Idle);
    pacer.schedule();
    assert_eq!(
        redraw_decision(&pacer, usable, due),
        Decision::At(due + pacer.interval())
    );
}
