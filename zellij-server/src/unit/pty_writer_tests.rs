use super::*;
use crate::os_input_output::{AsyncReader, ServerOsApi};
use crate::panes::PaneId;
use crate::thread_bus::Bus;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use zellij_utils::channels::SenderWithContext;
use zellij_utils::data::Palette;

use zellij_utils::input::command::RunCommand;
use zellij_utils::ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg};

use crate::ClientId;

/// Per-terminal write behavior for the mock.
#[derive(Clone)]
enum WriteBehavior {
    /// Accept all bytes (like a healthy terminal).
    AcceptAll,
    /// Accept at most `n` bytes per write call (simulates short writes).
    AcceptAtMost(usize),
    /// Accept zero bytes (simulates EAGAIN / full buffer).
    AcceptNone,
    /// Return an error (simulates EBADF, EIO, etc).
    Error,
}

/// Log of (terminal_id, bytes) for each successful write, shared with the test thread.
type WriteLog = Arc<Mutex<Vec<(u32, Vec<u8>)>>>;
/// The last resize each terminal was told about, shared with the test thread.
type CachedResizes = Arc<Mutex<Option<BTreeMap<u32, (u16, u16)>>>>;

#[derive(Clone)]
struct MockServerOsApi {
    /// Controls how each terminal behaves on write.
    write_behavior: Arc<Mutex<HashMap<u32, WriteBehavior>>>,
    /// Log of (terminal_id, bytes) for each successful write.
    write_log: WriteLog,
    cached_resizes: CachedResizes,
}

impl MockServerOsApi {
    fn new() -> Self {
        MockServerOsApi {
            write_behavior: Arc::new(Mutex::new(HashMap::new())),
            write_log: Arc::new(Mutex::new(Vec::new())),
            cached_resizes: Arc::new(Mutex::new(None)),
        }
    }
    fn set_behavior(&self, terminal_id: u32, behavior: WriteBehavior) {
        self.write_behavior
            .lock()
            .unwrap()
            .insert(terminal_id, behavior);
    }
    fn get_written_bytes(&self, terminal_id: u32) -> Vec<u8> {
        self.write_log
            .lock()
            .unwrap()
            .iter()
            .filter(|(tid, _)| *tid == terminal_id)
            .flat_map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

impl ServerOsApi for MockServerOsApi {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: u32,
        cols: u16,
        rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        if let Some(cached) = self.cached_resizes.lock().unwrap().as_mut() {
            cached.insert(id, (cols, rows));
        }
        Ok(())
    }
    fn cache_resizes(&mut self) {
        *self.cached_resizes.lock().unwrap() = Some(BTreeMap::new());
    }
    fn apply_cached_resizes(&mut self) -> Vec<(u32, u16, u16)> {
        self.cached_resizes
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, (cols, rows))| (id, cols, rows))
            .collect()
    }
    fn spawn_terminal(
        &self,
        _terminal_action: zellij_utils::input::command::TerminalAction,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, terminal_id: u32, buf: &[u8]) -> Result<usize> {
        let behavior = self
            .write_behavior
            .lock()
            .unwrap()
            .get(&terminal_id)
            .cloned()
            .unwrap_or(WriteBehavior::AcceptAll);
        match behavior {
            WriteBehavior::AcceptAll => {
                self.write_log
                    .lock()
                    .unwrap()
                    .push((terminal_id, buf.to_vec()));
                Ok(buf.len())
            },
            WriteBehavior::AcceptAtMost(max) => {
                let n = buf.len().min(max);
                if n > 0 {
                    self.write_log
                        .lock()
                        .unwrap()
                        .push((terminal_id, buf[..n].to_vec()));
                }
                Ok(n)
            },
            WriteBehavior::AcceptNone => Ok(0),
            WriteBehavior::Error => Err(anyhow::anyhow!(
                "simulated write error for terminal {}",
                terminal_id
            )),
        }
    }
    fn tcdrain(&self, _id: u32) -> Result<()> {
        Ok(())
    }
    fn kill(&self, _pid: u32) -> Result<()> {
        Ok(())
    }
    fn force_kill(&self, _pid: u32) -> Result<()> {
        Ok(())
    }
    fn send_sigint(&self, _pid: u32) -> Result<()> {
        Ok(())
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn send_to_client(&self, _client_id: ClientId, _msg: ServerToClientMsg) -> Result<()> {
        Ok(())
    }
    fn new_client(
        &mut self,
        _client_id: ClientId,
        _stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _client_id: ClientId) -> Result<()> {
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        Palette::default()
    }
    fn get_cwd(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
    fn write_to_file(&mut self, _buf: String, _file: Option<String>) -> Result<()> {
        Ok(())
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: u32,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: u32) -> Result<()> {
        Ok(())
    }
}

/// Helper: create a Bus and sender for PtyWriteInstruction with the given mock.
fn make_test_bus(
    mock: MockServerOsApi,
) -> (
    Bus<PtyWriteInstruction>,
    SenderWithContext<PtyWriteInstruction>,
) {
    let (sender, receiver) = channels::unbounded();
    let sender_with_ctx = SenderWithContext::new(sender);
    let bus = Bus::new(
        vec![receiver],
        None,
        None,
        None,
        None,
        None,
        None,
        Some(Box::new(mock)),
    );
    (bus, sender_with_ctx)
}

/// Helper: send instructions and then Exit, run pty_writer_main, return when done.
fn run_pty_writer(
    bus: Bus<PtyWriteInstruction>,
    sender: SenderWithContext<PtyWriteInstruction>,
    instructions: Vec<PtyWriteInstruction>,
) {
    // Send all instructions followed by Exit
    for instruction in instructions {
        sender.send(instruction).unwrap();
    }
    sender.send(PtyWriteInstruction::Exit).unwrap();

    pty_writer_main(bus).unwrap();
}

#[test]
fn full_write_completes_immediately() {
    let mock = MockServerOsApi::new();
    mock.set_behavior(1, WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::Write(b"hello world".to_vec(), 1, None)],
    );

    let written = mock_clone.get_written_bytes(1);
    assert_eq!(written, b"hello world");
}

#[test]
fn partial_write_is_buffered_and_drained() {
    let mock = MockServerOsApi::new();
    // Accept only 3 bytes at a time
    mock.set_behavior(1, WriteBehavior::AcceptAtMost(3));
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::Write(b"abcdefghij".to_vec(), 1, None)],
    );

    let written = mock_clone.get_written_bytes(1);
    assert_eq!(
        written, b"abcdefghij",
        "all bytes should eventually be written"
    );
}

#[test]
fn eagain_on_one_terminal_does_not_block_another() {
    let mock = MockServerOsApi::new();
    // Terminal 1 is stuck (EAGAIN)
    mock.set_behavior(1, WriteBehavior::AcceptNone);
    // Terminal 2 accepts everything
    mock.set_behavior(2, WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"stuck data".to_vec(), 1, None),
            PtyWriteInstruction::Write(b"good data".to_vec(), 2, None),
        ],
    );

    // Terminal 2 should have received its bytes despite terminal 1 being stuck
    let written_2 = mock_clone.get_written_bytes(2);
    assert_eq!(written_2, b"good data");

    // Terminal 1 data is lost (stuck until Exit, then dropped)
    // — this is expected behavior; no assertion on terminal 1
}

#[test]
fn writes_to_same_terminal_preserve_order() {
    let mock = MockServerOsApi::new();
    // Accept only 4 bytes at a time — forces buffering
    mock.set_behavior(1, WriteBehavior::AcceptAtMost(4));
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"AAAA".to_vec(), 1, None),
            PtyWriteInstruction::Write(b"BBBB".to_vec(), 1, None),
            PtyWriteInstruction::Write(b"CCCC".to_vec(), 1, None),
        ],
    );

    let written = mock_clone.get_written_bytes(1);
    assert_eq!(
        written, b"AAAABBBBCCCC",
        "writes must be delivered in order"
    );
}

#[test]
fn memory_cap_drops_buffer_when_exceeded() {
    let mock = MockServerOsApi::new();
    // Terminal is stuck — nothing drains
    mock.set_behavior(1, WriteBehavior::AcceptNone);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);

    // Send enough data to exceed MAX_PENDING_BYTES
    let chunk_size = 1024 * 1024; // 1 MB
    let mut instructions = Vec::new();
    for _ in 0..(MAX_PENDING_BYTES / chunk_size + 1) {
        instructions.push(PtyWriteInstruction::Write(vec![0x42; chunk_size], 1, None));
    }
    // After the cap is exceeded, send a write to terminal 2 to verify
    // the writer thread is still functional
    instructions.push(PtyWriteInstruction::Write(b"still alive".to_vec(), 2, None));
    mock_clone.set_behavior(2, WriteBehavior::AcceptAll);

    run_pty_writer(bus, sender, instructions);

    let written_2 = mock_clone.get_written_bytes(2);
    assert_eq!(
        written_2, b"still alive",
        "writer thread should continue functioning after dropping a buffer"
    );
}

#[test]
fn write_error_clears_terminal_queue() {
    let mock = MockServerOsApi::new();
    mock.set_behavior(1, WriteBehavior::Error);
    mock.set_behavior(2, WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"will fail".to_vec(), 1, None),
            PtyWriteInstruction::Write(b"also queued".to_vec(), 1, None),
            PtyWriteInstruction::Write(b"should work".to_vec(), 2, None),
        ],
    );

    // Terminal 1 should have nothing written (error cleared queue)
    let written_1 = mock_clone.get_written_bytes(1);
    assert!(
        written_1.is_empty(),
        "errored terminal should have no written bytes"
    );

    // Terminal 2 should be unaffected
    let written_2 = mock_clone.get_written_bytes(2);
    assert_eq!(written_2, b"should work");
}

#[test]
fn pty_writer_records_successful_input() {
    // Mock with AcceptAtMost(2) to cause two partial writes of "abcd" -> "ab" + "cd"
    let mock = MockServerOsApi::new();
    mock.set_behavior(1, WriteBehavior::AcceptAtMost(2));
    let mock_clone = mock.clone();

    // Create recorder channel
    let (rec_sender, rec_receiver) = crossbeam::channel::unbounded();
    let handle = crate::session_recorder::SessionRecorderHandle {
        sender: rec_sender,
        started_at: std::time::Instant::now(),
        capture_input: true,
        enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        dropped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };

    let (bus_inner, sender) = make_test_bus(mock_clone);
    // inject recorder handle into bus
    let mut bus = bus_inner;
    bus.senders.session_recorder = Some(handle);

    // Single write of "abcd" should be drained as two input events
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::Write(b"abcd".to_vec(), 1, None)],
    );

    // Collect recorded input events
    let mut input_events = Vec::new();
    while let Ok(instr) = rec_receiver.try_recv() {
        if let crate::session_recorder::RecorderInstruction::Events(ev) = instr {
            for e in ev {
                if e.kind == zellij_utils::session_recording::EventKind::Input {
                    input_events.push(e);
                }
            }
        }
    }
    assert_eq!(
        input_events.len(),
        2,
        "expected two input events for partial writes"
    );
    assert_eq!(input_events[0].data, b"ab");
    assert_eq!(input_events[1].data, b"cd");
    let concat: Vec<u8> = input_events.iter().flat_map(|e| e.data.clone()).collect();
    assert_eq!(concat, b"abcd");
}

#[test]
fn pty_writer_records_successful_resize() {
    let mock = MockServerOsApi::new();
    // First case: success should produce resize event
    let (rec_sender, rec_receiver) = crossbeam::channel::unbounded();
    let handle = crate::session_recorder::SessionRecorderHandle {
        sender: rec_sender,
        started_at: std::time::Instant::now(),
        capture_input: false,
        enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        dropped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let (bus_inner, sender) = make_test_bus(mock.clone());
    let mut bus = bus_inner;
    bus.senders.session_recorder = Some(handle);
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::ResizePty(1, 80, 24, None, None)],
    );
    let mut resize_events = Vec::new();
    while let Ok(instr) = rec_receiver.try_recv() {
        if let crate::session_recorder::RecorderInstruction::Events(ev) = instr {
            for e in ev {
                if e.kind == zellij_utils::session_recording::EventKind::Resize {
                    resize_events.push(e);
                }
            }
        }
    }
    assert_eq!(
        resize_events.len(),
        1,
        "success resize should produce one event"
    );
    // verify resize data encodes cols/rows via RecordedEvent::resize logic (format "80x24")
    assert_eq!(resize_events[0].data, "80x24".to_string().into_bytes());
    assert_eq!(resize_events[0].terminal_id, 1);

    let (rec_sender, rec_receiver) = crossbeam::channel::unbounded();
    let handle = crate::session_recorder::SessionRecorderHandle {
        sender: rec_sender,
        started_at: std::time::Instant::now(),
        capture_input: false,
        enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        dropped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let (mut bus, sender) = make_test_bus(MockServerOsApi::new());
    bus.senders.session_recorder = Some(handle);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::StartCachingResizes,
            PtyWriteInstruction::ResizePty(1, 80, 24, None, None),
            PtyWriteInstruction::ResizePty(1, 100, 30, None, None),
            PtyWriteInstruction::ApplyCachedResizes,
        ],
    );
    let resize_events: Vec<_> = rec_receiver
        .try_iter()
        .flat_map(|instruction| match instruction {
            crate::session_recorder::RecorderInstruction::Events(events) => events,
            _ => Vec::new(),
        })
        .filter(|event| event.kind == zellij_utils::session_recording::EventKind::Resize)
        .collect();
    assert_eq!(resize_events.len(), 1);
    assert_eq!(resize_events[0].data, b"100x30");

    // Second case: error should produce no resize event
    // Use a mock that errors on resize by overriding ServerOsApi impl: we need a variant that fails
    // Since MockServerOsApi always returns Ok for resize, simulate error via a custom mock
    struct FailingResizeMock {
        inner: MockServerOsApi,
    }
    impl Clone for FailingResizeMock {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }
    impl crate::os_input_output::ServerOsApi for FailingResizeMock {
        fn set_terminal_size_using_terminal_id(
            &self,
            _id: u32,
            _cols: u16,
            _rows: u16,
            _w: Option<u16>,
            _h: Option<u16>,
        ) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("simulated resize error"))
        }
        fn spawn_terminal(
            &self,
            _a: zellij_utils::input::command::TerminalAction,
            _b: Box<
                dyn Fn(crate::panes::PaneId, Option<i32>, zellij_utils::input::command::RunCommand)
                    + Send,
            >,
            _c: Option<std::path::PathBuf>,
        ) -> anyhow::Result<(
            u32,
            Box<dyn crate::os_input_output::AsyncReader>,
            Option<u32>,
        )> {
            unimplemented!()
        }
        fn write_to_tty_stdin(&self, _id: u32, _buf: &[u8]) -> anyhow::Result<usize> {
            Ok(0)
        }
        fn tcdrain(&self, _id: u32) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill(&self, _pid: u32) -> anyhow::Result<()> {
            Ok(())
        }
        fn force_kill(&self, _pid: u32) -> anyhow::Result<()> {
            Ok(())
        }
        fn send_sigint(&self, _pid: u32) -> anyhow::Result<()> {
            Ok(())
        }
        fn box_clone(&self) -> Box<dyn crate::os_input_output::ServerOsApi> {
            Box::new(self.clone())
        }
        fn send_to_client(
            &self,
            _id: crate::ClientId,
            _msg: zellij_utils::ipc::ServerToClientMsg,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn new_client(
            &mut self,
            _id: crate::ClientId,
            _s: interprocess::local_socket::Stream,
        ) -> anyhow::Result<
            zellij_utils::ipc::IpcReceiverWithContext<zellij_utils::ipc::ClientToServerMsg>,
        > {
            unimplemented!()
        }
        fn remove_client(&mut self, _id: crate::ClientId) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_palette(&self) -> zellij_utils::data::Palette {
            zellij_utils::data::Palette::default()
        }
        fn get_cwd(&self, _pid: u32) -> Option<std::path::PathBuf> {
            None
        }
        fn write_to_file(&mut self, _buf: String, _file: Option<String>) -> anyhow::Result<()> {
            Ok(())
        }
        fn re_run_command_in_terminal(
            &self,
            _id: u32,
            _c: zellij_utils::input::command::RunCommand,
            _q: Box<
                dyn Fn(crate::panes::PaneId, Option<i32>, zellij_utils::input::command::RunCommand)
                    + Send,
            >,
        ) -> anyhow::Result<(Box<dyn crate::os_input_output::AsyncReader>, Option<u32>)> {
            unimplemented!()
        }
        fn clear_terminal_id(&self, _id: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let failing_mock = FailingResizeMock {
        inner: MockServerOsApi::new(),
    };
    let (rec_sender2, rec_receiver2) = crossbeam::channel::unbounded();
    let handle2 = crate::session_recorder::SessionRecorderHandle {
        sender: rec_sender2,
        started_at: std::time::Instant::now(),
        capture_input: false,
        enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        dropped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let (sender2, receiver2) = zellij_utils::channels::unbounded();
    let sender2_ctx = zellij_utils::channels::SenderWithContext::new(sender2);
    let bus2 = crate::thread_bus::Bus::new(
        vec![receiver2],
        None,
        None,
        None,
        None,
        None,
        None,
        Some(Box::new(failing_mock)),
    );
    let mut bus2 = bus2;
    bus2.senders.session_recorder = Some(handle2);
    // send resize then exit
    sender2_ctx
        .send(PtyWriteInstruction::ResizePty(2, 100, 30, None, None))
        .unwrap();
    sender2_ctx.send(PtyWriteInstruction::Exit).unwrap();
    crate::pty_writer::pty_writer_main(bus2).unwrap();
    let mut resize_events2 = Vec::new();
    while let Ok(instr) = rec_receiver2.try_recv() {
        if let crate::session_recorder::RecorderInstruction::Events(ev) = instr {
            for e in ev {
                if e.kind == zellij_utils::session_recording::EventKind::Resize {
                    resize_events2.push(e);
                }
            }
        }
    }
    assert_eq!(
        resize_events2.len(),
        0,
        "failed resize should produce no event"
    );
}
