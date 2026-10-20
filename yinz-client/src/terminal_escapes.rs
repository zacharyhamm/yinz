//! Terminal control sequences and the helpers that emit them.
//!
//! Both client entry points (`start_client` and `start_remote_client`) drive
//! the controlling terminal through the same lifecycle: enter the alternate
//! screen and clear inherited attributes on startup, announce ourselves if we
//! are nested inside another Yinz pane, and unwind all of that on the way
//! out. Keeping the sequences and the code that writes them together means
//! the setup and teardown halves cannot drift apart.

use crate::os_input_output::{StdioAccess, TtyControl, DISABLE_MOUSE_SUPPORT};
use crate::stdin_ansi_parser::SyncOutput;
use std::io::{IoSlice, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::TrySendError;
use std::time::{Duration, Instant};
use yinz_ipc::nested_session;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;

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
/// Push kitty keyboard flag 1 - `DISAMBIGUATE_ESCAPE_CODES` - which is what lets a modified
/// key be told from the legacy escape sequence that happens to spell the same bytes.
pub const ENTER_KITTY_KEYBOARD_MODE: &str = "\u{1b}[>1u";
/// Push flags 1|2|4|8: disambiguate, plus `REPORT_EVENT_TYPES`, `REPORT_ALTERNATE_KEYS` and
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES`. Event types and all-keys-as-escape-codes are the only way
/// a terminal ever reports a modifier key on its own, and so the only way anything can know that
/// Alt is being *held*.
///
/// Flag 4 is here to pay for flag 8. Under flag 8 every printable key arrives as
/// `CSI <codepoint> u`, and the codepoint kitty reports is the *unshifted* key on the layout -
/// so `!` would arrive as `CSI 49;2u`, a `1` with Shift, and be typed into a pane as that. Flag 4
/// adds the shifted codepoint as a sub-parameter, which is what lets
/// [`crate::keyboard_parser::KittyKeyboardParser`] resolve the key back to the character a
/// terminal in the default mode would simply have sent as text.
///
/// Off by default and gated behind `report_modifier_keys` all the same, because flag 8 is still
/// not a free upgrade: US-ASCII survives the round trip now, but AltGr glyphs, dead keys and IME
/// commits are text the terminal refuses to send and no alternate key brings them back - a
/// multi-codepoint commit does not map onto a key event at all. Nobody should get that without
/// asking for it.
pub const ENTER_KITTY_KEYBOARD_MODE_WITH_MODIFIERS: &str = "\u{1b}[>15u";
/// Pop one entry off the flag stack, whichever of the two above pushed it.
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
/// CSI reset is harmless on hosts without synchronized output.
pub const END_SYNCHRONIZED_OUTPUT: &str = "\u{1b}[?2026l";
// Bits: 1 = CSI may be open, 2 = DCS may be open. Stdout serializes writers.
static OPEN_SYNC_OUTPUT: AtomicU8 = AtomicU8::new(0);

/// Set by the first thread to tear the terminal down during startup, so a second one does not
/// write a competing ribbon and message over it.
static STARTUP_EXIT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Write a run of escape sequences to the controlling terminal and flush.
pub fn write_seqs<O: StdioAccess + ?Sized>(os_input: &O, seqs: &[&str]) {
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
pub fn enter_terminal<O: StdioAccess + ?Sized>(
    os_input: &O,
    kitty_keyboard: bool,
    report_modifier_keys: bool,
) {
    let mut seqs = vec![ENTER_ALTERNATE_SCREEN, CLEAR_CLIENT_TERMINAL_ATTRIBUTES];
    if kitty_keyboard {
        seqs.push(match report_modifier_keys {
            true => ENTER_KITTY_KEYBOARD_MODE_WITH_MODIFIERS,
            false => ENTER_KITTY_KEYBOARD_MODE,
        });
    }
    seqs.push(ENABLE_HOST_THEME_NOTIFY);
    seqs.push(QUERY_HOST_THEME);
    write_seqs(os_input, &seqs);
}

/// Take the controlling terminal for a client, without letting it echo.
///
/// Order matters, and it is the whole point of this function. Raw mode goes on *before* the first
/// byte is written, because [`enter_terminal`] asks the host for its theme and the reply comes
/// back on a terminal that is still echoing everything it receives. Turning raw mode on later -
/// after the server has been spawned and the socket connected, which is where it used to happen -
/// leaves a window long enough on a new session for that reply to be printed at the top-left of
/// the alternate screen we have just switched to.
///
/// The raw mode taken here keeps the signal keys working ([`TtyControl::set_raw_mode_keeping_signals`]).
/// Echo is what has to stop before we write a query; Ctrl-C is what has to keep working while the
/// client forks a server and waits on a socket with nothing yet reading stdin. `start_client`
/// promotes this to full raw mode once it owns the keyboard.
///
/// `is_a_reconnect` skips the escape ribbon: the controlling terminal already has the attributes
/// we want, and some terminals do not treat these atomically. It does not skip the raw-mode
/// switch, because the previous pass left focus reporting on and those reports echo the same way.
pub fn claim_terminal<O>(
    os_input: &mut O,
    kitty_keyboard: bool,
    report_modifier_keys: bool,
    is_a_reconnect: bool,
) where
    O: TtyControl + StdioAccess + ?Sized,
{
    // Normalises the terminal when a reconnect brings us through here a second time; a no-op on
    // the first pass, where raw mode was never enabled.
    if let Err(e) = os_input.unset_raw_mode() {
        log::error!("failed to reset the terminal before claiming it: {e}");
    }
    os_input.set_raw_mode_keeping_signals();
    if !is_a_reconnect {
        enter_terminal(os_input, kitty_keyboard, report_modifier_keys);
    }
}

/// Write terminal output, wrapped in the host's synchronised-output markers
/// when it supports them, so a frame is presented atomically.
pub fn write_synchronised(
    w: &mut dyn Write,
    sync: Option<SyncOutput>,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_synchronised_tracking(w, sync, bytes, &OPEN_SYNC_OUTPUT)
}

fn write_synchronised_tracking(
    w: &mut dyn Write,
    sync: Option<SyncOutput>,
    bytes: &[u8],
    open: &AtomicU8,
) -> std::io::Result<()> {
    let sync_bit = match sync {
        Some(SyncOutput::Csi) => 1,
        Some(SyncOutput::Dcs) => 2,
        None => 0,
    };
    let (start, end) = sync.map_or((&[][..], &[][..]), |sync| {
        (sync.start_seq(), sync.end_seq())
    });
    // One vectored write, so a host that takes the whole frame gets it in one syscall.
    let mut slices = [IoSlice::new(start), IoSlice::new(bytes), IoSlice::new(end)];
    let mut pending = &mut slices[..];
    let total = start.len() + bytes.len() + end.len();
    let mut accepted = 0;
    let written = loop {
        if accepted == total {
            break Ok(());
        }
        match w.write_vectored(pending) {
            Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                // Record the first accepted byte: even a partial introducer needs cleanup.
                open.fetch_or(sync_bit, Ordering::SeqCst);
                accepted += n;
                IoSlice::advance_slices(&mut pending, n);
            },
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {},
            Err(e) => break Err(e),
        }
    };
    // A frame cut short before its end marker still closes what it, or an earlier frame,
    // opened. One cut short inside its end marker is not retried.
    let closed = if written.is_err()
        && accepted <= start.len() + bytes.len()
        && open.load(Ordering::SeqCst) & sync_bit != 0
    {
        w.write_all(end).map(|()| true)
    } else {
        Ok(written.is_ok())
    };
    let flushed = w.flush();
    if matches!(closed, Ok(true)) && flushed.is_ok() {
        open.fetch_and(!sync_bit, Ordering::SeqCst);
    }
    written.and(closed.map(drop)).and(flushed)
}

/// Set when the client starts quitting; cleared before the next interactive client starts.
static ABANDON_STALLED_OUTPUT: AtomicBool = AtomicBool::new(false);

/// How long a quitting client keeps waiting on a host terminal that has stopped taking output,
/// for instance one paused with Ctrl-S. Long enough for a busy host to catch up.
pub const STALLED_OUTPUT_LIMIT: Duration = Duration::from_secs(2);

/// The longest pause between retries when the host keeps refusing output that readiness said
/// it would take.
const MAX_STALL_BACKOFF: Duration = Duration::from_millis(25);

/// Stop waiting indefinitely on a stalled host terminal: ordinary writes that have made no
/// progress for [`STALLED_OUTPUT_LIMIT`] now fail. Called when a quit signal arrives, so
/// frame writers release stdout's lock and let terminal restoration take over.
pub fn abandon_stalled_output() {
    ABANDON_STALLED_OUTPUT.store(true, Ordering::SeqCst);
}

pub(crate) fn reset_output_abandonment() {
    ABANDON_STALLED_OUTPUT.store(false, Ordering::SeqCst);
}

pub(crate) fn stalled_output_abandoned() -> bool {
    ABANDON_STALLED_OUTPUT.load(Ordering::SeqCst)
}

/// A writer to the host terminal that waits out a full terminal rather than failing.
///
/// Stdout's file description is shared with whatever else runs on the host terminal,
/// and any of them may have made it non-blocking; a large write then meets `EAGAIN`
/// exactly when the host falls behind, and `write_all` would give up partway through
/// an escape sequence. `wait` pauses between attempts until the host may take more.
///
/// Temporary stalls have no deadline, just like writes to a blocking stdout, until
/// [`abandon_stalled_output`] is called. Retries back off when the host keeps refusing
/// output that `wait` said it would take, so a lying readiness check cannot spin.
pub struct HostWriter<W> {
    inner: W,
    wait: fn(&W) -> std::io::Result<()>,
    abandoned: fn() -> bool,
    stall_limit: Duration,
    stall: Option<Stall>,
}

/// The current run of refused writes, cleared whenever the host takes output.
struct Stall {
    since: Instant,
    retries: u32,
}

impl<W: Write> HostWriter<W> {
    pub fn new(inner: W, wait: fn(&W) -> std::io::Result<()>) -> Self {
        Self::with_stall_policy(inner, wait, stalled_output_abandoned, STALLED_OUTPUT_LIMIT)
    }

    /// `abandoned` says whether a stall longer than `stall_limit` should fail.
    pub(crate) fn with_stall_policy(
        inner: W,
        wait: fn(&W) -> std::io::Result<()>,
        abandoned: fn() -> bool,
        stall_limit: Duration,
    ) -> Self {
        HostWriter {
            inner,
            wait,
            abandoned,
            stall_limit,
            stall: None,
        }
    }

    #[cfg(test)]
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    fn retry<T>(&mut self, mut op: impl FnMut(&mut W) -> std::io::Result<T>) -> std::io::Result<T> {
        loop {
            match op(&mut self.inner) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {},
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => self.wait_out_stall()?,
                result => {
                    self.stall = None;
                    return result;
                },
            }
        }
    }

    fn wait_out_stall(&mut self) -> std::io::Result<()> {
        let now = Instant::now();
        let stall = self.stall.get_or_insert(Stall {
            since: now,
            retries: 0,
        });
        if now.duration_since(stall.since) >= self.stall_limit && (self.abandoned)() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the host terminal stopped accepting output while the client was quitting",
            ));
        }
        if stall.retries > 0 {
            // The last wait said the host was ready, yet it refused again.
            let backoff = Duration::from_millis(1) * 2u32.saturating_pow(stall.retries - 1);
            std::thread::sleep(backoff.min(MAX_STALL_BACKOFF));
        }
        stall.retries = stall.retries.saturating_add(1);
        match (self.wait)(&self.inner) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(()),
            result => result,
        }
    }
}

impl<W: Write> Write for HostWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.retry(|inner| inner.write(bytes))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
        self.retry(|inner| inner.write_vectored(bufs))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.retry(|inner| inner.flush())
    }
}

/// Writes host pixel-size queries from a thread of its own.
///
/// A resize is noticed on the signal thread, which must never block on the terminal: if it
/// waited behind a paused host, it would never get to a quit signal. `request` only queues;
/// one pending request covers any number of resizes, since the reply describes the host as
/// it is when the query is read.
pub(crate) struct PixelSizeQueries(std::sync::mpsc::SyncSender<()>);

impl PixelSizeQueries {
    pub fn spawn(stdout: impl Fn() -> Box<dyn Write> + Send + 'static) -> Result<Self> {
        let (requests, pending) = std::sync::mpsc::sync_channel::<()>(1);
        std::thread::Builder::new()
            .name("pixel_size_queries".to_string())
            .spawn(move || {
                // Ends when the last sender is dropped.
                for () in pending {
                    let mut stdout = stdout();
                    stdout
                        .write_all(crate::stdin_handler::PIXEL_SIZE_QUERY.as_bytes())
                        .and_then(|()| stdout.flush())
                        .context("failed to query the host terminal's pixel size")
                        .non_fatal();
                }
            })
            .context("failed to spawn the pixel-size query thread")?;
        Ok(Self(requests))
    }

    /// Ask for a query without waiting for it to be written.
    pub fn request(&self) {
        match self.0.try_send(()) {
            // Full means a query is already waiting to be written, which answers this one too.
            Ok(()) | Err(TrySendError::Full(())) => {},
            Err(e @ TrySendError::Disconnected(())) => Err::<(), _>(e)
                .context("the pixel-size query thread has stopped")
                .non_fatal(),
        }
    }
}

/// Tell an enclosing Yinz pane that a client is running inside it, so the
/// outer session can route nested control frames to us.
pub fn announce_nested(os_input: &dyn StdioAccess, session_name: &SessionName) {
    let announce = nested_session::NestedSessionMessage::Announce {
        session_name: session_name.to_owned(),
        capabilities: vec![nested_session::NestedSessionCapability::NestedControl],
    };
    write_nested_frame(os_input, &announce);
}

/// Tell an enclosing Yinz pane that this client is going away.
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

    /// The whole escape ribbon that restores the terminal, with nothing printed after it.
    /// [`Self::restore_with`] writes it in two flushed halves around restoring input.
    ///
    /// Safe to write even if the terminal was never fully claimed: popping an alternate screen
    /// that was never entered, and an empty kitty keyboard stack, are both no-ops.
    #[cfg(test)]
    pub fn restore_sequences(&self) -> String {
        // Tests without an explicit open frame must not depend on other tests' writes.
        self.restore_sequences_with_sync(0)
    }

    #[cfg(test)]
    pub(crate) fn restore_sequences_with_sync(&self, open_sync: u8) -> String {
        format!(
            "{}{}",
            Self::release_sequences(open_sync),
            self.mode_sequences()
        )
    }

    /// Closes any frame the host is holding back, then stops mouse reports. Written and flushed
    /// before raw mode goes off, so no report arrives while the shell's terminal echoes it.
    fn release_sequences(open_sync: u8) -> String {
        format!(
            "{}{}{}",
            if open_sync & 2 != 0 {
                "\x1bP=2s\x1b\\"
            } else {
                ""
            },
            END_SYNCHRONIZED_OUTPUT,
            DISABLE_MOUSE_SUPPORT,
        )
    }

    /// Everything else the client pushed, unwound once input is restored.
    fn mode_sequences(&self) -> String {
        let kitty_exit = if self.include_kitty_exit {
            EXIT_KITTY_KEYBOARD_MODE
        } else {
            ""
        };
        format!(
            "{}{}{}{}{}{}",
            kitty_exit,
            DISABLE_HOST_THEME_NOTIFY,
            DISABLE_FOCUS_REPORTING,
            EXIT_ALTERNATE_SCREEN,
            RESET_STYLE,
            SHOW_CURSOR,
        )
    }

    /// Where the parting message goes, followed by the message.
    fn placed_message(&self, message: &str) -> String {
        // Without a known row count we can't place the cursor, so break the
        // line instead to keep the message off the restored screen's content.
        let position = match self.rows {
            Some(rows) => format!("\u{1b}[{};{}H", rows, 1),
            None => "\r\n".to_owned(),
        };
        format!("{}{}\n", position, message)
    }

    /// The escape ribbon that restores the terminal, followed by `message`.
    #[cfg(test)]
    pub fn message(&self, message: &str) -> String {
        format!(
            "{}{}",
            self.restore_sequences(),
            self.placed_message(message)
        )
    }

    /// Restore the terminal and exit, for failures during startup.
    ///
    /// `teardown` is `None` when the terminal was never claimed (the detached
    /// server path), in which case there is nothing to restore and the
    /// message goes to stderr. Uses the shared stdout writer without a
    /// `ClientOsApi`, since startup can fail before one exists.
    pub fn exit_after_startup_error(teardown: Option<Self>, message: String) -> ! {
        Self::exit_during_startup(teardown, message, 1)
    }

    /// As [`Self::exit_after_startup_error`], with the exit status spelled out.
    ///
    /// Interrupting startup is not an error, so it exits with the shell's usual 128 + SIGINT
    /// rather than 1.
    pub fn exit_during_startup(teardown: Option<Self>, message: String, exit_status: i32) -> ! {
        if STARTUP_EXIT_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            // Someone got here first. A Ctrl-C during startup is delivered to the whole process
            // group, so the startup signal guard and the server child's non-zero exit arrive
            // together; the first one to arrive owns the terminal, the message and the exit
            // status, and the second waits here for the process to go away.
            loop {
                std::thread::park();
            }
        }
        log::error!("{}", message);
        match teardown {
            Some(teardown) => teardown.restore_startup(Some(&message)),
            None => eprintln!("{}", message),
        }
        std::process::exit(exit_status);
    }

    /// Hand the terminal back and print `message`, without exiting.
    ///
    /// Used on the path that returns a `SessionConnection` to the caller, which
    /// needs the process to stay alive to reconnect.
    pub fn restore<O: TtyControl + StdioAccess + ?Sized>(&self, os_input: &O, message: &str) {
        abandon_stalled_output();
        let mut stdout = os_input.get_stdout_writer_for_teardown();
        self.restore_with(
            &mut *stdout,
            || {
                os_input
                    .unset_raw_mode()
                    .context("failed to restore terminal input mode")
                    .non_fatal();
            },
            Some(message),
        );
    }

    pub(crate) fn restore_startup(&self, message: Option<&str>) {
        abandon_stalled_output();
        let mut stdout = crate::os_input_output::stdout_writer(true);
        self.restore_with(
            &mut *stdout,
            || {
                crossterm::terminal::disable_raw_mode()
                    .context("failed to restore terminal input mode during startup")
                    .non_fatal();
            },
            message,
        );
    }

    pub(crate) fn restore_with(
        &self,
        stdout: &mut dyn Write,
        restore_input: impl FnOnce(),
        message: Option<&str>,
    ) {
        self.restore_with_sync(stdout, restore_input, message, &OPEN_SYNC_OUTPUT);
    }

    /// [`Self::restore_with`], against `open` rather than the process-wide frame state.
    pub(crate) fn restore_with_sync(
        &self,
        stdout: &mut dyn Write,
        restore_input: impl FnOnce(),
        message: Option<&str>,
        open: &AtomicU8,
    ) {
        let written =
            stdout.write_all(Self::release_sequences(open.load(Ordering::SeqCst)).as_bytes());
        let released = written.and(stdout.flush());
        if released.is_ok() {
            open.store(0, Ordering::SeqCst);
        }
        released
            .context("failed to close output and disable mouse support during terminal restoration")
            .non_fatal();
        // Always restore input, even after a permanent output failure.
        restore_input();
        let mut ribbon = self.mode_sequences();
        if let Some(message) = message {
            ribbon.push_str(&self.placed_message(message));
        }
        stdout
            .write_all(ribbon.as_bytes())
            .context("failed to restore terminal output mode")
            .non_fatal();
        stdout
            .flush()
            .context("failed to flush terminal restoration")
            .non_fatal();
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

#[cfg(test)]
#[path = "unit/frame_write_tests.rs"]
mod frame_write_tests;
