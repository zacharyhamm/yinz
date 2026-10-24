use super::*;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;

use crate::pty_host::protocol;
use nix::pty::openpty;
use nix::sys::termios;
use std::io::Cursor;
use std::os::fd::AsFd;

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

#[test]
fn framed_messages_round_trip() {
    let msgs = vec![
        ServerToHostMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            yinz_version: yinz_runtime::consts::VERSION.to_owned(),
        },
        ServerToHostMsg::Spawn {
            job_control: true,
            terminal_id: 7,
            session_name: Some("warp-test".to_owned().into()),
            cmd: sample_run_command(),
            termios: None,
            initial_size: Some(WireWinsize {
                cols: 80,
                rows: 24,
                width_in_pixels: 640,
                height_in_pixels: 384,
            }),
            arg0: None,
            env: vec![],
            login_session: false,
        },
        ServerToHostMsg::AgentTheme(yinz_types::agent::theme::AgentThemeEvent {
            version: 1,
            launch_id: "ended".into(),
            snapshot: None,
        }),
        ServerToHostMsg::Terminate,
    ];

    let mut buf: Vec<u8> = Vec::new();
    for msg in &msgs {
        write_msg(&mut buf, msg).expect("write failed");
    }

    let mut cursor = Cursor::new(buf);
    for expected in &msgs {
        let got: ServerToHostMsg = read_msg(&mut cursor)
            .expect("read failed")
            .expect("unexpected EOF");
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            serde_json::to_string(expected).unwrap(),
        );
    }
    let trailing: Option<ServerToHostMsg> = read_msg(&mut cursor).expect("read failed");
    assert!(trailing.is_none(), "expected clean EOF after last message");
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

    let message = ServerToHostMsg::Spawn {
        terminal_id: 7,
        session_name: None,
        cmd: sample_run_command(),
        termios: None,
        initial_size: None,
        arg0: None,
        env: vec![],
        login_session: false,
        job_control: true,
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
        write_msg(&mut frame, &incoming).unwrap();
        let decoded: ServerToHostMsg = read_msg(&mut Cursor::new(frame)).unwrap().unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), json);
    }
}

/// A truncated stream must report EOF, not hang or mis-parse. This is the case
/// that matters in production: the peer process died mid-frame.
#[test]
fn eof_mid_frame_is_an_error_not_a_silent_none() {
    let mut buf: Vec<u8> = Vec::new();
    write_msg(&mut buf, &ServerToHostMsg::Terminate).expect("write failed");
    buf.truncate(buf.len() - 1);
    let mut cursor = Cursor::new(buf);
    let result: Result<Option<ServerToHostMsg>> = read_msg(&mut cursor);
    assert!(
        result.is_err(),
        "truncated frame should not read as a message"
    );
}

#[test]
fn eof_at_frame_boundary_reads_as_none() {
    let mut cursor = Cursor::new(Vec::new());
    let result: Option<ServerToHostMsg> = read_msg(&mut cursor).expect("read failed");
    assert!(result.is_none());
}

/// The supervisor rebuilds the user's terminal settings from this, so a lossy
/// round trip would silently change every pane's line discipline.
#[test]
fn wire_termios_round_trips_a_real_terminal() {
    let pty = openpty(None, &None).expect("openpty failed");
    let mut original = termios::tcgetattr(pty.slave.as_fd()).expect("tcgetattr failed");
    // Move it away from the defaults so the test would notice fields being
    // dropped on the floor.
    termios::cfmakeraw(&mut original);
    original.control_chars[termios::SpecialCharacterIndices::VMIN as usize] = 3;
    original.control_chars[termios::SpecialCharacterIndices::VTIME as usize] = 7;
    termios::tcsetattr(pty.slave.as_fd(), termios::SetArg::TCSANOW, &original)
        .expect("tcsetattr failed");
    let original = termios::tcgetattr(pty.slave.as_fd()).expect("tcgetattr failed");

    let wire = WireTermios::from_termios(&original);
    let json = serde_json::to_string(&wire).expect("serialize failed");
    let decoded: WireTermios = serde_json::from_str(&json).expect("deserialize failed");
    assert_eq!(wire, decoded);

    let rebuilt = decoded.to_termios();
    assert_eq!(original.input_flags, rebuilt.input_flags);
    assert_eq!(original.output_flags, rebuilt.output_flags);
    assert_eq!(original.control_flags, rebuilt.control_flags);
    assert_eq!(original.local_flags, rebuilt.local_flags);
    assert_eq!(original.control_chars, rebuilt.control_chars);
    assert_eq!(
        WireTermios::from_termios(&rebuilt),
        wire,
        "termios did not survive a second trip through the wire form"
    );
}

/// The rebuilt termios has to actually take effect when handed to `openpty` —
/// nix keeps the authoritative `libc::termios` privately and only syncs the
/// public fields into it on the way out, so this asserts we set the right ones.
#[test]
fn rebuilt_termios_is_applied_by_openpty() {
    let source = openpty(None, &None).expect("openpty failed");
    let mut desired = termios::tcgetattr(source.slave.as_fd()).expect("tcgetattr failed");
    termios::cfmakeraw(&mut desired);
    desired.control_chars[termios::SpecialCharacterIndices::VMIN as usize] = 5;

    let rebuilt = WireTermios::from_termios(&desired).to_termios();
    let derived = openpty(None, &rebuilt).expect("openpty with rebuilt termios failed");
    let applied = termios::tcgetattr(derived.slave.as_fd()).expect("tcgetattr failed");

    assert_eq!(desired.local_flags, applied.local_flags);
    assert_eq!(desired.input_flags, applied.input_flags);
    assert_eq!(
        desired.control_chars[termios::SpecialCharacterIndices::VMIN as usize],
        applied.control_chars[termios::SpecialCharacterIndices::VMIN as usize],
    );
}

/// A supervisor outlives the server that spawned it, so a newer yinz can find
/// itself talking to an older host. Additive fields must not break it.
#[test]
fn unknown_fields_are_tolerated() {
    let mut value = serde_json::to_value(&ServerToHostMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        yinz_version: "9.9.9".to_owned(),
    })
    .expect("serialize failed");
    value["Hello"]["some_field_from_the_future"] = serde_json::json!(true);

    let decoded: ServerToHostMsg =
        serde_json::from_value(value).expect("a future field should not break an older peer");
    match decoded {
        ServerToHostMsg::Hello { yinz_version, .. } => assert_eq!(yinz_version, "9.9.9"),
        other => panic!("decoded as the wrong variant: {other:?}"),
    }
}

#[test]
fn host_ids_are_short_and_distinct() {
    let a = new_host_id();
    let b = new_host_id();
    assert_eq!(a.as_str().len(), 12);
    assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn socket_paths_fit_in_sun_path() {
    let path = socket_path(&new_host_id());
    check_socket_path_length(&path, 0).expect("pty-host socket path does not fit in sockaddr_un");
}

// ---- property-based tests (hegel) ----

use hegel::generators;
use std::collections::HashSet;

/// The paths in a `RunCommand` travel as JSON, which cannot carry a non-UTF-8
/// `PathBuf` at all, so they are generated from `text()` rather than raw bytes —
/// a non-UTF-8 command would fail to serialize for reasons that have nothing to
/// do with the framing under test. `originating_plugin` stays `None`: nothing on
/// the pty-host path ever sets it.
#[hegel::composite]
fn run_commands(tc: &hegel::TestCase) -> RunCommand {
    let arg_count = tc.draw(generators::integers::<usize>().max_value(4));
    RunCommand {
        agent_theme: None,
        command: PathBuf::from(tc.draw(generators::text())),
        args: (0..arg_count)
            .map(|_| tc.draw(generators::text()))
            .collect(),
        cwd: tc
            .draw(generators::optional(generators::text()))
            .map(PathBuf::from),
        hold_on_close: tc.draw(generators::booleans()),
        hold_on_start: tc.draw(generators::booleans()),
        originating_plugin: None,
        use_terminal_title: tc.draw(generators::booleans()),
        target: Default::default(),
        remote_default_shell: false,
    }
}

/// Flag words and speeds are drawn unbounded because `to_termios` casts them
/// straight into `tcflag_t`/`speed_t` and must not panic on any of them, and
/// `control_chars` is allowed to be both shorter and longer than a real `c_cc`
/// because the truncation at the `min(raw.c_cc.len())` in `to_termios` is exactly
/// the behaviour these tests exist to pin down.
#[hegel::composite]
fn wire_termioses(tc: &hegel::TestCase) -> WireTermios {
    WireTermios {
        iflag: tc.draw(generators::integers::<u64>()),
        oflag: tc.draw(generators::integers::<u64>()),
        cflag: tc.draw(generators::integers::<u64>()),
        lflag: tc.draw(generators::integers::<u64>()),
        control_chars: tc.draw(generators::binary().max_size(512)),
        ispeed: tc.draw(generators::integers::<u64>()),
        ospeed: tc.draw(generators::integers::<u64>()),
        line_discipline: tc.draw(generators::integers::<u8>()),
    }
}

#[hegel::composite]
fn wire_winsizes(tc: &hegel::TestCase) -> WireWinsize {
    WireWinsize {
        cols: tc.draw(generators::integers::<u16>()),
        rows: tc.draw(generators::integers::<u16>()),
        width_in_pixels: tc.draw(generators::integers::<u16>()),
        height_in_pixels: tc.draw(generators::integers::<u16>()),
    }
}

#[hegel::composite]
fn host_statuses(tc: &hegel::TestCase) -> HostStatus {
    HostStatus {
        supports_relabel: true,
        child_group_terminated: false,
        host_id: SupervisorId::new(tc.draw(generators::text())),
        session_name: tc
            .draw(generators::optional(generators::text()))
            .map(Into::into),
        terminal_id: tc.draw(generators::integers::<u32>()),
        command: tc.draw(generators::text()),
        cwd: tc
            .draw(generators::optional(generators::text()))
            .map(PathBuf::from),
        child_pid: tc
            .draw(generators::optional(generators::integers::<u32>()))
            .map(ProcessId::new),
        host_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
        started_at_unix: yinz_types::units::UnixSeconds::new(
            tc.draw(generators::integers::<u64>()),
        ),
        attached: tc.draw(generators::booleans()),
        child_exit_status: tc.draw(generators::optional(generators::optional(
            generators::integers::<i32>(),
        ))),
        buffered_bytes: tc.draw(generators::integers::<usize>()),
    }
}

/// `protocol_version` is drawn unbounded rather than pinned to `PROTOCOL_VERSION`:
/// a mismatched peer is the case the version field exists for, and its `Hello`
/// still has to survive the framing in order to be *recognised* as mismatched.
#[hegel::composite]
fn server_to_host_msgs(tc: &hegel::TestCase) -> ServerToHostMsg {
    match tc.draw(generators::integers::<u8>().max_value(5)) {
        0 => ServerToHostMsg::Hello {
            protocol_version: tc.draw(generators::integers::<u32>()),
            yinz_version: tc.draw(generators::text()),
        },
        1 => ServerToHostMsg::Spawn {
            job_control: tc.draw(generators::booleans()),
            terminal_id: tc.draw(generators::integers::<u32>()),
            session_name: tc
                .draw(generators::optional(generators::text()))
                .map(Into::into),
            cmd: tc.draw(run_commands()),
            termios: tc.draw(generators::optional(wire_termioses())),
            initial_size: tc.draw(generators::optional(wire_winsizes())),
            arg0: tc.draw(generators::optional(generators::text())),
            env: vec![],
            login_session: false,
        },
        2 => ServerToHostMsg::Terminate,
        3 => ServerToHostMsg::Release,
        4 => ServerToHostMsg::Attach {
            protocol_version: tc.draw(generators::integers::<u32>()),
            yinz_version: tc.draw(generators::text()),
            terminal_id: tc.draw(generators::integers::<u32>()),
            session_name: tc
                .draw(generators::optional(generators::text()))
                .map(Into::into),
        },
        _ => ServerToHostMsg::Query,
    }
}

#[hegel::composite]
fn host_to_server_msgs(tc: &hegel::TestCase) -> HostToServerMsg {
    match tc.draw(generators::integers::<u8>().max_value(8)) {
        7 => HostToServerMsg::ChildStopped {
            signal: tc.draw(generators::integers::<i32>()),
        },
        8 => HostToServerMsg::ChildContinued,
        5 => HostToServerMsg::AttachAck {
            protocol_version: tc.draw(generators::integers::<u32>()),
            yinz_version: tc.draw(generators::text()),
            host_id: SupervisorId::new(tc.draw(generators::text())),
            host_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
            child_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
            cmd: tc.draw(run_commands()),
            status: tc.draw(host_statuses()),
            buffered_base64: protocol::encode_buffered(&tc.draw(generators::binary().max_size(64))),
            dropped_bytes: tc.draw(generators::integers::<u64>()),
        },
        6 => HostToServerMsg::AttachRefused {
            reason: match tc.draw(generators::integers::<u8>().max_value(2)) {
                0 => AttachRefusal::AlreadyAttached,
                1 => AttachRefusal::ProtocolMismatch,
                _ => AttachRefusal::NoSession,
            },
            message: tc.draw(generators::text()),
        },
        0 => HostToServerMsg::HelloAck {
            protocol_version: tc.draw(generators::integers::<u32>()),
            yinz_version: tc.draw(generators::text()),
            host_id: SupervisorId::new(tc.draw(generators::text())),
            host_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
            control_socket: tc
                .draw(generators::optional(generators::text()))
                .map(PathBuf::from),
        },
        1 => HostToServerMsg::Spawned {
            child_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
        },
        2 => HostToServerMsg::SpawnFailed {
            kind: if tc.draw(generators::booleans()) {
                SpawnFailureKind::CommandNotFound
            } else {
                SpawnFailureKind::Other
            },
            message: tc.draw(generators::text()),
        },
        3 => HostToServerMsg::ChildExited {
            status: tc.draw(generators::optional(generators::integers::<i32>())),
        },
        _ => HostToServerMsg::Status(tc.draw(host_statuses())),
    }
}

/// Neither message enum derives `PartialEq`, so round trips are compared on their
/// serialized form — which is the representation that actually crosses the wire.
fn as_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize failed")
}

#[hegel::test]
fn any_server_to_host_message_survives_the_frame_round_trip(tc: hegel::TestCase) {
    let msg = tc.draw(server_to_host_msgs());
    let mut buf: Vec<u8> = Vec::new();
    write_msg(&mut buf, &msg).expect("write failed");

    let mut cursor = Cursor::new(buf);
    let got: ServerToHostMsg = read_msg(&mut cursor)
        .expect("read failed")
        .expect("unexpected EOF");
    assert_eq!(
        as_json(&got),
        as_json(&msg),
        "{msg:?} did not survive the frame round trip"
    );
}

#[hegel::test]
fn any_host_to_server_message_survives_the_frame_round_trip(tc: hegel::TestCase) {
    let msg = tc.draw(host_to_server_msgs());
    let mut buf: Vec<u8> = Vec::new();
    write_msg(&mut buf, &msg).expect("write failed");

    let mut cursor = Cursor::new(buf);
    let got: HostToServerMsg = read_msg(&mut cursor)
        .expect("read failed")
        .expect("unexpected EOF");
    assert_eq!(
        as_json(&got),
        as_json(&msg),
        "{msg:?} did not survive the frame round trip"
    );
}

/// The control channel is a stream, so several frames routinely arrive in one
/// read. They must come back out one at a time, in order, and the stream must
/// then report a clean end rather than a spurious extra message.
#[hegel::test]
fn a_batch_of_frames_reads_back_in_order_then_none(tc: hegel::TestCase) {
    let count = tc.draw(generators::integers::<usize>().max_value(8));
    let msgs: Vec<ServerToHostMsg> = (0..count).map(|_| tc.draw(server_to_host_msgs())).collect();

    let mut buf: Vec<u8> = Vec::new();
    for msg in &msgs {
        write_msg(&mut buf, msg).expect("write failed");
    }

    let mut cursor = Cursor::new(buf);
    for (i, expected) in msgs.iter().enumerate() {
        let got: ServerToHostMsg = read_msg(&mut cursor)
            .expect("read failed")
            .unwrap_or_else(|| panic!("stream ended early at message {i} of {count}"));
        assert_eq!(as_json(&got), as_json(expected), "message {i} of {count}");
    }
    let trailing: Option<ServerToHostMsg> = read_msg(&mut cursor).expect("read failed");
    assert!(
        trailing.is_none(),
        "expected a clean end after {count} messages"
    );
}

/// Hands `read_msg` one byte at a time. A socket is free to return fewer bytes
/// than were asked for, so both `read_exact` calls inside `read_msg` have to cope
/// with short reads — a path nothing else in this suite exercises.
struct ChokedReader<R> {
    inner: R,
}

impl<R: Read> Read for ChokedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.inner.read(&mut buf[..1])
    }
}

#[hegel::test]
fn frames_read_back_the_same_through_a_one_byte_at_a_time_reader(tc: hegel::TestCase) {
    let msgs: Vec<ServerToHostMsg> = {
        let count = tc.draw(generators::integers::<usize>().max_value(4));
        (0..count).map(|_| tc.draw(server_to_host_msgs())).collect()
    };

    let mut buf: Vec<u8> = Vec::new();
    for msg in &msgs {
        write_msg(&mut buf, msg).expect("write failed");
    }

    let mut reader = ChokedReader {
        inner: Cursor::new(buf),
    };
    for (i, expected) in msgs.iter().enumerate() {
        let got: ServerToHostMsg = read_msg(&mut reader)
            .expect("read failed")
            .unwrap_or_else(|| panic!("stream ended early at message {i}"));
        assert_eq!(as_json(&got), as_json(expected), "message {i}");
    }
}

/// Pins the exact truncation contract. A frame cut off before its 4-byte length
/// prefix is complete is reported as a clean disconnect, not an error, because
/// `read_exact` raises `UnexpectedEof` for a partial read just as it does for an
/// empty one. That is acceptable here — `serve()`'s control handler treats a clean
/// disconnect and a read error identically, both meaning "the server detached" —
/// but it should not drift without someone noticing.
#[hegel::test]
fn truncating_a_frame_errors_once_the_length_prefix_is_complete(tc: hegel::TestCase) {
    let msg = tc.draw(server_to_host_msgs());
    let mut buf: Vec<u8> = Vec::new();
    write_msg(&mut buf, &msg).expect("write failed");

    let cut = tc.draw(generators::integers::<usize>().max_value(buf.len() - 1));
    buf.truncate(cut);
    let result: Result<Option<ServerToHostMsg>> = read_msg(&mut Cursor::new(buf));

    if cut < 4 {
        assert!(
            matches!(result, Ok(None)),
            "a frame cut at {cut} bytes is indistinguishable from a clean hangup, \
             but read_msg returned {result:?}"
        );
    } else {
        assert!(
            result.is_err(),
            "a frame cut at {cut} bytes claims a length it cannot deliver \
             and must be an error, got {result:?}"
        );
    }
}

/// Garbage in the body must surface as an error, never a panic and never a
/// silent "the peer hung up" — the length prefix already promised a body.
#[hegel::test]
fn arbitrary_bytes_after_a_valid_length_prefix_error_rather_than_panic(tc: hegel::TestCase) {
    let body = tc.draw(generators::binary().max_size(4096));
    let mut buf = (body.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(&body);

    let result: Result<Option<ServerToHostMsg>> = read_msg(&mut Cursor::new(buf));
    assert!(
        !matches!(result, Ok(None)),
        "a complete frame must never read as a clean disconnect, body was {body:?}"
    );
}

#[test]
fn a_length_prefix_over_the_frame_limit_is_refused() {
    for claimed in [MAX_FRAME_LEN + 1, u32::MAX] {
        let buf = claimed.to_le_bytes().to_vec();
        let result: Result<Option<ServerToHostMsg>> = read_msg(&mut Cursor::new(buf));
        assert!(
            result.is_err(),
            "a frame claiming {claimed} bytes should be refused rather than allocated"
        );
    }
}

/// The other half of the frame-limit guard. An oversized message must fail before
/// anything reaches the wire — a partial write would desynchronise the channel
/// for every frame after it.
#[test]
fn write_msg_refuses_a_body_over_the_frame_limit() {
    let mut cmd = sample_run_command();
    cmd.args = vec!["x".repeat(MAX_FRAME_LEN as usize + 1)];
    let msg = ServerToHostMsg::Spawn {
        job_control: true,
        terminal_id: 0,
        session_name: None,
        cmd,
        termios: None,
        initial_size: None,
        arg0: None,
        env: vec![],
        login_session: false,
    };

    let mut buf: Vec<u8> = Vec::new();
    let err = write_msg(&mut buf, &msg).expect_err("an oversized message should not be framed");
    assert!(
        buf.is_empty(),
        "an oversized message put {} bytes on the wire",
        buf.len()
    );
    assert!(
        format!("{err:#}").contains("too large"),
        "unexpected error: {err:#}"
    );
}

#[hegel::test]
fn wire_termios_survives_a_json_round_trip(tc: hegel::TestCase) {
    let wire = tc.draw(wire_termioses());
    let json = serde_json::to_string(&wire).expect("serialize failed");
    let decoded: WireTermios = serde_json::from_str(&json).expect("deserialize failed");
    assert_eq!(wire, decoded);
}

/// `from_termios(to_termios(w)) == w` is **not** true for an arbitrary `w`:
/// `to_termios` truncates `control_chars` to what a `c_cc` can hold and ignores
/// speeds that are not valid `speed_t` constants. What must hold is that the
/// conversion settles after a single trip — otherwise a supervisor's idea of the
/// terminal would drift a little further every time the settings were re-sent.
#[hegel::test]
fn normalizing_a_wire_termios_through_termios_is_idempotent(tc: hegel::TestCase) {
    let wire = tc.draw(wire_termioses());
    let once = WireTermios::from_termios(&wire.to_termios());
    let twice = WireTermios::from_termios(&once.to_termios());
    assert_eq!(
        once, twice,
        "normalizing {wire:?} did not settle after one trip"
    );
}

/// A peer on a platform with a different `NCCS` can legitimately send more (or
/// fewer) control characters than we can hold. The overflow must be dropped
/// rather than panicking, and everything that does fit must be preserved.
#[hegel::test]
fn control_chars_are_truncated_to_what_a_terminal_can_hold(tc: hegel::TestCase) {
    let wire = tc.draw(wire_termioses());
    let rebuilt = WireTermios::from_termios(&wire.to_termios());

    assert_eq!(
        rebuilt.control_chars.len(),
        libc::NCCS,
        "a rebuilt termios should always carry exactly one full c_cc"
    );
    let shared = wire.control_chars.len().min(rebuilt.control_chars.len());
    assert_eq!(
        &wire.control_chars[..shared],
        &rebuilt.control_chars[..shared],
        "the control characters that fit were not preserved (sent {} of {})",
        shared,
        wire.control_chars.len()
    );
}

/// The check is `>=`, not `>`, because `sun_path` has to fit a trailing NUL. A
/// path of exactly the limit is one byte too long, and getting this boundary
/// wrong surfaces as an opaque `EINVAL` from `bind` rather than our own message.
#[test]
fn a_socket_path_at_the_length_limit_is_refused_and_one_under_it_is_accepted() {
    use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;

    let just_fits = PathBuf::from("a".repeat(YINZ_SOCK_MAX_LENGTH - 1));
    check_socket_path_length(&just_fits, 0)
        .expect("a path one byte under the limit should be accepted");

    let one_too_many = PathBuf::from("a".repeat(YINZ_SOCK_MAX_LENGTH));
    check_socket_path_length(&one_too_many, 0)
        .expect_err("a path exactly at the limit leaves no room for the trailing NUL");
}

/// A `ControlPath` ssh accepts outright can still fail deep inside ssh, because
/// ssh binds its `ControlMaster` at a longer temp name first (see
/// `SSH_CONTROL_PATH_RENAME_MARGIN`). The margin has to catch that before ssh
/// does.
#[test]
fn a_control_path_that_only_fits_without_the_ssh_rename_margin_is_refused() {
    use crate::pty_host::protocol::SSH_CONTROL_PATH_RENAME_MARGIN;
    use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;

    let control_path =
        PathBuf::from("a".repeat(YINZ_SOCK_MAX_LENGTH - SSH_CONTROL_PATH_RENAME_MARGIN));
    check_socket_path_length(&control_path, 0)
        .expect("fits on its own, without reserving ssh's rename margin");
    check_socket_path_length(&control_path, SSH_CONTROL_PATH_RENAME_MARGIN)
        .expect_err("does not leave room for ssh's ControlMaster temp-rename suffix");
}

/// Two samples cannot tell a working id generator from one that repeats every
/// few hundred draws, and a collision would mean two supervisors sharing a socket
/// path.
#[test]
fn host_ids_are_unique_across_many_draws() {
    let ids: HashSet<SupervisorId> = (0..1000).map(|_| new_host_id()).collect();
    assert_eq!(ids.len(), 1000, "new_host_id() produced a collision");
    assert!(
        ids.iter()
            .all(|id| id.as_str().len() == 12 && id.as_str().chars().all(|c| c.is_ascii_hexdigit())),
        "every host id should be 12 hex characters"
    );
}

// ---- the detached ring's wire encoding ----

/// Pty output is arbitrary bytes — half-written UTF-8, raw escape sequences — so
/// whatever carries it across the attach has to be exact, not merely lossy-safe.
#[hegel::test]
fn buffered_output_survives_its_wire_encoding(tc: hegel::TestCase) {
    let bytes = tc.draw(generators::binary().max_size(4096));
    let encoded = protocol::encode_buffered(&bytes);
    let decoded = protocol::decode_buffered(&encoded).expect("decode failed");
    assert_eq!(
        decoded,
        bytes,
        "{} bytes did not survive base64 round trip",
        bytes.len()
    );
}

/// The ring holds up to `RING_CAPACITY`, and all of it rides in a single frame on
/// attach. Base64 is 4 bytes per 3, so a full ring has to stay inside
/// `MAX_FRAME_LEN` — a JSON array of decimals would not have.
#[test]
fn a_full_ring_fits_in_one_frame() {
    // 256 KiB, matching `host::RING_CAPACITY`. Not imported because it is private
    // to the supervisor; if it grows, this test is where the consequence lands.
    let full_ring = vec![0xffu8; 256 * 1024];
    let msg = HostToServerMsg::AttachAck {
        protocol_version: PROTOCOL_VERSION,
        yinz_version: "0.0.0-a-generously-long-version-string".to_owned(),
        host_id: SupervisorId::new("a3f1c2d4e5b6".to_owned()),
        host_pid: ProcessId::new(u32::MAX),
        child_pid: ProcessId::new(u32::MAX),
        cmd: RunCommand {
            agent_theme: None,
            command: PathBuf::from("/usr/bin/some-fairly-long-command-name"),
            args: vec!["--with".to_owned(), "--several".to_owned()],
            cwd: Some(PathBuf::from("/home/someone/a/reasonably/deep/directory")),
            hold_on_close: true,
            hold_on_start: false,
            originating_plugin: None,
            use_terminal_title: false,
            target: Default::default(),
            remote_default_shell: false,
        },
        status: HostStatus {
            supports_relabel: true,
            child_group_terminated: false,
            host_id: SupervisorId::new("a3f1c2d4e5b6".to_owned()),
            session_name: Some("a-session-with-a-name".to_owned().into()),
            terminal_id: u32::MAX,
            command: "some-fairly-long-command-name --with --several".to_owned(),
            cwd: Some(PathBuf::from("/home/someone/a/reasonably/deep/directory")),
            child_pid: Some(ProcessId::new(u32::MAX)),
            host_pid: ProcessId::new(u32::MAX),
            started_at_unix: yinz_types::units::UnixSeconds::new(u64::MAX),
            attached: true,
            child_exit_status: None,
            buffered_bytes: 256 * 1024,
        },
        buffered_base64: protocol::encode_buffered(&full_ring),
        dropped_bytes: u64::MAX,
    };

    let mut buf: Vec<u8> = Vec::new();
    write_msg(&mut buf, &msg).expect("a full ring should fit in one frame");
    // `write_msg` enforces the limit itself, so reaching here is the assertion —
    // but say the margin out loud, because it is the thing that could erode.
    assert!(
        buf.len() < 4 * 1024 * 1024,
        "an AttachAck carrying a full ring came to {} bytes, against a 4 MiB frame limit",
        buf.len()
    );
}

#[test]
fn legacy_attach_frame_preserves_numeric_terminal_id() {
    let payload = br#"{"Attach":{"protocol_version":1,"yinz_version":"legacy","terminal_id":4294967295,"session_name":null}}"#;
    let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(payload);
    let message: ServerToHostMsg = read_msg(&mut Cursor::new(&frame)).unwrap().unwrap();
    assert!(matches!(
        message,
        ServerToHostMsg::Attach {
            terminal_id: u32::MAX,
            ..
        }
    ));
    let mut encoded = Vec::new();
    write_msg(&mut encoded, &message).unwrap();
    assert_eq!(encoded, frame);
}
