use crate::{os_input_output::AsyncReader, screen::ScreenInstruction, thread_bus::ThreadSenders};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::task;
use zellij_utils::{
    errors::{get_current_ctx, prelude::*, ContextType},
    logging::debug_to_file,
};

pub(crate) struct TerminalBytes {
    terminal_id: u32,
    senders: ThreadSenders,
    async_reader: Box<dyn AsyncReader>,
    debug: bool,
    activity_flag: Arc<AtomicBool>,
}

impl TerminalBytes {
    pub fn new(
        terminal_id: u32,
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
        err_ctx.add_call(ContextType::AsyncTask);
        struct PaneClosedGuard {
            recorder: Option<crate::session_recorder::SessionRecorderHandle>,
            terminal_id: u32,
        }
        impl Drop for PaneClosedGuard {
            fn drop(&mut self) {
                if let Some(recorder) = &self.recorder {
                    recorder.pane_closed(self.terminal_id);
                }
            }
        }
        let _pane_closed_guard = PaneClosedGuard {
            recorder: self.senders.session_recorder.clone(),
            terminal_id: self.terminal_id,
        };
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
                        let _ = debug_to_file(bytes, self.terminal_id as i32);
                    }
                    // record output if enabled, before sending to screen
                    if let Some(recorder) = &self.senders.session_recorder {
                        recorder.output(self.terminal_id, bytes.to_vec());
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
        drop(_pane_closed_guard);

        // Ignore any errors that happen here.
        // We only leave the loop above when the pane exits. This can happen in a lot of ways, but
        // the most problematic is when quitting zellij with `Ctrl+q`. That is because the channel
        // for `Screen` will have exited already, so this send *will* fail. This isn't a problem
        // per-se because the application terminates anyway, but it will print a lengthy error
        // message into the log for every pane that was still active when we quit the application.
        // This:
        //
        // 1. Makes the log rather pointless, because even when the application exits "normally",
        //    there will be errors inside and
        // 2. Leaves the impression we have a bug in the code and can't terminate properly
        //
        // FIXME: Ideally we detect whether the application is being quit and only ignore the error
        // in that particular case?
        let _ = self.async_send_to_screen(ScreenInstruction::Render).await;

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
    use crate::session_recorder::{RecorderInstruction, SessionRecorderHandle};
    use async_trait::async_trait;
    use std::io;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Instant;

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

    #[tokio::test]
    async fn records_output_and_pane_closed() -> anyhow::Result<()> {
        let (sender, receiver) = crossbeam::channel::unbounded();
        let handle = SessionRecorderHandle {
            sender: sender.clone(),
            started_at: Instant::now(),
            capture_input: false,
            enabled: Arc::new(AtomicBool::new(true)),
        };
        // create thread senders with recorder
        let mut senders = ThreadSenders::default();
        senders.session_recorder = Some(handle);
        senders.should_silently_fail = true;
        let mut terminal_bytes = TerminalBytes::new(
            1,
            Box::new(FakeReader {
                data: Some(b"hello".to_vec()),
            }),
            senders,
            false,
            Arc::new(AtomicBool::new(false)),
        );
        terminal_bytes.listen().await?;
        // should have received one output event and then pane closed
        let mut events = Vec::new();
        let mut closed = None;
        while let Ok(instr) = receiver.try_recv() {
            match instr {
                RecorderInstruction::Events(ev) => events.extend(ev),
                RecorderInstruction::PaneClosed { terminal_id, .. } => closed = Some(terminal_id),
                _ => {},
            }
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].terminal_id, 1);
        assert_eq!(events[0].data, b"hello");
        assert_eq!(
            events[0].kind,
            zellij_utils::session_recording::EventKind::Output
        );
        assert_eq!(closed, Some(1));
        Ok(())
    }
}
