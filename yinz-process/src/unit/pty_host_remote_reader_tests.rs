use super::*;
use yinz_test_support::Watched;

use std::io::{Read as _, Write};
use std::os::unix::net::UnixStream;

use crate::pty_host::protocol::WireWinsize;
use crate::pty_host::remote::protocol::{write_control, write_output, ServerToBrokerMsg};

fn reader_pair() -> (RemoteReader, Arc<RemoteConn>, UnixStream) {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let for_conn = ours.try_clone().expect("clone failed");
    let conn = Arc::new(
        RemoteConn::new(
            for_conn,
            "build-box".to_owned(),
            "tok".to_owned(),
            RunCommand::default(),
            None,
        )
        .expect("conn failed"),
    );
    let reader = RemoteReader::new(conn.clone(), ours).expect("reader failed");
    (reader, conn, theirs)
}

fn send(peer: &mut UnixStream, bytes: &[u8]) {
    peer.write_all(bytes).expect("peer write failed");
    peer.flush().expect("peer flush failed");
}

async fn read_to_string(reader: &mut RemoteReader, want: usize) -> String {
    let mut got = Vec::new();
    while got.len() < want {
        let mut buf = [0u8; 1024];
        let n = reader.read(&mut buf).await.expect("read failed");
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test]
async fn output_frames_arrive_as_plain_pty_bytes() {
    let (mut reader, _conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_output(&mut framed, b"hello ").expect("frame failed");
    write_output(&mut framed, b"world").expect("frame failed");
    send(&mut peer, &framed);

    assert_eq!(read_to_string(&mut reader, 11).await, "hello world");
}

#[tokio::test]
async fn accepted_input_and_control_drain_without_another_write() {
    let (mut reader, conn, mut peer) = reader_pair();
    let read = tokio::spawn(async move { reader.read(&mut [0; 16]).await });
    // Let the reader start waiting for output before input makes the socket full.
    tokio::task::yield_now().await;
    let payload = vec![b'x'; 64 * 1024];
    let mut accepted = 0;
    loop {
        let count = conn.write_input(&payload).unwrap();
        accepted += count;
        if count == 0 {
            break;
        }
    }
    assert!(conn.has_pending_writes());
    let size = WireWinsize {
        cols: 120,
        rows: 40,
        width_in_pixels: 0,
        height_in_pixels: 0,
    };
    conn.resize(size).unwrap();
    let peer = tokio::task::spawn_blocking(move || {
        peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut delivered = 0;
        let mut resized = false;
        while delivered < accepted || !resized {
            match protocol::read_frame(&mut peer).unwrap().unwrap() {
                Frame::Input(bytes) => {
                    assert!(bytes.iter().all(|byte| *byte == b'x'));
                    delivered += bytes.len();
                },
                frame @ Frame::Control(_) => {
                    assert_eq!(
                        frame.decode_control::<ServerToBrokerMsg>().unwrap(),
                        ServerToBrokerMsg::Resize(size)
                    );
                    resized = true;
                },
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        assert_eq!(delivered, accepted);
        write_control(
            &mut peer,
            &BrokerToServerMsg::ChildExited { status: Some(0) },
        )
        .unwrap();
    });
    peer.await.unwrap();
    assert_eq!(read.await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn gaps_between_replay_chunks_advance_offsets_without_counting_notices() {
    let (mut reader, conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_output(&mut framed, b"before").unwrap();
    for (dropped, output) in [(4096, "after"), (10, "tail"), (0, "end")] {
        write_control(&mut framed, &BrokerToServerMsg::OutputGap { dropped }).unwrap();
        write_output(&mut framed, output.as_bytes()).unwrap();
    }
    write_control(
        &mut framed,
        &BrokerToServerMsg::ChildExited { status: Some(0) },
    )
    .unwrap();
    send(&mut peer, &framed);

    assert_eq!(read_to_string(&mut reader, 6).await, "before");
    let mut received = 6;
    assert_eq!(conn.received(), received);
    for (dropped, output) in [(4096, "after"), (10, "tail"), (0, "end")] {
        received += dropped;
        if dropped > 0 {
            let notice = format!(
                "\x1b[2J\x1b[H\x1b[33m── reconnected; {dropped} bytes of output were lost ──\x1b[0m\r\n"
            );
            for expected in notice.bytes() {
                let mut byte = [0];
                assert_eq!(reader.read(&mut byte).await.unwrap(), 1);
                assert_eq!(byte[0], expected);
                assert_eq!(conn.received(), received, "a notice is not remote output");
            }
        }
        for expected in output.bytes() {
            let mut byte = [0];
            assert_eq!(reader.read(&mut byte).await.unwrap(), 1);
            assert_eq!(byte[0], expected);
            received += 1;
            assert_eq!(conn.received(), received);
        }
    }
    assert_eq!(reader.read(&mut [0]).await.unwrap(), 0);
}

/// Frames are split across reads routinely over a network, so the decoder must
/// hold a partial frame rather than mistaking it for a corrupt one.
#[tokio::test]
async fn a_frame_split_across_reads_is_reassembled() {
    let (mut reader, _conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_output(&mut framed, b"split payload").expect("frame failed");

    let (head, tail) = framed.split_at(5);
    send(&mut peer, head);
    let head = head.to_vec();
    let tail = tail.to_vec();
    let handle = tokio::task::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = head;
        send(&mut peer, &tail);
        peer
    });

    assert_eq!(read_to_string(&mut reader, 13).await, "split payload");
    handle.await.expect("peer task failed");
}

/// The rule this whole type exists to enforce. `TerminalBytes` reads `Ok(0)` as
/// EOF and tears the pane down, killing the child — so a transport failure must
/// surface as an error instead, leaving the still-running remote process alone.
#[tokio::test]
async fn a_transport_loss_is_not_reported_as_a_child_exit() {
    let (mut reader, conn, peer) = reader_pair();
    drop(peer);

    let mut buf = [0u8; 64];
    let err = reader
        .read(&mut buf)
        .await
        .expect_err("a transport loss must not look like EOF");
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset);
    assert!(conn.transport_lost());
    assert_eq!(
        conn.child_exit(),
        None,
        "no exit status should be invented for a dropped connection"
    );
}

/// The other half of the same rule: a real exit *is* EOF, so the pane closes
/// normally and `quit_cb` fires with the true status.
#[tokio::test]
async fn a_reported_child_exit_ends_the_stream_cleanly() {
    let (mut reader, conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_output(&mut framed, b"bye").expect("frame failed");
    write_control(
        &mut framed,
        &BrokerToServerMsg::ChildExited { status: Some(130) },
    )
    .expect("frame failed");
    send(&mut peer, &framed);
    drop(peer);

    // The child's last output is delivered before the EOF, not discarded with it.
    assert_eq!(read_to_string(&mut reader, 3).await, "bye");
    let mut buf = [0u8; 64];
    assert_eq!(reader.read(&mut buf).await.expect("read failed"), 0);
    assert_eq!(conn.child_exit(), Some(Some(130)));
}

/// A recorder for what the pane's end was reported as, and how often.
fn exit_recorder() -> (Arc<Mutex<Vec<Option<i32>>>>, QuitCb) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let cb: QuitCb = Box::new(move |_pane_id, status, _cmd| {
        sink.lock().expect("poisoned").push(status);
    });
    (seen, cb)
}

/// `TerminalBytes::listen` only breaks its loop at `Ok(0)`; the instruction that
/// actually retires the pane comes from this callback and nowhere else. A remote
/// pane that never fires it sits on screen, dead, until the user closes it.
#[tokio::test]
async fn a_child_exit_is_reported_to_the_pty_thread() {
    let (reader, _conn, mut peer) = reader_pair();
    let (seen, cb) = exit_recorder();
    let mut reader = reader.on_exit(TerminalId::new(7), RunCommand::default(), cb);

    let mut framed = Vec::new();
    write_control(
        &mut framed,
        &BrokerToServerMsg::ChildExited { status: Some(3) },
    )
    .expect("frame failed");
    send(&mut peer, &framed);
    drop(peer);

    let mut buf = [0u8; 64];
    assert_eq!(reader.read(&mut buf).await.expect("read failed"), 0);
    assert_eq!(
        *seen.lock().expect("poisoned"),
        vec![Some(3)],
        "the child's exit never reached the pty thread, so nothing will close the pane"
    );
}

/// Giving up on a reconnect ends the pane just as surely as the child exiting,
/// and has to be reported the same way — with no status, exactly as a dead
/// supervisor is reported on the local path.
#[tokio::test]
async fn giving_up_on_a_reconnect_is_reported_as_the_end_of_the_pane() {
    let (reader, _conn, peer) = reader_pair();
    let (seen, cb) = exit_recorder();
    // No reconnector, so the first transport loss is terminal.
    let mut reader = reader.on_exit(TerminalId::new(7), RunCommand::default(), cb);
    drop(peer);

    let mut buf = [0u8; 64];
    reader
        .read(&mut buf)
        .await
        .expect_err("an unreconnectable transport loss must surface as an error");
    assert_eq!(
        *seen.lock().expect("poisoned"),
        vec![None],
        "a pane that can never come back was left on screen with nothing to close it"
    );
}

/// Two exits would close a pane twice, and the second may land on whatever pane
/// has since taken the id.
#[tokio::test]
async fn the_end_of_a_pane_is_reported_only_once() {
    let (reader, _conn, mut peer) = reader_pair();
    let (seen, cb) = exit_recorder();
    let mut reader = reader.on_exit(TerminalId::new(7), RunCommand::default(), cb);

    let mut framed = Vec::new();
    write_control(
        &mut framed,
        &BrokerToServerMsg::ChildExited { status: None },
    )
    .expect("frame failed");
    send(&mut peer, &framed);
    drop(peer);

    let mut buf = [0u8; 64];
    for _ in 0..3 {
        assert_eq!(reader.read(&mut buf).await.expect("read failed"), 0);
    }
    assert_eq!(
        seen.lock().expect("poisoned").len(),
        1,
        "the pane's end was reported more than once"
    );
}

/// Status is cached from the broker's own machine, because the local server
/// cannot inspect a process it does not have.
#[tokio::test]
async fn a_status_report_updates_the_cache_without_disturbing_the_stream() {
    let (mut reader, conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_control(
        &mut framed,
        &BrokerToServerMsg::StatusReport {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned()]),
        },
    )
    .expect("frame failed");
    write_output(&mut framed, b"prompt$ ").expect("frame failed");
    send(&mut peer, &framed);

    assert_eq!(read_to_string(&mut reader, 8).await, "prompt$ ");
    let status = conn.status();
    assert_eq!(status.cwd.as_deref(), Some("/home/me/work"));
    assert_eq!(status.foreground_argv, Some(vec!["claude".to_owned()]));
}

/// An agent's hook event is an event, not state: it goes to the callback at
/// once, without touching the byte stream the pane renders.
#[tokio::test]
async fn an_agent_report_control_frame_reaches_the_callback() {
    let (reader, _conn, mut peer) = reader_pair();
    let seen: Arc<Mutex<Vec<(TerminalId, String, String)>>> = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let mut reader = reader
        .on_exit(
            TerminalId::new(7),
            RunCommand::default(),
            Box::new(|_, _, _| {}),
        )
        .on_agent_report(Arc::new(move |terminal_id, kind, json| {
            sink.lock()
                .expect("poisoned")
                .push((terminal_id, kind, json));
        }));
    let mut framed = Vec::new();
    write_control(
        &mut framed,
        &BrokerToServerMsg::AgentReport {
            kind: "claude".to_owned(),
            json: r#"{"hook_event_name":"Stop"}"#.to_owned(),
        },
    )
    .expect("frame failed");
    write_output(&mut framed, b"prompt$ ").expect("frame failed");
    send(&mut peer, &framed);

    assert_eq!(read_to_string(&mut reader, 8).await, "prompt$ ");
    assert_eq!(
        seen.lock()
            .expect("poisoned")
            .iter()
            .map(|(id, kind, json)| {
                (
                    *id,
                    kind.clone(),
                    serde_json::from_str::<serde_json::Value>(json).expect("valid report JSON"),
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            TerminalId::new(7),
            "claude".to_owned(),
            serde_json::json!({"hook_event_name": "Stop"})
        )]
    );
}

/// A caller's buffer is whatever size it is; a frame larger than it must be
/// handed over across several reads rather than truncated.
#[tokio::test]
async fn output_larger_than_the_callers_buffer_is_not_truncated() {
    let (mut reader, _conn, mut peer) = reader_pair();
    let payload = vec![b'z'; 4096];
    let mut framed = Vec::new();
    write_output(&mut framed, &payload).expect("frame failed");
    send(&mut peer, &framed);

    let mut got = Vec::new();
    while got.len() < payload.len() {
        let mut buf = [0u8; 100];
        let n = reader.read(&mut buf).await.expect("read failed");
        assert!(n > 0, "unexpected EOF part way through a payload");
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, payload);
}

// ---- reconnect ----
//
// The point of all of this: a dropped link must not end a pane whose process is
// still running on the other machine.

use crate::pty_host::remote::conn::{Reconnected, Reconnector};
use std::sync::mpsc;
use std::sync::Mutex;

/// Hands back a fresh socketpair each time, and tells the test about it so it
/// can play the broker on the other end.
struct TestReconnector {
    peers: mpsc::Sender<UnixStream>,
    dropped: u64,
    /// The `received` value each reconnect was asked to resume from — the number
    /// a real broker would replay from.
    seen: Mutex<Vec<u64>>,
    fail_first: Mutex<u32>,
}

impl Reconnector for TestReconnector {
    fn reconnect(
        &self,
        _host: &str,
        _pane_token: &str,
        received: u64,
    ) -> yinz_runtime::errors::prelude::Result<Reconnected> {
        {
            let mut fails = self.fail_first.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return Err(yinz_runtime::errors::prelude::anyhow!("still down"));
            }
        }
        self.seen.lock().unwrap().push(received);
        let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
        let read_socket = ours.try_clone().expect("clone failed");
        self.peers.send(theirs).expect("peer send failed");
        Ok(Reconnected {
            write_socket: ours,
            read_socket,
            dropped: self.dropped,
        })
    }
}

type ReconnectingPair = (
    RemoteReader,
    Arc<RemoteConn>,
    UnixStream,
    mpsc::Receiver<UnixStream>,
    Arc<TestReconnector>,
);

fn reconnecting_pair(dropped: u64, fail_first: u32) -> ReconnectingPair {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let for_conn = ours.try_clone().expect("clone failed");
    let (peer_tx, peer_rx) = mpsc::channel();
    let reconnector = Arc::new(TestReconnector {
        peers: peer_tx,
        dropped,
        seen: Mutex::new(Vec::new()),
        fail_first: Mutex::new(fail_first),
    });
    let conn = Arc::new(
        RemoteConn::new(
            for_conn,
            "build-box".to_owned(),
            "tok".to_owned(),
            RunCommand::default(),
            Some(reconnector.clone() as Arc<dyn Reconnector>),
        )
        .expect("conn failed"),
    );
    let reader = RemoteReader::new(conn.clone(), ours).expect("reader failed");
    (reader, conn, theirs, peer_rx, reconnector)
}

/// Serve replayed bytes on whichever socket the next reconnect produces.
fn answer_next_reconnect(peer_rx: mpsc::Receiver<UnixStream>, replay: &'static [u8]) {
    std::thread::spawn(move || {
        if let Ok(mut peer) = peer_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            let mut framed = Vec::new();
            write_output(&mut framed, replay).expect("frame failed");
            let _ = peer.write_all(&framed);
            let _ = peer.flush();
            // Hold the socket open, or the reader would see another EOF.
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

#[tokio::test]
async fn a_dropped_link_reconnects_instead_of_ending_the_pane() {
    let (mut reader, conn, mut peer, peer_rx, _rc) = reconnecting_pair(0, 0);
    let mut framed = Vec::new();
    write_output(&mut framed, b"before").expect("frame failed");
    send(&mut peer, &framed);
    assert_eq!(read_to_string(&mut reader, 6).await, "before");

    answer_next_reconnect(peer_rx, b"after");
    drop(peer);

    // Everything from here is the reconnect: a notice, then the pane's output
    // again. Crucially, not `Ok(0)` and not an error.
    let mut got = String::new();
    while !got.contains("after") {
        let mut buf = [0u8; 256];
        let n = reader
            .read(&mut buf)
            .await
            .expect("a dropped link must not end the pane");
        assert!(n > 0, "a dropped link must not look like EOF");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(
        got.contains("disconnected"),
        "the user should be told: {got}"
    );
    assert!(
        got.contains("reconnected"),
        "and told when it came back: {got}"
    );
    assert_eq!(
        conn.child_exit(),
        None,
        "no exit status may be invented for a transport failure"
    );
}

/// A resize made while the link was down never reached the broker: the send
/// failed on a broken connection and the outbound queues were cleared behind it.
/// Nothing else will ever mention it either — the size is only sent when it
/// changes — so the reconnect has to, or the remote application keeps drawing at
/// the old shape inside a grid that has since changed.
#[tokio::test]
async fn a_resize_made_during_an_outage_is_sent_again_on_reconnect() {
    let (mut reader, conn, peer, peer_rx, _rc) = reconnecting_pair(0, 0);
    conn.resize(WireWinsize {
        cols: 80,
        rows: 24,
        width_in_pixels: 0,
        height_in_pixels: 0,
    })
    .expect("the first resize goes over a healthy link");

    // The link dies, and the user reshapes the window while it is down.
    drop(peer);
    // The reconnect is picked up on a helper thread and handed back through a `Watched`, so the
    // wait below is for the socket actually arriving rather than for a duration. The two used to
    // disagree - the helper allowed 10s, the main flow gave up after ~5 and then read the slot -
    // so a reconnect landing in between failed the test with "never produced a socket".
    let taken: Watched<Option<UnixStream>> = Watched::default();
    let sink = taken.clone();
    std::thread::spawn(move || {
        if let Ok(peer) = peer_rx.recv_timeout(yinz_test_support::default_timeout()) {
            *sink.lock() = Some(peer);
        }
    });

    let mut buf = [0u8; 256];
    let read = reader.read(&mut buf);
    tokio::pin!(read);
    // Resize into the outage: `last_size` records it, the send cannot land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = conn.resize(WireWinsize {
        cols: 120,
        rows: 40,
        width_in_pixels: 0,
        height_in_pixels: 0,
    });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut read).await;

    // Whatever the broker is told after the reconnect must include the new size.
    taken.wait_until(
        "the reconnect to produce a socket",
        yinz_test_support::default_timeout(),
        |slot| slot.is_some().then_some(()),
    );
    let mut peer = taken.lock().take().expect("just waited for it to arrive");
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("failed to set a read timeout");
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = peer.read(&mut chunk) {
        if n == 0 {
            break;
        }
        seen.extend_from_slice(&chunk[..n]);
        if seen.len() > 2048 {
            break;
        }
    }
    let resized_to_the_new_size = std::iter::successors(Some(seen.as_slice()), |rest| {
        protocol::try_decode_frame(rest)
            .ok()
            .flatten()
            .map(|(_, consumed)| &rest[consumed..])
    })
    .filter_map(|rest| protocol::try_decode_frame(rest).ok().flatten())
    .any(|(frame, _)| {
        matches!(
            frame.decode_control::<ServerToBrokerMsg>(),
            Ok(ServerToBrokerMsg::Resize(size)) if size.cols == 120 && size.rows == 40
        )
    });
    assert!(
        resized_to_the_new_size,
        "the size the pane was given during the outage never reached the broker, so the \
         remote application is still drawing at the old one"
    );
}

/// The accounting the replay depends on. Notices this side made up must not
/// count as delivered output, or the broker resumes from the wrong offset and
/// silently skips real bytes.
#[tokio::test]
async fn injected_notices_do_not_count_as_delivered_output() {
    let (mut reader, conn, mut peer, peer_rx, rc) = reconnecting_pair(0, 0);
    let mut framed = Vec::new();
    write_output(&mut framed, b"0123456789").expect("frame failed");
    send(&mut peer, &framed);
    assert_eq!(read_to_string(&mut reader, 10).await, "0123456789");
    assert_eq!(conn.received(), 10);

    answer_next_reconnect(peer_rx, b"more");
    drop(peer);

    let mut got = String::new();
    while !got.contains("more") {
        let mut buf = [0u8; 256];
        let n = reader.read(&mut buf).await.expect("read failed");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }

    // The reconnect must have asked to resume from 10 — the real bytes taken —
    // not from 10 plus the length of the banners.
    let seen = rc.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![10],
        "resumed from the wrong offset; synthetic bytes leaked into the count"
    );
    assert_eq!(
        conn.received(),
        14,
        "only real output counts: 10 before, 4 after"
    );
}

/// A replay with a hole cannot be spliced into the grid — it would look like
/// real output. Clear the screen and say so instead.
#[tokio::test]
async fn a_lossy_replay_clears_the_screen_and_says_so() {
    let (mut reader, conn, peer, peer_rx, _rc) = reconnecting_pair(4096, 0);
    answer_next_reconnect(peer_rx, b"fresh");
    drop(peer);

    let mut got = String::new();
    while !got.contains("fresh") {
        let mut buf = [0u8; 256];
        let n = reader.read(&mut buf).await.expect("read failed");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(
        got.contains("\x1b[2J"),
        "a lossy replay must clear the grid rather than splice a torn stream: {got:?}"
    );
    assert!(
        got.contains("4096 bytes of output were lost"),
        "and must say how much was lost: {got}"
    );
    assert_eq!(
        conn.received(),
        4101,
        "lost bytes must advance the stream offset"
    );
}

#[test]
fn successive_lossy_reconnects_resume_after_delivered_and_lost_bytes() {
    let (_reader, conn, _peer, _peer_rx, rc) = reconnecting_pair(4096, 0);
    // A warp restores both its old offset and the gap encountered during adoption.
    conn.restore_received(10, 4);
    conn.reconnect().unwrap();
    conn.note_received(5);
    conn.reconnect().unwrap();
    assert_eq!(*rc.seen.lock().unwrap(), [14, 4115]);
    assert_eq!(conn.received(), 8211);
}

#[tokio::test]
async fn a_gap_received_before_transport_loss_is_in_the_next_resume_offset() {
    let (mut reader, conn, mut peer, peer_rx, rc) = reconnecting_pair(0, 0);
    write_control(&mut peer, &BrokerToServerMsg::OutputGap { dropped: 17 }).unwrap();
    drop(peer);

    let notice = read_to_string(&mut reader, 1).await;
    assert!(notice.contains("17 bytes of output were lost"));
    assert_eq!(conn.received(), 17);

    let broker = std::thread::spawn(move || {
        let mut peer = peer_rx
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        write_output(&mut peer, b"tail").unwrap();
        write_control(
            &mut peer,
            &BrokerToServerMsg::ChildExited { status: Some(0) },
        )
        .unwrap();
    });
    let mut output = Vec::new();
    loop {
        let mut bytes = [0; 256];
        let received = reader.read(&mut bytes).await.unwrap();
        if received == 0 {
            break;
        }
        output.extend_from_slice(&bytes[..received]);
    }
    broker.join().unwrap();
    assert!(output.ends_with(b"tail"));
    assert_eq!(*rc.seen.lock().unwrap(), [17]);
    assert_eq!(conn.received(), 21);
}

/// A link that is down for a moment is the ordinary case, so one failure must
/// not end the pane.
#[tokio::test]
async fn a_reconnect_retries_before_giving_up() {
    let (mut reader, _conn, peer, peer_rx, rc) = reconnecting_pair(0, 2);
    answer_next_reconnect(peer_rx, b"eventually");
    drop(peer);

    let mut got = String::new();
    while !got.contains("eventually") {
        let mut buf = [0u8; 256];
        let n = reader
            .read(&mut buf)
            .await
            .expect("two failures must not end the pane");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert_eq!(
        *rc.fail_first.lock().unwrap(),
        0,
        "both scheduled failures should have been consumed"
    );
}

#[test]
fn git_probe_target_changed_during_an_outage_is_restored_on_reconnect() {
    use crate::pty_host::remote::protocol::read_frame;
    use std::time::Duration;
    let (_reader, conn, mut peer, peer_rx, _rc) = reconnecting_pair(0, 0);
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(conn.git_probe("/first").unwrap(), None);
    read_frame(&mut peer).unwrap().unwrap();
    conn.mark_transport_lost();
    assert!(conn
        .git_probe_with_refresh("/agent", Some("fresh"))
        .is_err());
    let _ = conn.reconnect().unwrap();
    let mut resumed = peer_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    resumed
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let message = read_frame(&mut resumed)
        .unwrap()
        .unwrap()
        .decode_control::<ServerToBrokerMsg>()
        .unwrap();
    assert!(
        matches!(message, ServerToBrokerMsg::GetGitProbe { cwd, refresh_token } if cwd == "/agent" && refresh_token.as_deref() == Some("fresh"))
    );
}
