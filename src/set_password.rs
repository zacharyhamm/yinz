//! Strictly local password provisioning. Secrets never enter argv or remote routing.
use std::{
    io::{BufRead, IsTerminal},
    path::Path,
};
use yinz_runtime::errors::prelude::*;
use zeroize::Zeroizing;

fn read_password(mut reader: impl BufRead) -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new(Vec::new());
    // read_until on Take bounds allocation even for an endless line.
    std::io::Read::take(&mut reader, 1025)
        .read_until(b'\n', &mut bytes)
        .context("read password line")?;
    if bytes.len() > 1024 {
        bail!("password input exceeds 1024 bytes");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let text = std::str::from_utf8(&bytes).context("password must be UTF-8")?;
    validate(text)?;
    Ok(Zeroizing::new(text.to_owned()))
}
fn validate(text: &str) -> Result<()> {
    if text.contains('\0') || text.chars().count() < 12 {
        bail!("password must contain at least 12 characters and no NUL");
    }
    if text.len() > 1024 {
        bail!("password input exceeds 1024 bytes");
    }
    Ok(())
}

// dialoguer's password reader uses canonical stdin. SIGINT can terminate that read
// before console restores ECHO, so install an async-signal-safe restore-and-exit
// handler only around the local prompt. No filesystem mutation precedes the prompt.
struct PromptRestore {
    original: nix::sys::termios::Termios,
    handlers: Vec<signal_hook::SigId>,
}
impl PromptRestore {
    fn new(original: nix::sys::termios::Termios) -> Result<Self> {
        let mut guard = Self {
            original,
            handlers: Vec::new(),
        };
        for signal in [nix::libc::SIGINT, nix::libc::SIGTERM, nix::libc::SIGHUP] {
            let raw: nix::libc::termios = guard.original.clone().into();
            // SAFETY: the handler uses only async-signal-safe POSIX operations with
            // captured immutable data, and exits before any password can be published.
            let id = unsafe {
                signal_hook::low_level::register(signal, move || {
                    nix::libc::tcsetattr(0, nix::libc::TCSANOW, &raw);
                    let message = b"\x1b[?25h\r\nPassword entry cancelled; no files changed.\r\n";
                    nix::libc::write(2, message.as_ptr().cast(), message.len());
                    nix::libc::_exit(128 + signal);
                })
            }
            .context("install password prompt cancellation handler")?;
            guard.handlers.push(id);
        }
        Ok(guard)
    }
}
impl Drop for PromptRestore {
    fn drop(&mut self) {
        use nix::sys::termios::{tcsetattr, SetArg};
        tcsetattr(std::io::stdin(), SetArg::TCSANOW, &self.original)
            .context("restore password terminal settings")
            .non_fatal();
        console::Term::stderr()
            .show_cursor()
            .context("restore password cursor")
            .non_fatal();
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}

fn prompt() -> Result<Zeroizing<String>> {
    use nix::sys::termios::tcgetattr;
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("password entry requires a terminal; use --password-stdin explicitly");
    }
    let original = tcgetattr(&stdin).context("read terminal settings")?;
    let _restore = PromptRestore::new(original)?;
    let term = console::Term::stderr();
    let answer = dialoguer::Password::new()
        .with_prompt("Web password")
        .with_confirmation("Confirm web password", "Passwords do not match")
        .validate_with(|text: &String| validate(text).map_err(|e| e.to_string()))
        .interact_on(&term)
        .map(Zeroizing::new);
    match answer {
        Ok(password) => Ok(password),
        Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
            bail!("password entry cancelled; no files changed")
        },
        Err(e) => Err(e).context("read password from terminal"),
    }
}

pub(crate) fn run(
    data_dir: &Path,
    password_stdin: bool,
    clear: bool,
    revoke: bool,
    status_guidance: &str,
) -> Result<()> {
    use yinz_web::auth::password;
    if clear {
        password::clear(data_dir)?;
        println!(
            "Web password and sessions cleared. Enabled listeners will close within one second."
        );
    } else if revoke {
        password::revoke_sessions(data_dir)?;
        println!("Web session verifier rotated; existing sessions invalidated.");
    } else {
        let password = if password_stdin {
            read_password(std::io::stdin().lock())?
        } else {
            prompt()?
        };
        password::write(data_dir, &password)?;
        println!("Web password saved under {}. Enable web {{ enabled true }} and start a session. Config changes apply at the next session start. Use {status_guidance} for listener and Tailscale guidance. Native bearer login is ready; the web app is a later milestone.", data_dir.display());
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/set_password.rs"]
mod tests;
