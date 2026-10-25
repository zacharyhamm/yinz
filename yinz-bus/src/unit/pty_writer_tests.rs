use super::*;
use crate::os_input_output::AsyncReader;
use crate::os_input_output::{
    ClientTransport, RemotePanes, ServerEnvironment, TerminalIo, TerminalProcesses,
    TerminalSupervision,
};
use crate::thread_bus::Bus;
use crate::PaneId;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use yinz_runtime::channels::SenderWithContext;
use yinz_types::internal_ids::ProcessId;
use yinz_types::style::Palette;

use yinz_types::input::command::RunCommand;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::ServerToClientMsg,
};

use crate::ClientId;

/// Per-terminal write behavior for the mock.
#[derive(Clone)]
enum WriteBehavior {
    /// Accept all bytes (like a healthy terminal).
    AcceptAll,
    /// Accept at most `n` bytes per write call (simulates short writes).
    AcceptAtMost(usize),
    AcceptThenError(usize),
    /// Accept zero bytes (simulates EAGAIN / full buffer).
    AcceptNone,
    /// Return an error (simulates EBADF, EIO, etc).
    Error,
}

/// Log of (terminal_id, bytes) for each successful write, shared with the test thread.
type WriteLog = Arc<Mutex<Vec<(TerminalId, Vec<u8>)>>>;
/// The last resize each terminal was told about, shared with the test thread.
type CachedResizes = Arc<Mutex<Option<BTreeMap<TerminalId, (u16, u16)>>>>;

#[derive(Clone)]
struct MockServerOsApi {
    /// Controls how each terminal behaves on write.
    write_behavior: Arc<Mutex<HashMap<TerminalId, WriteBehavior>>>,
    /// Log of (terminal_id, bytes) for each successful write.
    write_log: WriteLog,
    cached_resizes: CachedResizes,
    applied_resizes: Arc<Mutex<Vec<(TerminalId, u16, u16)>>>,
}

impl MockServerOsApi {
    fn new() -> Self {
        MockServerOsApi {
            write_behavior: Arc::new(Mutex::new(HashMap::new())),
            write_log: Arc::new(Mutex::new(Vec::new())),
            cached_resizes: Arc::new(Mutex::new(None)),
            applied_resizes: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn set_behavior(&self, terminal_id: TerminalId, behavior: WriteBehavior) {
        self.write_behavior
            .lock()
            .unwrap()
            .insert(terminal_id, behavior);
    }
    fn get_written_bytes(&self, terminal_id: TerminalId) -> Vec<u8> {
        self.write_log
            .lock()
            .unwrap()
            .iter()
            .filter(|(tid, _)| *tid == terminal_id)
            .flat_map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

#[test]
fn unsupported_terminal_reservation_returns_an_error() {
    let os = MockServerOsApi::new();
    let processes: &dyn TerminalProcesses = &os;
    assert_eq!(
        processes.reserve_terminal_id().unwrap_err().to_string(),
        "this backend does not support reserving terminal IDs"
    );
}

impl TerminalIo for MockServerOsApi {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        if let Some(cached) = self.cached_resizes.lock().unwrap().as_mut() {
            cached.insert(id, (cols, rows));
        } else {
            self.applied_resizes.lock().unwrap().push((id, cols, rows));
        }
        Ok(())
    }
    fn cache_resizes(&mut self) {
        *self.cached_resizes.lock().unwrap() = Some(BTreeMap::new());
    }
    fn apply_cached_resizes(&mut self) -> Vec<(TerminalId, u16, u16)> {
        let resizes: Vec<_> = self
            .cached_resizes
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, (cols, rows))| (id, cols, rows))
            .collect();
        self.applied_resizes.lock().unwrap().extend(&resizes);
        resizes
    }
    fn write_to_tty_stdin(&self, terminal_id: TerminalId, buf: &[u8]) -> Result<usize> {
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
            WriteBehavior::AcceptThenError(max) => {
                self.set_behavior(terminal_id, WriteBehavior::Error);
                let n = buf.len().min(max);
                self.write_log
                    .lock()
                    .unwrap()
                    .push((terminal_id, buf[..n].to_vec()));
                Ok(n)
            },
            WriteBehavior::AcceptNone => Ok(0),
            WriteBehavior::Error => Err(anyhow::anyhow!(
                "simulated write error for terminal {}",
                terminal_id
            )),
        }
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        Ok(())
    }
}

impl TerminalProcesses for MockServerOsApi {
    fn spawn_terminal(
        &self,
        _terminal_action: yinz_types::input::command::TerminalAction,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn kill(&self, _pid: ProcessId) -> Result<()> {
        Ok(())
    }
    fn force_kill(&self, _pid: ProcessId) -> Result<()> {
        Ok(())
    }
    fn send_sigint(&self, _pid: ProcessId) -> Result<()> {
        Ok(())
    }
    fn send_sigcont(&self, _pid: ProcessId) -> Result<()> {
        Ok(())
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: TerminalId) -> Result<()> {
        Ok(())
    }
}

impl TerminalSupervision for MockServerOsApi {}

impl RemotePanes for MockServerOsApi {}

impl ClientTransport for MockServerOsApi {
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
}

impl ServerEnvironment for MockServerOsApi {
    fn load_palette(&self) -> Palette {
        Palette::default()
    }
    fn get_cwd(&self, _pid: ProcessId) -> Option<PathBuf> {
        None
    }
    fn write_to_file(&mut self, _buf: String, _file: Option<String>) -> Result<()> {
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
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::Write(
            b"hello world".to_vec(),
            crate::TerminalId::new(1),
            None,
        )],
    );

    let written = mock_clone.get_written_bytes(crate::TerminalId::new(1));
    assert_eq!(written, b"hello world");
}

#[test]
fn partial_write_is_buffered_and_drained() {
    let mock = MockServerOsApi::new();
    // Accept only 3 bytes at a time
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::AcceptAtMost(3));
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![PtyWriteInstruction::Write(
            b"abcdefghij".to_vec(),
            crate::TerminalId::new(1),
            None,
        )],
    );

    let written = mock_clone.get_written_bytes(crate::TerminalId::new(1));
    assert_eq!(
        written, b"abcdefghij",
        "all bytes should eventually be written"
    );
}

#[test]
fn eagain_on_one_terminal_does_not_block_another() {
    let mock = MockServerOsApi::new();
    // Terminal 1 is stuck (EAGAIN)
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::AcceptNone);
    // Terminal 2 accepts everything
    mock.set_behavior(crate::TerminalId::new(2), WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"stuck data".to_vec(), crate::TerminalId::new(1), None),
            PtyWriteInstruction::Write(b"good data".to_vec(), crate::TerminalId::new(2), None),
        ],
    );

    // Terminal 2 should have received its bytes despite terminal 1 being stuck
    let written_2 = mock_clone.get_written_bytes(crate::TerminalId::new(2));
    assert_eq!(written_2, b"good data");

    // Terminal 1 data is lost (stuck until Exit, then dropped)
    // — this is expected behavior; no assertion on terminal 1
}

#[test]
fn writes_to_same_terminal_preserve_order() {
    let mock = MockServerOsApi::new();
    // Accept only 4 bytes at a time — forces buffering
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::AcceptAtMost(4));
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"AAAA".to_vec(), crate::TerminalId::new(1), None),
            PtyWriteInstruction::Write(b"BBBB".to_vec(), crate::TerminalId::new(1), None),
            PtyWriteInstruction::Write(b"CCCC".to_vec(), crate::TerminalId::new(1), None),
        ],
    );

    let written = mock_clone.get_written_bytes(crate::TerminalId::new(1));
    assert_eq!(
        written, b"AAAABBBBCCCC",
        "writes must be delivered in order"
    );
}

#[test]
fn memory_cap_drops_buffer_when_exceeded() {
    let mock = MockServerOsApi::new();
    // Terminal is stuck — nothing drains
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::AcceptNone);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);

    // Send enough data to exceed MAX_PENDING_BYTES
    let chunk_size = 1024 * 1024; // 1 MB
    let mut instructions = Vec::new();
    for _ in 0..(MAX_PENDING_BYTES / chunk_size + 1) {
        instructions.push(PtyWriteInstruction::Write(
            vec![0x42; chunk_size],
            crate::TerminalId::new(1),
            None,
        ));
    }
    // After the cap is exceeded, send a write to terminal 2 to verify
    // the writer thread is still functional
    instructions.push(PtyWriteInstruction::Write(
        b"still alive".to_vec(),
        crate::TerminalId::new(2),
        None,
    ));
    mock_clone.set_behavior(crate::TerminalId::new(2), WriteBehavior::AcceptAll);

    run_pty_writer(bus, sender, instructions);

    let written_2 = mock_clone.get_written_bytes(crate::TerminalId::new(2));
    assert_eq!(
        written_2, b"still alive",
        "writer thread should continue functioning after dropping a buffer"
    );
}

#[test]
fn write_error_clears_terminal_queue() {
    let mock = MockServerOsApi::new();
    mock.set_behavior(crate::TerminalId::new(1), WriteBehavior::Error);
    mock.set_behavior(crate::TerminalId::new(2), WriteBehavior::AcceptAll);
    let mock_clone = mock.clone();

    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(b"will fail".to_vec(), crate::TerminalId::new(1), None),
            PtyWriteInstruction::Write(b"also queued".to_vec(), crate::TerminalId::new(1), None),
            PtyWriteInstruction::Write(b"should work".to_vec(), crate::TerminalId::new(2), None),
        ],
    );

    // Terminal 1 should have nothing written (error cleared queue)
    let written_1 = mock_clone.get_written_bytes(crate::TerminalId::new(1));
    assert!(
        written_1.is_empty(),
        "errored terminal should have no written bytes"
    );

    // Terminal 2 should be unaffected
    let written_2 = mock_clone.get_written_bytes(crate::TerminalId::new(2));
    assert_eq!(written_2, b"should work");
}

#[test]
fn paste_barrier_reports_partial_write_failure_overflow_and_shutdown() {
    for (behavior, size, succeeds) in [
        (WriteBehavior::AcceptAtMost(2), 8, true),
        (WriteBehavior::Error, 8, false),
        (WriteBehavior::AcceptThenError(2), 8, false),
        (WriteBehavior::AcceptNone, MAX_PENDING_BYTES + 1, false),
        (WriteBehavior::AcceptNone, 8, false),
    ] {
        let mock = MockServerOsApi::new();
        let terminal = TerminalId::new(1);
        mock.set_behavior(terminal, behavior);
        let (bus, sender) = make_test_bus(mock);
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        run_pty_writer(
            bus,
            sender,
            vec![
                PtyWriteInstruction::Write(vec![b'a'; size], terminal, None),
                PtyWriteInstruction::Write(
                    b"end".to_vec(),
                    terminal,
                    Some(NotificationEnd::new(tx)),
                ),
            ],
        );
        assert_eq!(rx.try_recv().unwrap().error_message.is_none(), succeeds);
    }
}

#[test]
fn overflowing_a_new_paste_after_an_older_barrier_fails_both_barriers() {
    let terminal = TerminalId::new(1);
    let mock = MockServerOsApi::new();
    mock.set_behavior(terminal, WriteBehavior::AcceptNone);
    let (bus, sender) = make_test_bus(mock);
    let (first, mut first_result) = tokio::sync::oneshot::channel();
    let (last, mut last_result) = tokio::sync::oneshot::channel();
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::Write(
                b"old".to_vec(),
                terminal,
                Some(NotificationEnd::new(first)),
            ),
            PtyWriteInstruction::Write(vec![b'x'; MAX_PENDING_BYTES], terminal, None),
            PtyWriteInstruction::Write(Vec::new(), terminal, Some(NotificationEnd::new(last))),
        ],
    );
    assert!(first_result
        .try_recv()
        .unwrap()
        .error_message
        .unwrap()
        .contains("overflow"));
    assert!(last_result
        .try_recv()
        .unwrap()
        .error_message
        .unwrap()
        .contains("overflow"));
}

#[test]
fn pty_writer_applies_direct_and_cached_resizes() {
    let mock = MockServerOsApi::new();
    let applied = mock.applied_resizes.clone();
    let terminal = TerminalId::new(1);
    let (bus, sender) = make_test_bus(mock);
    run_pty_writer(
        bus,
        sender,
        vec![
            PtyWriteInstruction::ResizePty(terminal, 80, 24, None, None),
            PtyWriteInstruction::StartCachingResizes,
            PtyWriteInstruction::ResizePty(terminal, 90, 25, None, None),
            PtyWriteInstruction::ResizePty(terminal, 100, 30, None, None),
            PtyWriteInstruction::ApplyCachedResizes,
        ],
    );
    assert_eq!(
        *applied.lock().unwrap(),
        vec![(terminal, 80, 24), (terminal, 100, 30)]
    );
}
