use super::*;

use std::io::Write;
use std::os::unix::net::UnixStream;

use crate::pty_host::remote::protocol::{write_control, write_output};

fn reader_pair() -> (RemoteReader, Arc<RemoteConn>, UnixStream) {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let for_conn = ours.try_clone().expect("clone failed");
    let conn = Arc::new(RemoteConn::new(for_conn, "build-box".to_owned()).expect("conn failed"));
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

/// Status is cached from the broker's own machine, because the local server
/// cannot inspect a process it does not have.
#[tokio::test]
async fn a_status_report_updates_the_cache_without_disturbing_the_stream() {
    let (mut reader, conn, mut peer) = reader_pair();
    let mut framed = Vec::new();
    write_control(
        &mut framed,
        &BrokerToServerMsg::StatusReport {
            cwd: Some("/home/me/work".to_owned()),
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
