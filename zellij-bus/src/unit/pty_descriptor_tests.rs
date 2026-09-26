use super::*;
use std::io::Read;
use std::os::unix::net::UnixStream;

#[test]
fn local_transport_and_inflight_writes_survive_reader_teardown() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let master = Arc::new(OwnedFd::from(socket));
    let reader = RawFdAsyncReader::new(master.clone()).unwrap();
    let backend = UnixPtyBackend::with_termios(None).unwrap();
    let terminal = TerminalId::new(0);
    backend
        .terminal_id_to_transport
        .lock()
        .unwrap()
        .insert(terminal, PaneTransport::Local(master));

    drop(reader);
    assert_eq!(backend.write_to_tty_stdin(terminal, b"first").unwrap(), 5);
    let mut bytes = [0; 5];
    peer.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"first");

    // A writer can release the map lock before teardown removes this entry.
    let writing = backend
        .terminal_id_to_transport
        .lock()
        .unwrap()
        .get(&terminal)
        .unwrap()
        .local_fd()
        .unwrap();
    backend.clear_terminal_id(terminal).unwrap();
    assert_eq!(try_write_to_fd(writing.as_raw_fd(), b"final").unwrap(), 5);
    peer.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"final");

    drop(writing);
    assert_eq!(peer.read(&mut bytes).unwrap(), 0);
}
