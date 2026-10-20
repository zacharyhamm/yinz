//! Signal handling for the stretch of startup before the client's own signal thread exists.
//!
//! The terminal is claimed - alternate screen, kitty keyboard, raw mode - before the server is
//! forked and the socket connected, and either of those can block for a long time (the connect
//! retries for `CONNECT_TO_SERVER_TIMEOUT`). Nothing is reading stdin yet and the real signal
//! listener is not running, so without this a Ctrl-C or a `kill` in that window would leave the
//! shell on an alternate screen it cannot get off.

use crate::terminal_escapes::ClientTeardown;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};
use std::thread;

/// The exit status a shell expects from a process killed by SIGINT.
pub const INTERRUPTED_EXIT_STATUS: i32 = 130;

/// Restores the terminal and exits if a quit signal arrives during startup.
///
/// Armed before the terminal is claimed and disarmed once the client's own `signal_listener`
/// thread takes over, so there is no window in which a quit signal is nobody's job.
pub(crate) struct StartupSignalGuard {
    handle: Handle,
}

impl StartupSignalGuard {
    /// Start watching for quit signals, tearing the terminal down with `teardown` if one arrives.
    ///
    /// Returns `None` if the signals could not be registered, which is not worth failing startup
    /// over - it only costs us this guard.
    pub fn arm(teardown: ClientTeardown) -> Option<Self> {
        let signals = match Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGHUP]) {
            Ok(signals) => signals,
            Err(e) => {
                log::error!("could not watch for signals during startup: {e}");
                return None;
            },
        };
        let handle = signals.handle();
        let spawned = thread::Builder::new()
            .name("startup_signal_listener".to_string())
            .spawn(move || {
                let mut signals = signals;
                // Ends without exiting when `disarm` closes the handle, which is the ordinary case.
                if signals.forever().next().is_some() {
                    ClientTeardown::exit_during_startup(
                        Some(teardown),
                        "Interrupted while starting up".to_owned(),
                        INTERRUPTED_EXIT_STATUS,
                    );
                }
            });
        match spawned {
            Ok(_) => Some(Self { handle }),
            Err(e) => {
                log::error!("could not spawn the startup signal listener: {e}");
                handle.close();
                None
            },
        }
    }

    /// Stop watching, handing quit signals over to whoever comes next.
    pub fn disarm(self) {
        self.handle.close();
    }
}
