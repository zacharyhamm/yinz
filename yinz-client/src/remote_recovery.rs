//! Supervises an established remote attachment without releasing the terminal workers.
use crate::{
    command_is_executing::CommandIsExecuting,
    os_input_output::ClientOsApi,
    remote_failure::{ConnectionFailure, FailureKind},
    session_switch::{self, PreparedConnection},
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthChar;
use yinz_ipc::ipc::ClientToServerMsg;
use yinz_runtime::errors::prelude::*;
use yinz_types::{input::cli_assets::CliAssets, pane_size::Size};

pub(crate) const READ_POLL: Duration = Duration::from_millis(100);
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Connected,
    Reconnecting,
    Repainting,
    Stopped,
}

struct State {
    generation: u64,
    phase: Phase,
    failure: Option<String>,
    reader_drained: bool,
    capabilities: Vec<ClientToServerMsg>,
}

/// Shared with the transport and keyboard workers. It never holds an IPC lock.
#[derive(Clone)]
pub struct RecoveryControl {
    state: Arc<Mutex<State>>,
    cancelled: Arc<AtomicBool>,
    command: CommandIsExecuting,
}
impl RecoveryControl {
    pub(crate) fn new(command: CommandIsExecuting) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                generation: 0,
                phase: Phase::Connected,
                failure: None,
                reader_drained: false,
                capabilities: Vec::new(),
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
            command,
        }
    }
    pub(crate) fn stamp(&self) -> (u64, bool) {
        let state = self.state.lock().unwrap();
        (state.generation, state.phase == Phase::Connected)
    }
    pub(crate) fn phase(&self) -> Phase {
        self.state.lock().unwrap().phase
    }
    pub(crate) fn can_write(&self) -> bool {
        matches!(self.phase(), Phase::Connected | Phase::Repainting)
    }
    pub(crate) fn accepts(&self, generation: u64) -> bool {
        self.stamp().0 == generation
    }
    pub(crate) fn can_read(&self, generation: u64) -> bool {
        let state = self.state.lock().unwrap();
        state.generation == generation && state.phase != Phase::Stopped
    }
    /// Called by the client loop after all preceding router messages were handled.
    pub(crate) fn reader_drained(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if state.generation == generation && state.phase == Phase::Reconnecting {
            state.reader_drained = true;
        }
    }
    pub(crate) fn lost(&self, generation: u64, diagnostic: String) {
        let mut state = self.state.lock().unwrap();
        if state.generation == generation
            && matches!(state.phase, Phase::Connected | Phase::Repainting)
        {
            state.phase = Phase::Reconnecting;
            state.failure = Some(diagnostic);
            state.reader_drained = false;
            self.command.clone().unblock_input_thread();
        }
    }
    pub(crate) fn cancel(&self) {
        crate::terminal_escapes::abandon_stalled_output();
        self.cancelled.store(true, Ordering::Release);
        self.command.clone().unblock_input_thread();
    }
    pub(crate) fn stop(&self) {
        self.state.lock().unwrap().phase = Phase::Stopped;
        self.cancelled.store(true, Ordering::Release);
        self.command.clone().unblock_input_thread();
    }
    pub(crate) fn cache(&self, message: &ClientToServerMsg) {
        if matches!(
            message,
            ClientToServerMsg::TerminalPixelDimensions { .. }
                | ClientToServerMsg::BackgroundColor { .. }
                | ClientToServerMsg::ForegroundColor { .. }
                | ClientToServerMsg::ColorRegisters { .. }
                | ClientToServerMsg::HostTerminalThemeChanged { .. }
                | ClientToServerMsg::HostTerminalFocusChanged { .. }
                | ClientToServerMsg::KittyGraphicsSupport { .. }
                | ClientToServerMsg::SixelSupport { .. }
                | ClientToServerMsg::SynchronizedOutputSupport { .. }
        ) {
            let mut state = self.state.lock().unwrap();
            if let ClientToServerMsg::ColorRegisters { color_registers } = message {
                let mut registers = std::collections::BTreeMap::new();
                for cached in &state.capabilities {
                    if let ClientToServerMsg::ColorRegisters { color_registers } = cached {
                        for register in color_registers {
                            registers.insert(register.index, register.clone());
                        }
                    }
                }
                for register in color_registers {
                    registers.insert(register.index, register.clone());
                }
                state
                    .capabilities
                    .retain(|old| !matches!(old, ClientToServerMsg::ColorRegisters { .. }));
                state.capabilities.push(ClientToServerMsg::ColorRegisters {
                    color_registers: registers.into_values().collect(),
                });
                return;
            }
            if let ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } = message
                && let Some(ClientToServerMsg::TerminalPixelDimensions {
                    pixel_dimensions: cached,
                }) = state
                    .capabilities
                    .iter_mut()
                    .find(|old| matches!(old, ClientToServerMsg::TerminalPixelDimensions { .. }))
            {
                cached.merge(*pixel_dimensions);
                return;
            }
            state
                .capabilities
                .retain(|old| std::mem::discriminant(old) != std::mem::discriminant(message));
            state.capabilities.push(message.clone());
        }
    }
    pub(crate) fn replay(&self, os: &dyn ClientOsApi) {
        os.replay_terminal_state(self, os.get_terminal_size());
    }
    /// Snapshot only while serialized with live observations by the transport's sender lock.
    pub(crate) fn replay_messages(&self, new_size: Size) -> Vec<ClientToServerMsg> {
        let mut messages = vec![ClientToServerMsg::TerminalResize { new_size }];
        messages.extend(self.state.lock().unwrap().capabilities.clone());
        messages.push(ClientToServerMsg::ModifiersHeld {
            modifiers: Default::default(),
        });
        messages
    }
    pub(crate) fn replacing(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.phase = Phase::Repainting;
        state.failure = None;
        state.reader_drained = false;
    }
    pub(crate) fn repainted(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.phase != Phase::Repainting {
            return false;
        }
        state.phase = Phase::Connected;
        true
    }
}

#[derive(Default)]
pub(crate) struct Backoff {
    retries: usize,
}
impl Backoff {
    pub(crate) fn delay(&mut self, sample: u16) -> Duration {
        let seconds = match self.retries {
            0 => 0,
            1..=5 => 1 << (self.retries - 1),
            _ => 30,
        };
        self.retries = self.retries.saturating_add(1);
        // Integer milliseconds make the endpoints deterministic in tests.
        Duration::from_millis((seconds * (800 + u64::from(sample.min(400)))).min(30_000))
    }
    pub(crate) fn healthy(&mut self, elapsed: Duration) {
        if elapsed >= Duration::from_secs(60) {
            self.retries = 0;
        }
    }
}

struct Attempt {
    cancelled: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<PreparedConnection, ConnectionFailure>>,
    worker: Option<JoinHandle<()>>,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        // Preparation polls cancellation, including while waiting for the first render.
        if let Some(worker) = self.worker.take() {
            join(worker);
        }
    }
}

pub(crate) struct Recovery {
    pub control: RecoveryControl,
    host: String,
    attempt: Option<Attempt>,
    next_retry: Option<Instant>,
    backoff: Backoff,
    attempt_number: usize,
    healthy_since: Option<Instant>,
    pub rendered: bool,
    diagnostic: String,
    status: Option<(Size, String)>,
    pub routers: Vec<JoinHandle<()>>,
}
impl Recovery {
    pub fn new(host: String, command: CommandIsExecuting) -> Self {
        Self {
            control: RecoveryControl::new(command),
            host,
            attempt: None,
            next_retry: None,
            backoff: Backoff::default(),
            attempt_number: 0,
            healthy_since: None,
            rendered: false,
            diagnostic: String::new(),
            status: None,
            routers: Vec::new(),
        }
    }
    pub fn first_render(&mut self, os: &dyn ClientOsApi, name: &str, now: Instant) {
        self.rendered = true;
        if self.control.phase() == Phase::Repainting {
            self.control.replay(os);
            if self.control.repainted() {
                self.healthy_since = Some(now);
                self.status = None;
                // The old connection's ancestry was cleared on disconnect. Host pings
                // can keep the silence timer satisfied, so force a new handshake once
                // input handling is connected and can forward the host's reply again.
                crate::terminal_escapes::announce_nested(os, &name.into());
            }
        } else if self.healthy_since.is_none() {
            self.healthy_since = Some(now);
        }
    }
    /// Called after queued server instructions, so an explicit exit or handoff wins over EOF.
    pub fn poll(
        &mut self,
        os: &dyn ClientOsApi,
        name: &str,
        now: Instant,
    ) -> Result<Option<PreparedConnection>> {
        if self.control.cancelled.load(Ordering::Acquire) {
            bail!("Interrupted while reconnecting");
        }
        for router in self.routers.extract_if(.., |router| router.is_finished()) {
            join(router);
        }
        let failure = {
            let mut state = self.control.state.lock().unwrap();
            if state.reader_drained {
                state.failure.take()
            } else {
                None
            }
        };
        if let Some(failure) = failure {
            self.diagnostic = failure;
            anyhow::ensure!(self.rendered, "{}", self.diagnostic);
            if let Some(since) = self.healthy_since.take() {
                let healthy = now.saturating_duration_since(since);
                self.backoff.healthy(healthy);
                if healthy >= Duration::from_secs(60) {
                    self.attempt_number = 0;
                }
            }
            os.disconnect_session_connection();
            let diagnostics = crate::remote_sessions::release_forward();
            if !diagnostics.is_empty() {
                self.diagnostic.push_str(&format!("\n{diagnostics}"));
            }
            log::warn!(
                "Remote attachment to {} / {name} interrupted: {}",
                self.host,
                self.diagnostic
            );
            if crate::remote_failure::permanent_diagnostic(&self.diagnostic) {
                bail!("{}", self.diagnostic);
            }
            self.schedule(now);
        }
        if let Some(attempt) = &self.attempt {
            match attempt.result.try_recv() {
                Ok(result) => {
                    self.attempt = None;
                    match result {
                        Ok(prepared) => return Ok(Some(prepared)),
                        Err(failure) if failure.kind == FailureKind::Permanent => {
                            bail!("{}", failure.diagnostic)
                        },
                        Err(failure) if failure.kind == FailureKind::Cancelled => {
                            bail!("Interrupted while reconnecting")
                        },
                        Err(failure) => {
                            self.diagnostic = failure.diagnostic;
                            log::warn!(
                                "Remote attachment attempt {} failed: {}",
                                self.attempt_number,
                                self.diagnostic
                            );
                            self.schedule(now);
                        },
                    }
                },
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => {
                    bail!("Remote attachment preparation worker failed")
                },
            }
        }
        if self.next_retry.is_some_and(|retry| now >= retry) {
            self.next_retry = None;
            self.attempt_number = self.attempt_number.saturating_add(1);
            let cancelled = Arc::new(AtomicBool::new(false));
            let stop = cancelled.clone();
            let host = self.host.clone();
            let name = name.to_owned();
            let assets = CliAssets {
                terminal_window_size: os.get_terminal_size(),
                supports_session_requests: true,
                host_terminal_env: yinz_runtime::host_terminal_env::host_terminal_env(),
                ..Default::default()
            };
            let (sender, result) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name("remote-reconnect".into())
                .spawn(move || {
                    let prepared = session_switch::prepare_recovery(
                        host,
                        name,
                        assets,
                        &stop,
                        Instant::now() + ATTEMPT_TIMEOUT,
                    )
                    .map_err(|error| ConnectionFailure::classify(error, &stop));
                    let _ = sender.send(prepared);
                })
                .context("start remote reconnection worker")?;
            self.attempt = Some(Attempt {
                cancelled,
                result,
                worker: Some(worker),
            });
        }
        Ok(None)
    }
    fn schedule(&mut self, now: Instant) {
        let random = uuid::Uuid::new_v4().as_u128() % 401;
        self.next_retry = Some(now + self.backoff.delay(random as u16));
    }
    pub fn install(
        &mut self,
        os: &dyn ClientOsApi,
        prepared: &mut PreparedConnection,
    ) -> Result<()> {
        self.control.replacing();
        let sender = prepared
            .sender
            .take()
            .context("replacement sender is missing")?;
        sender
            .set_send_timeout(Some(WRITE_TIMEOUT))
            .context("bound remote writes")?;
        let receiver = prepared
            .receiver
            .take()
            .context("replacement receiver is missing")?;
        receiver
            .set_recv_timeout(Some(READ_POLL))
            .context("bound remote reads")?;
        os.install_session_connection(sender, receiver)
            .context("install remote replacement")?;
        crate::remote_sessions::set_owned_forward(prepared.forward.take());
        // An attach's first frame is a complete screen; clear stale cells and the status row.
        if let Some((yinz_ipc::ipc::ServerToClientMsg::Render { content }, _)) = prepared
            .buffered
            .iter_mut()
            .find(|(message, _)| matches!(message, yinz_ipc::ipc::ServerToClientMsg::Render { .. }))
        {
            content.insert_str(0, crate::terminal_escapes::CLEAR_SCREEN);
        }
        Ok(())
    }
    pub fn paint_status(&mut self, os: &dyn ClientOsApi, name: &str, now: Instant) -> Result<()> {
        if self.control.phase() != Phase::Reconnecting {
            return Ok(());
        }
        let size = os.get_terminal_size();
        let retry = self
            .next_retry
            .map(|at| {
                format!(
                    "next retry {}s",
                    at.saturating_duration_since(now).as_secs()
                        + u64::from(at.saturating_duration_since(now).subsec_nanos() != 0)
                )
            })
            .unwrap_or_else(|| "connecting (30s limit)".into());
        let (identity, suffix) = if size.cols < 80 {
            // Keep the cancellation hint visible when a narrow terminal cannot fit the labels.
            let suffix = if size.cols < 26 {
                " Ctrl-C to cancel".to_owned()
            } else {
                format!(
                    " #{} {} Ctrl-C to cancel",
                    self.attempt_number,
                    retry.strip_prefix("next retry ").unwrap_or("now")
                )
            };
            (format!("{} / {name}", self.host), suffix)
        } else {
            (
                format!("Reconnecting {} / {name}", self.host),
                format!(
                    " | attempt {} | {retry} | Ctrl-C to cancel",
                    self.attempt_number
                ),
            )
        };
        let identity: String = identity
            .chars()
            .scan(
                size.cols.saturating_sub(suffix.len()),
                |remaining, character| {
                    let width = character.width().unwrap_or(0);
                    if width > *remaining {
                        return None;
                    }
                    *remaining -= width;
                    Some(character)
                },
            )
            .collect();
        let message = format!("{identity}{suffix}");
        if self
            .status
            .as_ref()
            .is_some_and(|(old_size, old)| *old_size == size && *old == message)
        {
            return Ok(());
        }
        let text: String = message
            .chars()
            .scan(size.cols, |remaining, character| {
                let width = character.width().unwrap_or(0);
                if width > *remaining {
                    return None;
                }
                *remaining -= width;
                Some(character)
            })
            .collect();
        let clear_previous = self.status.as_ref().and_then(|(old_size, _)| {
            let old_row = old_size.rows.max(1);
            (old_row != size.rows.max(1) && old_row <= size.rows.max(1))
                .then(|| format!("\x1b[{old_row};1H\x1b[0m\x1b[2K"))
        });
        let output = format!(
            "\x1b7{}\x1b[{};1H\x1b[0m\x1b[7m{text}\x1b[K\x1b[0m\x1b8",
            clear_previous.unwrap_or_default(),
            size.rows.max(1)
        );
        let mut out = os.get_stdout_writer();
        out.write_all(output.as_bytes())
            .context("write reconnection status")?;
        out.flush().context("flush reconnection status")?;
        self.status = Some((size, message));
        Ok(())
    }
    pub fn interrupted(&self) -> bool {
        self.control.cancelled.load(Ordering::Acquire)
    }
    pub fn shutdown(&mut self) {
        self.control.stop();
        self.attempt = None;
    }
}
impl Drop for Recovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}
pub(crate) fn join(worker: JoinHandle<()>) {
    if worker.join().is_err() {
        log::error!("Remote attachment worker panicked");
    }
}

#[cfg(test)]
#[path = "unit/remote_recovery.rs"]
mod tests;
