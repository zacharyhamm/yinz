//! Owned OpenSSH command and Unix-socket forwarding lifecycle.
use crate::pty_host::protocol::check_socket_path_length;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;

const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const SSH_OUTPUT_CAPACITY: usize = 64 * 1024;

/// A live ssh forward to one host. Dropping it tears the forward down.
pub struct HostForward {
    pub(crate) host: String,
    pub(crate) local_socket: PathBuf,
    pub(crate) ssh: Child,
    pub(crate) stderr: Option<SshOutputCapture>,
}

/// Drain SSH output throughout its lifetime, retaining only the final 64 KiB.
// shortcut: one polling thread per capture; share readiness polling if host counts grow large.
pub(crate) struct SshOutputCapture {
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl SshOutputCapture {
    pub(crate) fn new(mut pipe: impl Read + AsFd + Send + 'static) -> Result<Self> {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        let flags =
            fcntl(pipe.as_fd(), FcntlArg::F_GETFL).context("failed to read SSH output flags")?;
        fcntl(
            pipe.as_fd(),
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .context("failed to make SSH output nonblocking")?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let reader = std::thread::Builder::new()
            .name("ssh-output".into())
            .spawn(move || {
                let mut output = std::collections::VecDeque::with_capacity(SSH_OUTPUT_CAPACITY);
                let mut chunk = [0; 8192];
                let mut final_drain: usize = 1024 * 1024;
                loop {
                    let stopping = stopping.load(Ordering::Acquire);
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => {
                            output.extend(&chunk[..read]);
                            output.drain(..output.len().saturating_sub(SSH_OUTPUT_CAPACITY));
                            // Drain up to 1 MiB queued after exit; descendants still
                            // writing output cannot keep teardown alive forever.
                            if stopping {
                                final_drain = final_drain.saturating_sub(read);
                                if final_drain == 0 {
                                    break;
                                }
                            }
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if stopping {
                                break;
                            }
                            let mut ready = [nix::poll::PollFd::new(
                                pipe.as_fd(),
                                nix::poll::PollFlags::POLLIN,
                            )];
                            match nix::poll::poll(&mut ready, 50u16) {
                                Err(error) if error != nix::errno::Errno::EINTR => {
                                    Err::<(), _>(error)
                                        .context("failed to poll SSH output")
                                        .non_fatal();
                                    break;
                                },
                                _ => {},
                            }
                        },
                        Err(error) => {
                            Err::<(), _>(error)
                                .context("failed to read SSH output")
                                .non_fatal();
                            break;
                        },
                    }
                }
                output.into_iter().collect()
            })
            .context("failed to start the SSH output reader")?;
        Ok(Self {
            stop,
            reader: Some(reader),
        })
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>> {
        self.stop.store(true, Ordering::Release);
        let Some(reader) = self.reader.take() else {
            return Ok(Vec::new());
        };
        reader
            .join()
            .map_err(|_| anyhow!("the SSH output reader panicked"))
    }
}

impl Drop for SshOutputCapture {
    fn drop(&mut self) {
        self.finish().non_fatal();
    }
}

impl Drop for HostForward {
    fn drop(&mut self) {
        let _ = self.ssh.kill();
        let _ = self.ssh.wait();
        let _ = std::fs::remove_file(&self.local_socket);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SshOptions {
    pub identity_file: Option<PathBuf>,
    pub known_hosts_file: Option<PathBuf>,
}

impl SshOptions {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![];
        if let Some(path) = &self.identity_file {
            // `-i` takes its whole argument as the path, so it needs no quoting.
            args.extend([
                "-o".to_owned(),
                "IdentitiesOnly=yes".to_owned(),
                "-i".to_owned(),
                path.display().to_string(),
            ]);
        }
        if let Some(path) = &self.known_hosts_file {
            args.extend([
                "-o".to_owned(),
                format!("UserKnownHostsFile={}", ssh_option_value(path)),
                "-o".to_owned(),
                "StrictHostKeyChecking=yes".to_owned(),
            ]);
        }
        args
    }
}

/// A path as the value of an `-o Name=value` ssh option.
///
/// ssh reads the option string with its config-file tokenizer, which splits on
/// whitespace even when the whole `Name=value` arrived as one argv entry, so an
/// unquoted path with a space in it is silently cut at the space. Double quotes
/// keep the value whole; a quote or backslash inside the path is escaped, which
/// OpenSSH 8.7 and later understand.
pub(crate) fn ssh_option_value(path: &Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Reject options and shell syntax before passing a destination to OpenSSH.
pub fn validate_ssh_host(host: &str) -> Result<()> {
    if host.is_empty() {
        return Err(anyhow!("refusing to ssh: the remote host is empty"));
    }
    if host.starts_with('-') {
        return Err(anyhow!(
            "refusing remote host {host:?}: a host beginning with '-' would be \
             interpreted as an ssh option, not a destination"
        ));
    }
    if let Some(bad) = host.chars().find(|c| c.is_whitespace() || c.is_control()) {
        return Err(anyhow!(
            "refusing remote host {host:?}: it contains {bad:?}, which is not valid \
             in a remote host"
        ));
    }
    anyhow::ensure!(
        host.chars()
            .all(|c| c.is_alphanumeric() || "-._@:/[]%+".contains(c)),
        "refusing remote host {host:?}: shell metacharacters are not allowed in a destination"
    );
    Ok(())
}

pub fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<()> {
    anyhow::ensure!(
        !cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)),
        "Interrupted while starting up"
    );
    Ok(())
}

pub fn bootstrap_output(
    command: &mut Command,
    timeout: Duration,
    cancelled: Option<&AtomicBool>,
) -> Result<Output> {
    check_cancelled(cancelled)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn the bootstrap ssh")?;
    let result = (|| {
        let mut stdout = SshOutputCapture::new(
            child
                .stdout
                .take()
                .context("bootstrap stdout was unavailable")?,
        )?;
        let mut stderr = SshOutputCapture::new(
            child
                .stderr
                .take()
                .context("bootstrap stderr was unavailable")?,
        )?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            check_cancelled(cancelled)?;
            match child
                .try_wait()
                .context("failed waiting for the bootstrap ssh")?
            {
                Some(status) => break status,
                None if Instant::now() < deadline => std::thread::sleep(FORWARD_POLL_INTERVAL),
                None => {
                    return Err(anyhow!(
                        "timed out waiting for the bootstrap ssh after {timeout:?}"
                    ))
                },
            }
        };
        Ok(Output {
            status,
            stdout: stdout
                .finish()
                .context("failed to collect bootstrap stdout")?,
            stderr: stderr
                .finish()
                .context("failed to collect bootstrap stderr")?,
        })
    })();
    if result.is_err() {
        // Includes capture setup failures: no spawned child can escape an error path.
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Multiplexing transfers the forward to a longer-lived master; disable it so
/// dropping the dedicated SSH process also closes the forward.
pub fn ssh_forward_args(options: &SshOptions) -> Vec<String> {
    let mut args = vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=no".to_owned(),
        "-o".to_owned(),
        "ControlPath=none".to_owned(),
        "-o".to_owned(),
        "ControlPersist=no".to_owned(),
        "-o".to_owned(),
        "ForkAfterAuthentication=no".to_owned(),
        "-o".to_owned(),
        // The bootstrap master may already own forwards from the user's SSH config.
        "ExitOnForwardFailure=no".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
    ];
    args.extend(options.args());
    args
}

impl HostForward {
    pub fn socket(&self) -> &Path {
        &self.local_socket
    }

    pub fn diagnostics(&mut self) -> Result<String> {
        let output = self
            .stderr
            .as_mut()
            .map(SshOutputCapture::finish)
            .transpose()?
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(&output).trim().to_owned())
    }

    pub fn start(
        host: &str,
        options: &SshOptions,
        local_socket: &Path,
        remote_socket: &str,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Self> {
        Self::start_until(
            host,
            options,
            local_socket,
            remote_socket,
            cancelled,
            Instant::now() + FORWARD_READY_TIMEOUT,
        )
    }

    /// The caller's deadline also bounds forwarding setup.
    pub fn start_until(
        host: &str,
        options: &SshOptions,
        local_socket: &Path,
        remote_socket: &str,
        cancelled: Option<&AtomicBool>,
        deadline: Instant,
    ) -> Result<Self> {
        check_cancelled(cancelled)?;
        anyhow::ensure!(Instant::now() < deadline, "SSH forwarding deadline expired");
        validate_ssh_host(host)?;
        check_socket_path_length(local_socket, 0)?;
        match std::fs::symlink_metadata(local_socket) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error).context("inspect SSH forwarding socket"),
            Ok(_) => bail!(
                "refusing to replace existing forwarding socket {}",
                local_socket.display()
            ),
        }
        let ssh = Command::new("ssh")
            .args(ssh_forward_args(options))
            .args(["-o", "StdinNull=no"])
            .arg("-T")
            .arg("-L")
            .arg(format!(
                "{}:{}",
                forward_path(local_socket)?,
                forward_path(Path::new(remote_socket))?
            ))
            // End of options: see `validate_ssh_host`.
            .arg("--")
            .arg(host)
            // A command makes sshd enable TCP_NODELAY; forwarding alone delays small updates.
            .arg("exec cat")
            // Child owns this open pipe until teardown, keeping the remote command alive.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start the ssh forward to {host}"))?;

        let mut forward = HostForward {
            host: host.to_owned(),
            local_socket: local_socket.to_owned(),
            ssh,
            stderr: None,
        };
        let stderr = forward
            .ssh
            .stderr
            .take()
            .context("SSH stderr was unavailable")?;
        forward.stderr = Some(SshOutputCapture::new(stderr)?);

        // Wait for the forward to actually carry a connection. `ssh` returning
        // immediately does not mean the socket is bound yet.
        let deadline = deadline.min(Instant::now() + FORWARD_READY_TIMEOUT);
        loop {
            check_cancelled(cancelled)?;
            if UnixStream::connect(&forward.local_socket).is_ok() {
                return Ok(forward);
            }
            if let Ok(Some(status)) = forward.ssh.try_wait() {
                let stderr = forward
                    .stderr
                    .as_mut()
                    .map(SshOutputCapture::finish)
                    .transpose()?
                    .unwrap_or_default();
                let stderr = String::from_utf8_lossy(&stderr);
                return Err(anyhow!(
                    "the ssh forward to {} exited ({status}): {}",
                    forward.host,
                    stderr.trim()
                ));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for the ssh forward to {} to come up. Does its sshd allow \
                 unix-socket forwarding? (OpenSSH only; Dropbear and most embedded sshd's do \
                 not implement it)",
                    forward.host
                ));
            }
            std::thread::sleep(FORWARD_POLL_INTERVAL);
        }
    }
}

fn forward_path(path: &Path) -> Result<String> {
    let path = path.to_str().context("SSH socket path is not UTF-8")?;
    anyhow::ensure!(
        path.starts_with('/') && !path.chars().any(char::is_control),
        "SSH socket path must be absolute and contain no control characters"
    );
    // Escape percent tokens and braces before OpenSSH expands %... and ${...}.
    // Its forwarding parser removes backslash escapes.
    Ok(path
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('{', "\\{"))
}

/// OpenSSH joins argv with spaces before invoking the remote shell.
pub fn shell_argument(value: &str) -> Result<String> {
    anyhow::ensure!(!value.contains('\0'), "remote argument contains NUL");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

#[cfg(test)]
#[path = "unit/ssh_tests.rs"]
mod tests;
