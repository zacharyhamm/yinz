//! Terminal control sequences and the helpers that emit them.
//!
//! Both client entry points (`start_client` and `start_remote_client`) drive
//! the controlling terminal through the same lifecycle: enter the alternate
//! screen and clear inherited attributes on startup, announce ourselves if we
//! are nested inside another Zellij pane, and unwind all of that on the way
//! out. Keeping the sequences and the code that writes them together means
//! the setup and teardown halves cannot drift apart.

use crate::os_input_output::{StdioAccess, TtyControl};
use crate::stdin_ansi_parser::SyncOutput;
use std::io::Write;
use zellij_utils::errors::FatalError;
use zellij_utils::nested_session;

pub const ENTER_ALTERNATE_SCREEN: &str = "\u{1b}[?1049h";
pub const EXIT_ALTERNATE_SCREEN: &str = "\u{1b}[?1049l";
pub const ENABLE_BRACKETED_PASTE: &str = "\u{1b}[?2004h";
/// Ask the host to report focus changes as `CSI I` / `CSI O` (DECSET 1004),
/// so panes that track focus can be told about the host's.
pub const ENABLE_FOCUS_REPORTING: &str = "\u{1b}[?1004h";
/// Stop the host's focus reports on the way out.
pub const DISABLE_FOCUS_REPORTING: &str = "\u{1b}[?1004l";
pub const RESET_STYLE: &str = "\u{1b}[m";
pub const SHOW_CURSOR: &str = "\u{1b}[?25h";
pub const ENTER_KITTY_KEYBOARD_MODE: &str = "\u{1b}[>1u";
pub const EXIT_KITTY_KEYBOARD_MODE: &str = "\u{1b}[<1u";
pub const CLEAR_CLIENT_TERMINAL_ATTRIBUTES: &str = "\u{1b}[?1l\u{1b}=\u{1b}[r\u{1b}[?1000l\u{1b}[?1002l\u{1b}[?1003l\u{1b}[?1005l\u{1b}[?1006l\u{1b}[?12l";
/// Subscribe to host color-palette theme notifications (CSI 2031). Hosts
/// that support it begin emitting unsolicited DSR 997 reports on theme
/// change after this is sent.
pub const ENABLE_HOST_THEME_NOTIFY: &str = "\u{1b}[?2031h";
/// Cancel the CSI 2031 subscription (sent on detach / shutdown so we
/// don't leave the host emitting DSR 997s into nothing).
pub const DISABLE_HOST_THEME_NOTIFY: &str = "\u{1b}[?2031l";
/// Actively query the current host theme (DSR 996). Reply arrives in the
/// same `CSI ? 997 ; {1|2} n` form as unsolicited notifications, so the
/// stdin parser handles both uniformly.
pub const QUERY_HOST_THEME: &str = "\u{1b}[?996n";
pub const CLEAR_SCREEN: &str = "\u{1b}[2J";

/// Write a run of escape sequences to the controlling terminal and flush.
pub fn write_seqs(os_input: &dyn StdioAccess, seqs: &[&str]) {
    let mut stdout = os_input.get_stdout_writer();
    for seq in seqs {
        stdout
            .write_all(seq.as_bytes())
            .expect("cannot write to stdout");
    }
    stdout.flush().expect("could not flush");
}

/// Take over the controlling terminal on startup.
///
/// `ENABLE_HOST_THEME_NOTIFY` and `QUERY_HOST_THEME` are sent directly after
/// `CLEAR_CLIENT_TERMINAL_ATTRIBUTES` so there is no window in which the host
/// is unsubscribed. Hosts that don't support CSI 2031 ignore both.
pub fn enter_terminal(os_input: &dyn StdioAccess, kitty_keyboard: bool) {
    let mut seqs = vec![ENTER_ALTERNATE_SCREEN, CLEAR_CLIENT_TERMINAL_ATTRIBUTES];
    if kitty_keyboard {
        seqs.push(ENTER_KITTY_KEYBOARD_MODE);
    }
    seqs.push(ENABLE_HOST_THEME_NOTIFY);
    seqs.push(QUERY_HOST_THEME);
    write_seqs(os_input, &seqs);
}

/// Write terminal output, wrapped in the host's synchronised-output markers
/// when it supports them, so a frame is presented atomically.
pub fn write_synchronised(w: &mut dyn Write, sync: Option<SyncOutput>, bytes: &[u8]) {
    if let Some(sync) = sync {
        w.write_all(sync.start_seq())
            .expect("cannot write to stdout");
    }
    w.write_all(bytes).expect("cannot write to stdout");
    if let Some(sync) = sync {
        w.write_all(sync.end_seq()).expect("cannot write to stdout");
    }
    w.flush().expect("could not flush");
}

/// Tell an enclosing Zellij pane that a client is running inside it, so the
/// outer session can route nested control frames to us.
pub fn announce_nested(os_input: &dyn StdioAccess, session_name: &str) {
    let announce = nested_session::NestedSessionMessage::Announce {
        session_name: session_name.to_owned(),
        capabilities: vec![nested_session::NestedSessionCapability::NestedControl],
    };
    write_nested_frame(os_input, &announce);
}

/// Tell an enclosing Zellij pane that this client is going away.
pub fn say_bye(os_input: &dyn StdioAccess) {
    write_nested_frame(os_input, &nested_session::NestedSessionMessage::Bye);
}

fn write_nested_frame(os_input: &dyn StdioAccess, message: &nested_session::NestedSessionMessage) {
    let mut stdout = os_input.get_stdout_writer();
    let _ = stdout.write_all(&nested_session::encode_frame(message));
    let _ = stdout.flush();
}

/// Hand the controlling terminal back to the shell.
///
/// Every exit path in the client — clean shutdown, fatal error, and failures
/// during startup before the terminal was fully claimed — unwinds through
/// this, so the escape ribbon is written in exactly one place.
#[derive(Debug, Clone, Copy)]
pub struct ClientTeardown {
    /// Rows in the controlling terminal, used to place the parting message on
    /// the last line. `None` before the terminal size is known, in which case
    /// the message is written where the cursor already is.
    pub rows: Option<usize>,
    /// Whether kitty keyboard mode was entered and so must be exited.
    pub include_kitty_exit: bool,
}

impl ClientTeardown {
    pub fn new(rows: usize, include_kitty_exit: bool) -> Self {
        Self {
            rows: Some(rows),
            include_kitty_exit,
        }
    }

    /// A teardown for failures during startup, before the terminal size is
    /// known.
    pub fn at_startup(include_kitty_exit: bool) -> Self {
        Self {
            rows: None,
            include_kitty_exit,
        }
    }

    /// The escape ribbon that restores the terminal, followed by `message`.
    pub fn message(&self, message: &str) -> String {
        let kitty_exit = if self.include_kitty_exit {
            EXIT_KITTY_KEYBOARD_MODE
        } else {
            ""
        };
        // Without a known row count we can't place the cursor, so break the
        // line instead to keep the message off the restored screen's content.
        let position = match self.rows {
            Some(rows) => format!("\u{1b}[{};{}H", rows, 1),
            None => "\r\n".to_owned(),
        };
        format!(
            "{}{}{}{}{}{}{}{}\n",
            kitty_exit,
            DISABLE_HOST_THEME_NOTIFY,
            DISABLE_FOCUS_REPORTING,
            EXIT_ALTERNATE_SCREEN,
            RESET_STYLE,
            SHOW_CURSOR,
            position,
            message
        )
    }

    /// Restore the terminal and exit, for failures during startup.
    ///
    /// `teardown` is `None` when the terminal was never claimed (the detached
    /// server path), in which case there is nothing to restore and the
    /// message goes to stderr. Writes straight to `io::stdout` rather than
    /// through a `ClientOsApi`, since startup can fail before one exists.
    pub fn exit_after_startup_error(teardown: Option<Self>, message: String) -> ! {
        log::error!("{}", message);
        match teardown {
            Some(teardown) => {
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(teardown.message(&message).as_bytes());
                let _ = stdout.flush();
            },
            None => eprintln!("{}", message),
        }
        std::process::exit(1);
    }

    /// Hand the terminal back and print `message`, without exiting.
    ///
    /// Used on the path that returns a `ConnectToSession` to the caller, which
    /// needs the process to stay alive to reconnect.
    pub fn restore<O: TtyControl + StdioAccess + ?Sized>(&self, os_input: &O, message: &str) {
        os_input.disable_mouse().non_fatal();
        let _ = os_input.unset_raw_mode();
        let mut stdout = os_input.get_stdout_writer();
        let _ = stdout.write_all(self.message(message).as_bytes());
        let _ = stdout.flush();
    }

    /// Restore the terminal, print `message`, and exit the process.
    ///
    /// A zero `exit_status` is an ordinary shutdown and logs at info level;
    /// anything else logs at error level.
    pub fn tear_down<O: TtyControl + StdioAccess + ?Sized>(
        &self,
        os_input: &O,
        message: String,
        exit_status: i32,
    ) -> ! {
        if exit_status == 0 {
            log::info!("{}", message);
        } else {
            log::error!("{}", message);
        }
        self.restore(os_input, &message);
        std::process::exit(exit_status);
    }
}
