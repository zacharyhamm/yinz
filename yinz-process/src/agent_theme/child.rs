//! Foreground child supervision shared by the synchronous launchers and the Codex bridge.
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};
use yinz_runtime::errors::prelude::*;

pub struct AgentChild {
    child: Child,
    signals: signal_hook::iterator::Signals,
    terminal_fd: i32,
    foreground: bool,
    finished: bool,
}

impl AgentChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        use signal_hook::consts::*;
        let watched = [SIGINT, SIGQUIT, SIGTERM, SIGHUP, SIGTSTP, SIGCONT];
        let mut ignored = Vec::new();
        for signal in watched {
            let mut action = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) } < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("read inherited signal disposition");
            }
            if action.sa_sigaction == libc::SIG_IGN {
                ignored.push(signal);
            }
        }
        let signals =
            signal_hook::iterator::Signals::new(watched).context("watch managed agent signals")?;
        // exec resets caught handlers, but preserves ignored signals. Installing
        // the forwarding handlers must not change the provider's inherited state.
        unsafe {
            command.pre_exec(move || {
                for signal in &ignored {
                    libc::signal(*signal, libc::SIG_IGN);
                }
                Ok(())
            });
        }
        // The provider gets terminal-generated signals directly. Signals aimed at the
        // launcher are forwarded to this separate group, including its descendants.
        // stdin may be redirected while stdout still belongs to the controlling
        // terminal. Job control must work for that invocation too (eg. TOSTOP).
        let terminal_fd = [0, 1, 2]
            .into_iter()
            .find(|fd| unsafe { libc::tcgetpgrp(*fd) >= 0 })
            .unwrap_or(0);
        let foreground = unsafe { libc::tcgetpgrp(terminal_fd) == libc::getpgrp() };
        if foreground {
            // Hand off before exec: a provider that reads immediately must not race
            // the parent's tcsetpgrp and stop itself with SIGTTIN.
            unsafe {
                command.pre_exec(move || {
                    set_foreground(terminal_fd, libc::getpgrp());
                    Ok(())
                });
            }
        }
        let child = match command.process_group(0).spawn() {
            Ok(child) => child,
            Err(error) => {
                // pre_exec may already have handed the terminal to the failed
                // child. Reclaim it before the lifecycle guard writes its end.
                if foreground {
                    set_foreground(terminal_fd, unsafe { libc::getpgrp() });
                }
                return Err(error).context("launch managed agent");
            },
        };
        let managed = Self {
            child,
            signals,
            terminal_fd,
            foreground,
            finished: false,
        };
        if foreground {
            managed.set_foreground(managed.child.id() as i32);
        }
        Ok(managed)
    }

    fn set_foreground(&self, group: i32) {
        set_foreground(self.terminal_fd, group);
    }

    pub fn poll(&mut self) -> Result<Option<ExitStatus>> {
        for signal in self.signals.pending() {
            unsafe {
                libc::kill(-(self.child.id() as i32), signal);
            }
        }
        let mut status = 0;
        let result = unsafe {
            libc::waitpid(
                self.child.id() as i32,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).context("wait for managed agent");
            }
        } else if result > 0 {
            if libc::WIFSTOPPED(status) {
                if self.foreground {
                    self.set_foreground(unsafe { libc::getpgrp() });
                }
                // Expose the stop to the shell or pane supervisor. After `fg`/resume,
                // restore the provider's foreground group before waking it.
                unsafe {
                    libc::raise(libc::SIGSTOP);
                }
                self.foreground = unsafe { libc::tcgetpgrp(self.terminal_fd) == libc::getpgrp() };
                if self.foreground {
                    self.set_foreground(self.child.id() as i32);
                }
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGCONT);
                }
            } else {
                self.finished = true;
                return Ok(Some(ExitStatus::from_raw(status)));
            }
        }
        Ok(None)
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            if let Some(status) = self.poll()? {
                return Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for AgentChild {
    fn drop(&mut self) {
        if !self.finished {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            self.child.wait().context("reap managed agent").non_fatal();
        }
        if self.foreground {
            self.set_foreground(unsafe { libc::getpgrp() });
        }
    }
}

/// Called after guards have dropped, so signal termination also ends the palette.
pub fn exit_status(status: ExitStatus) -> i32 {
    if let Some(signal) = status.signal() {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
        return 128 + signal;
    }
    status.code().unwrap_or(1)
}

fn set_foreground(terminal_fd: i32, group: i32) {
    // Temporarily block SIGTTOU while taking the foreground from a background group.
    unsafe {
        let mut set = std::mem::zeroed();
        let mut previous = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTTOU);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut previous);
        libc::tcsetpgrp(terminal_fd, group);
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
    }
}
