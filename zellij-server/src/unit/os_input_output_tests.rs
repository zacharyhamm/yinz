use super::*;
use zellij_utils::input::command::RunCommand;

fn make_server() -> ServerOsInputOutput {
    get_server_os_input().expect("failed to create server os input")
}

#[allow(dead_code)]
fn long_running_cmd() -> Command {
    let mut cmd = Command::new("sleep");
    cmd.arg("60");
    cmd
}

#[allow(dead_code)]
fn echo_cmd(msg: &str) -> Command {
    let mut cmd = Command::new("echo");
    cmd.arg(msg);
    cmd
}

#[allow(dead_code)]
fn stdin_reader_cmd() -> Command {
    let mut cmd = Command::new("cat");
    cmd.stdin(std::process::Stdio::piped());
    cmd
}

#[test]
fn get_cwd() {
    let server = make_server();

    let pid = std::process::id();
    assert!(
        server.get_cwd(pid).is_some(),
        "Get current working directory from PID {}",
        pid
    );
}

// --- Signal delivery tests ---

#[test]
fn kill_sends_sighup_to_process() {
    let mut child = long_running_cmd()
        .spawn()
        .expect("failed to spawn long-running process");
    let pid = child.id();

    let server = make_server();

    server.kill(pid).expect("kill should succeed");

    // Give the signal time to be delivered, then reap the child so it can't linger as a zombie.
    // The second kill is belt and braces: if the signal under test somehow left the process
    // running, `wait` would block this test forever.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn force_kill_sends_sigkill_to_process() {
    let mut child = long_running_cmd()
        .spawn()
        .expect("failed to spawn long-running process");
    let pid = child.id();

    let server = make_server();

    server.force_kill(pid).expect("force_kill should succeed");

    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn send_sigint_to_process() {
    let mut child = stdin_reader_cmd()
        .spawn()
        .expect("failed to spawn stdin-reader process");
    let pid = child.id();

    let server = make_server();

    server.send_sigint(pid).expect("send_sigint should succeed");

    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn spawn_and_read_output() {
    use crate::panes::PaneId;
    use zellij_utils::input::command::TerminalAction;

    let server = make_server();
    let test_message = "hello_zellij_test";
    std::env::set_var(
        crate::pty_host::PANE_REF_ENV_KEY,
        "an-outer-panes-reference",
    );

    let cmd = RunCommand {
        command: PathBuf::from("sh"),
        args: vec![
            "-c".to_owned(),
            format!("echo {test_message} ref=[$ZELLIJ_PANE_REF]"),
        ],
        ..Default::default()
    };
    let action = TerminalAction::RunCommand(cmd);
    let quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send> =
        Box::new(|_pane_id, _exit_status, _run_command| {});

    let (_terminal_id, mut reader, _child_pid) = server
        .spawn_terminal(action, quit_cb, None)
        .expect("spawn_terminal should succeed");

    // Read output from the spawned terminal
    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        loop {
            if std::time::Instant::now() > deadline {
                break;
            }
            match tokio::time::timeout(std::time::Duration::from_millis(500), reader.read(&mut buf))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    output.extend_from_slice(&buf[..n]);
                    let s = String::from_utf8_lossy(&output);
                    if s.contains(test_message) {
                        break;
                    }
                },
                Ok(Err(_)) => break,
                Err(_) => {
                    // timeout — check if we already have enough
                    let s = String::from_utf8_lossy(&output);
                    if s.contains(test_message) {
                        break;
                    }
                },
            }
        }
    });

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains(test_message),
        "expected output to contain '{}', got: '{}'",
        test_message,
        output_str
    );
    assert!(
        output_str.contains("ref=[]"),
        "an in-process pane reused an outer pane's supervisor: {output_str:?}"
    );
    std::env::remove_var(crate::pty_host::PANE_REF_ENV_KEY);
}

#[cfg(unix)]
#[test]
fn a_local_rerun_terminates_the_previous_remote_transport() {
    use crate::pty_host::remote::{
        protocol::{read_frame, ServerToBrokerMsg},
        RemoteConn,
    };
    use std::os::unix::net::UnixStream;

    let server = make_server();
    let (write_socket, mut broker_peer) = UnixStream::pair().unwrap();
    let (frame_tx, frame_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = frame_tx.send(read_frame(&mut broker_peer));
    });
    let (read_socket, _reader_peer) = UnixStream::pair().unwrap();
    let remote_command = RunCommand {
        command: PathBuf::from("remote-step"),
        remote_host: Some("build-box".to_owned()),
        ..Default::default()
    };
    let connection = Arc::new(
        RemoteConn::new(
            write_socket,
            "build-box".to_owned(),
            "token".to_owned(),
            remote_command.clone(),
            None,
        )
        .unwrap(),
    );
    let _remote_reader = server
        .pty_backend
        .register_remote_terminal(
            7,
            connection,
            read_socket,
            Box::new(|_, _, _| {}),
            remote_command,
        )
        .unwrap();

    let (_local_reader, child_pid) = server
        .re_run_command_in_terminal(
            7,
            RunCommand {
                command: PathBuf::from("sleep"),
                args: vec!["60".to_owned()],
                ..Default::default()
            },
            Box::new(|_, _, _| {}),
        )
        .unwrap();

    let frame = frame_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("the old broker did not receive a termination frame")
        .unwrap()
        .unwrap();
    assert_eq!(
        frame.decode_control::<ServerToBrokerMsg>().unwrap(),
        ServerToBrokerMsg::Terminate
    );
    if let Some(child_pid) = child_pid {
        server.force_kill(child_pid).unwrap();
    }
    server.clear_terminal_id(7).unwrap();
}

#[cfg(unix)]
#[test]
fn tcgetpgrp_returns_foreground_group() {
    use crate::panes::PaneId;
    use zellij_utils::input::command::TerminalAction;

    let server = make_server();

    // `login_tty` in the child makes the spawned command the controlling terminal's
    // foreground process group, so tcgetpgrp(master) should return its pid
    let cmd = RunCommand {
        command: PathBuf::from("sleep"),
        args: vec!["60".to_string()],
        ..Default::default()
    };
    let quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send> = Box::new(|_, _, _| {});
    let (terminal_id, _reader, child_pid) = server
        .spawn_terminal(TerminalAction::RunCommand(cmd), quit_cb, None)
        .expect("spawn_terminal should succeed");

    // poll (bounded to ~2s) for the child to finish setting up its controlling terminal
    let mut fpgid = None;
    for _ in 0..100 {
        fpgid = server.pty_backend.tcgetpgrp(terminal_id);
        let ready = match (fpgid, child_pid) {
            (Some(p), Some(child_pid)) => p > 0 && p as u32 == child_pid,
            (Some(p), None) => p > 0,
            _ => false,
        };
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        matches!(fpgid, Some(p) if p > 0),
        "tcgetpgrp should return the foreground process group, got {:?}",
        fpgid
    );
    if let (Some(fpgid), Some(child_pid)) = (fpgid, child_pid) {
        assert_eq!(
            fpgid as u32, child_pid,
            "the spawned command leads its own foreground group"
        );
    }

    // An unknown terminal id has no fd and must not panic.
    assert_eq!(server.pty_backend.tcgetpgrp(u32::MAX), None);

    if let Some(child_pid) = child_pid {
        let _ = server.force_kill(child_pid);
    }
}

mod coalescing_send_queue {
    use super::*;
    use zellij_utils::data::PaneId as DataPaneId;

    fn render(content: &str) -> ServerToClientMsg {
        ServerToClientMsg::Render {
            content: content.to_owned(),
        }
    }

    fn control(line: &str) -> ServerToClientMsg {
        ServerToClientMsg::Log {
            lines: vec![line.to_owned()],
        }
    }

    fn pane_render(pane_id: DataPaneId, content: &str) -> ServerToClientMsg {
        ServerToClientMsg::PaneRenderUpdate {
            pane_id,
            viewport: vec![content.to_owned()],
            scrollback: None,
            is_initial: false,
        }
    }

    #[test]
    fn renders_coalesce_to_the_newest() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(queue.push(render("first")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(render("second")),
            SendQueuePush::CoalescedRender
        ));
        assert!(matches!(
            queue.push(render("third")),
            SendQueuePush::CoalescedRender
        ));
        assert_eq!(queue.len(), 1);
        queue.close();
        assert_eq!(queue.pop(), Some(render("third")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn control_messages_stay_fifo_and_precede_the_render() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(queue.push(render("stale")), SendQueuePush::Queued));
        assert!(matches!(queue.push(control("a")), SendQueuePush::Queued));
        assert!(matches!(queue.push(control("b")), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(control("a")));
        assert_eq!(queue.pop(), Some(control("b")));
        assert_eq!(queue.pop(), Some(render("stale")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn control_queue_fills_while_renders_still_coalesce() {
        let queue = CoalescingSendQueue::new();
        for i in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert!(matches!(
                queue.push(control(&i.to_string())),
                SendQueuePush::Queued
            ));
        }
        assert!(matches!(
            queue.push(control("overflow")),
            SendQueuePush::ControlQueueFull
        ));
        assert!(matches!(queue.push(render("r1")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(render("r2")),
            SendQueuePush::CoalescedRender
        ));
    }

    #[test]
    fn close_drains_before_ending() {
        let queue = CoalescingSendQueue::new();
        queue.push(control("queued-before-close"));
        queue.push(render("render-before-close"));
        queue.close();
        assert_eq!(queue.pop(), Some(control("queued-before-close")));
        assert_eq!(queue.pop(), Some(render("render-before-close")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn blocking_pop_wakes_on_push() {
        let queue = Arc::new(CoalescingSendQueue::new());
        let popper = {
            let queue = queue.clone();
            std::thread::spawn(move || queue.pop())
        };
        // give the popper a moment to block
        std::thread::sleep(std::time::Duration::from_millis(50));
        queue.push(render("wake up"));
        assert_eq!(popper.join().unwrap(), Some(render("wake up")));
    }

    #[test]
    fn pop_returns_none_when_closed_while_blocked() {
        let queue = Arc::new(CoalescingSendQueue::new());
        let popper = {
            let queue = queue.clone();
            std::thread::spawn(move || queue.pop())
        };
        // give the popper a moment to block
        std::thread::sleep(std::time::Duration::from_millis(50));
        queue.close();
        assert_eq!(popper.join().unwrap(), None);
    }

    #[test]
    fn push_after_close_is_dropped() {
        // late sends after the writer is gone pretend to succeed so senders
        // don't error, but nothing is queued
        let queue = CoalescingSendQueue::new();
        queue.close();
        assert!(matches!(queue.push(control("late")), SendQueuePush::Queued));
        assert!(matches!(queue.push(render("late")), SendQueuePush::Queued));
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn pane_renders_coalesce_per_pane() {
        let queue = CoalescingSendQueue::new();
        let pane_a = DataPaneId::Terminal(1);
        let pane_b = DataPaneId::Terminal(2);
        assert!(matches!(
            queue.push(pane_render(pane_a, "a1")),
            SendQueuePush::Queued
        ));
        assert!(matches!(
            queue.push(pane_render(pane_b, "b1")),
            SendQueuePush::Queued
        ));
        assert!(matches!(
            queue.push(pane_render(pane_a, "a2")),
            SendQueuePush::CoalescedRender
        ));
        assert_eq!(queue.len(), 2);
        queue.close();
        assert_eq!(queue.pop(), Some(pane_render(pane_a, "a2")));
        assert_eq!(queue.pop(), Some(pane_render(pane_b, "b1")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn initial_and_scrollback_pane_renders_stay_fifo() {
        // initial snapshots and scrollback responses are one-offs, not a stream
        // of repaints - they take the FIFO path, so they precede coalesced
        // updates for the same pane
        let queue = CoalescingSendQueue::new();
        let pane = DataPaneId::Terminal(1);
        let initial = ServerToClientMsg::PaneRenderUpdate {
            pane_id: pane,
            viewport: vec!["initial".to_owned()],
            scrollback: None,
            is_initial: true,
        };
        let with_scrollback = ServerToClientMsg::PaneRenderUpdate {
            pane_id: pane,
            viewport: vec!["current".to_owned()],
            scrollback: Some(vec!["older".to_owned()]),
            is_initial: false,
        };
        assert!(matches!(
            queue.push(pane_render(pane, "coalesced")),
            SendQueuePush::Queued
        ));
        assert!(matches!(queue.push(initial.clone()), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(with_scrollback.clone()),
            SendQueuePush::Queued
        ));
        queue.close();
        assert_eq!(queue.pop(), Some(initial));
        assert_eq!(queue.pop(), Some(with_scrollback));
        assert_eq!(queue.pop(), Some(pane_render(pane, "coalesced")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn drain_order_is_control_then_pane_renders_then_full_render() {
        let queue = CoalescingSendQueue::new();
        let pane = DataPaneId::Plugin(7);
        assert!(matches!(queue.push(render("full")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(pane_render(pane, "pane")),
            SendQueuePush::Queued
        ));
        assert!(matches!(queue.push(control("ctl")), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(control("ctl")));
        assert_eq!(queue.pop(), Some(pane_render(pane, "pane")));
        assert_eq!(queue.pop(), Some(render("full")));
        assert_eq!(queue.pop(), None);
    }

    // ---- property-based tests (hegel) ----

    use hegel::generators;

    /// Small pane pool on purpose: coalescing only shows up when the same pane
    /// keeps rendering.
    #[hegel::composite]
    fn any_data_pane_id(tc: &hegel::TestCase) -> DataPaneId {
        let n = tc.draw(generators::integers::<u32>().max_value(2));
        if tc.draw(generators::booleans()) {
            DataPaneId::Terminal(n)
        } else {
            DataPaneId::Plugin(n)
        }
    }

    struct QueueModel {
        real: CoalescingSendQueue,
        control: VecDeque<ServerToClientMsg>,
        pane_renders: BTreeMap<DataPaneId, ServerToClientMsg>,
        full_render: Option<ServerToClientMsg>,
        closed: bool,
        seq: u32,
    }

    impl QueueModel {
        fn new() -> Self {
            QueueModel {
                real: CoalescingSendQueue::new(),
                control: VecDeque::new(),
                pane_renders: BTreeMap::new(),
                full_render: None,
                closed: false,
                seq: 0,
            }
        }
        fn next_seq(&mut self) -> String {
            self.seq += 1;
            self.seq.to_string()
        }
    }

    #[hegel::state_machine]
    impl QueueModel {
        #[rule]
        fn push_control(&mut self, _tc: hegel::TestCase) {
            let msg = control(&self.next_seq());
            assert!(matches!(self.real.push(msg.clone()), SendQueuePush::Queued));
            if !self.closed {
                self.control.push_back(msg);
            }
        }

        #[rule]
        fn push_full_render(&mut self, _tc: hegel::TestCase) {
            let msg = render(&self.next_seq());
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else {
                assert_eq!(
                    matches!(outcome, SendQueuePush::CoalescedRender),
                    self.full_render.is_some(),
                );
                self.full_render = Some(msg);
            }
        }

        #[rule]
        fn push_pane_render(&mut self, tc: hegel::TestCase) {
            let pane_id = tc.draw(any_data_pane_id());
            let msg = pane_render(pane_id, &self.next_seq());
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else {
                assert_eq!(
                    matches!(outcome, SendQueuePush::CoalescedRender),
                    self.pane_renders.contains_key(&pane_id),
                );
                self.pane_renders.insert(pane_id, msg);
            }
        }

        #[rule]
        fn push_initial_pane_render(&mut self, tc: hegel::TestCase) {
            // initial snapshots take the FIFO path like any control message
            let pane_id = tc.draw(any_data_pane_id());
            let msg = ServerToClientMsg::PaneRenderUpdate {
                pane_id,
                viewport: vec![self.next_seq()],
                scrollback: None,
                is_initial: true,
            };
            assert!(matches!(self.real.push(msg.clone()), SendQueuePush::Queued));
            if !self.closed {
                self.control.push_back(msg);
            }
        }

        #[rule]
        fn pop(&mut self, tc: hegel::TestCase) {
            let model_has_message = !self.control.is_empty()
                || !self.pane_renders.is_empty()
                || self.full_render.is_some();
            // pop blocks on an open empty queue; only pop when it can't block
            tc.assume(model_has_message || self.closed);
            let expected = if let Some(msg) = self.control.pop_front() {
                Some(msg)
            } else if let Some((_, msg)) = self.pane_renders.pop_first() {
                Some(msg)
            } else {
                self.full_render.take()
            };
            assert_eq!(self.real.pop(), expected);
        }

        #[rule]
        fn close(&mut self, _tc: hegel::TestCase) {
            self.real.close();
            self.closed = true;
        }

        #[invariant]
        fn depth_agrees_with_the_model(&self, _: hegel::TestCase) {
            assert_eq!(
                self.real.len(),
                self.control.len()
                    + self.pane_renders.len()
                    + usize::from(self.full_render.is_some()),
            );
        }
    }

    #[hegel::test]
    fn queue_agrees_with_a_sequential_model(tc: hegel::TestCase) {
        hegel::stateful::run(QueueModel::new(), tc);
    }
}

/// The wait that keeps a queued `Exit` from dying with the process.
///
/// `send_to_client` only queues; a detached writer thread does the write. Every
/// server shutdown path sends clients an `Exit` and then returns from `main`, so
/// without a wait in between the client can see a bare hangup instead of being
/// told why the session ended — which is exactly what "Received empty unknown
/// from server" was.
mod client_writer_flush {
    use super::super::{ClientSender, LiveWriters};
    use interprocess::local_socket::{prelude::*, ListenerOptions};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zellij_utils::ipc::{
        ClientToServerMsg, ExitReason, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext,
        ServerToClientMsg,
    };

    fn socket_pair() -> (
        tempfile::TempDir,
        interprocess::local_socket::Stream,
        interprocess::local_socket::Stream,
    ) {
        use interprocess::local_socket::{GenericFilePath, Stream as LocalSocketStream};
        let dir = tempfile::TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("flush-test.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .expect("bind failed");
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .expect("connect failed");
        let server = listener.incoming().next().unwrap().expect("accept failed");
        (dir, server, client)
    }

    #[test]
    fn a_queued_exit_reaches_the_socket_before_the_wait_returns() {
        let (_guard, server_end, client_end) = socket_pair();
        let live_writers = Arc::new(LiveWriters::default());
        let server_fd = {
            use std::os::fd::AsFd;
            match &server_end {
                interprocess::local_socket::Stream::UdSocket(uds) => {
                    uds.as_fd().try_clone_to_owned().expect("dup failed")
                },
            }
        };
        let sender = ClientSender::new(
            1,
            IpcSenderWithContext::<ServerToClientMsg>::new(server_end),
            live_writers.clone(),
            server_fd,
        );

        // What every break in the server main loop does on its way out.
        sender
            .send_or_buffer(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            })
            .expect("queueing the exit failed");
        sender.close();

        assert!(
            live_writers.wait_until_drained(Duration::from_secs(5)),
            "the writer should finish draining well inside the deadline"
        );

        // Only now would the process exit. The bytes must already be there.
        let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
            IpcReceiverWithContext::new(client_end);
        match receiver.recv_server_msg() {
            IpcRecv::Msg(
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Normal,
                },
                _,
            ) => {},
            other => panic!(
                "the client should have been told why the session ended, got {:?}",
                match other {
                    IpcRecv::Msg(msg, _) => format!("{msg:?}"),
                    IpcRecv::Eof => "a hangup".to_string(),
                    IpcRecv::Malformed => "a malformed message".to_string(),
                    IpcRecv::Idle => "nothing".to_string(),
                }
            ),
        }
    }

    #[test]
    fn waiting_gives_up_rather_than_holding_the_process_open() {
        let live_writers = LiveWriters::default();
        // A writer that never finishes - a client whose socket buffer is full
        // and which has stopped reading looks exactly like this.
        live_writers.register();
        let started = std::time::Instant::now();
        assert!(
            !live_writers.wait_until_drained(Duration::from_millis(50)),
            "an undrained writer should be reported as such"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the wait must be bounded, it took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn draining_is_reported_once_every_writer_is_done() {
        let live_writers = LiveWriters::default();
        assert!(
            live_writers.wait_until_drained(Duration::from_millis(50)),
            "no writers means nothing to wait for"
        );
        live_writers.register();
        live_writers.register();
        live_writers.deregister();
        assert!(
            !live_writers.wait_until_drained(Duration::from_millis(50)),
            "one writer is still going"
        );
        live_writers.deregister();
        assert!(
            live_writers.wait_until_drained(Duration::from_millis(50)),
            "both writers are done"
        );
    }

    /// The router thread must not outlive the client the server has removed.
    ///
    /// `remove_client` drops the `ClientSender`, which retires the write half.
    /// The read half is a `dup` of the same socket held by the router thread,
    /// blocked in `recv_client_msg` with no deadline and leaving only on EOF —
    /// and nothing produced that EOF for a peer that connected and then kept
    /// its descriptor open. Each one stranded a router thread and an open
    /// socket for the life of the process; one server here reached 65,538 of
    /// them, at which point its listen backlog was permanently full and a
    /// `connect` to it never returned, hanging every zellij command on the
    /// machine.
    ///
    /// `shutdown(2)` acts on the socket rather than the descriptor, so this
    /// reaches the router's `dup` — which is the whole point, since the server
    /// has no handle on that thread to interrupt any other way.
    #[test]
    fn shutting_a_client_down_unblocks_the_router_reading_it() {
        let (_guard, server_end, client_end) = socket_pair();
        let live_writers = Arc::new(LiveWriters::default());
        let server_fd = {
            use std::os::fd::AsFd;
            match &server_end {
                interprocess::local_socket::Stream::UdSocket(uds) => {
                    uds.as_fd().try_clone_to_owned().expect("dup failed")
                },
            }
        };
        // Exactly how `new_client` wires a connection up: the receiver owns the
        // stream and the sender is a clone of it.
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let sender = ClientSender::new(1, receiver.get_sender(), live_writers.clone(), server_fd);

        // The client is deliberately kept alive for the whole test: a peer that
        // hangs up would produce the EOF on its own, which is the case that
        // already worked.
        let (blocked, is_blocked) = std::sync::mpsc::channel();
        let router = std::thread::spawn(move || {
            blocked.send(()).expect("nobody waiting for the router");
            receiver.recv_client_msg()
        });
        is_blocked.recv().expect("the router thread never started");

        sender.shutdown_reads();

        let outcome = router.join().expect("the router thread panicked");
        let described = match outcome {
            IpcRecv::Eof => "Eof",
            IpcRecv::Idle => "Idle",
            IpcRecv::Malformed => "Malformed",
            IpcRecv::Msg(..) => "Msg",
        };
        assert_eq!(
            described, "Eof",
            "shutting the read half down should end the router's read as a hangup"
        );
        drop(client_end);
    }

    /// The other way a router can be freed of a peer that will never hang up:
    /// a read deadline, applied by the router until the first message arrives,
    /// so that a connection which never speaks does not hold its slot forever.
    #[test]
    fn a_silent_peer_reads_as_idle_once_the_deadline_passes() {
        let (_guard, server_end, client_end) = socket_pair();
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let deadline = Duration::from_millis(50);
        receiver
            .set_recv_timeout(Some(deadline))
            .expect("setting a receive timeout failed");

        // The client is kept alive and silent for the whole read: a hangup
        // would be EOF, which is the case that already worked.
        let started_at = Instant::now();
        let outcome = receiver.recv_client_msg();
        let waited = started_at.elapsed();

        assert!(
            matches!(outcome, IpcRecv::Idle),
            "a read that ran out of time should be Idle, not a hangup or a bad frame"
        );
        assert!(
            waited >= deadline,
            "the read returned after {waited:?}, before its {deadline:?} deadline"
        );
        // Generous: the assertion is "bounded at all".
        assert!(
            waited < deadline * 40,
            "the read should have given up near its {deadline:?} deadline, but took {waited:?}"
        );
        drop(client_end);
    }

    /// Lifting the deadline is what keeps it from ever cutting a real client's
    /// frame short: once it is cleared, a message sent later than the old
    /// deadline still arrives whole.
    #[test]
    fn a_lifted_deadline_no_longer_bounds_the_read() {
        let (_guard, server_end, client_end) = socket_pair();
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let deadline = Duration::from_millis(20);
        receiver
            .set_recv_timeout(Some(deadline))
            .expect("setting a receive timeout failed");
        receiver
            .set_recv_timeout(None)
            .expect("clearing the receive timeout failed");

        let writer = std::thread::spawn(move || {
            std::thread::sleep(deadline * 5);
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(client_end);
            sender
                .send_client_msg(ClientToServerMsg::ConnStatus)
                .expect("send failed");
            sender
        });

        let outcome = receiver.recv_client_msg();
        assert!(
            matches!(outcome, IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)),
            "a message sent after the lifted deadline should still arrive"
        );
        drop(writer.join().expect("the writer thread panicked"));
    }
}
