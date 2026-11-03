use super::*;
use crate::os_input_output::AsyncReader;
use async_trait::async_trait;
use std::io;
use std::sync::{atomic::AtomicBool, Arc};
use yinz_runtime::channels::{self, Shutdown};

struct FakeReader {
    data: Option<Vec<u8>>,
}

#[async_trait]
impl AsyncReader for FakeReader {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(data) = self.data.take() {
            let len = std::cmp::min(buf.len(), data.len());
            buf[..len].copy_from_slice(&data[..len]);
            Ok(len)
        } else {
            Ok(0) // EOF
        }
    }
}

/// Senders whose screen receiver is gone, as after the screen thread exits.
fn senders_with_exited_screen(shutdown: &Shutdown) -> ThreadSenders {
    let (to_screen, screen_rx) = channels::bounded(1);
    drop(screen_rx);
    ThreadSenders {
        to_screen: Some(shutdown.sender(to_screen)),
        should_silently_fail: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_failed_final_render_is_logged_not_returned() -> anyhow::Result<()> {
    // Shutdown never began, so the `Render` sent at EOF fails.
    let mut terminal_bytes = TerminalBytes::new(
        TerminalId::new(1),
        Box::new(FakeReader { data: None }),
        senders_with_exited_screen(&Shutdown::default()),
        false,
        Arc::new(AtomicBool::new(false)),
    );
    terminal_bytes.listen().await?;
    Ok(())
}

#[tokio::test]
async fn output_after_the_screen_exits_at_shutdown_is_not_an_error() -> anyhow::Result<()> {
    // Pins the `PtyBytes` send in the read loop, whose error `listen` returns, to the
    // shutdown-tolerant path. The final `Render` is logged either way; pty_tests pins its
    // routing.
    let shutdown = Shutdown::default();
    shutdown.begin();
    let mut terminal_bytes = TerminalBytes::new(
        TerminalId::new(1),
        Box::new(FakeReader {
            data: Some(b"late".to_vec()),
        }),
        senders_with_exited_screen(&shutdown),
        false,
        Arc::new(AtomicBool::new(false)),
    );
    terminal_bytes.listen().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_pause_waits_for_the_last_screen_send_and_resumes_without_loss() {
    struct NotifyingReader {
        data: Option<Vec<u8>>,
        read: Option<tokio::sync::oneshot::Sender<()>>,
    }
    #[async_trait]
    impl AsyncReader for NotifyingReader {
        async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(data) = self.data.take() {
                buf[..data.len()].copy_from_slice(&data);
                if let Some(read) = self.read.take() {
                    let _ = read.send(());
                }
                Ok(data.len())
            } else {
                Ok(0)
            }
        }
    }
    let (to_screen, screen) = channels::bounded(1);
    let shutdown = Shutdown::default();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(to_screen)),
        ..Default::default()
    };
    senders.send_to_screen(ScreenInstruction::Render).unwrap();
    let (read_tx, read_rx) = tokio::sync::oneshot::channel();
    let mut bytes = TerminalBytes::new(
        TerminalId::new(8),
        Box::new(NotifyingReader {
            data: Some(b"last output".to_vec()),
            read: Some(read_tx),
        }),
        senders,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let mut control = bytes.control();
    let task = tokio::spawn(async move { bytes.listen().await });
    tokio::time::timeout(yinz_test_support::default_timeout(), read_rx)
        .await
        .unwrap()
        .unwrap();
    control.pause();
    let (paused_tx, paused_rx) = tokio::sync::oneshot::channel();
    let waiter_control = control.clone();
    let waiter = tokio::spawn(async move {
        control.wait_until_paused().await.unwrap();
        let _ = paused_tx.send(());
    });
    // The first render filled the channel. The final byte delivery must precede
    // the pause acknowledgement even though it runs on a blocking sender task.
    assert!(matches!(
        screen
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .0,
        ScreenInstruction::Render
    ));
    let event = screen
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .0;
    assert!(
        matches!(event,ScreenInstruction::PtyBytes(id,ref bytes) if id==TerminalId::new(8) && bytes==b"last output")
    );
    tokio::time::timeout(yinz_test_support::default_timeout(), paused_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    waiter_control.resume();
    assert!(matches!(
        screen
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .0,
        ScreenInstruction::Render
    ));
    tokio::time::timeout(yinz_test_support::default_timeout(), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    waiter.await.unwrap();
}

#[tokio::test]
async fn migration_can_pause_before_any_terminal_byte_is_read() {
    let (to_screen, screen) = channels::bounded(4);
    let shutdown = Shutdown::default();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(to_screen)),
        ..Default::default()
    };
    let mut bytes = TerminalBytes::new(
        TerminalId::new(4),
        Box::new(FakeReader {
            data: Some(b"preserved".to_vec()),
        }),
        senders,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let mut control = bytes.control();
    control.pause();
    let task = tokio::spawn(async move { bytes.listen().await });
    tokio::time::timeout(
        yinz_test_support::default_timeout(),
        control.wait_until_paused(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(screen.try_recv().is_err());
    control.resume();
    tokio::time::timeout(yinz_test_support::default_timeout(), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(screen.try_recv().unwrap().0,ScreenInstruction::PtyBytes(_,ref bytes) if bytes==b"preserved")
    );
}

#[tokio::test]
async fn migration_pause_does_not_cancel_a_reader_mid_reconnection() {
    struct ReconnectingReader {
        safe: watch::Sender<bool>,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        complete: tokio::sync::oneshot::Receiver<()>,
        delivered: bool,
    }
    #[async_trait]
    impl AsyncReader for ReconnectingReader {
        fn migration_pause_safety(&self) -> Option<watch::Receiver<bool>> {
            Some(self.safe.subscribe())
        }
        async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.delivered {
                return Ok(0);
            }
            self.safe.send_replace(false);
            self.started.take().unwrap().send(()).unwrap();
            // Restarting this read after cancellation would consume `started`
            // twice, just as a reconnecting RemoteReader would lose its socket.
            (&mut self.complete).await.unwrap();
            self.safe.send_replace(true);
            self.delivered = true;
            buf[..9].copy_from_slice(b"recovered");
            Ok(9)
        }
    }
    let (to_screen, screen) = channels::bounded(4);
    let shutdown = Shutdown::default();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(to_screen)),
        ..Default::default()
    };
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (complete, complete_rx) = tokio::sync::oneshot::channel();
    let mut bytes = TerminalBytes::new(
        TerminalId::new(9),
        Box::new(ReconnectingReader {
            safe: watch::channel(true).0,
            started: Some(started),
            complete: complete_rx,
            delivered: false,
        }),
        senders,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let mut control = bytes.control();
    let task = tokio::spawn(async move { bytes.listen().await });
    tokio::time::timeout(yinz_test_support::default_timeout(), started_rx)
        .await
        .unwrap()
        .unwrap();
    control.pause();
    // Drive the select after requesting pause while its read is unsafe. The
    // read must remain alive until the simulated reconnect resolves.
    tokio::task::yield_now().await;
    assert!(!*control.paused.borrow());
    complete.send(()).unwrap();
    tokio::time::timeout(
        yinz_test_support::default_timeout(),
        control.wait_until_paused(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(screen.try_recv().unwrap().0,
        ScreenInstruction::PtyBytes(_, ref bytes) if bytes == b"recovered"));
    control.resume();
    tokio::time::timeout(yinz_test_support::default_timeout(), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
