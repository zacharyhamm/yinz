use crate::os_input_output::SignalEvent;

use async_trait::async_trait;
use nix::sys::termios;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use tokio::signal::unix::{signal, SignalKind};

use anyhow::{Context, Result};
use std::io;
use std::io::Write;
use std::path::Path;
use {yinz_ipc::ipc::IpcReceiverWithContext, yinz_ipc::ipc::IpcSenderWithContext};

/// Async signal listener that maps Unix signals to `SignalEvent` variants.
pub(crate) struct AsyncSignalListener {
    sigwinch: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    sigquit: tokio::signal::unix::Signal,
    sighup: tokio::signal::unix::Signal,
}

impl AsyncSignalListener {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            sigwinch: signal(SignalKind::window_change())?,
            sigterm: signal(SignalKind::terminate())?,
            sigint: signal(SignalKind::interrupt())?,
            sigquit: signal(SignalKind::quit())?,
            sighup: signal(SignalKind::hangup())?,
        })
    }
}

#[async_trait]
impl crate::os_input_output::AsyncSignals for AsyncSignalListener {
    async fn recv(&mut self) -> Option<SignalEvent> {
        tokio::select! {
            result = self.sigwinch.recv() => result.map(|_| SignalEvent::Resize),
            result = self.sigterm.recv() => result.map(|_| SignalEvent::Quit),
            result = self.sigint.recv() => result.map(|_| SignalEvent::Quit),
            result = self.sigquit.recv() => result.map(|_| SignalEvent::Quit),
            result = self.sighup.recv() => result.map(|_| SignalEvent::Quit),
        }
    }
}

/// Blocking signal iterator that maps Unix signals to `SignalEvent` variants.
/// Used by `start_signal_listener()` on a dedicated thread.
pub(crate) struct BlockingSignalIterator {
    signals: Signals,
}

impl BlockingSignalIterator {
    pub fn new() -> io::Result<Self> {
        let signals = Signals::new([SIGWINCH, SIGTERM, SIGINT, SIGQUIT, SIGHUP])?;
        Ok(Self { signals })
    }

    pub fn handle(&self) -> signal_hook::iterator::Handle {
        self.signals.handle()
    }
}

impl Iterator for BlockingSignalIterator {
    type Item = SignalEvent;

    fn next(&mut self) -> Option<SignalEvent> {
        for signal in self.signals.forever() {
            match signal {
                SIGWINCH => return Some(SignalEvent::Resize),
                SIGTERM | SIGINT | SIGQUIT | SIGHUP => return Some(SignalEvent::Quit),
                _ => {},
            }
        }
        None
    }
}

/// Set up client IPC channels from a connected socket.
///
/// On Unix a single socket is cloned for both send and receive directions.
pub(crate) fn setup_ipc(
    socket: interprocess::local_socket::Stream,
    _path: &Path,
) -> io::Result<(
    IpcSenderWithContext<yinz_ipc::ipc::ClientToServerMsg>,
    IpcReceiverWithContext<yinz_ipc::ipc::ServerToClientMsg>,
)> {
    let sender = IpcSenderWithContext::new(socket);
    let receiver = sender.get_receiver()?;
    Ok((sender, receiver))
}

pub(crate) fn enable_mouse_support(stdout: &mut dyn Write) -> Result<()> {
    let err_context = "failed to enable mouse mode";
    stdout
        .write_all(super::os_input_output::ENABLE_MOUSE_SUPPORT.as_bytes())
        .context(err_context)?;
    stdout.flush().context(err_context)?;
    Ok(())
}

pub(crate) fn disable_mouse_support(stdout: &mut dyn Write) -> Result<()> {
    let err_context = "failed to disable mouse mode";
    stdout
        .write_all(super::os_input_output::DISABLE_MOUSE_SUPPORT.as_bytes())
        .context(err_context)?;
    stdout.flush().context(err_context)?;
    Ok(())
}

/// Read the controlling terminal's attributes, hand them to `f`, and write them back.
///
/// A no-op that reports its error when stdin is not a tty - the client runs that way under test
/// and in a pipeline, and neither case wants a failure here to be fatal.
fn tweak_stdin_termios(f: impl FnOnce(&mut termios::Termios)) -> nix::Result<()> {
    let mut attrs = termios::tcgetattr(io::stdin())?;
    f(&mut attrs);
    termios::tcsetattr(io::stdin(), termios::SetArg::TCSANOW, &attrs)
}

/// Turn the signal keys (`ISIG`: Ctrl-C, Ctrl-\, Ctrl-Z) on or off, independently of raw mode.
///
/// Raw mode clears `ISIG` along with everything else, which is right once the client owns the
/// keyboard and forwards Ctrl-C to a pane. It is wrong during startup: the terminal has to stop
/// echoing before we query the host, but the client then spends that stretch forking a server and
/// retrying a socket connect with nothing reading stdin, so a Ctrl-C that does not become a SIGINT
/// is a Ctrl-C that does nothing at all.
pub(crate) fn set_signal_keys(enabled: bool) {
    if let Err(e) = tweak_stdin_termios(|attrs| {
        attrs.local_flags.set(termios::LocalFlags::ISIG, enabled);
    }) {
        log::debug!("could not set ISIG={enabled} on the controlling terminal: {e}");
    }
}

/// Turn output post-processing (`OPOST`/`ONLCR`, ie. "\n" means "\r\n") on or off.
///
/// Raw mode clears it, which is what we want for our own output - we place the cursor ourselves.
/// It is restored around forking the server, whose child can print setup errors to the inherited
/// terminal before it daemonizes; without `ONLCR` those staircase down the screen.
pub(crate) fn set_output_processing(enabled: bool) {
    if let Err(e) = tweak_stdin_termios(|attrs| {
        attrs.output_flags.set(
            termios::OutputFlags::OPOST | termios::OutputFlags::ONLCR,
            enabled,
        );
    }) {
        log::debug!("could not set OPOST={enabled} on the controlling terminal: {e}");
    }
}
