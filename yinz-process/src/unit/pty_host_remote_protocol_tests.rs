use super::*;

#[test]
fn legacy_remote_status_does_not_verify_group_termination() {
    let status: BrokerToServerMsg = serde_json::from_str(
        r#"{"StatusReport":{"cwd":null,"shell_argv":null,"foreground_argv":null}}"#,
    )
    .unwrap();
    assert!(matches!(
        status,
        BrokerToServerMsg::StatusReport {
            child_group_terminated: false,
            ..
        }
    ));
}

use hegel::generators;
use std::io::Cursor;

fn sample_run_command() -> RunCommand {
    RunCommand {
        agent_theme: None,
        command: std::path::PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), "echo hi".to_owned()],
        cwd: Some(std::path::PathBuf::from("/tmp")),
        hold_on_close: true,
        hold_on_start: false,
        originating_plugin: None,
        use_terminal_title: true,
        target: Default::default(),
        remote_default_shell: false,
    }
}

fn server_messages() -> Vec<ServerToBrokerMsg> {
    vec![
        ServerToBrokerMsg::Hello {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            yinz_version: yinz_runtime::consts::VERSION.to_owned(),
        },
        ServerToBrokerMsg::Spawn {
            cmd: sample_run_command(),
            // Non-empty, so the round-trip actually exercises the carried
            // environment rather than an absent one.
            env: vec![
                ("TERM".to_owned(), "xterm-256color".to_owned()),
                ("LC_TIME".to_owned(), "C".to_owned()),
            ],
            terminal_id: 7,
            initial_size: Some(WireWinsize {
                cols: 80,
                rows: 24,
                width_in_pixels: 640,
                height_in_pixels: 384,
            }),
        },
        ServerToBrokerMsg::Resize(WireWinsize {
            cols: 120,
            rows: 40,
            width_in_pixels: 0,
            height_in_pixels: 0,
        }),
        ServerToBrokerMsg::SignalChild(ChildSignal::Hup),
        ServerToBrokerMsg::SignalChild(ChildSignal::Int),
        ServerToBrokerMsg::SignalChild(ChildSignal::Kill),
        ServerToBrokerMsg::Terminate,
        ServerToBrokerMsg::Resume {
            pane_token: "abc123".to_owned(),
            received: 123_456,
        },
        ServerToBrokerMsg::Release,
        ServerToBrokerMsg::ListPanes,
    ]
}

fn broker_messages() -> Vec<BrokerToServerMsg> {
    vec![
        BrokerToServerMsg::HelloAck {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            yinz_version: yinz_runtime::consts::VERSION.to_owned(),
        },
        BrokerToServerMsg::Spawned {
            pane_token: "abc123".to_owned(),
        },
        BrokerToServerMsg::Resumed { dropped: 0 },
        BrokerToServerMsg::Resumed { dropped: 4096 },
        BrokerToServerMsg::OutputGap { dropped: 4096 },
        BrokerToServerMsg::ResumeRefused {
            kind: ResumeRefusal::Unknown,
            reason: "no such pane".to_owned(),
        },
        BrokerToServerMsg::ResumeRefused {
            kind: ResumeRefusal::AlreadyAttached,
            reason: "another server still holds this pane".to_owned(),
        },
        BrokerToServerMsg::SpawnFailed {
            kind: SpawnFailureKind::CommandNotFound,
            message: "no such command".to_owned(),
        },
        BrokerToServerMsg::ChildExited { status: Some(130) },
        BrokerToServerMsg::ChildExited { status: None },
        BrokerToServerMsg::StatusReport {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned(), "--resume".to_owned()]),
        },
        BrokerToServerMsg::StatusReport {
            child_group_terminated: false,
            cwd: None,
            shell_argv: None,
            foreground_argv: None,
        },
        BrokerToServerMsg::Released,
        BrokerToServerMsg::PaneList { entries: vec![] },
        BrokerToServerMsg::PaneList {
            entries: vec![
                RemotePaneEntry {
                    pane_token: "abc123".to_owned(),
                    command: "/bin/zsh".to_owned(),
                    argv: Some(vec!["/bin/zsh".to_owned()]),
                    attached: true,
                    unattached_secs: None,
                },
                RemotePaneEntry {
                    pane_token: "def456".to_owned(),
                    command: "cargo watch -x test".to_owned(),
                    argv: None,
                    attached: false,
                    unattached_secs: Some(1_800),
                },
            ],
        },
    ]
}

#[test]
fn control_messages_round_trip_in_both_directions() {
    for msg in server_messages() {
        let mut buf: Vec<u8> = Vec::new();
        write_control(&mut buf, &msg).expect("write failed");
        let frame = read_frame(&mut Cursor::new(buf))
            .expect("read failed")
            .expect("unexpected EOF");
        let got: ServerToBrokerMsg = frame.decode_control().expect("decode failed");
        assert_eq!(got, msg);
    }
    for msg in broker_messages() {
        let mut buf: Vec<u8> = Vec::new();
        write_control(&mut buf, &msg).expect("write failed");
        let frame = read_frame(&mut Cursor::new(buf))
            .expect("read failed")
            .expect("unexpected EOF");
        let got: BrokerToServerMsg = frame.decode_control().expect("decode failed");
        assert_eq!(got, msg);
    }
}

#[test]
fn pane_list_argv_is_compatible_with_legacy_readers_and_writers() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct LegacyEntry {
        pane_token: String,
        command: String,
        attached: bool,
        unattached_secs: Option<u64>,
    }
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum LegacyMessage {
        PaneList { entries: Vec<LegacyEntry> },
    }
    let legacy = LegacyMessage::PaneList {
        entries: vec![LegacyEntry {
            pane_token: "abc123".to_owned(),
            command: "display text".to_owned(),
            attached: false,
            unattached_secs: Some(90),
        }],
    };
    let mut bytes = vec![];
    write_control(&mut bytes, &legacy).unwrap();
    let decoded: BrokerToServerMsg = read_frame(&mut Cursor::new(bytes))
        .unwrap()
        .unwrap()
        .decode_control()
        .unwrap();
    let BrokerToServerMsg::PaneList { mut entries } = decoded else {
        panic!("expected a pane list")
    };
    assert_eq!(entries[0].argv, None);
    assert!(serde_json::to_value(&entries[0])
        .unwrap()
        .get("argv")
        .is_none());
    entries[0].argv = Some(vec![
        "/my shell".to_owned(),
        "".to_owned(),
        "one two".to_owned(),
    ]);
    let message = BrokerToServerMsg::PaneList { entries };
    let mut bytes = vec![];
    write_control(&mut bytes, &message).unwrap();
    let frame = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();
    assert_eq!(frame.decode_control::<LegacyMessage>().unwrap(), legacy);
    assert_eq!(
        frame.decode_control::<BrokerToServerMsg>().unwrap(),
        message
    );
    assert_eq!(REMOTE_PROTOCOL_VERSION, 8);
}

/// The kind byte is the whole reason pty bytes can stay raw. If output were ever
/// read back as input — or either as a control body — a pane's keystrokes and
/// its output would cross, so pin the discrimination rather than trusting it.
#[test]
fn a_frames_kind_survives_the_round_trip() {
    let payload = b"\x1b[31mred\x1b[0m \xff\xfe not utf-8".to_vec();
    let mut buf: Vec<u8> = Vec::new();
    write_output(&mut buf, &payload).expect("write failed");
    write_input(&mut buf, &payload).expect("write failed");
    write_control(&mut buf, &ServerToBrokerMsg::Terminate).expect("write failed");

    let mut cursor = Cursor::new(buf);
    let read = |cursor: &mut Cursor<Vec<u8>>| {
        read_frame(cursor)
            .expect("read failed")
            .expect("unexpected EOF")
    };
    assert_eq!(read(&mut cursor), Frame::Output(payload.clone()));
    assert_eq!(read(&mut cursor), Frame::Input(payload));
    assert!(matches!(read(&mut cursor), Frame::Control(_)));
    assert!(
        read_frame(&mut cursor).expect("read failed").is_none(),
        "expected a clean EOF after the last frame"
    );
}

/// Pty output is arbitrary bytes — half-written UTF-8, raw escape sequences, and
/// embedded NULs — so the framing must be byte-transparent for anything at all.
#[hegel::test]
fn any_byte_payload_survives_the_frame_round_trip(tc: hegel::TestCase) {
    let payload = tc.draw(generators::binary().max_size(4096));
    let mut buf: Vec<u8> = Vec::new();
    write_output(&mut buf, &payload).expect("write failed");

    let mut cursor = Cursor::new(buf);
    let mut got: Vec<u8> = Vec::new();
    while let Some(frame) = read_frame(&mut cursor).expect("read failed") {
        match frame {
            Frame::Output(bytes) => got.extend_from_slice(&bytes),
            other => panic!("expected an output frame, got {other:?}"),
        }
    }
    assert_eq!(got, payload, "payload did not survive framing");
}

/// A frame is a natural read rather than a coalesced batch, so anything larger
/// than one read's worth is split rather than refused.
#[test]
fn an_oversized_payload_is_split_rather_than_refused() {
    let payload = vec![b'x'; MAX_STREAM_PAYLOAD * 2 + 7];
    let mut buf: Vec<u8> = Vec::new();
    write_output(&mut buf, &payload).expect("write failed");

    let mut cursor = Cursor::new(buf);
    let mut frames = 0;
    let mut got: Vec<u8> = Vec::new();
    while let Some(frame) = read_frame(&mut cursor).expect("read failed") {
        match frame {
            Frame::Output(bytes) => {
                assert!(
                    bytes.len() <= MAX_STREAM_PAYLOAD,
                    "a frame carried {} bytes, over the {MAX_STREAM_PAYLOAD} cap",
                    bytes.len()
                );
                got.extend_from_slice(&bytes);
                frames += 1;
            },
            other => panic!("expected an output frame, got {other:?}"),
        }
    }
    assert_eq!(frames, 3);
    assert_eq!(got, payload);
}

/// The reader must not trust a length it has not read, or a hostile or corrupt
/// peer allocates whatever it likes on this side.
#[test]
fn a_frame_claiming_an_absurd_length_is_refused() {
    let mut buf = (MAX_FRAME_LEN + 1).to_le_bytes().to_vec();
    buf.push(KIND_OUTPUT);
    assert!(read_frame(&mut Cursor::new(buf)).is_err());
}

#[test]
fn a_frame_with_no_kind_byte_is_refused() {
    let buf = 0u32.to_le_bytes().to_vec();
    assert!(read_frame(&mut Cursor::new(buf)).is_err());
}

/// An unknown kind is a version skew or a corrupt stream. Either way it must be
/// an error rather than a frame silently dropped or guessed at.
#[test]
fn an_unknown_frame_kind_is_refused() {
    let mut buf = 2u32.to_le_bytes().to_vec();
    buf.push(0x7f);
    buf.push(b'!');
    assert!(read_frame(&mut Cursor::new(buf)).is_err());
}

/// A stream frame carries no JSON, so asking for a control body must fail rather
/// than parse whatever the pty happened to print.
#[test]
fn a_stream_frame_is_not_mistaken_for_a_control_body() {
    let frame = Frame::Output(b"{\"Terminate\":null}".to_vec());
    assert!(frame.decode_control::<ServerToBrokerMsg>().is_err());
}

#[test]
fn a_clean_eof_at_a_frame_boundary_is_not_an_error() {
    let buf: Vec<u8> = Vec::new();
    assert!(read_frame(&mut Cursor::new(buf))
        .expect("a clean EOF is not an error")
        .is_none());
}

/// Frames arrive over ssh, so a read returning less than a whole frame is the
/// normal case rather than an edge case.
#[test]
fn frames_read_back_the_same_through_a_one_byte_at_a_time_reader() {
    struct OneByteAtATime<R: std::io::Read>(R);
    impl<R: std::io::Read> std::io::Read for OneByteAtATime<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.0.read(&mut buf[..1])
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    write_control(&mut buf, &ServerToBrokerMsg::Terminate).expect("write failed");
    write_output(&mut buf, b"hello").expect("write failed");

    let mut reader = OneByteAtATime(Cursor::new(buf));
    let frame = read_frame(&mut reader)
        .expect("read failed")
        .expect("unexpected EOF");
    let got: ServerToBrokerMsg = frame.decode_control().expect("decode failed");
    assert_eq!(got, ServerToBrokerMsg::Terminate);
    assert_eq!(
        read_frame(&mut reader)
            .expect("read failed")
            .expect("unexpected EOF"),
        Frame::Output(b"hello".to_vec())
    );
}

#[test]
fn spawn_json_is_compatible_with_legacy_editor_fallback() {
    // The removed field was optional without an explicit serde default.
    #[derive(serde::Deserialize)]
    enum LegacyMessage {
        Spawn {
            cmd: RunCommand,
            failover_cmd: Option<RunCommand>,
        },
    }

    let message = ServerToBrokerMsg::Spawn {
        cmd: sample_run_command(),
        initial_size: None,
        env: vec![],
        terminal_id: 7,
    };
    let json = serde_json::to_value(&message).unwrap();
    assert!(json["Spawn"].get("failover_cmd").is_none());
    let LegacyMessage::Spawn { cmd, failover_cmd } = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(cmd, sample_run_command());
    assert!(failover_cmd.is_none());

    for fallback in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::to_value(sample_run_command()).unwrap()),
    ] {
        let mut incoming = json.clone();
        if let Some(fallback) = fallback {
            incoming["Spawn"]["failover_cmd"] = fallback;
        }
        let mut frame = Vec::new();
        write_control(&mut frame, &incoming).unwrap();
        let decoded: ServerToBrokerMsg = read_frame(&mut Cursor::new(frame))
            .unwrap()
            .unwrap()
            .decode_control()
            .unwrap();
        assert_eq!(decoded, message);
    }
}

/// The cross-platform guard, pinned rather than left to review.
///
/// `WireTermios` ships raw termios flag words and control characters by index,
/// and those constants differ between Linux and the BSDs. Sending one across a
/// machine boundary would not fail loudly — it would set a plausible-looking
/// garbage terminal mode — so a `Spawn` that grew a termios field back would be
/// a silent, and very confusing, bug.
#[test]
fn a_remote_spawn_carries_no_termios() {
    let spawn = ServerToBrokerMsg::Spawn {
        cmd: sample_run_command(),
        initial_size: None,
        env: vec![],
        terminal_id: 0,
    };
    let encoded = serde_json::to_string(&spawn).expect("encode failed");
    assert!(
        !encoded.to_lowercase().contains("termios"),
        "a remote Spawn must not carry platform-shaped termios data, got {encoded}"
    );
}

/// The two protocols version independently on purpose: a supervisor outlives the
/// server that spawned it, so bumping `PROTOCOL_VERSION` orphans running panes,
/// while a broker is reached by ssh-ing to a binary that either matches or does
/// not. Tying them together would make every remote-protocol tweak cost a pane.
#[test]
fn the_remote_protocol_versions_independently_of_the_supervisor_protocol() {
    // v2: a Spawn may carry no command, meaning "this machine's login shell".
    // v3: Release/Released, so a pane can be handed to another server rather
    // than only picked back up by the one that lost it, and ListPanes, so one
    // stranded by a failed handoff can be found again.
    // v4: `Spawn` carries a `PaneTarget` instead of a loose host field.
    // v5: `AgentReport`, a hook's event relayed on the pane's channel.
    // v6: `GetGitProbe` selects a directory independently of the process cwd.
    // v7: refresh tokens correlate requests with completed probes.
    // v8: OutputGap reports losses incurred during catch-up.
    assert_eq!(
        REMOTE_PROTOCOL_VERSION, 8,
        "bumping this is cheap; make sure it is deliberate"
    );
    assert_eq!(
        crate::pty_host::protocol::PROTOCOL_VERSION,
        2,
        "the supervisor protocol must not be dragged along by a remote change"
    );
}

/// What crosses is the terminal, not the filesystem. A `PATH` or a `HOME` from
/// the asking machine names directories that do not exist on the far one, so the
/// far side recomputes those from its own passwd entry instead.
#[test]
fn a_remote_spawn_carries_the_asking_sides_terminal_but_not_its_filesystem() {
    let spawn = ServerToBrokerMsg::Spawn {
        cmd: sample_run_command(),
        initial_size: None,
        env: crate::pty_host::remote::env::select_carried(
            [
                ("TERM", "xterm-256color"),
                ("PATH", "/home/me/.local/bin"),
                ("HOME", "/home/me"),
                ("SSH_AUTH_SOCK", "/tmp/agent"),
            ]
            .into_iter()
            .map(|(name, value)| (name.into(), value.into())),
        ),
        terminal_id: 3,
    };
    let encoded = serde_json::to_string(&spawn).expect("encode failed");

    assert!(encoded.contains("xterm-256color"));
    for absent in ["/home/me", "SSH_AUTH_SOCK"] {
        assert!(
            !encoded.contains(absent),
            "a remote Spawn must not carry {absent}, got {encoded}"
        );
    }
}

#[test]
fn legacy_spawn_frame_preserves_numeric_ids() {
    let payload = br#"{"Spawn":{"cmd":{"command":"/bin/sh","args":[],"cwd":null,"hold_on_close":false,"hold_on_start":false,"originating_plugin":{"plugin_id":4294967295,"client_id":65535,"context":{}},"use_terminal_title":false,"target":"Local","remote_default_shell":false},"failover_cmd":null,"initial_size":null,"env":[],"terminal_id":0}}"#;
    let mut frame = (payload.len() as u32 + 1).to_le_bytes().to_vec();
    frame.push(KIND_CONTROL);
    frame.extend_from_slice(payload);
    let Frame::Control(body) = read_frame(&mut Cursor::new(&frame)).unwrap().unwrap() else {
        panic!("expected a control frame");
    };
    let message: ServerToBrokerMsg = serde_json::from_slice(&body).unwrap();
    assert!(matches!(
        message,
        ServerToBrokerMsg::Spawn { terminal_id: 0, .. }
    ));
    let mut encoded = Vec::new();
    write_control(&mut encoded, &message).unwrap();
    let Frame::Control(body) = read_frame(&mut Cursor::new(encoded)).unwrap().unwrap() else {
        panic!("expected a control frame");
    };
    let mut expected: serde_json::Value = serde_json::from_slice(payload).unwrap();
    expected["Spawn"]
        .as_object_mut()
        .unwrap()
        .remove("failover_cmd");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        expected
    );
}
