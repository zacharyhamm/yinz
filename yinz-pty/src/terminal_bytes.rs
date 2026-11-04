use crate::{os_input_output::AsyncReader, screen::ScreenInstruction, thread_bus::ThreadSenders};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
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
    pause: watch::Receiver<bool>,
    paused: watch::Sender<bool>,
    pause_safe: Option<watch::Receiver<bool>>,
    control: ReaderControl,
}

/// Pausing is cooperative: the acknowledgement follows the last completed
/// screen send. Aborting a task cannot provide that guarantee because its
/// `spawn_blocking` send may still be running after the task is gone.
#[derive(Clone)]
pub(crate) struct ReaderControl {
    pause: watch::Sender<bool>,
    paused: watch::Receiver<bool>,
}

impl ReaderControl {
    pub(crate) fn pause(&self) -> bool {
        self.pause.send_replace(true)
    }

    pub(crate) fn resume(&self) {
        self.pause.send_replace(false);
    }

    pub(crate) async fn wait_until_paused(&mut self) -> Result<()> {
        loop {
            if *self.paused.borrow_and_update() {
                return Ok(());
            }
            // A completed reader has already sent its final output too.
            if self.paused.changed().await.is_err() {
                return Ok(());
            }
        }
    }
}

impl TerminalBytes {
    pub fn new(
        terminal_id: TerminalId,
        async_reader: Box<dyn AsyncReader>,
        senders: ThreadSenders,
        debug: bool,
        activity_flag: Arc<AtomicBool>,
    ) -> Self {
        let (pause_tx, pause) = watch::channel(false);
        let (paused, paused_rx) = watch::channel(false);
        let pause_safe = async_reader.migration_pause_safety();
        TerminalBytes {
            terminal_id,
            senders,
            debug,
            async_reader,
            activity_flag,
            pause,
            paused,
            pause_safe,
            control: ReaderControl {
                pause: pause_tx,
                paused: paused_rx,
            },
        }
    }
    pub(crate) fn control(&self) -> ReaderControl {
        self.control.clone()
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
            while *self.pause.borrow_and_update() {
                self.paused.send_replace(true);
                if self.pause.changed().await.is_err() {
                    return Ok(());
                }
            }
            let was_paused = *self.paused.borrow();
            if was_paused {
                self.paused.send_replace(false);
            }
            let read = tokio::select! {
                biased;
                requested = wait_for_safe_pause(&mut self.pause, &mut self.pause_safe) => {
                    if !requested { return Ok(()); }
                    continue;
                }
                read = self.async_reader.read(&mut buf) => read,
            };
            match read {
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

/// Poll this alongside `read`, rather than testing safety in a select guard:
/// a read can enter an unsafe transition after the guard was evaluated.
async fn wait_for_safe_pause(
    pause: &mut watch::Receiver<bool>,
    safe: &mut Option<watch::Receiver<bool>>,
) -> bool {
    loop {
        if !*pause.borrow_and_update() {
            if pause.changed().await.is_err() {
                return false;
            }
            continue;
        }
        let Some(safe) = safe.as_mut() else {
            return true;
        };
        if *safe.borrow_and_update() {
            return true;
        }
        tokio::select! {
            changed = pause.changed() => if changed.is_err() { return false; },
            changed = safe.changed() => if changed.is_err() { return false; },
        }
    }
}

#[cfg(test)]
#[path = "unit/terminal_bytes_tests.rs"]
mod tests;
