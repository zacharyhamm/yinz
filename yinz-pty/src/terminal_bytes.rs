use crate::{os_input_output::AsyncReader, screen::ScreenInstruction, thread_bus::ThreadSenders};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::task;
use yinz_runtime::errors::get_current_ctx;
use yinz_runtime::errors::prelude::*;
use yinz_runtime::logging::debug_to_file;
use yinz_types::diagnostics::ContextType;
use yinz_types::internal_ids::TerminalId;

pub(crate) struct TerminalBytes {
    terminal_id: TerminalId,
    senders: ThreadSenders,
    async_reader: Box<dyn AsyncReader>,
    debug: bool,
    activity_flag: Arc<AtomicBool>,
}

impl TerminalBytes {
    pub fn new(
        terminal_id: TerminalId,
        async_reader: Box<dyn AsyncReader>,
        senders: ThreadSenders,
        debug: bool,
        activity_flag: Arc<AtomicBool>,
    ) -> Self {
        TerminalBytes {
            terminal_id,
            senders,
            debug,
            async_reader,
            activity_flag,
        }
    }
    pub async fn listen(&mut self) -> Result<()> {
        // This function reads bytes from the pty and then sends them as
        // ScreenInstruction::PtyBytes to screen to be parsed there
        // We also send a separate instruction to Screen to render as ScreenInstruction::Render
        //
        // We endeavour to send a Render instruction to screen immediately after having send bytes
        // to parse - this is so that the rendering is quick and smooth. However, this can cause
        // latency if the screen is backed up. For this reason, if we detect a peak in the time it
        // takes to send the render instruction, we assume the screen thread is backed up and so
        // only send a render instruction sparingly, giving screen time to process bytes and render
        // while still allowing the user to see an indication that things are happening (the
        // sparing render instructions)
        let err_context = || "failed to listen for bytes from PTY".to_string();

        let mut err_ctx = get_current_ctx();
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::AsyncTask);
        let mut buf = [0u8; 65536];
        loop {
            match self.async_reader.read(&mut buf).await {
                Ok(0) => break, // EOF
                Err(err) => {
                    log::error!("{}", err);
                    break;
                },
                Ok(n_bytes) => {
                    self.activity_flag.store(true, Ordering::Relaxed);
                    let bytes = &buf[..n_bytes];
                    if self.debug {
                        let _ = debug_to_file(bytes, self.terminal_id.get() as i32);
                    }
                    self.async_send_to_screen(ScreenInstruction::PtyBytes(
                        self.terminal_id,
                        bytes.to_vec(),
                    ))
                    .await
                    .with_context(err_context)?;
                },
            }
        }
        // Once `Shutdown::begin` has run, `SenderWithContext::send` reports a send to the exited
        // screen as success, so session teardown lands here as `Ok`. An error means the screen
        // went away some other way: worth a log line, but not the session.
        self.async_send_to_screen(ScreenInstruction::Render)
            .await
            .with_context(|| {
                format!(
                    "failed to request a render after terminal {} closed",
                    self.terminal_id
                )
            })
            .non_fatal();

        Ok(())
    }
    async fn async_send_to_screen(
        &self,
        screen_instruction: ScreenInstruction,
    ) -> Result<Duration> {
        // returns the time it blocked the thread for
        let sent_at = Instant::now();
        let senders = self.senders.clone();
        task::spawn_blocking(move || senders.send_to_screen(screen_instruction))
            .await
            .context("failed to async-send to screen")?
            .context("failed to block on sending message to screen")?;
        Ok(sent_at.elapsed())
    }
}

#[cfg(test)]
mod tests {
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
}
