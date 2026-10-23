//! Runs the `yinz attach` session picker on the terminal. The state, the drawing
//! and the key parsing are `yinz_cli::session_picker`'s; this sets the
//! terminal up, reads bytes and draws on stderr's alternate screen, leaving
//! stdout alone.
use console::Term;
use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::termios::{self, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices, Termios};
use std::io::{self, Write};
use std::os::fd::AsFd;
use yinz_cli::session_picker::{KeyParser, Outcome, PickerRow, PickerState};

/// Milliseconds to wait for the rest of a part-read key before taking a held
/// ESC as the Escape key: longer than a slow link splits a sequence by,
/// shorter than a person notices.
const ESCAPE_WAIT_MS: u16 = 50;
/// Milliseconds between looks at the terminal's size while no key comes, so
/// a resize is redrawn for.
const RESIZE_CHECK_MS: u16 = 200;

/// Keys unbuffered, unechoed and without signals for as long as the menu is
/// up, and the terminal's own settings back afterwards. Ctrl-C arrives as a
/// byte rather than a SIGINT, so every way out passes through `Screen`'s drop.
/// Output processing stays on, so lines still end in CR LF.
///
/// Keys are read here rather than with console's `read_key`, which on Ctrl-C
/// raises SIGINT itself and kills the process before anything is put back.
struct KeyMode {
    original: Termios,
}

impl KeyMode {
    fn enter() -> io::Result<Self> {
        let stdin = io::stdin();
        let original = termios::tcgetattr(&stdin)?;
        let mut keys = original.clone();
        keys.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG | LocalFlags::IEXTEN);
        keys.input_flags.remove(InputFlags::IXON);
        keys.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
        keys.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
        termios::tcsetattr(&stdin, SetArg::TCSADRAIN, &keys)?;
        Ok(KeyMode { original })
    }
}

impl Drop for KeyMode {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(io::stdin(), SetArg::TCSADRAIN, &self.original);
    }
}

/// The menu on the alternate screen, so every exit - a pick, Esc, an error, a
/// panic - hands back the shell's screen as it was, cursor showing. Drawn
/// below the prompt instead, a narrowed terminal could not be cleared after:
/// some terminals re-wrap the menu's lines and some cut them, and nothing
/// says which. `_keys` is dropped after `drop` has left, restoring the mode last.
struct Screen {
    term: Term,
    _keys: KeyMode,
}

impl Screen {
    fn enter() -> io::Result<Self> {
        let mut screen = Screen {
            term: Term::stderr(),
            _keys: KeyMode::enter()?,
        };
        screen.term.write_all(b"\x1b[?1049h")?;
        screen.term.hide_cursor()?;
        Ok(screen)
    }

    /// Replaces the screen with `lines`. The last has no newline after it, so
    /// a menu as tall as the screen does not scroll its first line away.
    fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        self.term.clear_screen()?;
        self.term.write_all(lines.join("\r\n").as_bytes())?;
        self.term.flush()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.term.write_all(b"\x1b[?1049l");
        let _ = self.term.show_cursor();
    }
}

/// Shows the menu until the person picks a session or leaves. `verb` is what
/// Enter does, for the footer.
pub fn run(rows: Vec<PickerRow>, verb: &'static str, hosts: Vec<String>) -> io::Result<Outcome> {
    let cancellation =
        yinz_client::remote_sessions::StartupCancellation::new().map_err(io::Error::other)?;
    let mut screen = Screen::enter()?;
    let mut discovery = Discovery::start(hosts);
    let mut state = PickerState::new(discovery.rows(&rows), verb);
    let mut parser = KeyParser::default();
    // Read unbuffered: bytes held in `StdinLock`'s buffer are invisible to
    // `poll`, which would then wait out a key that had already arrived.
    let stdin = io::stdin();
    let mut buf = [0u8; 256];
    let mut drawn_size = None;
    let mut changed = true;
    loop {
        if cancellation.is_cancelled() {
            return Ok(Outcome::Interrupted);
        }
        if discovery.update() {
            state.replace_rows(discovery.rows(&rows));
            changed = true;
        }
        let size = screen.term.size();
        if changed || drawn_size != Some(size) {
            let (height, width) = size;
            let max_lines = usize::from(height).max(1);
            state.set_page_size(max_lines.saturating_sub(2));
            // One column short, so a full-width line never leaves the
            // terminal holding a pending wrap.
            screen.draw(&state.render(usize::from(width).saturating_sub(1), max_lines))?;
            drawn_size = Some(size);
            changed = false;
        }
        let wait = if parser.has_pending() {
            ESCAPE_WAIT_MS
        } else {
            RESIZE_CHECK_MS
        };
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        let keys = match poll(&mut fds, wait) {
            Ok(0) => parser.flush(),
            Ok(_) => match nix::unistd::read(&stdin, &mut buf) {
                Ok(0) => return Ok(Outcome::Cancelled(None)),
                Ok(read) => parser.feed(&buf[..read]),
                Err(Errno::EINTR | Errno::EAGAIN) => continue,
                Err(e) => return Err(e.into()),
            },
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e.into()),
        };
        for key in keys {
            changed = true;
            match state.handle(key) {
                Outcome::Continue => {},
                outcome => return Ok(outcome),
            }
        }
    }
}

use yinz_client::session_discovery::Discovery;

#[cfg(test)]
#[path = "unit/session_menu.rs"]
mod tests;
