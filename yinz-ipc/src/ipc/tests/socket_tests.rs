use crate::ipc::{
    send_protobuf_client_to_server, send_protobuf_server_to_client, ClientToServerMsg,
    IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, IpcStream, ServerToClientMsg,
};
use interprocess::local_socket::{prelude::*, ListenerOptions};
use yinz_types::pane_size::Size;

use std::sync::atomic::{AtomicU32, Ordering};

struct FlushFailingStream;

impl std::io::Read for FlushFailingStream {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl std::io::Write for FlushFailingStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "flush failed",
        ))
    }
}

impl IpcStream for FlushFailingStream {
    fn try_clone_stream(&self) -> std::io::Result<Box<dyn IpcStream>> {
        Ok(Box::new(Self))
    }

    fn set_read_timeout(&self, _timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        Ok(())
    }

    fn set_write_timeout(&self, _timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

fn flush_failing_sender<T: serde::Serialize>() -> IpcSenderWithContext<T> {
    IpcSenderWithContext::from_boxed(Box::new(FlushFailingStream))
}

static IPC_COUNTER: AtomicU32 = AtomicU32::new(0);

#[test]
fn splitting_an_ipc_connection_reports_descriptor_exhaustion() {
    struct CloneFailingStream;
    impl std::io::Read for CloneFailingStream {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }
    impl std::io::Write for CloneFailingStream {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl IpcStream for CloneFailingStream {
        fn try_clone_stream(&self) -> std::io::Result<Box<dyn IpcStream>> {
            Err(std::io::Error::from_raw_os_error(libc::EMFILE))
        }
        fn set_read_timeout(&self, _: Option<std::time::Duration>) -> std::io::Result<()> {
            Ok(())
        }
        fn set_write_timeout(&self, _: Option<std::time::Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    let sender =
        IpcSenderWithContext::<ClientToServerMsg>::from_boxed(Box::new(CloneFailingStream));
    let receiver =
        IpcReceiverWithContext::<ClientToServerMsg>::from_boxed(Box::new(CloneFailingStream));
    assert_eq!(
        sender
            .get_receiver::<ServerToClientMsg>()
            .err()
            .unwrap()
            .raw_os_error(),
        Some(libc::EMFILE)
    );
    assert_eq!(
        receiver
            .get_sender::<ServerToClientMsg>()
            .err()
            .unwrap()
            .raw_os_error(),
        Some(libc::EMFILE)
    );
}

type IpcGuard = tempfile::TempDir;
type IpcName = std::path::PathBuf;
fn new_ipc() -> (IpcGuard, IpcName) {
    let dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let id = IPC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = dir
        .path()
        .join(format!("test-{}-{}.sock", std::process::id(), id));
    (dir, path)
}

fn bind_listener(name: &IpcName) -> interprocess::local_socket::Listener {
    use interprocess::local_socket::GenericFilePath;
    ListenerOptions::new()
        .name(name.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .expect("bind failed")
}

fn connect_stream(name: &IpcName) -> interprocess::local_socket::Stream {
    use interprocess::local_socket::{GenericFilePath, Stream as LocalSocketStream};
    LocalSocketStream::connect(name.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .expect("connect failed")
}

#[test]
fn send_apis_propagate_flush_failures() {
    assert_eq!(
        flush_failing_sender::<ClientToServerMsg>()
            .set_send_timeout(None)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    let errors = [
        flush_failing_sender::<ClientToServerMsg>()
            .send_client_msg(ClientToServerMsg::ConnStatus)
            .unwrap_err(),
        flush_failing_sender::<ServerToClientMsg>()
            .send_server_msg(ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            })
            .unwrap_err(),
        send_protobuf_client_to_server(&mut flush_failing_sender(), ClientToServerMsg::ConnStatus)
            .unwrap_err(),
        send_protobuf_server_to_client(
            &mut flush_failing_sender(),
            ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            },
        )
        .unwrap_err(),
    ];

    for error in errors {
        assert_eq!(
            error.downcast_ref::<std::io::Error>().map(|e| e.kind()),
            Some(std::io::ErrorKind::BrokenPipe)
        );
    }
}

#[test]
fn a_partial_write_permanently_closes_the_sender_without_retrying_on_drop() {
    struct PartialWriteStream(std::sync::Arc<AtomicU32>);
    impl std::io::Read for PartialWriteStream {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }
    impl std::io::Write for PartialWriteStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            match self.0.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(buf.len().min(2)),
                1 => Err(std::io::ErrorKind::TimedOut.into()),
                _ => Ok(buf.len()),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl IpcStream for PartialWriteStream {
        fn try_clone_stream(&self) -> std::io::Result<Box<dyn IpcStream>> {
            Ok(Box::new(Self(self.0.clone())))
        }
        fn set_read_timeout(&self, _: Option<std::time::Duration>) -> std::io::Result<()> {
            Ok(())
        }
        fn set_write_timeout(&self, _: Option<std::time::Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }
    for from_client in [true, false] {
        let writes = std::sync::Arc::new(AtomicU32::new(0));
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::from_boxed(Box::new(
            PartialWriteStream(writes.clone()),
        ));
        let result = if from_client {
            sender.send_client_msg(ClientToServerMsg::BackgroundColor {
                color: "x".repeat(16 * 1024),
            })
        } else {
            sender.send_server_msg(ServerToClientMsg::Render {
                content: "x".repeat(16 * 1024),
            })
        };
        assert!(result.is_err());
        assert!(sender
            .send_client_msg(ClientToServerMsg::KillSession)
            .is_err());
        assert!(sender
            .send_server_msg(ServerToClientMsg::UnblockInputThread)
            .is_err());
        drop(sender);
        assert_eq!(
            writes.load(Ordering::SeqCst),
            2,
            "failed frames must never be retried"
        );
    }
}

fn try_connect(name: &IpcName) -> std::io::Result<interprocess::local_socket::Stream> {
    use interprocess::local_socket::{GenericFilePath, Stream as LocalSocketStream};
    LocalSocketStream::connect(name.as_path().to_fs_name::<GenericFilePath>().unwrap())
}

#[test]
fn client_to_server_message_over_socket() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let client = std::thread::spawn({
        let name = name.clone();
        move || {
            let stream = connect_stream(&name);
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(stream);
            sender
                .send_client_msg(ClientToServerMsg::ConnStatus)
                .expect("send failed");
        }
    });

    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);

    let msg = receiver.recv_client_msg().msg();
    assert!(msg.is_some(), "should receive a message");
    let (msg, _ctx) = msg.unwrap();
    assert!(
        matches!(msg, ClientToServerMsg::ConnStatus),
        "should be ConnStatus, got: {:?}",
        msg
    );

    client.join().expect("client thread panicked");
}

#[test]
fn server_to_client_message_over_socket() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let server = std::thread::spawn(move || {
        let stream = listener.incoming().next().unwrap().expect("accept failed");
        let mut sender: IpcSenderWithContext<ServerToClientMsg> = IpcSenderWithContext::new(stream);
        sender
            .send_server_msg(ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            })
            .expect("send failed");
    });

    let stream = connect_stream(&name);
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
        IpcReceiverWithContext::new(stream);

    let msg = receiver.recv_server_msg().msg();
    assert!(msg.is_some(), "should receive a message");
    let (msg, _ctx) = msg.unwrap();
    assert!(
        matches!(
            msg,
            ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            }
        ),
        "should be Connected, got: {:?}",
        msg
    );

    server.join().expect("server thread panicked");
}

#[test]
fn bidirectional_communication_via_fd_duplication() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let server = std::thread::spawn(move || {
        let stream = listener.incoming().next().unwrap().expect("accept failed");
        let mut sender: IpcSenderWithContext<ServerToClientMsg> = IpcSenderWithContext::new(stream);

        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            sender.get_receiver().unwrap();
        sender
            .send_server_msg(ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            })
            .expect("send failed");

        let msg = receiver.recv_client_msg().msg();
        assert!(msg.is_some(), "server should receive client message");
        let (msg, _) = msg.unwrap();
        assert!(
            matches!(msg, ClientToServerMsg::ConnStatus),
            "should be ConnStatus"
        );
    });

    let stream = connect_stream(&name);
    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);

    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver().unwrap();

    let msg = receiver.recv_server_msg().msg();
    assert!(msg.is_some(), "client should receive server message");
    let (msg, _) = msg.unwrap();
    assert!(
        matches!(
            msg,
            ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            }
        ),
        "should be Connected"
    );

    sender
        .send_client_msg(ClientToServerMsg::ConnStatus)
        .expect("send failed");

    server.join().expect("server thread panicked");
}

#[test]
fn multiple_messages_in_sequence() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let client = std::thread::spawn({
        let name = name.clone();
        move || {
            let stream = connect_stream(&name);
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(stream);

            sender
                .send_client_msg(ClientToServerMsg::ConnStatus)
                .expect("send 1 failed");
            sender
                .send_client_msg(ClientToServerMsg::TerminalResize {
                    new_size: Size {
                        rows: 50,
                        cols: 120,
                    },
                })
                .expect("send 2 failed");
            sender
                .send_client_msg(ClientToServerMsg::KillSession)
                .expect("send 3 failed");
        }
    });

    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);

    let (msg1, _) = receiver.recv_client_msg().msg().expect("missing message 1");
    assert!(matches!(msg1, ClientToServerMsg::ConnStatus));

    let (msg2, _) = receiver.recv_client_msg().msg().expect("missing message 2");
    match msg2 {
        ClientToServerMsg::TerminalResize { new_size } => {
            assert_eq!(new_size.rows, 50);
            assert_eq!(new_size.cols, 120);
        },
        other => panic!("expected TerminalResize, got: {:?}", other),
    }

    let (msg3, _) = receiver.recv_client_msg().msg().expect("missing message 3");
    assert!(matches!(msg3, ClientToServerMsg::KillSession));

    client.join().expect("client thread panicked");
}

#[test]
fn receiver_returns_none_on_closed_connection() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let client = std::thread::spawn({
        let name = name.clone();
        move || {
            let stream = connect_stream(&name);
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(stream);
            sender
                .send_client_msg(ClientToServerMsg::ConnStatus)
                .expect("send failed");
        }
    });

    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);

    client.join().expect("client thread panicked");

    let msg = receiver.recv_client_msg().msg();
    assert!(msg.is_some(), "should receive the sent message");

    let msg = receiver.recv_client_msg().msg();
    assert!(msg.is_none(), "should return None after connection closed");
}

#[cfg(unix)]
#[test]
fn is_socket_identifies_bound_unix_socket() {
    use std::os::unix::fs::FileTypeExt;
    let (_guard, name) = new_ipc();
    let _listener = bind_listener(&name);

    let metadata = std::fs::metadata(&name).expect("metadata failed");
    assert!(
        metadata.file_type().is_socket(),
        "a bound LocalSocketListener path should be identified as a socket"
    );
}

#[cfg(unix)]
#[test]
fn is_socket_rejects_regular_file() {
    use std::os::unix::fs::FileTypeExt;
    let dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let file_path = dir.path().join("not_a_socket");
    std::fs::write(&file_path, b"regular file").expect("write failed");

    let metadata = std::fs::metadata(&file_path).expect("metadata failed");
    assert!(
        !metadata.file_type().is_socket(),
        "a regular file should NOT be identified as a socket"
    );
}

#[test]
fn session_probe_accepts_responding_socket() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let server = std::thread::spawn(move || {
        let stream = listener.incoming().next().unwrap().expect("accept failed");
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(stream);
        let mut sender: IpcSenderWithContext<ServerToClientMsg> = receiver.get_sender().unwrap();

        let msg = receiver.recv_client_msg().msg();
        assert!(matches!(msg, Some((ClientToServerMsg::ConnStatus, _))));

        sender
            .send_server_msg(ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            })
            .expect("send failed");
    });

    let stream = connect_stream(&name);
    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);
    sender
        .send_client_msg(ClientToServerMsg::ConnStatus)
        .expect("send failed");
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver().unwrap();

    let result = receiver.recv_server_msg().msg();
    assert!(
        matches!(
            result,
            Some((
                ServerToClientMsg::Connected {
                    warp_companions: true,
                    warp_held_terminals: false,
                },
                _
            ))
        ),
        "probing a live session socket should return Connected"
    );

    server.join().expect("server thread panicked");
}

#[test]
fn session_probe_rejects_dead_socket() {
    let (_guard, name) = new_ipc();

    {
        let _listener = bind_listener(&name);
    }

    let result = try_connect(&name);
    assert!(
        result.is_err(),
        "connecting to a dead socket should fail (no listener)"
    );
}

#[cfg(unix)]
#[test]
fn socket_directory_enumeration_finds_sockets() {
    use interprocess::local_socket::GenericFilePath;
    use std::os::unix::fs::FileTypeExt;

    let dir = tempfile::TempDir::new().expect("failed to create temp dir");

    let sock_path = dir.path().join("test-session");
    let _listener = ListenerOptions::new()
        .name(sock_path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .expect("bind failed");

    let file_path = dir.path().join("not-a-session");
    std::fs::write(&file_path, b"data").expect("write failed");

    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read_dir failed")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_socket() {
                entry.file_name().into_string().ok()
            } else {
                None
            }
        })
        .collect();

    assert_eq!(entries.len(), 1, "should find exactly one socket");
    assert_eq!(entries[0], "test-session");
}

/// A hangup is terminal, so it has to be reported as one on the *first* read.
/// The bug this guards against reported it as an unreadable message instead,
/// which sent both routers into a thousand-iteration zero-delay retry loop
/// before blaming "an empty message" for a server that had simply exited.
#[test]
fn a_closed_peer_reports_eof_immediately_and_keeps_reporting_it() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let client = std::thread::spawn(move || {
        let stream = connect_stream(&name);
        let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);
        sender
            .send_client_msg(ClientToServerMsg::ConnStatus)
            .expect("send failed");
        // and now hang up
    });

    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);

    assert!(
        matches!(receiver.recv_client_msg(), IpcRecv::Msg(..)),
        "the message sent before the hangup should still arrive"
    );

    client.join().expect("client thread panicked");

    for read in 0..3 {
        assert!(
            receiver.recv_client_msg().is_eof(),
            "read {read} after the peer closed should be Eof, not a malformed message"
        );
    }
}

#[test]
fn a_rejected_length_cannot_turn_its_payload_into_another_message() {
    use std::io::Write;

    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);
    let mut peer = connect_stream(&name);
    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);
    peer.write_all(&((crate::MAX_IPC_FRAME_LEN + 1) as u32).to_le_bytes())
        .unwrap();
    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(peer);
    sender
        .send_client_msg(ClientToServerMsg::KillSession)
        .unwrap();

    assert!(receiver.recv_client_msg().is_eof());
    assert!(receiver.recv_client_msg().is_eof());
    assert!(receiver.recv_server_msg().is_eof());
    assert!(crate::ipc::recv_protobuf_client_to_server(&mut receiver).is_none());
}

#[test]
fn fragmented_frames_resume_across_timeouts_in_both_directions() {
    use crate::client_server_contract::client_server_contract::{
        ClientToServerMsg as ProtoClient, ServerToClientMsg as ProtoServer,
    };
    use prost::Message;
    use std::io::Write;

    for server_message in [false, true] {
        let payload = if server_message {
            ProtoServer::from(ServerToClientMsg::Render {
                content: "fragmented render".into(),
            })
            .encode_to_vec()
        } else {
            ProtoClient::try_from(ClientToServerMsg::Paste("fragmented paste".into()))
                .unwrap()
                .encode_to_vec()
        };
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend(payload);
        let (_guard, name) = new_ipc();
        let listener = bind_listener(&name);
        let mut peer = connect_stream(&name);
        let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
            IpcReceiverWithContext::new(listener.accept().unwrap());
        receiver
            .set_recv_timeout(Some(std::time::Duration::from_millis(10)))
            .unwrap();
        let mut start = 0;
        for end in [1, 2, 3, 4, 5, frame.len() - 1] {
            peer.write_all(&frame[start..end]).unwrap();
            for _ in 0..2 {
                if server_message {
                    assert!(matches!(receiver.recv_server_msg(), IpcRecv::Idle));
                } else {
                    assert!(matches!(receiver.recv_client_msg(), IpcRecv::Idle));
                }
            }
            start = end;
        }
        receiver.set_recv_timeout(None).unwrap();
        peer.write_all(&frame[start..]).unwrap();
        peer.write_all(&frame).unwrap();
        for _ in 0..2 {
            if server_message {
                assert!(
                    matches!(receiver.recv_server_msg(), IpcRecv::Msg(ServerToClientMsg::Render { content }, _) if content == "fragmented render")
                );
            } else {
                assert!(
                    matches!(receiver.recv_client_msg(), IpcRecv::Msg(ClientToServerMsg::Paste(content), _) if content == "fragmented paste")
                );
            }
        }
    }
}

#[test]
fn server_frame_deadline_is_not_renewed_by_incoming_chunks() {
    use crate::client_server_contract::client_server_contract::ServerToClientMsg as ProtoServer;
    use prost::Message;
    use std::{
        io::Write,
        sync::mpsc,
        time::{Duration, Instant},
    };

    let content = "fragmented render".repeat(100);
    let payload = ProtoServer::from(ServerToClientMsg::Render {
        content: content.clone(),
    })
    .encode_to_vec();
    let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
    frame.extend(payload);
    for split in [1, 5] {
        let (_guard, name) = new_ipc();
        let listener = bind_listener(&name);
        let mut peer = connect_stream(&name);
        let mut receiver =
            IpcReceiverWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
        peer.write_all(&frame[..split]).unwrap();
        assert!(matches!(
            receiver.recv_server_msg_until(Instant::now()),
            IpcRecv::Idle
        ));

        let frame = frame.clone();
        let (finish, finished) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let mut sent = split;
            // Timing is under test: chunks arrive inside the per-read timeout for up to one second.
            for _ in 0..100 {
                if finished.recv_timeout(Duration::from_millis(10)).is_ok() {
                    break;
                }
                peer.write_all(&frame[sent..sent + 1]).unwrap();
                sent += 1;
            }
            peer.write_all(&frame[sent..]).unwrap();
        });
        let started = Instant::now();
        let result = receiver.recv_server_msg_until(started + Duration::from_millis(100));
        let elapsed = started.elapsed();
        let _ = finish.send(());
        writer.join().unwrap();
        assert!(matches!(result, IpcRecv::Idle));
        assert!(
            elapsed < Duration::from_secs(2),
            "deadline took {elapsed:?}"
        );
        receiver.set_recv_timeout(None).unwrap();
        assert!(matches!(receiver.recv_server_msg(), IpcRecv::Msg(
            ServerToClientMsg::Render { content: received }, _
        ) if received == content));
        assert!(receiver.recv_server_msg().is_eof());
    }
}

#[test]
fn eof_inside_a_frame_is_terminal_in_both_directions() {
    use std::io::Write;

    for server_message in [false, true] {
        for partial in [&[8u8][..], &[8, 0, 0][..], &[8, 0, 0, 0, 1][..]] {
            let (_guard, name) = new_ipc();
            let listener = bind_listener(&name);
            let mut peer = connect_stream(&name);
            let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
                IpcReceiverWithContext::new(listener.accept().unwrap());
            peer.write_all(partial).unwrap();
            drop(peer);
            if server_message {
                assert!(receiver.recv_server_msg().is_eof());
            } else {
                assert!(receiver.recv_client_msg().is_eof());
            }
            assert!(receiver.recv_client_msg().is_eof());
            assert!(receiver.recv_server_msg().is_eof());
        }
    }
}

/// The other half of the distinction: a frame that arrives but cannot be
/// decoded is *not* a hangup. The stream is still open and may well carry good
/// messages afterwards, so this one is worth retrying.
#[test]
fn a_garbage_frame_reports_malformed_not_eof() {
    use std::io::Write;

    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    let client = std::thread::spawn(move || {
        let mut stream = connect_stream(&name);
        // A well-formed length prefix followed by bytes that are not a
        // ClientToServerMsg. The frame is complete, so this is a decode
        // failure rather than a truncated read.
        let payload = [0xffu8; 8];
        stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .expect("write len failed");
        stream.write_all(&payload).expect("write payload failed");
        stream.flush().expect("flush failed");
        let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);
        sender
            .send_client_msg(ClientToServerMsg::ConnStatus)
            .unwrap();
    });

    let stream = listener.incoming().next().unwrap().expect("accept failed");
    let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(stream);

    assert!(
        matches!(receiver.recv_client_msg(), IpcRecv::Malformed),
        "an undecodable frame on a live socket should be Malformed, not Eof"
    );
    assert!(matches!(
        receiver.recv_client_msg(),
        IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
    ));

    client.join().expect("client thread panicked");
}

/// The third case, and the one that used to hang: a peer that is *there* and
/// simply does not answer.
///
/// This is what a wedged yinz server looks like from the outside — it accepts
/// the connection and then never replies, because the thread that would reply
/// has stopped turning. Without a receive timeout the read below blocks
/// forever, and since every session enumeration path goes through one of these
/// probes (`assert_socket` in `sessions.rs`), one such server hangs `yinz`,
/// `yinz attach` and `yinz ls` for every other session on the machine.
///
/// With the timeout it must come back, and it must come back as `Idle`:
/// nothing arrived, but the stream is open and still in sync, so calling it
/// `Malformed` would be the same conflation `IpcRecv` exists to prevent.
#[test]
fn a_peer_that_accepts_and_never_answers_times_out_as_idle() {
    let (_guard, name) = new_ipc();
    let listener = bind_listener(&name);

    // The accept thread holds its stream until this end is dropped — after the
    // assertions below, or on unwind if one of them fails. A sleep here would be
    // a guess at how long the timed read takes, and a wrong guess on a loaded
    // machine turns this into the Eof case.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let keep_accepting = std::thread::spawn(move || {
        // Accept and then sit on it. Holding the stream is the point: dropping
        // it would close the socket and turn this into the Eof case.
        let _stream = listener.incoming().next().unwrap().expect("accept failed");
        let _ = released.recv();
    });

    let stream = connect_stream(&name);
    stream
        .set_recv_timeout(Some(std::time::Duration::from_millis(100)))
        .expect("failed to set a recv timeout");

    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);
    sender
        .send_client_msg(ClientToServerMsg::ConnStatus)
        .expect("send failed");
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver().unwrap();

    let started_at = std::time::Instant::now();
    let received = receiver.recv_server_msg();
    let waited = started_at.elapsed();

    assert!(
        matches!(received, IpcRecv::Idle),
        "a silent peer should time out as Idle, not be reported as a hangup or as garbage"
    );
    assert!(
        waited < std::time::Duration::from_secs(1),
        "the read should have given up near its 100ms timeout, but it waited {waited:?}"
    );

    drop(release);
    keep_accepting.join().expect("listener thread panicked");
}

/// What the probe makes of each thing a connected socket can hand back.
///
/// The one that matters is `Idle`, the receive timeout: it must read as a
/// server that is *there and not answering*, never as one that is absent.
/// "Absent" is the answer that lets `yinz -s <name>` and `yinz attach
/// --create` spawn a new server on the same socket path, and a new server
/// starts by unlinking that path — stranding the live, wedged one where no
/// client can ever find it again.
#[test]
fn a_probe_timeout_is_an_unresponsive_server_not_an_absent_one() {
    use yinz_types::diagnostics::ErrorContext;
    use {crate::probe::classify_probe_reply, crate::probe::SessionProbe};

    assert_eq!(
        classify_probe_reply(IpcRecv::Msg(
            ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: false,
            },
            ErrorContext::new()
        )),
        SessionProbe::Alive
    );
    assert_eq!(
        classify_probe_reply(IpcRecv::Idle),
        SessionProbe::Unresponsive,
        "a timeout is a server that did not answer, not a missing one"
    );
    // A hangup or garbage after the connect succeeded is a server in some
    // trouble; the safe reading of trouble is still "do not build over it".
    assert_eq!(
        classify_probe_reply(IpcRecv::Eof),
        SessionProbe::Unresponsive
    );
    assert_eq!(
        classify_probe_reply(IpcRecv::Malformed),
        SessionProbe::Unresponsive
    );
}

#[test]
fn only_a_missing_listener_is_an_absent_session() {
    use std::io::ErrorKind;
    use {crate::probe::classify_probe_connect_error, crate::probe::SessionProbe};

    assert_eq!(
        classify_probe_connect_error(ErrorKind::NotFound),
        Ok(SessionProbe::Absent)
    );
    assert_eq!(
        classify_probe_connect_error(ErrorKind::ConnectionRefused),
        Ok(SessionProbe::Absent)
    );
    assert_eq!(
        classify_probe_connect_error(ErrorKind::PermissionDenied),
        Ok(SessionProbe::Denied),
        "a sandbox that refuses the connect says nothing about the server behind it"
    );
    // `Absent` is the answer that lets a new server unlink this socket and bind
    // its own. A connect that timed out came from a server that is *running*
    // and merely too far gone to accept, so it must never land there: unlinking
    // would leave that process alive with no path any client could reach.
    assert_eq!(
        classify_probe_connect_error(ErrorKind::TimedOut),
        Ok(SessionProbe::Unresponsive),
        "a connect that timed out is a wedged server, not an absent one"
    );
    assert_eq!(
        classify_probe_connect_error(ErrorKind::WouldBlock),
        Ok(SessionProbe::Unresponsive)
    );
}

/// A listener whose backlog is full, held open for the life of the guard.
///
/// Built with raw syscalls because the backlog size is the whole point and
/// `ListenerOptions` does not expose it: a default backlog would need hundreds
/// of connections to fill.
#[cfg(unix)]
struct FullBacklog {
    listener_fd: std::os::fd::OwnedFd,
    _pending: Vec<std::os::fd::OwnedFd>,
}

#[cfg(unix)]
impl FullBacklog {
    /// Take one connection off the queue, freeing a backlog slot.
    fn accept_one(&self) {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // Safety: an accept(2) on a listening socket this struct owns; the
        // descriptor it yields is owned and closed here.
        let accepted = unsafe {
            libc::accept(
                self.listener_fd.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert!(accepted >= 0, "accept() failed");
        // Safety: a fresh descriptor from accept(2), owned solely here.
        drop(unsafe { OwnedFd::from_raw_fd(accepted) });
    }
}

/// One condition, two errnos: Linux queues the connect and answers `EAGAIN`,
/// the BSDs refuse it outright. Everything else is a real failure to set the
/// test up.
#[cfg(unix)]
fn backlog_is_full(err: &std::io::Error) -> bool {
    use crate::consts::A_FULL_BACKLOG_IS_REFUSED;
    err.kind() == std::io::ErrorKind::WouldBlock
        || (A_FULL_BACKLOG_IS_REFUSED && err.kind() == std::io::ErrorKind::ConnectionRefused)
}

#[cfg(unix)]
fn listener_with_a_full_backlog(path: &std::path::Path) -> FullBacklog {
    use crate::consts::{nonblocking_unix_stream_socket, unix_socket_address};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // Safety for the block below: ordinary socket syscalls on fds this function
    // creates and owns; every raw fd is wrapped in an `OwnedFd` before it can
    // escape, and every return value is checked.
    unsafe {
        let listener = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        assert!(listener >= 0, "socket() failed");
        let listener_fd = OwnedFd::from_raw_fd(listener);
        let (addr, len) = unix_socket_address(path).expect("socket path too long for sockaddr_un");
        assert_eq!(
            libc::bind(listener, &addr as *const _ as *const libc::sockaddr, len),
            0,
            "bind() failed"
        );
        // The smallest backlog the kernel will take, so a couple of unaccepted
        // connections are enough to fill it.
        assert_eq!(libc::listen(listener, 1), 0, "listen() failed");

        // Fill it. Non-blocking connects, because a blocking one is exactly the
        // call this whole test exists to stop hanging on.
        let mut pending = Vec::new();
        for _ in 0..64 {
            let client_fd = nonblocking_unix_stream_socket().expect("client socket() failed");
            let client = client_fd.as_raw_fd();
            let (addr, len) =
                unix_socket_address(path).expect("socket path too long for sockaddr_un");
            if libc::connect(client, &addr as *const _ as *const libc::sockaddr, len) == 0 {
                pending.push(client_fd);
                continue;
            }
            let err = std::io::Error::last_os_error();
            if backlog_is_full(&err) {
                // The backlog will take no more, which is the state we wanted.
                return FullBacklog {
                    listener_fd,
                    _pending: pending,
                };
            }
            panic!("unexpected error filling the backlog: {err}");
        }
        panic!("the backlog never filled after 64 connections");
    }
}

#[cfg(unix)]
#[test]
fn unix_socket_addresses_use_the_initialized_path_length() {
    use crate::consts::unix_socket_address;
    use std::os::unix::ffi::OsStrExt;

    let path = std::path::Path::new("/tmp/yinz-test.sock");
    let (addr, len) = unix_socket_address(path).unwrap();
    let expected =
        std::mem::offset_of!(libc::sockaddr_un, sun_path) + path.as_os_str().as_bytes().len() + 1;

    assert_eq!(addr.sun_family, libc::AF_UNIX as libc::sa_family_t);
    assert_eq!(len as usize, expected);
    #[cfg(any(
        target_vendor = "apple",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    assert_eq!(addr.sun_len as usize, expected);
}

/// `ipc_connect` builds its socket by hand, and must not hand back one with
/// fewer guarantees than std would have given it: close-on-exec, so the
/// connection is not inherited by every process forked while it is open, and
/// blocking, because every caller reads and writes it as a blocking stream and
/// the non-blocking mode was only ever for the bounded connect.
#[cfg(unix)]
#[test]
fn a_connected_stream_is_close_on_exec_and_blocking() {
    use crate::consts::ipc_connect;
    use std::os::fd::{AsFd, AsRawFd};

    let (_guard, path) = new_ipc();
    let _listener = bind_listener(&path);

    let stream = ipc_connect(&path).expect("connect to a listening socket failed");
    let fd = match &stream {
        interprocess::local_socket::Stream::UdSocket(uds) => uds.as_fd().as_raw_fd(),
    };

    // Safety: fcntl queries on a descriptor the stream above keeps alive.
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(fd_flags >= 0, "F_GETFD failed");
    assert!(
        fd_flags & libc::FD_CLOEXEC != 0,
        "the connected socket must be close-on-exec, as std's would be"
    );
    let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(status_flags >= 0, "F_GETFL failed");
    assert!(
        status_flags & libc::O_NONBLOCK == 0,
        "the connected socket must be blocking; non-blocking mode was only for the connect"
    );
}

/// The regression test for a wedged server hanging every yinz command.
///
/// A server whose listen backlog is full neither accepts a connection nor
/// refuses one on Linux — a blocking `connect` to it simply never returns. That
/// is not covered by `PROBE_TIMEOUT`, which is applied to the stream a
/// *successful* connect hands back, so before `IPC_CONNECT_TIMEOUT` existed one
/// such server hung `yinz`, `yinz ls`, `yinz attach` and `yinz
/// kill-session` for every unrelated session on the machine, since all of them
/// sweep the socket directory. Observed in the wild: a server that had leaked
/// 65,538 connections and 131,085 threads over three days.
///
/// What can be promised at the end of the deadline differs by kernel, and the
/// difference is the kernel's, not this code's. Linux says `EAGAIN`, which means
/// "the backlog is full" and cannot mean anything else, so the probe can name
/// the server unresponsive and its socket is safe. The BSDs say `ECONNREFUSED`,
/// the same errno as a socket file whose server is gone, so after a whole
/// deadline of refusals the honest answer is still "absent" — see
/// `A_FULL_BACKLOG_IS_REFUSED`. Everything else here holds on both: the connect
/// is bounded, and it leaves the socket alone.
#[cfg(unix)]
#[test]
fn a_full_backlog_gives_up_instead_of_hanging_forever() {
    use crate::consts::{ipc_connect, A_FULL_BACKLOG_IS_REFUSED, IPC_CONNECT_TIMEOUT};
    use {crate::probe::classify_probe_connect_error, crate::probe::SessionProbe};

    let (_guard, path) = new_ipc();
    let backlog = listener_with_a_full_backlog(&path);

    let started_at = std::time::Instant::now();
    let result = ipc_connect(&path);
    let waited = started_at.elapsed();

    let err = result.expect_err(
        "connecting to a listener whose backlog is full should not succeed — if the kernel \
         accepted it, this test is no longer exercising the condition it was written for",
    );
    let expected = if A_FULL_BACKLOG_IS_REFUSED {
        SessionProbe::Absent
    } else {
        SessionProbe::Unresponsive
    };
    assert_eq!(
        classify_probe_connect_error(err.kind()),
        Ok(expected),
        "a full backlog is a running server that cannot accept. Where the kernel says so \
         it must read as unresponsive, because reading as absent would let a new server \
         unlink its socket; where the kernel only refuses, absent is all that can be \
         concluded. Got {err:?}"
    );
    if A_FULL_BACKLOG_IS_REFUSED {
        // Without this nothing here would notice the retry loop being deleted:
        // an immediate refusal satisfies every other assertion in this test.
        assert!(
            waited >= IPC_CONNECT_TIMEOUT,
            "a refusal must be retried for the whole {IPC_CONNECT_TIMEOUT:?} before it is \
             believed, but the connect gave up after {waited:?}"
        );
    }
    // Generous: the assertion is "bounded at all", and the old behaviour was
    // unbounded. A tight bound here would only buy flakes on a loaded machine.
    assert!(
        waited < IPC_CONNECT_TIMEOUT * 8,
        "the connect should have given up near its {IPC_CONNECT_TIMEOUT:?} deadline, \
         but it waited {waited:?}"
    );

    // The socket is still a live server's, and nothing about giving up may
    // suggest otherwise.
    assert!(
        path.exists(),
        "the probe that gave up must leave the socket alone"
    );
    drop(backlog);
}

/// The other half of the deadline: it must not fire on a healthy server.
///
/// A backlog can fill for a moment on a busy session — a burst of `yinz
/// action` or hook clients arriving faster than the accept loop drains them —
/// and treating that as a wedged server would be a worse bug than the one the
/// deadline fixes, since `Unresponsive` is what stops a person attaching. So
/// `EAGAIN` is retried rather than reported, and only the deadline ends it.
#[cfg(unix)]
#[test]
fn a_backlog_that_frees_up_is_waited_out_rather_than_failed() {
    use crate::consts::ipc_connect;

    let (_guard, path) = new_ipc();
    let backlog = listener_with_a_full_backlog(&path);

    // Well inside the deadline, and long enough that the first attempt is
    // certain to have hit `EAGAIN` — the point of the test is the retry, not
    // the race to be first.
    let connected = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            backlog.accept_one();
        });
        ipc_connect(&path)
    });

    assert!(
        connected.is_ok(),
        "a backlog that frees up inside the deadline should be waited out, not \
         reported as a wedged server: {:?}",
        connected.err()
    );
}

#[cfg(unix)]
#[test]
fn acknowledged_kill_waits_for_completion_and_rejects_eof_and_cleanup_errors() {
    use crate::ipc::{kill_session_with_completion, ExitReason};
    for reply in [
        Some(ServerToClientMsg::KillSessionComplete { error: None }),
        Some(ServerToClientMsg::KillSessionComplete {
            error: Some("cleanup failed".into()),
        }),
        Some(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        }),
        None,
    ] {
        let (_dir, name) = new_ipc();
        let listener = bind_listener(&name);
        let (received_tx, received_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let success = matches!(
            reply,
            Some(ServerToClientMsg::KillSessionComplete { error: None })
        );
        let server = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(
                    ClientToServerMsg::KillSessionWithCompletion {
                        report_progress: true
                    },
                    _
                )
            ));
            received_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            if let Some(reply) = reply {
                sender
                    .send_server_msg(ServerToClientMsg::UnblockInputThread)
                    .unwrap();
                sender.send_server_msg(reply).unwrap();
            }
        });
        let client = std::thread::spawn(move || kill_session_with_completion(&name));
        received_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            !client.is_finished(),
            "delivery was mistaken for completion"
        );
        release_tx.send(()).unwrap();
        assert_eq!(client.join().unwrap().is_ok(), success);
        server.join().unwrap();
    }
}

#[cfg(unix)]
async fn accept_confirmed_kill(listener: &tokio::net::UnixListener) -> tokio::net::UnixStream {
    use prost::Message;
    use tokio::io::AsyncReadExt;
    let (mut stream, _) = listener.accept().await.unwrap();
    let len = stream.read_u32_le().await.unwrap();
    let mut body = vec![0; len as usize];
    stream.read_exact(&mut body).await.unwrap();
    let request = crate::ipc::ProtoClientToServerMsg::decode(body.as_slice()).unwrap();
    assert_eq!(
        ClientToServerMsg::try_from(request).unwrap(),
        ClientToServerMsg::KillSessionWithCompletion {
            report_progress: true
        }
    );
    stream
}

#[cfg(unix)]
async fn send_kill_reply(stream: &mut tokio::net::UnixStream, reply: ServerToClientMsg) {
    use prost::Message;
    use tokio::io::AsyncWriteExt;
    let reply: crate::ipc::ProtoServerToClientMsg = reply.into();
    let body = reply.encode_to_vec();
    stream.write_u32_le(body.len() as u32).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn kill_progress_extends_the_deadline_but_does_not_complete_shutdown() {
    use std::time::Duration;
    let (_dir, name) = new_ipc();
    let listener = tokio::net::UnixListener::bind(&name).unwrap();
    let client =
        tokio::spawn(async move { crate::ipc::async_kill_session_with_completion(&name).await });
    let mut stream = accept_confirmed_kill(&listener).await;
    // Timing is the behavior under test: two progress replies span the original deadline.
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(
            !client.is_finished(),
            "progress was mistaken for completion or the deadline did not renew"
        );
        send_kill_reply(&mut stream, ServerToClientMsg::KillSessionProgress).await;
    }
    send_kill_reply(
        &mut stream,
        ServerToClientMsg::KillSessionComplete { error: None },
    )
    .await;
    client.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn kill_progress_that_stalls_still_times_out() {
    use std::time::Duration;
    let (_dir, name) = new_ipc();
    let listener = tokio::net::UnixListener::bind(&name).unwrap();
    let client =
        tokio::spawn(async move { crate::ipc::async_kill_session_with_completion(&name).await });
    let mut stream = accept_confirmed_kill(&listener).await;
    send_kill_reply(&mut stream, ServerToClientMsg::KillSessionProgress).await;
    let error = tokio::time::timeout(Duration::from_secs(15), client)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[cfg(unix)]
#[tokio::test]
async fn unrelated_kill_broadcast_does_not_renew_the_deadline() {
    use std::time::Duration;
    let (_dir, name) = new_ipc();
    let listener = tokio::net::UnixListener::bind(&name).unwrap();
    let client =
        tokio::spawn(async move { crate::ipc::async_kill_session_with_completion(&name).await });
    let mut stream = accept_confirmed_kill(&listener).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    send_kill_reply(&mut stream, ServerToClientMsg::UnblockInputThread).await;
    let error = tokio::time::timeout(Duration::from_secs(7), client)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[cfg(unix)]
#[tokio::test]
async fn partial_kill_progress_frame_does_not_renew_the_deadline() {
    use prost::Message;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    let (_dir, name) = new_ipc();
    let listener = tokio::net::UnixListener::bind(&name).unwrap();
    let client =
        tokio::spawn(async move { crate::ipc::async_kill_session_with_completion(&name).await });
    let mut stream = accept_confirmed_kill(&listener).await;
    let reply: crate::ipc::ProtoServerToClientMsg = ServerToClientMsg::KillSessionProgress.into();
    let body = reply.encode_to_vec();
    tokio::time::sleep(Duration::from_secs(6)).await;
    stream.write_u32_le(body.len() as u32).await.unwrap();
    stream.write_all(&body[..1]).await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(7), client)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_kill_replies_do_not_confirm_completion() {
    use tokio::io::AsyncWriteExt;
    for body in [vec![0xff], vec![]] {
        let (_dir, name) = new_ipc();
        let listener = tokio::net::UnixListener::bind(&name).unwrap();
        let client =
            tokio::spawn(
                async move { crate::ipc::async_kill_session_with_completion(&name).await },
            );
        let mut stream = accept_confirmed_kill(&listener).await;
        stream.write_u32_le(body.len() as u32).await.unwrap();
        stream.write_all(&body).await.unwrap();
        assert!(client.await.unwrap().is_err());
    }
}
