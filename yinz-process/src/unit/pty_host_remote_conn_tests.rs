use super::*;

use std::io::Read;
use std::os::unix::net::UnixStream;

use crate::pty_host::remote::protocol::{try_decode_frame, Frame};

fn conn_pair() -> (RemoteConn, UnixStream) {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let conn = RemoteConn::new(
        ours,
        "build-box".to_owned(),
        "tok".to_owned(),
        RunCommand::default(),
        None,
    )
    .expect("conn failed");
    (conn, theirs)
}

/// Read every *complete* frame currently sitting in the socket, keeping any
/// partial tail in `leftover` for the next call.
///
/// The socket buffer filling mid-frame is the normal case in the backpressure
/// tests, and so is being called repeatedly as the peer drains. Discarding the
/// partial tail between calls would resume mid-frame and read payload bytes as
/// a length.
fn drain_frames(peer: &mut UnixStream, leftover: &mut Vec<u8>) -> Vec<Frame> {
    peer.set_nonblocking(true).expect("nonblocking failed");
    let mut chunk = [0u8; 8192];
    loop {
        match peer.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => leftover.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("peer read failed: {e}"),
        }
    }
    let mut frames = Vec::new();
    let mut offset = 0usize;
    while let Some((frame, consumed)) =
        try_decode_frame(&leftover[offset..]).expect("frame decode failed")
    {
        frames.push(frame);
        offset += consumed;
    }
    leftover.drain(..offset);
    frames
}

/// Convenience for the tests that drain exactly once.
fn drain_all(peer: &mut UnixStream) -> Vec<Frame> {
    drain_frames(peer, &mut Vec::new())
}

#[test]
fn input_reaches_the_peer_as_an_input_frame() {
    let (conn, mut peer) = conn_pair();
    let written = conn.write_input(b"ls -la\n").expect("write failed");
    assert_eq!(written, 7);
    assert_eq!(
        drain_all(&mut peer),
        vec![Frame::Input(b"ls -la\n".to_vec())]
    );
}

/// The contract `pty_writer` depends on: a short count means "not accepted, ask
/// again", never "written". If a full queue silently swallowed input, a paste
/// into a stopped remote app would vanish instead of arriving late.
#[test]
fn a_full_queue_short_counts_rather_than_dropping_input() {
    let (conn, mut peer) = conn_pair();
    // Never read from `peer`, so the socket buffer fills and the queue backs up.
    // Every four-byte position differs: duplicated or reordered frames must
    // fail even if the byte count is right. Include NUL and non-UTF-8 bytes.
    let payload: Vec<u8> = (0..512 * 1024u32).flat_map(u32::to_le_bytes).collect();
    let mut accepted_total = 0usize;
    for _ in 0..200 {
        let remaining = &payload[accepted_total..];
        let accepted = conn.write_input(remaining).expect("write failed");
        assert!(
            accepted <= remaining.len(),
            "accepted more than it was given: {accepted}"
        );
        if accepted == 0 {
            break;
        }
        accepted_total += accepted;
    }
    assert!(
        (1..payload.len()).contains(&accepted_total),
        "the peer must stop accepting before all input is sent for this to exercise backpressure"
    );
    assert_eq!(conn.write_input(&payload[accepted_total..]).unwrap(), 0);

    // Resume draining and retry exactly the unaccepted suffix, as pty_writer
    // does. Once it is accepted, empty writes flush the remaining queued data.
    // Both sockets are driven here, so a bounded loop needs no sleeps or threads.
    let mut delivered = Vec::new();
    let mut leftover = Vec::new();
    for _ in 0..2000 {
        for frame in drain_frames(&mut peer, &mut leftover) {
            match frame {
                Frame::Input(bytes) => delivered.extend(bytes),
                other => panic!("expected input frames, got {other:?}"),
            }
        }
        assert!(delivered.len() <= accepted_total);
        if delivered.len() == payload.len() {
            break;
        }
        let remaining = &payload[accepted_total..];
        let accepted = conn.write_input(remaining).expect("retry failed");
        assert!(accepted <= remaining.len());
        accepted_total += accepted;
    }
    assert_eq!(accepted_total, payload.len(), "input never resumed");
    assert_eq!(delivered.len(), payload.len(), "accepted input was lost");
    assert_eq!(
        delivered
            .iter()
            .zip(&payload)
            .position(|(got, want)| got != want),
        None,
        "input was corrupted, duplicated, or reordered at this byte offset"
    );
    assert_eq!(conn.write_input(&[]).unwrap(), 0);
    assert!(drain_frames(&mut peer, &mut leftover).is_empty());
    assert!(leftover.is_empty(), "a partial frame was left on the wire");
}

/// A delayed resize is not a slowdown, it is a lasting wrong state: the remote
/// app stays convinced of the old window size until something else resizes it.
/// For a local pane a resize is a `TIOCSWINSZ` ioctl, entirely independent of
/// the pty write queue, and a remote pane should not be meaningfully worse.
///
/// Note what is *not* claimed. When the kernel socket buffer is full nothing can
/// leave it, control or otherwise; no queue discipline escapes that. The
/// guarantee is that control overtakes everything still *queued*, so it goes out
/// on the next opportunity rather than after the backlog.
#[test]
fn a_control_frame_overtakes_input_that_is_still_queued() {
    let (conn, mut peer) = conn_pair();
    // Fill until the socket stops accepting, so there is a real backlog.
    let payload = vec![b'x'; 64 * 1024];
    let mut accepted = 0usize;
    for _ in 0..200 {
        let taken = conn.write_input(&payload).expect("write failed");
        accepted += taken;
        if taken == 0 {
            break;
        }
    }
    assert!(
        accepted > payload.len(),
        "the queue took only {accepted} bytes, so there is no backlog for the \
         resize to overtake and this test would prove nothing"
    );
    conn.resize(WireWinsize {
        cols: 120,
        rows: 40,
        width_in_pixels: 0,
        height_in_pixels: 0,
    })
    .expect("resize failed");

    // Drain the peer and pump the connection, the way `pty_writer` re-drives it
    // every `PENDING_DRAIN_TIMEOUT`, until everything that was accepted has come
    // out. The end condition is the byte count, not an empty read: a drain yields
    // whole frames only, so where the socket buffer is smaller than a frame -
    // macOS gives a `socketpair` 8KiB against Linux's ~200KiB - it comes back
    // empty *between* frames while the queue is still full. Treating that as
    // "drained" stopped the loop before the frames this test is about.
    let mut frames = Vec::new();
    let mut leftover = Vec::new();
    let mut delivered = 0usize;
    for _ in 0..2000 {
        for frame in drain_frames(&mut peer, &mut leftover) {
            if let Frame::Input(bytes) = &frame {
                delivered += bytes.len();
            }
            frames.push(frame);
        }
        if delivered >= accepted {
            break;
        }
        // An empty write flushes without enqueuing anything.
        let _ = conn.write_input(&[]);
    }
    assert_eq!(
        delivered, accepted,
        "only {delivered} of the {accepted} accepted bytes ever reached the wire"
    );

    let control_at = frames
        .iter()
        .position(|f| matches!(f, Frame::Control(_)))
        .expect("the resize never reached the wire at all");
    let input_after = frames[control_at + 1..]
        .iter()
        .any(|f| matches!(f, Frame::Input(_)));
    assert!(
        input_after,
        "the resize arrived only after the entire input backlog, so it did not \
         overtake anything ({} frames, control at {control_at})",
        frames.len()
    );
}

#[test]
fn a_signal_is_sent_symbolically_rather_than_as_a_pid() {
    let (conn, mut peer) = conn_pair();
    conn.signal_child(ChildSignal::Int).expect("signal failed");
    let frames = drain_all(&mut peer);
    assert_eq!(frames.len(), 1);
    let msg: ServerToBrokerMsg = frames[0].decode_control().expect("decode failed");
    assert_eq!(msg, ServerToBrokerMsg::SignalChild(ChildSignal::Int));
}

/// The child's exit status is latched by the first report. A transport failure
/// racing a real exit must not overwrite the real answer.
#[test]
fn the_first_reported_exit_status_wins() {
    let (conn, _peer) = conn_pair();
    assert_eq!(conn.child_exit(), None);
    conn.set_child_exit(Some(130));
    conn.set_child_exit(Some(0));
    assert_eq!(conn.child_exit(), Some(Some(130)));
}

#[test]
fn a_lost_transport_refuses_further_writes_rather_than_pretending() {
    let (conn, _peer) = conn_pair();
    conn.mark_transport_lost();
    assert!(conn.transport_lost());
    assert!(conn.write_input(b"hello").is_err());
}

#[test]
fn the_first_failed_socket_write_is_reported_to_input_and_control_callers() {
    for input in [true, false] {
        let (conn, peer) = conn_pair();
        drop(peer);
        let result = if input {
            conn.write_input(b"must not be acknowledged").map(|_| ())
        } else {
            conn.terminate()
        };
        assert!(
            result.is_err(),
            "the write that discovers the failure must fail"
        );
        assert!(conn.write_input(b"retry").is_err());
        assert!(conn.terminate().is_err());
        let out = conn.outbound.lock().unwrap();
        assert!(out.control.is_empty() && out.input.is_empty());
        assert_eq!(out.queued_input_bytes, 0);
    }
}

/// A budget of zero is a caller that has already given up, and std refuses to
/// bound a socket by it. Refused here, up front, the socket is left exactly as
/// it was — non-blocking, nothing written — rather than flipped to blocking on
/// the way to an error about "a 0 duration timeout".
#[test]
fn a_release_with_no_time_left_is_refused_before_the_socket_is_touched() {
    let (conn, mut peer) = conn_pair();

    let error = format!(
        "{:#}",
        conn.release_for_warp(std::time::Duration::ZERO)
            .expect_err("a release with no budget must be refused")
    );

    assert!(
        error.contains("no time was left"),
        "the refusal should describe the situation, not the socket: {error}"
    );
    assert!(
        drain_all(&mut peer).is_empty(),
        "nothing should have been asked of the broker"
    );
}

#[test]
fn termination_is_sent_without_waiting_for_an_acknowledgement() {
    let (conn, mut peer) = conn_pair();
    conn.terminate().unwrap();
    let frames = drain_all(&mut peer);
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].decode_control::<ServerToBrokerMsg>().unwrap(),
        ServerToBrokerMsg::Terminate
    );
}

#[test]
fn a_broken_connection_refuses_termination_immediately() {
    let (conn, _peer) = conn_pair();
    conn.mark_transport_lost();
    assert!(conn.terminate().is_err());
}

#[test]
fn git_probe_requests_only_the_selected_directory_and_filters_stale_results() {
    use crate::pty_host::remote::conn::RemoteGitProbe;
    use crate::pty_host::remote::protocol::ServerToBrokerMsg;
    let (conn, mut peer) = conn_pair();
    conn.set_status(RemoteStatus {
        cwd: Some("/process".into()),
        ..RemoteStatus::default()
    });
    for cwd in ["/agent", "/changed"] {
        assert_eq!(conn.git_probe(cwd).unwrap(), None);
        let frames = drain_all(&mut peer);
        assert_eq!(frames.len(), 1);
        assert!(
            matches!(frames[0].decode_control::<ServerToBrokerMsg>().unwrap(),
            ServerToBrokerMsg::GetGitProbe { cwd: requested, .. } if requested == cwd)
        );
        assert_eq!(conn.git_probe(cwd).unwrap(), None);
        assert!(
            drain_all(&mut peer).is_empty(),
            "pending polls do not flood the broker"
        );
        let probe = RemoteGitProbe {
            cwd: cwd.into(),
            host: "build-box".into(),
            exit_code: Some(128),
            ..RemoteGitProbe::default()
        };
        conn.set_git_probe(probe.clone());
        assert_eq!(conn.git_probe(cwd).unwrap(), Some(probe));
    }
    assert_eq!(conn.status().cwd.as_deref(), Some("/process"));
    assert!(conn.git_probe("").is_err());
    assert!(drain_all(&mut peer).is_empty());
}

#[test]
fn git_probe_refresh_requires_its_own_reply_even_at_the_same_directory() {
    use crate::pty_host::remote::conn::RemoteGitProbe;
    use crate::pty_host::remote::protocol::ServerToBrokerMsg;
    let (conn, mut peer) = conn_pair();
    let cached = RemoteGitProbe {
        cwd: "/work".into(),
        ..Default::default()
    };
    conn.set_git_probe(cached.clone());
    for token in ["first", "second"] {
        assert_eq!(
            conn.git_probe_with_refresh("/work", Some(token)).unwrap(),
            None
        );
        let frames = drain_all(&mut peer);
        assert_eq!(frames.len(), 1);
        assert!(
            matches!(frames[0].decode_control::<ServerToBrokerMsg>().unwrap(),
            ServerToBrokerMsg::GetGitProbe { cwd, refresh_token } if cwd == "/work" && refresh_token.as_deref() == Some(token))
        );
        conn.set_git_probe(cached.clone());
        assert_eq!(
            conn.git_probe_with_refresh("/work", Some(token)).unwrap(),
            None
        );
        // Background readers must not cancel the pending refresh.
        let _ = conn.git_probe("/work").unwrap();
        assert!(drain_all(&mut peer).is_empty());
        let fresh = RemoteGitProbe {
            refresh_token: Some(token.into()),
            ..cached.clone()
        };
        conn.set_git_probe(fresh.clone());
        // A late background/reconnect replay must not overwrite an acknowledged result.
        conn.set_git_probe(cached.clone());
        assert_eq!(
            conn.git_probe_with_refresh("/work", Some(token)).unwrap(),
            Some(fresh)
        );
    }
}
