//! A forwarded native viewer assembles a chunked 100 MiB Kitty image over real SSH
//! forwarding, through Yinz's own remote bootstrap and forward. Run with:
//! cargo x remote-test --build
//! cargo x remote-test --test -- --exact tests::native_media_ssh::sequential_ssh::forwarded_viewer_assembles_a_chunked_100_mib_image
//!
//! The `sequential_ssh` module name routes the test to the root binary's remote suite.

pub(super) mod sequential_ssh {
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use yinz_runtime::native_media::MAX_SHARED_IMAGE_BYTES;

    /// The pixel side of the fixture's image in `images.py`'s `large` mode.
    const SIDE: usize = 5120;
    /// The ignored test's path as `cargo x remote-test --test -- --exact` selects it.
    const FORWARDED_TEST: &str =
        "tests::native_media_ssh::sequential_ssh::forwarded_viewer_assembles_a_chunked_100_mib_image";
    const IMAGES: &str = include_str!("native_terminal/images.py");
    const HOST: &str = "yinz-remote-pane";
    const TIMEOUT: Duration = Duration::from_secs(120);

    #[test]
    fn routes_to_the_sequential_remote_suite() {
        let module = module_path!();
        assert!(module.ends_with("::sequential_ssh"), "{module}");
        // `cargo x remote-test --test` sends tests whose path contains this marker to
        // `-p yinz --bin yinz`, where the module path drops the crate name.
        let (_, path) = module.split_once("::").unwrap();
        assert_eq!(
            format!("{path}::forwarded_viewer_assembles_a_chunked_100_mib_image"),
            FORWARDED_TEST
        );
        assert!(FORWARDED_TEST.contains("::sequential_ssh::"));
        // The fixture sends exactly the largest image a reader accepts.
        assert_eq!(SIDE * SIDE * 4, MAX_SHARED_IMAGE_BYTES);
        assert!(IMAGES.contains(&format!("transmit(1, {SIDE}, {SIDE}, \"i=1,\")")));
    }

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn shim() -> PathBuf {
        repo().join("target/remote-test/bin/ssh")
    }

    fn quote(text: &str) -> String {
        format!("'{}'", text.replace('\'', "'\\''"))
    }

    /// Run `script` on the far machine through the fixture's ssh, as the bootstrap does.
    fn far(script: &str, stdin: Option<&str>) -> String {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new(shim())
            .args([HOST, script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(stdin) = stdin {
            input.write_all(stdin.as_bytes()).unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{script}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    mod ipc {
        use prost::Message;
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use yinz_ipc::{
            client_server_contract::client_server_contract as wire,
            ipc::{ClientToServerMsg as Client, ServerToClientMsg as Server},
        };

        pub fn read_packet(stream: &mut UnixStream) -> Vec<u8> {
            let mut prefix = [0; 4];
            stream
                .read_exact(&mut prefix)
                .expect("read IPC packet length");
            let length = u32::from_le_bytes(prefix) as usize;
            assert!(length <= yinz_ipc::MAX_IPC_FRAME_LEN);
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).expect("read IPC packet");
            bytes
        }

        pub fn server_message(bytes: &[u8]) -> Server {
            wire::ServerToClientMsg::decode(bytes)
                .unwrap()
                .try_into()
                .unwrap()
        }

        pub fn send(stream: &mut UnixStream, message: Client) {
            let bytes = wire::ClientToServerMsg::try_from(message)
                .unwrap()
                .encode_to_vec();
            stream
                .write_all(&(bytes.len() as u32).to_le_bytes())
                .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    }

    /// A text-only attachment through the forward that reports 8x16 cells, which a
    /// headless forwarded consumer never does, and waits for the fixture to see them.
    fn pixel_viewer(socket: &Path) -> std::os::unix::net::UnixStream {
        use yinz_ipc::ipc::{
            ClientToServerMsg as Client, PixelDimensions, ServerToClientMsg as Server,
        };
        use yinz_native_terminal::emulation::terminal::TerminalState;
        use yinz_types::{
            input::cli_assets::CliAssets,
            pane_size::{Size, SizeInPixels},
            render_negotiation as caps,
            structured_render::decode,
        };
        let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(TIMEOUT)).unwrap();
        ipc::send(
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
        while !matches!(
            ipc::server_message(&ipc::read_packet(&mut stream)),
            Server::Connected {
                native_render_revision: Some(1),
                ..
            }
        ) {}
        let mut text = caps::Offer::reader(None);
        text.features = 0;
        ipc::send(
            &mut stream,
            Client::NativeRenderRequest(caps::Request::Offer(text)),
        );
        ipc::send(
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
        let mut state = TerminalState::new(40, 120);
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "the fixture never saw cell pixels"
            );
            if let Server::RenderFrame { frame } =
                ipc::server_message(&ipc::read_packet(&mut stream))
            {
                assert!(decode(&frame).unwrap().graphics().is_empty());
                let seq = state.apply_frame(&frame).unwrap().seq;
                ipc::send(&mut stream, Client::RenderFrameAck { seq });
                if shows(&state, "PIXELS-READY") {
                    return stream;
                }
            }
        }
    }

    fn shows(
        state: &yinz_native_terminal::emulation::terminal::TerminalState,
        marker: &str,
    ) -> bool {
        let size = state.size();
        (0..size.rows).any(|r| {
            (0..size.cols)
                .map(|c| state.screen().cell(r, c).character())
                .collect::<String>()
                .contains(marker)
        })
    }

    /// Frames of a growing recording, replayed as `--record` captures are.
    struct Replay {
        state: yinz_native_terminal::emulation::terminal::TerminalState,
        applied: usize,
        frames: usize,
    }

    impl Replay {
        fn read(&mut self, path: &Path) {
            use yinz_types::structured_render::{GraphicsMedium, GraphicsRecord};
            let Ok(bytes) = std::fs::read(path) else {
                return;
            };
            let mut rest = &bytes[self.applied..];
            while let Some(length) = rest.get(..4) {
                let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
                let Some(frame) = rest.get(4..4 + length) else {
                    break;
                };
                // The frame travelled in one IPC envelope, so it fit the frame limit.
                assert!(frame.len() < yinz_ipc::MAX_IPC_FRAME_LEN);
                let view = yinz_types::structured_render::decode(frame).unwrap();
                assert!(!view.graphics().iter().any(|record| matches!(
                    record,
                    GraphicsRecord::Residency {
                        medium: GraphicsMedium::SharedFile,
                        ..
                    }
                )));
                self.state.apply_frame(frame).unwrap();
                self.frames += 1;
                rest = &rest[4 + length..];
            }
            self.applied = bytes.len() - rest.len();
        }

        fn image(&self) -> Option<&[u8]> {
            let graphics = self.state.graphics();
            graphics
                .placements()
                .filter_map(|placement| graphics.image(placement.image_id))
                .map(|image| image.pixels.as_slice())
                .find(|pixels| pixels.len() == SIDE * SIDE * 4)
        }
    }

    #[test]
    #[ignore = "requires the Docker SSH fixture"]
    fn forwarded_viewer_assembles_a_chunked_100_mib_image() {
        use interprocess::os::unix::uds_local_socket::Stream;
        assert!(
            shim().exists(),
            "no ssh shim; run 'cargo x remote-test --build' first"
        );
        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..10];
        let session = format!("native-media-{suffix}");
        let root = format!("/tmp/ynm-{suffix}");
        far(
            &format!("mkdir -p {root} && cat > {root}/images.py"),
            Some(IMAGES),
        );
        let layout = format!(
            "layout {{ pane command=\"python3\" {{ args \"{root}/images.py\" \"large\"; }}; }}"
        );
        far(
            &format!(
                "yinz --layout-string {} attach --create-background {session}",
                quote(&layout)
            ),
            None,
        );
        let path = format!(
            "{}:{}",
            shim().parent().unwrap().display(),
            std::env::var("PATH").unwrap()
        );
        let env = yinz_types::envs::EnvironmentVariables::from_data(
            std::collections::HashMap::from([("PATH".into(), path)]),
        );
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let forward =
            yinz_client::remote_sessions::connect(HOST, &session, false, &cancelled, &env).unwrap();
        let socket = forward.socket().to_owned();
        let _pixels = pixel_viewer(&socket);
        let local = tempfile::tempdir().unwrap();
        let capture = local.path().join("forwarded.frames");
        let args = yinz_cli::args::TerminalArgs {
            headless: true,
            session_name: Some(session.clone()),
            record: Some(capture.clone()),
            ..Default::default()
        };
        let opts = yinz_cli::args::CliArgs {
            config: Some("/missing/local-config".into()),
            ..Default::default()
        };
        let consumer = {
            let session = session.clone();
            let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            std::thread::spawn(move || {
                yinz_native_terminal::run_forwarded(
                    args,
                    opts,
                    Stream::from(stream).into(),
                    Some(HOST.into()),
                    session,
                    &Default::default(),
                )
            })
        };
        let mut replay = Replay {
            state: yinz_native_terminal::emulation::terminal::TerminalState::new(40, 120),
            applied: 0,
            frames: 0,
        };
        replay.state.set_shared_media(false);
        // The first repaint is recorded before the image is triggered.
        let deadline = std::time::Instant::now() + TIMEOUT;
        while replay.frames == 0 {
            assert!(!consumer.is_finished(), "the forwarded consumer ended");
            assert!(std::time::Instant::now() < deadline, "no first repaint");
            std::thread::park_timeout(Duration::from_millis(50));
            replay.read(&capture);
        }
        far(&format!("touch {root}/image-1"), None);
        let expected = [1, 72, 13, 255].repeat(SIDE * SIDE);
        loop {
            replay.read(&capture);
            if replay.image().is_some() && shows(&replay.state, "LARGE-SENT") {
                break;
            }
            assert!(!consumer.is_finished(), "the forwarded consumer ended");
            assert!(
                std::time::Instant::now() < deadline,
                "the image never assembled"
            );
            std::thread::park_timeout(Duration::from_millis(50));
        }
        assert!(replay.image() == Some(expected.as_slice()));
        far(&format!("yinz kill-session {session}; rm -rf {root}"), None);
        consumer.join().unwrap().unwrap();
        drop(forward);
    }
}
