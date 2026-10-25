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
