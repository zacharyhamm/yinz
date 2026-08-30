//! Detachable panes: every pane's child process is owned by a small supervisor
//! process ("pty-host") rather than by zellij-server itself.
//!
//! The supervisor creates the pty, forks the child, and hands zellij-server a dup
//! of the pty *master* descriptor over `SCM_RIGHTS`. The server then reads,
//! writes and ioctls that descriptor exactly as it did when it owned the pty, so
//! everything downstream — `RawFdAsyncReader`, `try_write_to_fd`'s partial-write
//! backpressure, `TIOCSWINSZ`, `tcdrain`, `tcgetpgrp` — is unchanged.
//!
//! What changes is lifetime. The child is a child of the supervisor, not of the
//! server, and the supervisor holds the master open. When the server exits the
//! child gets no `SIGHUP`; the supervisor notices the control channel closed and
//! starts draining the pty so the child never blocks on a full buffer. Only an
//! explicit `Terminate` ends a supervisor, which every ordinary teardown path
//! sends.

pub mod cli;
pub mod client;
pub mod fdpass;
pub mod host;
pub mod protocol;
pub mod remote;

pub use host::{run_pty_host, serve, ServeConfig};

fn ensure_secure_dir(path: &std::path::Path) -> zellij_utils::errors::prelude::Result<()> {
    use zellij_utils::errors::prelude::*;

    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    zellij_utils::shared::set_permissions(path, 0o700)
        .with_context(|| format!("failed to secure {}", path.display()))
}

fn secure_socket(path: &std::path::Path, mode: u32) -> zellij_utils::errors::prelude::Result<()> {
    secure_socket_with(path, mode, zellij_utils::shared::set_permissions)
}

fn secure_socket_with(
    path: &std::path::Path,
    mode: u32,
    set_permissions: impl FnOnce(&std::path::Path, u32) -> std::io::Result<()>,
) -> zellij_utils::errors::prelude::Result<()> {
    use zellij_utils::errors::prelude::*;

    if let Err(error) = set_permissions(path, mode) {
        std::fs::remove_file(path).with_context(|| {
            format!(
                "failed to remove {} after permission finalization failed",
                path.display()
            )
        })?;
        return Err(error).with_context(|| {
            format!("failed to set permissions {mode:04o} on {}", path.display())
        });
    }
    Ok(())
}

/// The argv flag that turns this binary into a supervisor.
pub const PTY_HOST_FLAG: &str = "--pty-host";

/// Used when the config says nothing. Kept here so there is one place to flip.
pub const DEFAULT_DETACHABLE_PANES: bool = true;

/// Detect the supervisor invocation without going through clap, so `main()` can
/// branch before the logger, the cache-folder setup and the clap builder — none
/// of which a supervisor needs, and all of which it would pay for on every pane.
///
/// Returns the label the server passed for `ps` readability, if any.
pub fn pty_host_invocation() -> Option<String> {
    invocation_from_args(std::env::args())
}

/// Split out from `pty_host_invocation` so the argv rule can be tested:
/// `std::env::args()` is process-global, and the "only as the first argument"
/// half of the rule is what `main`'s stray-`--pty-host` guard depends on.
fn invocation_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter();
    let _bin = args.next();
    if args.next().as_deref() != Some(PTY_HOST_FLAG) {
        return None;
    }
    Some(args.next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    use hegel::generators;
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_string()).collect()
    }

    /// The supervisor branch runs before clap, the logger and the cache-folder
    /// setup, so it has to be sure: a `--pty-host` anywhere but immediately after
    /// the binary name belongs to an ordinary session. `main` carries a guard for
    /// exactly that case and nothing verified the rule it relies on.
    #[test]
    fn only_a_leading_pty_host_flag_starts_a_supervisor() {
        assert_eq!(
            invocation_from_args(argv(&["zellij", PTY_HOST_FLAG, "session/3"])),
            Some("session/3".to_owned())
        );
        assert_eq!(
            invocation_from_args(argv(&["zellij", "-s", "mysession", PTY_HOST_FLAG, "x"])),
            None,
            "a flag after another argument is not a supervisor invocation"
        );
        assert_eq!(invocation_from_args(argv(&["zellij"])), None);
        assert_eq!(invocation_from_args(argv(&[])), None);
    }

    /// The label is only there to make `ps` readable, so a missing one is an
    /// empty label rather than a refusal — refusing would strand a pane.
    #[test]
    fn a_pty_host_flag_with_no_label_still_starts_a_supervisor() {
        assert_eq!(
            invocation_from_args(argv(&["zellij", PTY_HOST_FLAG])),
            Some(String::new())
        );
    }

    /// `--pty-host` is drawn far more often than random text would produce it, so
    /// that it lands at every position — including the one that counts.
    #[hegel::composite]
    fn argvs(tc: &hegel::TestCase) -> Vec<String> {
        let count = tc.draw(generators::integers::<usize>().max_value(5));
        (0..count)
            .map(|_| {
                if tc.draw(generators::booleans()) {
                    PTY_HOST_FLAG.to_owned()
                } else {
                    tc.draw(generators::text())
                }
            })
            .collect()
    }

    #[hegel::test]
    fn the_flag_is_recognised_at_the_second_position_and_nowhere_else(tc: hegel::TestCase) {
        let args = tc.draw(argvs());
        let expected = if args.get(1).map(String::as_str) == Some(PTY_HOST_FLAG) {
            Some(args.get(2).cloned().unwrap_or_default())
        } else {
            None
        };
        assert_eq!(
            invocation_from_args(args.clone()),
            expected,
            "argv {args:?} was classified wrongly"
        );
    }

    #[test]
    fn privileged_socket_paths_get_owner_only_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sockets");
        ensure_secure_dir(&dir).unwrap();
        assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);

        for (name, mode) in [("supervisor", 0o1700), ("broker", 0o600)] {
            let path = dir.join(name);
            let _listener = UnixListener::bind(&path).unwrap();
            secure_socket(&path, mode).unwrap();
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o7777, mode);
        }
    }

    #[test]
    fn failed_socket_permission_finalization_removes_the_socket() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("socket");
        let _listener = UnixListener::bind(&path).unwrap();

        let error = secure_socket_with(&path, 0o600, |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to set permissions 0600"));
        assert!(!path.exists());
        assert!(std::os::unix::net::UnixStream::connect(path).is_err());
    }
}
