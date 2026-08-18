//! What a remote pane's process gets for an environment, and where each piece of
//! it comes from.
//!
//! A remote pane is not an ssh session, and that is the whole problem. The broker
//! that owns the pty is started once, by `ssh <host> zellij --pty-broker ensure`
//! — a non-interactive exec with no pty — and then `setsid`s away from it. Its
//! environment is a snapshot of that one arbitrary login, frozen for the life of
//! the daemon and shared by every pane, every session and every client that ever
//! reaches this machine. Inheriting from it gives a pane no `TERM` at all (sshd
//! sends none without a pty request), no locale, and a `PATH`, `HOME` and
//! `SSH_CONNECTION` belonging to somebody else's connection from hours ago.
//!
//! So nothing is inherited. Each variable is taken from wherever it is actually
//! authoritative:
//!
//!   * **The asking machine**, sampled per pane and carried on the wire: `TERM`,
//!     `COLORTERM` and the locale. Only that machine knows what terminal is on
//!     the other end of the pane.
//!   * **This machine's passwd entry**, recomputed per pane: `HOME`, `USER`,
//!     `LOGNAME`, `SHELL`, `PATH`, `MAIL`. Facts about where the pane runs.
//!   * **The supervisor**, after `openpty`: `SSH_TTY`, which no other process can
//!     name.
//!
//! and the frozen `SSH_*`/`ZELLIJ_*` leftovers are actively removed rather than
//! passed on. See `SCRUBBED_VARS` for why `SSH_CLIENT`/`SSH_CONNECTION` are
//! omitted outright instead of being given a plausible value.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// What the asking machine is entitled to say about the pane, and nothing else.
///
/// This is deliberately an allowlist: a variable crossing a machine boundary
/// should be a decision someone made, not a default. `TERM` and `COLORTERM`
/// describe the terminal the pane is drawn into, which is on the asking side;
/// the locale describes the person typing. Everything else that looks temptingly
/// portable is not — `PATH`, `HOME`, `SHELL`, `TMPDIR` and `PWD` are paths on the
/// wrong filesystem, and `DISPLAY` names a server on the wrong machine.
///
/// This is roughly ssh's own `SendEnv`/`AcceptEnv` default, for the same reasons.
pub const CARRIED_VARS: &[&str] = &["TERM", "COLORTERM", "LANG", "LANGUAGE"];

/// Locale variables are open-ended (`LC_TIME`, `LC_MESSAGES`, and whatever libc
/// adds next), so they are matched by prefix rather than enumerated.
pub const CARRIED_PREFIXES: &[&str] = &["LC_"];

/// Frozen leftovers of the *bootstrap* ssh session that must never reach a pane.
///
/// `SSH_CLIENT` and `SSH_CONNECTION` are the interesting entries, because the
/// honest thing to do with them is nothing at all. A pane's transport is a
/// forwarded unix socket, so the pane has no TCP peer of its own; `SO_PEERCRED`
/// on that socket yields a pid and a uid but never a 4-tuple, and sshd does not
/// carry these in its own environ to be read out of `/proc` (`do_setup_env`
/// builds the child's `envp` at exec time rather than `setenv`-ing into itself).
/// The bootstrap ssh does have real values — but they describe a *different TCP
/// connection* than the one the pane's bytes travel on, and that connection is
/// torn down and rebuilt on every outage while the pane's shell survives. Any
/// value exported into the shell would be right for at most one connection and
/// then silently wrong, frozen inside an already-exec'd process, which is the
/// exact failure this module exists to remove.
///
/// These are positional field lists (`ip port [ip] port`) that consumers pull
/// apart with `awk`, so a half-true value is not degradable. Absent is the truth:
/// this pane has no TCP peer. `SSH_TTY` — which the supervisor *can* answer
/// honestly — is set, and is the other conventional "am I remote" probe.
///
/// `SSH_AUTH_SOCK` goes for the same reason: it is a lease on the bootstrap's
/// connection, and it dies unpredictably mid-session. Real per-pane agent
/// forwarding would hook in here.
pub const SCRUBBED_VARS: &[&str] = &[
    "SSH_CLIENT",
    "SSH_CONNECTION",
    "SSH_TTY",
    "SSH_AUTH_SOCK",
    "SSH_ORIGINAL_COMMAND",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "ZELLIJ_SOCKET_DIR",
];

/// OpenSSH's `_PATH_STDPATH` — what an ssh login gets before any profile runs.
const STD_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// OpenSSH's `SUPERUSER_PATH`.
const SUPERUSER_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub fn is_carried(name: &str) -> bool {
    CARRIED_VARS.contains(&name) || CARRIED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Sample this machine's environment for the parts a remote pane may have.
///
/// Called once per pane on the *asking* side, deliberately: sampling it once when
/// the host table is built would reintroduce exactly the freezing this module
/// exists to undo.
pub fn carry_from_local() -> Vec<(String, String)> {
    select_carried(std::env::vars_os())
}

/// The seam `carry_from_local` is built on, so the policy can be tested without
/// mutating the process environment.
pub(crate) fn select_carried<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut carried: Vec<(String, String)> = vars
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.to_str()?;
            if !is_carried(name) {
                return None;
            }
            // A non-UTF-8 value cannot cross as JSON, and a lossy conversion of a
            // locale name is worse than not sending it: the far side would set a
            // locale that does not exist rather than fall back to the default.
            match value.to_str() {
                Some(value) => Some((name.to_owned(), value.to_owned())),
                None => {
                    log::debug!("not carrying {name} to a remote pane: its value is not utf-8");
                    None
                },
            }
        })
        .collect();
    // Stable order so the wire form of two identical environments is identical.
    carried.sort();
    carried
}

/// What this machine's passwd entry says about whoever the broker runs as.
///
/// Deliberately separate from `broker::ShellSource`, which answers a different
/// question and so has different fallback rules: resolving a *shell* may fall
/// back to a stale `$SHELL` because failing over to `/bin/sh` is worse, whereas
/// falling back to a stale `$HOME` or `$PATH` here would reintroduce the frozen
/// value this module exists to remove. Both are built from one passwd lookup.
#[derive(Debug, Default, Clone)]
pub(crate) struct LoginSource {
    pub name: Option<String>,
    pub uid: Option<u32>,
    pub home: Option<PathBuf>,
    pub shell: Option<PathBuf>,
}

impl LoginSource {
    /// Built from a passwd lookup the caller already did: `broker::start_pane`
    /// needs one for the shell as well, and there is no reason to ask twice.
    pub(crate) fn from_user(user: &Option<nix::unistd::User>) -> Self {
        LoginSource {
            name: user.as_ref().map(|u| u.name.clone()),
            uid: user.as_ref().map(|u| u.uid.as_raw()),
            home: user.as_ref().map(|u| u.dir.clone()),
            shell: user.as_ref().map(|u| u.shell.clone()),
        }
    }
}

/// `HOME`, `USER`, `LOGNAME`, `SHELL`, `PATH` and `MAIL`, as `do_setup_env` would
/// have built them for a login on this machine.
///
/// `PATH` is a constant rather than anything inherited: a login shell's profile is
/// this machine's own answer for `PATH` and runs after this, so the job here is to
/// provide the same starting point ssh would, not to guess at a better one.
pub(crate) fn login_env(source: &LoginSource) -> Vec<(String, String)> {
    let mut env = vec![];
    if let Some(home) = source.home.as_ref().filter(|h| !h.as_os_str().is_empty()) {
        env.push(("HOME".to_owned(), home.to_string_lossy().into_owned()));
    }
    if let Some(name) = source.name.as_ref().filter(|n| !n.is_empty()) {
        env.push(("USER".to_owned(), name.clone()));
        env.push(("LOGNAME".to_owned(), name.clone()));
        env.push(("MAIL".to_owned(), format!("/var/mail/{name}")));
    }
    if let Some(shell) = source.shell.as_ref().filter(|s| !s.as_os_str().is_empty()) {
        env.push(("SHELL".to_owned(), shell.to_string_lossy().into_owned()));
    }
    let path = match source.uid {
        Some(0) => SUPERUSER_PATH,
        _ => STD_PATH,
    };
    env.push(("PATH".to_owned(), path.to_owned()));
    env
}

/// The pane's environment, in application order — later entries win.
///
/// The passwd-derived values come first because they describe the machine the
/// pane runs on; the carried ones win over them because `TERM` and the locale
/// describe the client, and nothing on this machine knows those. A login shell's
/// profile then runs after all of it and may override anything, which is correct:
/// under real ssh it has exactly that power.
pub(crate) fn compose(
    source: &LoginSource,
    carried: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut env = login_env(source);
    env.extend(carried);
    env
}

/// The `PATH` a composed environment will give the child, for the pre-flight
/// command lookup that would otherwise search the *supervisor's* `PATH`.
pub(crate) fn path_in(env: &[(String, String)]) -> Option<&OsStr> {
    env.iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| OsStr::new(value.as_str()))
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_env_tests.rs"]
mod pty_host_remote_env_tests;
