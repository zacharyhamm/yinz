use super::*;
use prost::Message;
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::Arc,
};
use yinz_ipc::{
    client_server_contract::client_server_contract as wire,
    ipc::{ClientToServerMsg as Client, ExitReason, PixelDimensions, ServerToClientMsg as Server},
};
use yinz_runtime::native_media::MediaRoot;
use yinz_types::{
    input::cli_assets::CliAssets,
    pane_size::{Size, SizeInPixels},
    render_negotiation as caps,
    structured_render::{GraphicsMedium, GraphicsRecord},
};

const SESSION: &str = "native-media";
const BUDGET: Duration = Duration::from_secs(25);

fn bounded(stream: UnixStream) -> UnixStream {
    stream.set_read_timeout(Some(BUDGET)).unwrap();
    stream.set_write_timeout(Some(BUDGET)).unwrap();
    stream
}

fn read_packet(stream: &mut UnixStream) -> Vec<u8> {
    let mut prefix = [0; 4];
    stream
        .read_exact(&mut prefix)
        .expect("read IPC packet length");
    let length = u32::from_le_bytes(prefix) as usize;
    assert!(length <= yinz_ipc::MAX_IPC_FRAME_LEN);
    let mut bytes = vec![0; length];
    stream
        .read_exact(&mut bytes)
        .expect("read complete IPC packet");
    bytes
}

fn write_packet(stream: &mut UnixStream, bytes: &[u8]) {
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
}

fn client_message(bytes: &[u8]) -> Client {
    wire::ClientToServerMsg::decode(bytes)
        .unwrap()
        .try_into()
        .unwrap()
}
fn server_message(bytes: &[u8]) -> Server {
    wire::ServerToClientMsg::decode(bytes)
        .unwrap()
        .try_into()
        .unwrap()
}
fn send(stream: &mut UnixStream, msg: Client) {
    write_packet(
        stream,
        &wire::ClientToServerMsg::try_from(msg)
            .unwrap()
            .encode_to_vec(),
    );
}

fn start_server(image_count: u8) -> NativeFixture {
    start_images(&image_count.to_string())
}

fn start_images(mode: &str) -> NativeFixture {
    let fixture = NativeFixture::new();
    let path = fixture.fixture.path();
    let script = path.join("images.py");
    fs::write(&script, include_str!("images.py")).unwrap();
    fs::write(
        &fixture.layout,
        format!(
            "layout {{ pane command=\"python3\" {{ args \"{}\" \"{mode}\"; }}; }}",
            script.display()
        ),
    )
    .unwrap();
    let capture = path.join("create.frames");
    fixture.capture_until(
        SESSION,
        &mut fixture.terminal(SESSION, true, &capture),
        &capture,
        "initial native repaint",
        |recording| {
            recording
                .frames
                .first()
                .is_some_and(FrameHeader::full_repaint)
        },
    );
    fixture
}

fn server(fixture: &NativeFixture) -> UnixStream {
    bounded(
        UnixStream::connect(
            fixture
                .fixture
                .socket_dir()
                .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
                .join(SESSION),
        )
        .unwrap(),
    )
}

fn attach(fixture: &NativeFixture) -> UnixStream {
    advertised_attachment(fixture).0
}

fn advertised_attachment(fixture: &NativeFixture) -> (UnixStream, Vec<u8>) {
    let mut stream = server(fixture);
    send(
        &mut stream,
        Client::AttachClient {
            cli_assets: CliAssets {
                terminal_window_size: Size {
                    rows: 40,
                    cols: 120,
                },
                ..Default::default()
            },
            tab_number_to_focus: None,
            pane_to_focus: None,
        },
    );
    loop {
        let packet = read_packet(&mut stream);
        match server_message(&packet) {
            Server::Connected {
                native_render_revision,
                ..
            } => {
                assert_eq!(native_render_revision, Some(1));
                let envelope = wire::ServerToClientMsg::decode(packet.as_slice()).unwrap();
                let Some(wire::server_to_client_msg::Message::Connected(connected)) =
                    envelope.message
                else {
                    unreachable!()
                };
                let owner = connected.attachment_generation.unwrap();
                assert_eq!(owner.len(), 16);
                return (stream, owner);
            },
            Server::RenderFrame { .. } | Server::NativeRenderResult(_) => {
                panic!("native output before attachment advertisement")
            },
            _ => {},
        }
    }
}

fn text_viewer(fixture: &NativeFixture) -> UnixStream {
    let mut stream = attach(fixture);
    let mut text = caps::Offer::reader(None);
    text.features = 0;
    send(
        &mut stream,
        Client::NativeRenderRequest(caps::Request::Offer(text)),
    );
    send(
        &mut stream,
        Client::TerminalPixelDimensions {
            pixel_dimensions: PixelDimensions {
                character_cell_size: Some(SizeInPixels {
                    width: 8,
                    height: 16,
                }),
                ..Default::default()
            },
        },
    );
    wait_text(&mut stream, "PIXELS-READY");
    stream
}

fn wait_text(stream: &mut UnixStream, marker: &str) {
    let deadline = Instant::now() + BUDGET;
    let mut state = TerminalState::new(40, 120);
    loop {
        assert!(
            Instant::now() < deadline,
            "text viewer did not receive {marker}"
        );
        if let Server::RenderFrame { frame } = server_message(&read_packet(stream)) {
            assert!(decode(&frame).unwrap().graphics().is_empty());
            let seq = state.apply_frame(&frame).unwrap().seq;
            send(stream, Client::RenderFrameAck { seq });
            if (0..state.size().rows).any(|r| {
                (0..state.size().cols)
                    .map(|c| state.screen().cell(r, c).character())
                    .collect::<String>()
                    .contains(marker)
            }) {
                return;
            }
        }
    }
}

// Inspect and pass the original packet bytes in both directions. The first full frame is
// consumed independently of the prefix that the barrier will hold.
fn exchange(
    client: &mut UnixStream,
    server: &mut UnixStream,
    shared: bool,
) -> (TerminalState, Vec<u8>) {
    let opening = read_packet(client);
    let Client::AttachClient {
        cli_assets,
        tab_number_to_focus,
        pane_to_focus,
    } = client_message(&opening)
    else {
        panic!("unexpected opening")
    };
    if !shared {
        assert_eq!(
            tab_number_to_focus,
            Some(yinz_types::internal_ids::TabNumber::new(1))
        );
        assert_eq!(pane_to_focus, None);
        let expected = CliAssets {
            terminal_window_size: Size {
                rows: 40,
                cols: 120,
            },
            host_terminal_env: yinz_runtime::host_terminal_env::host_terminal_env_with_overrides(
                &Default::default(),
            ),
            timezone: yinz_runtime::client_time::capture_timezone(),
            ..Default::default()
        };
        assert_eq!(cli_assets, expected);
    }
    write_packet(server, &opening);
    loop {
        let packet = read_packet(server);
        write_packet(client, &packet);
        if matches!(
            server_message(&packet),
            Server::Connected {
                native_render_revision: Some(1),
                ..
            }
        ) {
            break;
        }
    }
    let request = read_packet(client);
    let generation = wire::ClientToServerMsg::decode(request.as_slice())
        .unwrap()
        .attachment_generation
        .unwrap();
    assert_eq!(generation.len(), 16);
    let Client::NativeRenderRequest(caps::Request::Offer(offer)) = client_message(&request) else {
        panic!("expected revision 1 offer")
    };
    assert_eq!(offer.proof.is_some(), shared);
    assert_eq!(offer.media & caps::SHARED != 0, shared);
    assert_eq!(offer.features & caps::SCROLL, 0);
    write_packet(server, &request);
    let mut state = TerminalState::new(40, 120);
    let mut selected = false;
    loop {
        let packet = read_packet(server);
        let message = server_message(&packet);
        if matches!(
            message,
            Server::NativeRenderResult(_) | Server::RenderFrame { .. }
        ) {
            assert_eq!(
                wire::ServerToClientMsg::decode(packet.as_slice())
                    .unwrap()
                    .attachment_generation
                    .as_ref(),
                Some(&generation)
            );
        }
        write_packet(client, &packet);
        match message {
            Server::NativeRenderResult(caps::Decision::Selected(a)) => {
                assert_eq!(a.media & caps::SHARED != 0, shared);
                let root = offer.proof.as_ref().map(|proof| {
                    let root = MediaRoot::open(Path::new(&proof.root)).unwrap();
                    // The live client may unlink the probe as soon as it gets this decision.
                    Arc::new(root)
                });
                state.negotiate(a, root);
                selected = true;
            },
            Server::RenderFrame { frame } => {
                assert!(selected);
                assert!(decode(&frame).unwrap().header().full_repaint());
                let seq = state.apply_frame(&frame).unwrap().seq;
                forward_ack(client, server, seq, &generation);
                return (state, generation);
            },
            _ => {},
        }
    }
}

fn forward_ack(client: &mut UnixStream, server: &mut UnixStream, expected: u64, generation: &[u8]) {
    let packet = read_packet(client);
    assert_eq!(
        wire::ClientToServerMsg::decode(packet.as_slice())
            .unwrap()
            .attachment_generation
            .as_deref(),
        Some(generation)
    );
    assert_eq!(
        client_message(&packet),
        Client::RenderFrameAck { seq: expected }
    );
    write_packet(server, &packet);
}

fn trigger(fixture: &NativeFixture, name: &str) {
    fs::write(fixture.fixture.path().join(name), []).unwrap();
}

pub(super) fn refusal() {
    let fixture = start_server(32);
    let mut text = text_viewer(&fixture);
    let proxy = tempfile::tempdir_in("/tmp").unwrap();
    let dir = proxy
        .path()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    fs::create_dir(&dir).unwrap();
    let listener = UnixListener::bind(dir.join(SESSION)).unwrap();
    let record = fixture.fixture.path().join("held.frames");
    let mut child = fixture
        .terminal(SESSION, false, &record)
        .env("YINZ_SOCKET_DIR", proxy.path())
        .arg("--headless")
        .spawn()
        .unwrap();
    // The local CLI discovers its named endpoint with a separate probe.
    let mut probe = bounded(listener.accept().unwrap().0);
    let mut upstream = server(&fixture);
    let request = read_packet(&mut probe);
    assert_eq!(client_message(&request), Client::ConnStatus);
    write_packet(&mut upstream, &request);
    loop {
        let packet = read_packet(&mut upstream);
        write_packet(&mut probe, &packet);
        if matches!(server_message(&packet), Server::Connected { .. }) {
            break;
        }
    }
    drop((probe, upstream));
    let mut client = bounded(listener.accept().unwrap().0);
    let mut upstream = server(&fixture);
    let (mut state, generation) = exchange(&mut client, &mut upstream, true);
    let mut held = Vec::new();
    let mut leases = std::collections::BTreeMap::new();
    let mut colors = std::collections::HashMap::new();
    for n in 1..=32 {
        trigger(&fixture, &format!("image-{n}"));
        while leases.len() < n {
            let packet = read_packet(&mut upstream);
            if let Server::RenderFrame { frame } = server_message(&packet) {
                let view = decode(&frame).unwrap();
                for record in view.graphics() {
                    if let GraphicsRecord::Residency {
                        id,
                        medium,
                        data,
                        byte_len,
                        ..
                    } = record
                    {
                        assert_eq!(medium, GraphicsMedium::SharedFile);
                        assert_eq!(byte_len, 256 * 1024);
                        let path = PathBuf::from(String::from_utf8(data).unwrap());
                        assert!(path.is_file());
                        colors.insert(id, n as u8);
                        leases.insert(id, (view.header().seq, path));
                    }
                }
            }
            held.push(packet);
        }
    }
    // The 32 held leases fill this viewer's entry budget, so another large image would
    // only be deferred. Seal instead with a frame that is too large on its own.
    seal_with_oversized_presentation(&mut upstream, &generation);
    let (terminal, reason) = loop {
        let packet = read_packet(&mut upstream);
        if let Server::Exit {
            exit_reason: ExitReason::Error(reason),
        } = server_message(&packet)
        {
            break (packet, reason);
        }
        held.push(packet);
    };
    assert!(
        reason.contains("send queue limit of 67108864 bytes"),
        "{reason}"
    );
    assert_eq!(upstream.read(&mut [0]).unwrap(), 0, "expected writer EOF");
    assert!(leases.values().all(|(_, path)| path.is_file()));
    trigger(&fixture, "during");
    wait_text(&mut text, "DURING-DRAIN");
    for packet in held {
        write_packet(&mut client, &packet);
        if let Server::RenderFrame { frame } = server_message(&packet) {
            let applied = state.apply_frame(&frame).unwrap();
            let covered: Vec<_> = leases
                .iter()
                .filter(|(_, (seq, _))| *seq == applied.seq)
                .collect();
            let ack = read_packet(&mut client);
            assert_eq!(
                client_message(&ack),
                Client::RenderFrameAck { seq: applied.seq }
            );
            for (&id, (_, path)) in covered.iter().copied() {
                assert!(
                    path.exists(),
                    "file disappeared before forwarding consuming ACK"
                );
                let image = state.graphics().image(id).unwrap();
                assert_eq!((image.width, image.height), (256, 256));
                assert_eq!(&image.pixels[..4], &[colors[&id], 72, 13, 255]);
                assert!(state.graphics().placements().any(|p| p.image_id == id));
            }
            write_packet(&mut upstream, &ack);
            for (&id, (_, path)) in covered {
                while_running(&mut child, "consuming ACK unlink", BUDGET, || {
                    (!path.exists()).then_some(())
                });
                assert_eq!(
                    &state.graphics().image(id).unwrap().pixels[..4],
                    &[colors[&id], 72, 13, 255]
                );
            }
        }
    }
    write_packet(&mut client, &terminal);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(&reason));
    assert_eq!(
        client.read(&mut [0]).unwrap(),
        0,
        "unexpected ACK beyond admitted prefix"
    );
    let capture = fs::read(&record).unwrap();
    let mut bytes = capture.as_slice();
    let mut recorded_paths = std::collections::BTreeMap::new();
    while !bytes.is_empty() {
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let (frame, next) = bytes[4..].split_at(len);
        let view = decode(frame).unwrap();
        for record in view.graphics() {
            if let GraphicsRecord::Residency {
                id, medium, data, ..
            } = record
            {
                assert_eq!(medium, GraphicsMedium::SharedFile);
                recorded_paths.insert(
                    id,
                    (
                        view.header().seq,
                        PathBuf::from(String::from_utf8(data).unwrap()),
                    ),
                );
            }
        }
        bytes = next;
    }
    assert_eq!(recorded_paths, leases);
    drop(upstream);
    trigger(&fixture, "after");
    wait_text(&mut text, "AFTER-DRAIN");
}

/// A terminal far larger than the session gets a centered presentation whose explicit
/// margin cells alone exceed the writer's 64 MiB frame limit, sealing the stream on that
/// one frame without relying on any timing.
fn seal_with_oversized_presentation(stream: &mut UnixStream, tag: &[u8]) {
    send_tagged(
        stream,
        tag,
        Client::TerminalResize {
            new_size: Size {
                rows: 2000,
                cols: 2000,
            },
        },
    );
}

#[test]
fn render_negotiation_established_forward_uses_portable_pixels_and_terminal_assets() {
    use interprocess::os::unix::uds_local_socket::Stream;
    let fixture = start_server(1);
    let _text = text_viewer(&fixture);
    let (local, client) = UnixStream::pair().unwrap();
    let mut client = bounded(client);
    let capture = fixture.fixture.path().join("forwarded.frames");
    let args = yinz_cli::args::TerminalArgs {
        headless: true,
        session_name: Some(SESSION.into()),
        working_directory: Some("/missing/remote-cwd".into()),
        record: Some(capture.clone()),
        ..Default::default()
    };
    let opts = yinz_cli::args::CliArgs {
        debug: true,
        config: Some("/missing/local-config".into()),
        layout: Some("/missing/local-layout".into()),
        max_panes: Some(1),
        ..Default::default()
    };
    let consumer = std::thread::spawn(move || {
        yinz_native_terminal::run_forwarded(
            args,
            opts,
            Stream::from(local).into(),
            Some("test-forward".into()),
            SESSION.into(),
            &yinz_types::data::SessionConnection {
                tab_position: Some(yinz_types::internal_ids::TabNumber::new(1)),
                ..Default::default()
            },
        )
    });
    let mut upstream = server(&fixture);
    let (mut state, generation) = exchange(&mut client, &mut upstream, false);
    trigger(&fixture, "image-1");
    let image_id = loop {
        let packet = read_packet(&mut upstream);
        let message = server_message(&packet);
        write_packet(&mut client, &packet);
        if let Server::RenderFrame { frame } = message {
            let applied = state.apply_frame(&frame).unwrap();
            forward_ack(&mut client, &mut upstream, applied.seq, &generation);
            if let Some(id) = decode(&frame)
                .unwrap()
                .graphics()
                .into_iter()
                .find_map(|r| match r {
                    GraphicsRecord::Residency {
                        id, medium, data, ..
                    } => {
                        assert_eq!(medium, GraphicsMedium::Inline);
                        assert_eq!(data, [1, 72, 13, 255].repeat(256 * 256));
                        Some(id)
                    },
                    _ => None,
                })
            {
                break id;
            }
        }
    };
    assert!(state
        .graphics()
        .placements()
        .any(|p| p.image_id == image_id));
    let mut exit = wire::ServerToClientMsg::from(Server::Exit {
        exit_reason: ExitReason::NormalDetached,
    });
    exit.attachment_generation = Some(generation);
    write_packet(&mut client, &exit.encode_to_vec());
    let closed = read_packet(&mut client);
    assert_eq!(client_message(&closed), Client::ClientExited);
    write_packet(&mut upstream, &closed);
    consumer.join().unwrap().unwrap();
    let mut replay = Recording {
        state: Some(TerminalState::new(40, 120)),
        ..Default::default()
    };
    replay.state.as_mut().unwrap().set_shared_media(false);
    replay.read(&capture);
    let state = replay.state.unwrap();
    assert_eq!(
        state.graphics().image(image_id).unwrap().pixels,
        [1, 72, 13, 255].repeat(256 * 256)
    );
    assert!(state
        .graphics()
        .placements()
        .any(|p| p.image_id == image_id));
}

fn advertisement(revision: Option<u32>) -> Vec<u8> {
    advertisement_with_generation(revision, None)
}

fn advertisement_with_generation(revision: Option<u32>, generation: Option<Vec<u8>>) -> Vec<u8> {
    wire::ServerToClientMsg {
        attachment_generation: None,
        message: Some(wire::server_to_client_msg::Message::Connected(
            wire::ConnectedMsg {
                native_render_revision: revision,
                attachment_generation: generation,
                ..Default::default()
            },
        )),
    }
    .encode_to_vec()
}

fn scripted_listener(session: &str) -> (tempfile::TempDir, UnixListener) {
    let proxy = tempfile::tempdir_in("/tmp").unwrap();
    let dir = proxy
        .path()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    fs::create_dir(&dir).unwrap();
    let listener = UnixListener::bind(dir.join(session)).unwrap();
    (proxy, listener)
}

fn scripted_attachment(
    fixture: &NativeFixture,
    proxy: &Path,
    listener: &UnixListener,
    session: &str,
    capture: &Path,
    duration: &str,
) -> (Child, UnixStream) {
    let child = fixture
        .terminal(session, false, capture)
        .env("YINZ_SOCKET_DIR", proxy)
        .args(["--headless", "--duration-secs", duration])
        .spawn()
        .unwrap();
    let mut probe = bounded(listener.accept().unwrap().0);
    assert_eq!(client_message(&read_packet(&mut probe)), Client::ConnStatus);
    write_packet(&mut probe, &advertisement(Some(1)));
    drop(probe);
    let mut peer = bounded(listener.accept().unwrap().0);
    assert!(matches!(
        client_message(&read_packet(&mut peer)),
        Client::AttachClient { .. }
    ));
    (child, peer)
}

#[test]
fn render_negotiation_headless_distinguishes_missing_unsupported_and_unanswered() {
    let fixture = NativeFixture::new();
    let session = "team demo";
    let (proxy, listener) = scripted_listener(session);
    let capture = proxy.path().join("opening.frames");
    fs::write(&capture, b"previous recording").unwrap();
    for (advertised, reason) in [
        (Some(Some(0)), "revision 0, supported 1"),
        (Some(Some(7)), "revision 7, supported 1"),
        (Some(None), "did not advertise"),
        (None, "did not advertise"),
        (Some(Some(1)), "did not answer"),
    ] {
        let (child, mut peer) =
            scripted_attachment(&fixture, proxy.path(), &listener, session, &capture, "30");
        if let Some(revision) = advertised {
            write_packet(&mut peer, &advertisement(revision));
        }
        if advertised == Some(Some(1)) {
            assert!(matches!(
                client_message(&read_packet(&mut peer)),
                Client::NativeRenderRequest(caps::Request::Offer(_))
            ));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(reason), "{stderr}");
        if advertised != Some(Some(1)) {
            assert!(stderr.contains("yinz attach -- 'team demo'"), "{stderr}");
        }
        assert_eq!(fs::read(&capture).unwrap(), b"previous recording");
        assert_eq!(peer.read(&mut [0]).unwrap(), 0, "unexpected native message");
    }
}

#[test]
fn render_negotiation_headless_reports_why_capture_ended_before_repaint() {
    let fixture = NativeFixture::new();
    let (proxy, listener) = scripted_listener(SESSION);
    let capture = proxy.path().join("kept.frames");
    fs::write(&capture, b"previous recording").unwrap();

    for (exit, expected) in [
        (Some(ExitReason::NormalDetached), "Session detached"),
        (Some(ExitReason::KickedByHost), "Disconnected by host"),
        (None, "Capture duration elapsed"),
    ] {
        let duration = if exit.is_some() { "30" } else { "1" };
        let (child, mut peer) = scripted_attachment(
            &fixture,
            proxy.path(),
            &listener,
            SESSION,
            &capture,
            duration,
        );
        write_packet(&mut peer, &advertisement(Some(1)));
        assert!(matches!(
            client_message(&read_packet(&mut peer)),
            Client::NativeRenderRequest(caps::Request::Offer(_))
        ));
        if let Some(exit_reason) = exit {
            write_packet(
                &mut peer,
                &wire::ServerToClientMsg::from(Server::Exit { exit_reason }).encode_to_vec(),
            );
        }
        let output = child.wait_with_output().unwrap();
        let message = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{message}");
        for part in [
            expected,
            "before the first repaint",
            capture.to_str().unwrap(),
        ] {
            assert!(message.contains(part), "{message}");
        }
        assert_eq!(fs::read(&capture).unwrap(), b"previous recording");
        assert_eq!(peer.read(&mut [0]).unwrap(), 0, "unexpected frame ACK");
    }
}

#[test]
fn render_negotiation_headless_switch_reports_capture_boundary() {
    let fixture = NativeFixture::new();
    let (proxy, listener) = scripted_listener(SESSION);
    let capture = proxy.path().join("switch.frames");
    for (destination, boundary) in [
        (Some("next session"), "after frame 9"),
        (None, "before the first repaint"),
    ] {
        fs::write(&capture, b"previous recording").unwrap();
        let (child, mut peer) =
            scripted_attachment(&fixture, proxy.path(), &listener, SESSION, &capture, "10");
        write_packet(&mut peer, &advertisement(Some(1)));
        let Client::NativeRenderRequest(caps::Request::Offer(offer)) =
            client_message(&read_packet(&mut peer))
        else {
            panic!("expected native offer")
        };
        let send_server = |peer: &mut UnixStream, msg: Server| {
            write_packet(peer, &wire::ServerToClientMsg::from(msg).encode_to_vec())
        };
        send_server(
            &mut peer,
            Server::NativeRenderResult(caps::Decision::Selected(offer.select(None).unwrap())),
        );
        let mut expected = b"previous recording".to_vec();
        if destination.is_some() {
            let mut frame = yinz_types::structured_render::FrameBuilder::new(80, 24, 9);
            frame.set_full_repaint(true);
            let frame = frame.finish();
            expected = (frame.len() as u32).to_le_bytes().to_vec();
            expected.extend_from_slice(&frame);
            send_server(&mut peer, Server::RenderFrame { frame });
            assert_eq!(
                client_message(&read_packet(&mut peer)),
                Client::RenderFrameAck { seq: 9 }
            );
        }
        send_server(
            &mut peer,
            Server::SwitchSession {
                connect_to_session: yinz_types::data::SessionConnection {
                    name: destination.map(Into::into),
                    ..Default::default()
                },
            },
        );
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        for part in [
            SESSION,
            "requested a switch",
            destination.unwrap_or("a new session"),
            boundary,
        ] {
            assert!(stderr.contains(part), "{stderr}");
        }
        assert_eq!(fs::read(&capture).unwrap(), expected);
    }
}

#[test]
fn render_negotiation_withdrawal_keeps_the_last_shared_residency_until_consumed() {
    use yinz_runtime::native_media::Probe;
    let fixture = start_server(1);
    let mut text = text_viewer(&fixture);
    let probe = Probe::create(&fixture.fixture.path().join("tmp")).unwrap();
    let offer = caps::Offer::reader(Some(probe.offer.clone()));
    let mut local = attach(&fixture);
    send(
        &mut local,
        Client::NativeRenderRequest(caps::Request::Offer(offer.clone())),
    );
    let mut state = TerminalState::new(40, 120);
    loop {
        match server_message(&read_packet(&mut local)) {
            Server::NativeRenderResult(caps::Decision::Selected(a)) => {
                assert!(offer.accepts(&a, Some(&probe.expected)));
                assert_ne!(a.media & caps::SHARED, 0);
                state.negotiate(
                    a,
                    Some(Arc::new(
                        MediaRoot::open(Path::new(&probe.offer.root)).unwrap(),
                    )),
                );
            },
            Server::RenderFrame { frame } => {
                let seq = state.apply_frame(&frame).unwrap().seq;
                send(&mut local, Client::RenderFrameAck { seq });
                break;
            },
            _ => {},
        }
    }
    trigger(&fixture, "image-1");
    let (path, id, r) = loop {
        let Server::RenderFrame { frame } = server_message(&read_packet(&mut local)) else {
            continue;
        };
        let seq = state.apply_frame(&frame).unwrap().seq;
        if let Some((id, path)) =
            decode(&frame)
                .unwrap()
                .graphics()
                .into_iter()
                .find_map(|record| match record {
                    GraphicsRecord::Residency {
                        id,
                        medium: GraphicsMedium::SharedFile,
                        data,
                        ..
                    } => Some((id, PathBuf::from(String::from_utf8(data).unwrap()))),
                    _ => None,
                })
        {
            break (path, id, seq);
        }
        send(&mut local, Client::RenderFrameAck { seq });
    };
    send(
        &mut local,
        Client::NativeRenderRequest(caps::Request::Withdraw),
    );
    let mut last = r;
    let high = loop {
        match server_message(&read_packet(&mut local)) {
            Server::NativeRenderResult(caps::Decision::Withdrawn(h)) => break h,
            Server::RenderFrame { frame } => last = state.apply_frame(&frame).unwrap().seq,
            _ => {},
        }
    };
    assert_eq!(last, high);
    // While the shared file is outstanding, a legacy viewer gets the retained
    // image inline. Hold its ACK across the attempted upgrade on that socket.
    let mut legacy = attach(&fixture);
    send(
        &mut legacy,
        Client::StructuredRenderSupport { supported: true },
    );
    let mut legacy_state = TerminalState::new(40, 120);
    let legacy_seq = loop {
        if let Server::RenderFrame { frame } = server_message(&read_packet(&mut legacy)) {
            let seq = legacy_state.apply_frame(&frame).unwrap().seq;
            if let Some(image) = legacy_state.graphics().image(id) {
                assert_eq!(image.pixels, [1, 72, 13, 255].repeat(256 * 256));
                break seq;
            }
        }
    };
    send(
        &mut legacy,
        Client::StructuredRenderSupport { supported: false },
    );
    for delayed_ack in [false, true] {
        if delayed_ack {
            send(&mut legacy, Client::RenderFrameAck { seq: legacy_seq });
        }
        send(
            &mut legacy,
            Client::NativeRenderRequest(caps::Request::Offer(offer.clone())),
        );
        loop {
            if let Server::NativeRenderResult(decision) = server_message(&read_packet(&mut legacy))
            {
                assert!(
                    matches!(decision, caps::Decision::Rejected(reason) if reason.contains("reattach"))
                );
                break;
            }
        }
        assert!(
            path.exists(),
            "legacy attachment released another viewer's media"
        );
    }
    // The following rejected offer is an ordered screen roundtrip after each ACK.
    for (seq, retained) in [
        (r - 1, true),
        (high + 1, true),
        (high, false),
        (high, false),
    ] {
        send(&mut local, Client::RenderFrameAck { seq });
        send(
            &mut local,
            Client::NativeRenderRequest(caps::Request::Offer(offer.clone())),
        );
        loop {
            match server_message(&read_packet(&mut local)) {
                Server::NativeRenderResult(caps::Decision::Rejected(_)) => break,
                Server::RenderFrame { .. } => panic!("native output resumed after withdrawal"),
                _ => {},
            }
        }
        assert_eq!(path.exists(), retained);
    }
    assert_eq!(
        state.graphics().image(id).unwrap().pixels,
        [1, 72, 13, 255].repeat(256 * 256)
    );
    trigger(&fixture, "during");
    wait_text(&mut text, "DURING-DRAIN");
    loop {
        match server_message(&read_packet(&mut local)) {
            Server::Render { content } if content.contains("DURING-DRAIN") => break,
            Server::RenderFrame { .. } => panic!("native output resumed after withdrawal"),
            _ => {},
        }
    }
}

#[test]
fn render_negotiation_legacy_and_incompatible_first_offer_on_live_attachments() {
    let fixture = start_server(1);
    let mut text = text_viewer(&fixture);
    let mut legacy = attach(&fixture);
    send(
        &mut legacy,
        Client::StructuredRenderSupport { supported: true },
    );
    let mut state = TerminalState::new(40, 120);
    let mut first = true;
    loop {
        match server_message(&read_packet(&mut legacy)) {
            Server::NativeRenderResult(result) => panic!("legacy client received {result:?}"),
            Server::RenderFrame { frame } => {
                let view = decode(&frame).unwrap();
                assert_eq!(view.header().version, 4);
                if first {
                    assert!(view.header().full_repaint());
                }
                let seq = state.apply_frame(&frame).unwrap().seq;
                send(&mut legacy, Client::RenderFrameAck { seq });
                if first {
                    trigger(&fixture, "image-1");
                    first = false;
                }
                if let Some((id, medium)) = view.graphics().into_iter().find_map(|r| match r {
                    GraphicsRecord::Residency { id, medium, .. } => Some((id, medium)),
                    _ => None,
                }) {
                    assert_eq!(medium, GraphicsMedium::Inline);
                    assert_eq!(
                        state.graphics().image(id).unwrap().pixels,
                        [1, 72, 13, 255].repeat(256 * 256)
                    );
                    break;
                }
            },
            _ => {},
        }
    }
    send(
        &mut legacy,
        Client::StructuredRenderSupport { supported: false },
    );
    trigger(&fixture, "during");
    wait_ansi_after_legacy(&mut legacy, &mut state, "DURING-DRAIN");
    wait_text(&mut text, "DURING-DRAIN");

    let mut incompatible = attach(&fixture);
    let mut offer = caps::Offer::reader(None);
    offer.versions = vec![3, 2];
    send(
        &mut incompatible,
        Client::NativeRenderRequest(caps::Request::Offer(offer)),
    );
    loop {
        match server_message(&read_packet(&mut incompatible)) {
            Server::NativeRenderResult(caps::Decision::Rejected(reason)) => {
                assert!(
                    reason.contains("no common native frame version"),
                    "{reason}"
                );
                assert!(reason.contains("offered [3, 2]"), "{reason}");
                assert!(reason.contains("writable [4]"), "{reason}");
                break;
            },
            Server::RenderFrame { .. } | Server::NativeRenderResult(_) => {
                panic!("incompatible offer enabled native output")
            },
            _ => {},
        }
    }
    trigger(&fixture, "after");
    loop {
        match server_message(&read_packet(&mut incompatible)) {
            Server::Render { content } if content.contains("AFTER-DRAIN") => break,
            Server::RenderFrame { .. } | Server::NativeRenderResult(_) => {
                panic!("unexpected native output after rejection")
            },
            _ => {},
        }
    }
    wait_text(&mut text, "AFTER-DRAIN");
}

fn wait_ansi_after_legacy(stream: &mut UnixStream, state: &mut TerminalState, marker: &str) {
    loop {
        match server_message(&read_packet(stream)) {
            Server::Render { content } if content.contains(marker) => return,
            Server::NativeRenderResult(result) => panic!("legacy client received {result:?}"),
            Server::RenderFrame { frame } => {
                // Frames queued before the boolean withdrawal may still arrive.
                let seq = state.apply_frame(&frame).unwrap().seq;
                send(stream, Client::RenderFrameAck { seq });
            },
            _ => {},
        }
    }
}

#[test]
fn attachment_generation_headless_reconnect_keeps_its_own_recording_and_ack() {
    let fixture = NativeFixture::new();
    let (proxy, listener) = scripted_listener(SESSION);
    let capture = proxy.path().join("reconnect.frames");
    let original = advertisement_with_generation(Some(1), Some(vec![0x11; 16]));
    let previous_capture = proxy.path().join("original.frames");
    let (old_child, mut old_peer) = scripted_attachment(
        &fixture,
        proxy.path(),
        &listener,
        SESSION,
        &previous_capture,
        "30",
    );
    write_packet(&mut old_peer, &original);
    let tagged = |owner: &[u8], message: Server| {
        let mut wire = wire::ServerToClientMsg::from(message);
        wire.attachment_generation = Some(owner.to_vec());
        wire.encode_to_vec()
    };
    let render = |seq, clear| {
        let mut frame = yinz_types::structured_render::FrameBuilder::new(80, 24, seq);
        frame.set_full_repaint(seq == 1);
        frame.set_clear(clear);
        frame.finish()
    };
    let ack = |peer: &mut UnixStream, owner: &[u8], seq| {
        let ack = wire::ClientToServerMsg::decode(read_packet(peer).as_slice()).unwrap();
        assert_eq!(ack.attachment_generation.as_deref(), Some(owner));
        assert_eq!(
            Client::try_from(ack).unwrap(),
            Client::RenderFrameAck { seq }
        );
    };
    let a = [0x11; 16];
    let request = wire::ClientToServerMsg::decode(read_packet(&mut old_peer).as_slice()).unwrap();
    assert_eq!(request.attachment_generation.as_deref(), Some(a.as_slice()));
    let Client::NativeRenderRequest(caps::Request::Offer(old_offer)) = request.try_into().unwrap()
    else {
        panic!("expected offer on original connection");
    };
    write_packet(
        &mut old_peer,
        &tagged(
            &a,
            Server::NativeRenderResult(caps::Decision::Selected(old_offer.select(None).unwrap())),
        ),
    );
    let old_first = render(1, true);
    let old_second = render(2, true);
    let replay = tagged(
        &a,
        Server::RenderFrame {
            frame: old_second.clone(),
        },
    );
    write_packet(
        &mut old_peer,
        &tagged(
            &a,
            Server::RenderFrame {
                frame: old_first.clone(),
            },
        ),
    );
    ack(&mut old_peer, &a, 1);
    write_packet(&mut old_peer, &replay);
    ack(&mut old_peer, &a, 2);
    let (child, mut peer) =
        scripted_attachment(&fixture, proxy.path(), &listener, SESSION, &capture, "30");
    let owner = vec![0x22; 16];
    write_packet(
        &mut peer,
        &advertisement_with_generation(Some(1), Some(owner.clone())),
    );
    let request = wire::ClientToServerMsg::decode(read_packet(&mut peer).as_slice()).unwrap();
    assert_eq!(request.attachment_generation.as_ref(), Some(&owner));
    let Client::NativeRenderRequest(caps::Request::Offer(offer)) = request.try_into().unwrap()
    else {
        panic!("expected one offer on the replacement connection");
    };
    // The same damaged LogError fails on its owner and is skipped on the replacement.
    let bad_log = [vec![0x32, 3, 0x0a, 1, 0xff, 0xb2, 2, 16], vec![0x11; 16]].concat();
    write_packet(&mut old_peer, &bad_log);
    let failed = old_child.wait_with_output().unwrap();
    let reason = String::from_utf8_lossy(&failed.stderr);
    assert!(
        !failed.status.success()
            && reason.contains("invalid IPC message")
            && reason.contains(SESSION),
        "{reason}"
    );
    write_packet(&mut peer, &original);
    // Foreign packets have intact top-level extents but undecodable message bodies.
    for mut packet in [vec![0x32, 3, 0x0a, 1, 0xff], vec![0xa2, 2, 0]] {
        packet.extend_from_slice(&[0xb2, 2, 16]);
        packet.extend_from_slice(&[0x11; 16]);
        write_packet(&mut peer, &packet);
    }
    let send_current = |peer: &mut UnixStream, msg: Server| {
        let mut wire = wire::ServerToClientMsg::from(msg);
        wire.attachment_generation = Some(owner.clone());
        write_packet(peer, &wire.encode_to_vec());
    };
    send_current(
        &mut peer,
        Server::NativeRenderResult(caps::Decision::Selected(offer.select(None).unwrap())),
    );
    let frame = render(1, false);
    send_current(
        &mut peer,
        Server::RenderFrame {
            frame: frame.clone(),
        },
    );
    ack(&mut peer, &owner, 1);
    write_packet(&mut peer, &replay);
    let second = render(2, false);
    send_current(
        &mut peer,
        Server::RenderFrame {
            frame: second.clone(),
        },
    );
    ack(&mut peer, &owner, 2);
    send_current(
        &mut peer,
        Server::Exit {
            exit_reason: ExitReason::NormalDetached,
        },
    );
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Any extra ACK for the replay precedes this orderly exit on B's socket.
    let exit = wire::ClientToServerMsg::decode(read_packet(&mut peer).as_slice()).unwrap();
    assert_eq!(exit.attachment_generation.as_ref(), Some(&owner));
    assert_eq!(Client::try_from(exit).unwrap(), Client::ClientExited);
    let recording = |frames: &[Vec<u8>]| {
        frames
            .iter()
            .flat_map(|frame| {
                (frame.len() as u32)
                    .to_le_bytes()
                    .into_iter()
                    .chain(frame.iter().copied())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fs::read(previous_capture).unwrap(),
        recording(&[old_first, old_second])
    );
    assert_eq!(fs::read(capture).unwrap(), recording(&[frame, second]));
}

fn send_tagged(stream: &mut UnixStream, tag: &[u8], message: Client) {
    let mut envelope = wire::ClientToServerMsg::try_from(message).unwrap();
    envelope.attachment_generation = Some(tag.to_vec());
    write_packet(stream, &envelope.encode_to_vec());
}

fn select_tagged(
    stream: &mut UnixStream,
    tag: &[u8],
    offer: caps::Offer,
    root: Option<Arc<MediaRoot>>,
) -> TerminalState {
    send_tagged(
        stream,
        tag,
        Client::NativeRenderRequest(caps::Request::Offer(offer)),
    );
    let mut state = TerminalState::new(40, 120);
    let mut selected = false;
    loop {
        let packet = wire::ServerToClientMsg::decode(read_packet(stream).as_slice()).unwrap();
        let matching = packet.attachment_generation.as_deref() == Some(tag);
        match Server::try_from(packet).unwrap() {
            Server::NativeRenderResult(caps::Decision::Selected(agreement)) => {
                assert!(matching && !selected);
                state.negotiate(agreement, root.clone());
                selected = true;
            },
            Server::RenderFrame { frame } => {
                assert!(selected && matching && decode(&frame).unwrap().header().full_repaint());
                let seq = state.apply_frame(&frame).unwrap().seq;
                send_tagged(stream, tag, Client::RenderFrameAck { seq });
                return state;
            },
            Server::NativeRenderResult(other) => panic!("unexpected opening decision {other:?}"),
            Server::Exit { exit_reason } => panic!("unexpected exit {exit_reason:?}"),
            _ => {},
        }
    }
}

#[test]
fn attachment_generation_invalid_offer_and_withdraw_diverge() {
    use yinz_runtime::native_media::Probe;
    let fixture = start_server(1);
    let mut second = text_viewer(&fixture);
    let (mut healthy, owner) = advertised_attachment(&fixture);
    let probe = Probe::create(&fixture.fixture.path().join("tmp")).unwrap();
    let root = Arc::new(MediaRoot::open(Path::new(&probe.offer.root)).unwrap());
    let mut state = select_tagged(
        &mut healthy,
        &owner,
        caps::Offer::reader(Some(probe.offer.clone())),
        Some(root),
    );

    for bytes in [0, 15] {
        let (mut offender, _) = advertised_attachment(&fixture);
        send_tagged(
            &mut offender,
            &vec![0; bytes],
            Client::NativeRenderRequest(caps::Request::Offer(caps::Offer::reader(None))),
        );
        loop {
            match server_message(&read_packet(&mut offender)) {
                Server::Exit {
                    exit_reason: ExitReason::Error(error),
                } => {
                    assert!(error.contains("attachment generation"));
                    break;
                },
                Server::NativeRenderResult(_) | Server::RenderFrame { .. } => {
                    panic!("invalid offer was admitted")
                },
                _ => {},
            }
        }
        let (mut replacement, tag) = advertised_attachment(&fixture);
        send_tagged(
            &mut replacement,
            &vec![0; bytes],
            Client::NativeRenderRequest(caps::Request::Withdraw),
        );
        send_tagged(
            &mut replacement,
            &owner,
            Client::NativeRenderRequest(caps::Request::Withdraw),
        );
        select_tagged(&mut replacement, &tag, caps::Offer::reader(None), None);
    }

    trigger(&fixture, "image-1");
    let path = loop {
        let packet = wire::ServerToClientMsg::decode(read_packet(&mut healthy).as_slice()).unwrap();
        assert_eq!(packet.attachment_generation.as_ref(), Some(&owner));
        if let Server::RenderFrame { frame } = Server::try_from(packet).unwrap() {
            let seq = state.apply_frame(&frame).unwrap().seq;
            let path =
                decode(&frame)
                    .unwrap()
                    .graphics()
                    .into_iter()
                    .find_map(|record| match record {
                        GraphicsRecord::Residency {
                            medium: GraphicsMedium::SharedFile,
                            data,
                            ..
                        } => Some(PathBuf::from(String::from_utf8(data).unwrap())),
                        _ => None,
                    });
            if let Some(ref path) = path {
                assert!(path.exists());
            }
            send_tagged(&mut healthy, &owner, Client::RenderFrameAck { seq });
            if let Some(path) = path {
                break path;
            }
        }
    };
    send_tagged(
        &mut healthy,
        &owner,
        Client::NativeRenderRequest(caps::Request::Withdraw),
    );
    while !matches!(
        server_message(&read_packet(&mut healthy)),
        Server::NativeRenderResult(caps::Decision::Withdrawn(_))
    ) {}
    assert!(
        !path.exists(),
        "healthy viewer's cumulative ACK was consumed"
    );
    trigger(&fixture, "during");
    wait_text(&mut second, "DURING-DRAIN");
}

#[test]
fn attachment_generation_graphical_keybinding_keeps_source_until_promotion() {
    let fixture = NativeFixture::new();
    let root = fixture.fixture.path();
    let config = fs::read_to_string(&fixture.config).unwrap();
    fs::write(&fixture.config, format!("{config}\nkeybinds {{ normal {{ bind \"Ctrl Alt y\" {{ SwitchSession name=\"destination\"; }}; }}; }}\n")).unwrap();
    let script = root.join("probe.py");
    fs::write(&script, "import os, sys, tty\ntty.setraw(0)\nname = sys.argv[1]\nprint(name+'-ready', flush=True)\nwhile True:\n c=os.read(0,1)\n if not c: break\n print('\\r'+name+'-'+c.decode('ascii','replace'),flush=True)\n").unwrap();
    for name in ["source", "destination"] {
        fs::write(
            &fixture.layout,
            format!(
                "layout {{ pane command=\"python3\" {{ args \"{}\" \"{name}\"; }}; }}",
                script.display()
            ),
        )
        .unwrap();
        let capture = root.join(format!("{name}.frames"));
        fixture.capture_until(
            name,
            &mut fixture.terminal(name, true, &capture),
            &capture,
            "pane ready",
            |recording| {
                recording
                    .grid_and_marker(&format!("{name}-ready"))
                    .is_some()
            },
        );
    }
    let mut driver = Command::new(std::env::current_exe().unwrap());
    fixture.fixture.inherit(&mut driver);
    let output = driver
        .args([
            "--exact",
            "negotiation::live_graphical_driver",
            "--ignored",
            "--nocapture",
        ])
        .env("YINZ_SWITCH_FIXTURE", root)
        .env("YINZ_SOCKET_DIR", fixture.fixture.socket_dir())
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("YINZ_TEST_CACHE_DIR", &fixture.cache)
        .output()
        .unwrap();
    let ran_driver = String::from_utf8_lossy(&output.stdout).contains("1 passed");
    assert_success(output);
    assert!(ran_driver, "the graphical switch driver must run");

    let mut ordinary = bounded(
        UnixStream::connect(
            fixture
                .fixture
                .socket_dir()
                .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
                .join("source"),
        )
        .unwrap(),
    );
    send(
        &mut ordinary,
        Client::AttachClient {
            cli_assets: CliAssets {
                terminal_window_size: Size { rows: 24, cols: 80 },
                ..Default::default()
            },
            tab_number_to_focus: None,
            pane_to_focus: None,
        },
    );
    while !matches!(
        server_message(&read_packet(&mut ordinary)),
        Server::Render { .. }
    ) {}
    fixture.action("source", &["switch-session", "destination"]);
    loop {
        if let Server::SwitchSession { connect_to_session } =
            server_message(&read_packet(&mut ordinary))
        {
            assert_eq!(connect_to_session.name, Some("destination".into()));
            break;
        }
    }
    // The ordinary source must be retired by the server after it delivers the handoff.
    ordinary.read_to_end(&mut Vec::new()).unwrap();
}

#[test]
#[ignore = "driven under the real fixture's socket namespace"]
fn live_graphical_driver() {
    let root = PathBuf::from(std::env::var_os("YINZ_SWITCH_FIXTURE").unwrap());
    let opts = yinz_cli::args::CliArgs {
        config: Some(root.join("config.kdl")),
        config_dir: Some(root.join("config-dir")),
        data_dir: Some(root.join("data")),
        ..Default::default()
    };
    yinz_native_terminal::exercise_live_switch(opts, || {
        let output = Command::new(env!("CARGO_BIN_EXE_yinz"))
            .args([
                "--session",
                "source",
                "action",
                "switch-session",
                "destination",
            ])
            .output()
            .unwrap();
        assert_success(output);
    })
    .unwrap();
}

// Shared-media budgets and cleanup through the real server, observed in the fixture's TMPDIR.

const REPLACEMENT_BYTES: u64 = 2048 * 2048 * 4;
const VIEWER_ENTRIES: usize = yinz_runtime::native_media::VIEWER_MEDIA_ENTRIES;
const VIEWER_BYTES: u64 = yinz_runtime::native_media::VIEWER_MEDIA_BYTES as u64;

/// Staged image files and their sizes. A file unlinked mid-listing is skipped.
fn staged(fixture: &NativeFixture) -> std::collections::BTreeMap<PathBuf, u64> {
    let dir = fixture.fixture.path().join("tmp").canonicalize().unwrap();
    fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            name.starts_with(yinz_runtime::native_media::IMAGE_PREFIX)
                .then_some(())?;
            Some((dir.join(name), entry.metadata().ok()?.len()))
        })
        .collect()
}

/// One shared viewer may retain at most its entry and byte budget.
fn assert_within_viewer_budget(fixture: &NativeFixture) {
    let files = staged(fixture);
    let bytes: u64 = files.values().sum();
    assert!(
        files.len() <= VIEWER_ENTRIES && bytes <= VIEWER_BYTES,
        "{} staged files, {bytes} bytes",
        files.len()
    );
}

fn wait_until<T>(what: &str, mut observe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + BUDGET;
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::park_timeout(Duration::from_millis(20));
    }
}

fn replace(fixture: &NativeFixture, text: &mut UnixStream, n: usize) {
    trigger(fixture, &format!("replace-{n}"));
    wait_text(text, &format!("REPLACED-{n:03}"));
}

#[derive(Debug, Default)]
struct Seen {
    seq: u64,
    shared: Vec<PathBuf>,
    inline: usize,
    placements: usize,
}

enum Event {
    Frame(Seen),
    Exit(String),
}

/// A local viewer that proved a shared directory, read through the production decoder.
struct SharedViewer {
    stream: UnixStream,
    tag: Vec<u8>,
    state: TerminalState,
    root: Arc<MediaRoot>,
    _probe: yinz_runtime::native_media::Probe,
}

impl SharedViewer {
    /// Attach, negotiate shared media and return the unacknowledged first full repaint.
    fn attach(fixture: &NativeFixture) -> (Self, Seen) {
        let (stream, tag) = advertised_attachment(fixture);
        let probe =
            yinz_runtime::native_media::Probe::create(&fixture.fixture.path().join("tmp")).unwrap();
        let root = Arc::new(MediaRoot::open(Path::new(&probe.offer.root)).unwrap());
        let offer = caps::Offer::reader(Some(probe.offer.clone()));
        let mut viewer = Self {
            stream,
            tag,
            state: TerminalState::new(40, 120),
            root,
            _probe: probe,
        };
        viewer.send(Client::NativeRenderRequest(caps::Request::Offer(offer)));
        let first = viewer.frame();
        assert!(first.seq > 0);
        (viewer, first)
    }

    fn send(&mut self, message: Client) {
        send_tagged(&mut self.stream, &self.tag.clone(), message);
    }

    fn ack(&mut self, seq: u64) {
        self.send(Client::RenderFrameAck { seq });
    }

    fn packet(&mut self) -> wire::ServerToClientMsg {
        wire::ServerToClientMsg::decode(read_packet(&mut self.stream).as_slice()).unwrap()
    }

    fn next(&mut self) -> Event {
        loop {
            let packet = self.packet();
            if let Some(event) = self.handle(packet) {
                return event;
            }
        }
    }

    fn handle(&mut self, packet: wire::ServerToClientMsg) -> Option<Event> {
        let owned = packet.attachment_generation.as_deref() == Some(self.tag.as_slice());
        match Server::try_from(packet).unwrap() {
            Server::NativeRenderResult(caps::Decision::Selected(agreement)) => {
                assert!(owned && agreement.media & caps::SHARED != 0);
                self.state.negotiate(agreement, Some(self.root.clone()));
            },
            Server::RenderFrame { frame } => {
                assert!(owned);
                let view = decode(&frame).unwrap();
                let mut seen = Seen {
                    seq: view.header().seq,
                    ..Default::default()
                };
                for record in view.graphics() {
                    match record {
                        GraphicsRecord::Residency {
                            medium: GraphicsMedium::SharedFile,
                            data,
                            ..
                        } => {
                            let path = PathBuf::from(String::from_utf8(data).unwrap());
                            assert!(path.is_file(), "staged before its frame");
                            seen.shared.push(path);
                        },
                        GraphicsRecord::Residency { .. } => seen.inline += 1,
                        GraphicsRecord::Placement { .. } => seen.placements += 1,
                        _ => {},
                    }
                }
                self.state.apply_frame(&frame).unwrap();
                return Some(Event::Frame(seen));
            },
            Server::Exit {
                exit_reason: ExitReason::Error(reason),
            } => return Some(Event::Exit(reason)),
            Server::Exit { exit_reason } => return Some(Event::Exit(format!("{exit_reason:?}"))),
            _ => {},
        }
        None
    }

    fn frame(&mut self) -> Seen {
        match self.next() {
            Event::Frame(seen) => seen,
            Event::Exit(reason) => panic!("viewer ended: {reason}"),
        }
    }

    fn shows(&self, marker: &str) -> bool {
        let size = self.state.size();
        (0..size.rows).any(|r| {
            (0..size.cols)
                .map(|c| self.state.screen().cell(r, c).character())
                .collect::<String>()
                .contains(marker)
        })
    }

    /// Frames up to the one showing `marker`.
    fn frames_until(&mut self, marker: &str) -> Vec<Seen> {
        let mut frames = Vec::new();
        while !self.shows(marker) {
            frames.push(self.frame());
        }
        frames
    }

    /// A rejected duplicate offer is an ordered screen roundtrip after anything sent
    /// before it on this connection. Frames that arrive first are returned.
    fn roundtrip(&mut self) -> Vec<Seen> {
        let offer = caps::Offer::reader(Some(self._probe.offer.clone()));
        self.send(Client::NativeRenderRequest(caps::Request::Offer(offer)));
        let mut frames = Vec::new();
        loop {
            let packet = self.packet();
            if let Ok(Server::NativeRenderResult(caps::Decision::Rejected(reason))) =
                Server::try_from(packet.clone())
            {
                assert!(reason.contains("reattach"), "{reason}");
                return frames;
            }
            match self.handle(packet) {
                Some(Event::Frame(seen)) => frames.push(seen),
                Some(Event::Exit(reason)) => panic!("viewer ended: {reason}"),
                None => {},
            }
        }
    }

    fn image_shade(&self) -> Option<u8> {
        self.state
            .graphics()
            .placements()
            .filter_map(|placement| self.state.graphics().image(placement.image_id))
            .map(|image| image.pixels[0])
            .max()
    }
}

#[test]
fn native_media_budget_defers_withheld_acks_and_releases_on_resume() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    let (_other, foreign) = advertised_attachment(&fixture);
    let (mut viewer, first) = SharedViewer::attach(&fixture);
    assert!(first.shared.is_empty());
    viewer.ack(first.seq);
    let mut held = Vec::new();
    let mut last = first.seq;
    let per_viewer = (VIEWER_BYTES / REPLACEMENT_BYTES) as usize;
    for n in 1..=per_viewer + 1 {
        trigger(&fixture, &format!("replace-{n}"));
        let frames = viewer.frames_until(&format!("REPLACED-{n:03}"));
        for frame in &frames {
            assert_eq!(
                frame.inline, 0,
                "a budget refusal never falls back to inline"
            );
            assert_within_viewer_budget(&fixture);
            last = frame.seq;
        }
        let shared: Vec<_> = frames.iter().flat_map(|f| f.shared.clone()).collect();
        if n <= per_viewer {
            assert_eq!(shared.len(), 1, "replacement {n}");
            held.push(shared[0].clone());
        } else {
            // Deferred: the frame carried the text but neither residency nor placement.
            assert!(
                shared.is_empty(),
                "replacement {n} staged beyond the budget"
            );
            assert_eq!(frames.iter().map(|f| f.placements).sum::<usize>(), 0);
        }
    }
    // The replaced raster was deleted and its successor waits, so no stale image remains.
    assert_eq!(viewer.image_shade(), None);
    assert_eq!(staged(&fixture).len(), per_viewer);
    assert_eq!(staged(&fixture).values().sum::<u64>(), VIEWER_BYTES);
    // Duplicate, stale, future and foreign-generation ACKs leave every file in place.
    let anomalies: [(&[u8], u64); 4] = [
        (&viewer.tag.clone(), first.seq),
        (&viewer.tag.clone(), first.seq - 1),
        (&viewer.tag.clone(), last + 1),
        (&foreign, last),
    ];
    for (tag, seq) in anomalies {
        send_tagged(&mut viewer.stream, tag, Client::RenderFrameAck { seq });
        assert!(viewer.roundtrip().is_empty(), "no retry without a release");
        assert!(held.iter().all(|path| path.is_file()), "ACK {seq} released");
    }
    assert!(fixture
        .log_text()
        .contains("native viewer media budget byte limit reached: deferring"));
    viewer.ack(last);
    wait_until("consumed files unlinked", || {
        held.iter().all(|path| !path.exists()).then_some(())
    });
    // The releasing ACK alone re-renders the deferred image as a shared file.
    let retried = loop {
        let frame = viewer.frame();
        if !frame.shared.is_empty() {
            break frame;
        }
    };
    assert_eq!((retried.shared.len(), retried.inline), (1, 0));
    assert!(retried.placements > 0);
    assert_eq!(viewer.image_shade(), Some(per_viewer as u8 + 1));
    viewer.ack(retried.seq);
    wait_until("retried file unlinked", || {
        (!retried.shared[0].exists()).then_some(())
    });
    replace(&fixture, &mut text, per_viewer + 2);
}

#[test]
fn native_media_stalled_reader_stays_within_budget() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    let (mut viewer, first) = SharedViewer::attach(&fixture);
    viewer.ack(first.seq);
    // The viewer stops reading; the text viewer keeps the session rendering.
    for n in 1..=12 {
        replace(&fixture, &mut text, n);
        assert_within_viewer_budget(&fixture);
    }
    // Staging ran for every render and stopped exactly at the byte budget.
    assert_eq!(staged(&fixture).values().sum::<u64>(), VIEWER_BYTES);
    drop(viewer);
    wait_until("stalled viewer's files unlinked", || {
        staged(&fixture).is_empty().then_some(())
    });
    replace(&fixture, &mut text, 13);
    assert!(staged(&fixture).is_empty());
}

#[test]
fn native_media_cleanup_on_detach_peer_loss_writer_failure_and_withdraw_close() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    let mut n = 1;
    replace(&fixture, &mut text, n);
    let mut modes = vec!["detach", "peer loss", "withdraw then close"];
    // After the peer shuts its read half, Linux fails the server's next write with EPIPE.
    if cfg!(target_os = "linux") {
        modes.push("writer failure");
    }
    for mode in modes {
        let (mut viewer, first) = SharedViewer::attach(&fixture);
        let [path] = first.shared.as_slice() else {
            panic!("{mode}: expected the visible image staged for this viewer")
        };
        let path = path.clone();
        match mode {
            "detach" => viewer.send(Client::ClientExited),
            "peer loss" => {},
            "withdraw then close" => {
                viewer.send(Client::NativeRenderRequest(caps::Request::Withdraw));
                loop {
                    let packet = server_message(&read_packet(&mut viewer.stream));
                    if matches!(
                        packet,
                        Server::NativeRenderResult(caps::Decision::Withdrawn(_))
                    ) {
                        break;
                    }
                }
                assert!(path.is_file(), "withdrawal released an unconsumed file");
            },
            "writer failure" => {
                viewer.stream.shutdown(std::net::Shutdown::Read).unwrap();
                n += 1;
                replace(&fixture, &mut text, n);
            },
            _ => unreachable!(),
        }
        // A writer failure keeps the socket open, so only the failed write can end it.
        let open = (mode == "writer failure").then_some(viewer);
        wait_until(&format!("{mode} unlink"), || (!path.exists()).then_some(()));
        drop(open);
        n += 1;
        replace(&fixture, &mut text, n);
    }
    assert!(staged(&fixture).is_empty());
    assert!(!fixture.log_text().contains("remove staged native media"));
}

#[test]
fn native_media_session_exit_unlinks_staged_files() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    replace(&fixture, &mut text, 1);
    let (mut viewer, first) = SharedViewer::attach(&fixture);
    assert_eq!(first.shared.len(), 1);
    assert_success(
        fixture
            .command()
            .args(["kill-session", SESSION])
            .output()
            .unwrap(),
    );
    // The server closes every attachment before it exits.
    loop {
        let mut prefix = [0; 4];
        match viewer.stream.read(&mut prefix) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let mut rest = [0; 4];
                viewer.stream.read_exact(&mut rest[..4 - read]).unwrap();
                prefix[read..].copy_from_slice(&rest[..4 - read]);
                let mut body = vec![0; u32::from_le_bytes(prefix) as usize];
                viewer.stream.read_exact(&mut body).unwrap();
            },
        }
    }
    let socket = fixture
        .fixture
        .socket_dir()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
        .join(SESSION);
    wait_until("session exit", || {
        (!socket.exists() && staged(&fixture).is_empty()).then_some(())
    });
    assert!(!first.shared[0].exists());
}

#[test]
fn native_media_reattach_keeps_leases_apart() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    replace(&fixture, &mut text, 1);
    let (mut a, a_first) = SharedViewer::attach(&fixture);
    let [a1] = a_first.shared.as_slice() else {
        panic!("expected A's first file")
    };
    let a1 = a1.clone();
    trigger(&fixture, "replace-2");
    let frames = a.frames_until("REPLACED-002");
    let a2: Vec<_> = frames.iter().flat_map(|f| f.shared.clone()).collect();
    let [a2] = a2.as_slice() else {
        panic!("expected A's second file")
    };
    let a2 = a2.clone();
    let high = frames.last().unwrap().seq;
    wait_text(&mut text, "REPLACED-002");
    seal_with_oversized_presentation(&mut a.stream, &a.tag.clone());
    let reason = loop {
        if let Event::Exit(reason) = a.next() {
            break reason;
        }
    };
    assert!(
        reason.contains("send queue limit of 67108864 bytes"),
        "{reason}"
    );
    assert!(a1.is_file() && a2.is_file());
    // B's full repaint stages its own copy of the visible image.
    let (mut b, b_first) = SharedViewer::attach(&fixture);
    let [b1] = b_first.shared.as_slice() else {
        panic!("expected B's own file")
    };
    let b1 = b1.clone();
    assert!(b1 != a1 && b1 != a2);
    // A's generation on B's connection is ignored.
    send_tagged(
        &mut b.stream,
        &a.tag.clone(),
        Client::RenderFrameAck { seq: b_first.seq },
    );
    assert!(b.roundtrip().is_empty());
    assert!(b1.is_file() && a1.is_file() && a2.is_file());
    // Above A's sealed high mark releases nothing; inside it, exactly the covered file.
    a.ack(high + 1);
    a.ack(a_first.seq);
    wait_until("A's first file unlinked", || (!a1.exists()).then_some(()));
    assert!(
        a2.is_file(),
        "an ACK above the sealed prefix released media"
    );
    assert!(b1.is_file(), "A's ACK released B's file");
    // Closing A releases the rest of its prefix and nothing of B's.
    drop(a);
    wait_until("A's prefix unlinked at removal", || {
        (!a2.exists()).then_some(())
    });
    assert!(b.roundtrip().is_empty());
    assert!(b1.is_file());
    b.ack(b_first.seq);
    wait_until("B's file unlinked", || (!b1.exists()).then_some(()));
    replace(&fixture, &mut text, 3);
}

#[test]
fn native_media_disconnect_races() {
    let fixture = start_images("replace");
    let mut text = text_viewer(&fixture);
    // (1) A frame in flight at peer loss: staged, never read, then the stream drops.
    let (mut first, opening) = SharedViewer::attach(&fixture);
    assert!(opening.shared.is_empty());
    first.ack(opening.seq);
    replace(&fixture, &mut text, 1);
    let in_flight = wait_until("staged file for the unread frame", || {
        staged(&fixture).into_keys().next()
    });
    drop(first);
    wait_until("in-flight file unlinked", || {
        (!in_flight.exists()).then_some(())
    });
    let (mut second, opening) = SharedViewer::attach(&fixture);
    let [own] = opening.shared.as_slice() else {
        panic!("expected the second viewer's own file")
    };
    let own = own.clone();
    assert_eq!(
        staged(&fixture).into_keys().collect::<Vec<_>>(),
        std::slice::from_ref(&own)
    );
    // (2) An ACK racing removal: written in the same burst as the shutdown.
    second.ack(opening.seq);
    second.stream.shutdown(std::net::Shutdown::Both).unwrap();
    drop(second);
    wait_until("acknowledged file unlinked", || {
        (!own.exists()).then_some(())
    });
    // (4) A render after removal stages nothing.
    replace(&fixture, &mut text, 2);
    assert!(staged(&fixture).is_empty());
    // (3) ACKs racing a seal are covered by native_media_reattach_keeps_leases_apart.
    // Each file was removed exactly once: a second unlink would log an error.
    assert!(!fixture.log_text().contains("remove staged native media"));
}
