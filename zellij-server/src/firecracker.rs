use nix::fcntl::{fcntl, FcntlArg, OFlag};
use serde::{Deserialize, Serialize};
use std::{
    io::{ErrorKind, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::validate_firecracker_vm_name;

#[cfg(target_os = "linux")]
pub mod provider;

const PROTOCOL_VERSION: u32 = 2;
const MAX_OUTPUT: usize = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// A stderr line the provider writes to say what it is doing. Any output resets
/// the inactivity timeout; lines with this prefix are also kept out of error
/// messages, and the last one names the phase a silent provider was in.
pub const PROGRESS_PREFIX: &str = "progress: ";

#[derive(Clone, Debug)]
pub struct Provider {
    path: PathBuf,
    timeout: Duration,
    invocation_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request<'a> {
    Ensure {
        version: u32,
        session: &'a str,
        name: &'a str,
        image: Option<&'a str>,
        worktree: Option<&'a Path>,
    },
    Stop {
        version: u32,
        session: &'a str,
        name: &'a str,
    },
    StopSession {
        version: u32,
        session: &'a str,
    },
    List {
        version: u32,
        session: Option<&'a str>,
    },
    Sync {
        version: u32,
        session: &'a str,
        name: &'a str,
    },
    RenameSession {
        version: u32,
        from: &'a str,
        to: &'a str,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnsureResponse {
    pub version: u32,
    pub ssh_host: String,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub ssh_identity_file: Option<PathBuf>,
    #[serde(default)]
    pub ssh_known_hosts_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VmEntry {
    pub session: String,
    pub name: String,
    pub state: String,
}

/// A guest disk the provider kept instead of deleting: a VM that crashed, failed to
/// provision, or could not be merged back into its worktree when it was released.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecoveryEntry {
    pub path: PathBuf,
    pub reason: String,
    /// Unix seconds.
    pub retained_at: u64,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub worktree: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Listing {
    pub vms: Vec<VmEntry>,
    pub recoveries: Vec<RecoveryEntry>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    version: u32,
    vms: Vec<VmEntry>,
    #[serde(default)]
    recoveries: Vec<RecoveryEntry>,
}

/// `stop`, `stop_session` and `sync` answer with the version and, since the worktree merge,
/// one line per file the merge could not decide. A provider without merges sends none.
#[derive(Debug, Deserialize)]
struct WarningsResponse {
    version: u32,
    #[serde(default)]
    warnings: Vec<String>,
}

impl Provider {
    pub fn new(path: PathBuf, timeout: Duration) -> Result<Self> {
        if !path.is_absolute() {
            return Err(anyhow!(
                "firecracker_provider must be an absolute path, got {}",
                path.display()
            ));
        }
        if timeout.is_zero() {
            return Err(anyhow!(
                "firecracker_provider_timeout_secs must be greater than zero"
            ));
        }
        Ok(Self {
            path,
            timeout,
            invocation_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn ensure(
        &self,
        session: &str,
        name: &str,
        image: Option<&str>,
        worktree: Option<&Path>,
    ) -> Result<EnsureResponse> {
        validate_firecracker_vm_name(name).map_err(|e| anyhow!(e))?;
        if let Some(image) = image {
            validate_firecracker_vm_name(image).map_err(|e| anyhow!(e))?;
        }
        if worktree.is_some_and(|path| !path.is_absolute()) {
            return Err(anyhow!("firecracker_worktree must be an absolute path"));
        }
        let response: EnsureResponse = self.invoke(Request::Ensure {
            version: PROTOCOL_VERSION,
            session,
            name,
            image,
            worktree,
        })?;
        self.check_version(response.version)?;
        if response.ssh_host.trim().is_empty() {
            return Err(anyhow!("firecracker provider returned an empty ssh_host"));
        }
        for (field, path) in [
            ("ssh_identity_file", response.ssh_identity_file.as_deref()),
            (
                "ssh_known_hosts_file",
                response.ssh_known_hosts_file.as_deref(),
            ),
        ] {
            if path.is_some_and(|path| !path.is_absolute()) {
                return Err(anyhow!("firecracker provider returned a relative {field}"));
            }
        }
        Ok(response)
    }

    /// Release one VM. The returned warnings describe files the worktree merge kept the host
    /// copy of; the VM is stopped either way.
    pub fn stop(&self, session: &str, name: &str) -> Result<Vec<String>> {
        validate_firecracker_vm_name(name).map_err(|e| anyhow!(e))?;
        let response: WarningsResponse = self.invoke(Request::Stop {
            version: PROTOCOL_VERSION,
            session,
            name,
        })?;
        self.check_version(response.version)?;
        Ok(response.warnings)
    }

    /// Release every VM the session holds; see `stop` for the warnings.
    pub fn stop_session(&self, session: &str) -> Result<Vec<String>> {
        let response: WarningsResponse = self.invoke(Request::StopSession {
            version: PROTOCOL_VERSION,
            session,
        })?;
        self.check_version(response.version)?;
        Ok(response.warnings)
    }

    /// Move every VM `from` owns or leases to `to`, after the Zellij session was renamed.
    ///
    /// Additive to protocol version 2: a provider that does not know the request
    /// fails it, and the caller keeps its VMs under the old name.
    pub fn rename_session(&self, from: &str, to: &str) -> Result<()> {
        let response: WarningsResponse = self.invoke(Request::RenameSession {
            version: PROTOCOL_VERSION,
            from,
            to,
        })?;
        self.check_version(response.version)
    }

    pub fn list(&self, session: Option<&str>) -> Result<Listing> {
        let response: ListResponse = self.invoke(Request::List {
            version: PROTOCOL_VERSION,
            session,
        })?;
        self.check_version(response.version)?;
        for entry in &response.vms {
            validate_firecracker_vm_name(&entry.name)
                .map_err(|e| anyhow!(e))
                .context("firecracker provider returned an invalid VM name")?;
        }
        Ok(Listing {
            vms: response.vms,
            recoveries: response.recoveries,
        })
    }

    /// Merge the guest into the host worktree while the VM keeps running; see `stop` for the
    /// warnings.
    pub fn sync(&self, session: &str, name: &str) -> Result<Vec<String>> {
        validate_firecracker_vm_name(name).map_err(|e| anyhow!(e))?;
        let response: WarningsResponse = self.invoke(Request::Sync {
            version: PROTOCOL_VERSION,
            session,
            name,
        })?;
        self.check_version(response.version)?;
        Ok(response.warnings)
    }

    fn check_version(&self, version: u32) -> Result<()> {
        if version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(anyhow!(
                "firecracker provider protocol mismatch: expected version {PROTOCOL_VERSION}, got {version}"
            ))
        }
    }

    fn invoke<T: for<'de> Deserialize<'de>>(&self, request: Request<'_>) -> Result<T> {
        let request_json = serde_json::to_vec(&request)
            .context("failed to encode the firecracker provider request")?;
        let _invocation = self.invocation_lock.lock().to_anyhow()?;
        // Its own process group, so a timeout takes the provider's helpers - an
        // rsync, an ssh, a git clone - down with it rather than leaving them to
        // race the next attempt. The VM itself is started in a group of its own
        // by the provider and is meant to outlive this.
        let mut child = Command::new(&self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start firecracker provider {}",
                    self.path.display()
                )
            })?;

        if let Err(error) = child
            .stdin
            .take()
            .context("firecracker provider stdin was unavailable")?
            .write_all(&request_json)
        {
            if error.kind() != ErrorKind::BrokenPipe {
                return Err(error).context("failed to write the firecracker provider request");
            }
        }

        let mut stdout = child
            .stdout
            .take()
            .context("firecracker provider stdout was unavailable")?;
        let mut stderr = child
            .stderr
            .take()
            .context("firecracker provider stderr was unavailable")?;
        set_nonblocking(&stdout).context("failed to make provider stdout nonblocking")?;
        set_nonblocking(&stderr).context("failed to make provider stderr nonblocking")?;

        // The timeout is one of inactivity: booting a guest and copying a large
        // worktree into it take as long as they take, and the provider reports
        // each phase on stderr while it works. Only a provider that has gone
        // silent for the whole timeout is killed.
        let mut deadline = Instant::now() + self.timeout;
        let mut status = None;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut stdout_oversized = false;
        let mut stderr_oversized = false;
        loop {
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(exit_status)) => status = Some(exit_status),
                    Ok(None) => {},
                    Err(error) => {
                        kill_group(&mut child);
                        return Err(error).context("failed waiting for the firecracker provider");
                    },
                }
            }

            let read_any = read_available(&mut stdout, &mut stdout_bytes, &mut stdout_oversized)
                .context("failed reading firecracker provider stdout")?
                | read_available(&mut stderr, &mut stderr_bytes, &mut stderr_oversized)
                    .context("failed reading firecracker provider stderr")?;
            if status.is_some() && !read_any {
                break;
            }
            if read_any {
                deadline = Instant::now() + self.timeout;
            } else if Instant::now() >= deadline {
                if status.is_none() {
                    kill_group(&mut child);
                }
                let (last_progress, _) = split_progress(&stderr_bytes);
                return Err(anyhow!(
                    "firecracker provider was silent for {:?}{}",
                    self.timeout,
                    last_progress
                        .map(|phase| format!(" (last: {phase})"))
                        .unwrap_or_default()
                ));
            } else {
                thread::sleep(POLL_INTERVAL);
            }
        }

        let status = status.context("the provider output loop ended before the provider exited")?;
        if !status.success() {
            // What the provider had to say about the failure comes before how much it
            // said: the kept tail of an oversize stderr still holds the message.
            let (_, message) = split_progress(&stderr_bytes);
            return Err(anyhow!(
                "firecracker provider exited with {status}: {}{}",
                if message.is_empty() {
                    "no error message".to_owned()
                } else {
                    message
                },
                if stderr_oversized {
                    format!(" (stderr truncated to its last {MAX_OUTPUT} bytes)")
                } else {
                    String::new()
                }
            ));
        }
        if stdout_oversized || stderr_oversized {
            return Err(anyhow!(
                "firecracker provider output exceeded {MAX_OUTPUT} bytes"
            ));
        }
        // A provider that succeeded still has things to say - a VM started without host
        // resource limits, a merge it could not decide - and this is the only place they
        // can be said: the clients never see the provider's stderr.
        let (_, message) = split_progress(&stderr_bytes);
        if !message.is_empty() {
            log::warn!("firecracker provider: {message}");
        }
        serde_json::from_slice(&stdout_bytes).with_context(|| {
            format!(
                "firecracker provider returned invalid JSON: {}",
                String::from_utf8_lossy(&stdout_bytes).trim()
            )
        })
    }
}

/// Kill the provider and everything it started, then reap it.
fn kill_group(child: &mut std::process::Child) {
    // The provider was spawned as the leader of its own process group.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Separate the provider's progress reports from what else it wrote to stderr:
/// the phase it last reported, and the rest as an error message.
fn split_progress(stderr: &[u8]) -> (Option<String>, String) {
    let mut last_progress = None;
    let mut message = Vec::new();
    let stderr = String::from_utf8_lossy(stderr);
    for line in stderr.lines() {
        match line.strip_prefix(PROGRESS_PREFIX) {
            Some(phase) => last_progress = Some(phase.trim().to_owned()),
            None => message.push(line),
        }
    }
    (last_progress, message.join("\n").trim().to_owned())
}

fn set_nonblocking(pipe: &impl AsFd) -> Result<()> {
    let flags = fcntl(pipe.as_fd(), FcntlArg::F_GETFL)?;
    let mut flags = OFlag::from_bits_truncate(flags);
    flags.insert(OFlag::O_NONBLOCK);
    fcntl(pipe.as_fd(), FcntlArg::F_SETFL(flags))?;
    Ok(())
}

fn read_available(
    pipe: &mut impl Read,
    kept: &mut Vec<u8>,
    oversized: &mut bool,
) -> std::io::Result<bool> {
    let mut chunk = [0; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok(false),
            Ok(read) => {
                // The last bytes are the ones worth keeping: a provider says what went
                // wrong as it exits, after whatever it had to log on the way there.
                kept.extend_from_slice(&chunk[..read]);
                if kept.len() > MAX_OUTPUT {
                    kept.drain(..kept.len() - MAX_OUTPUT);
                    *oversized = true;
                }
                return Ok(true);
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
        }
    }
}

pub fn provider_from_options(path: Option<&Path>, timeout_secs: Option<u64>) -> Result<Provider> {
    let path = path.context(
        "Firecracker is not configured; set firecracker_provider to an absolute executable path",
    )?;
    Provider::new(
        path.to_path_buf(),
        Duration::from_secs(timeout_secs.unwrap_or(90)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    /// Written by a child process rather than with `fs::write`. Under `cargo test`
    /// these tests share one process, and a child another test forks in the
    /// meantime inherits every open descriptor until it execs - a script still
    /// open for writing here then fails that exec with `Text file busy`. A
    /// writer in its own process leaves nothing to inherit.
    fn write_script(path: &Path, contents: &str) {
        let mut writer = Command::new("sh")
            .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
    }

    fn provider_script(body: &str, timeout: Duration) -> (TempDir, Provider) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        write_script(&path, &format!("#!/bin/sh\n{body}\n"));
        let provider = Provider::new(path, timeout).unwrap();
        (dir, provider)
    }

    #[test]
    fn provider_default_inactivity_timeout_is_90s() {
        let provider = provider_from_options(Some(Path::new("/provider")), None).unwrap();
        assert_eq!(provider.timeout, Duration::from_secs(90));
    }

    #[test]
    fn provider_requests_and_responses_follow_the_contract() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        let log = dir.path().join("requests");
        write_script(
            &path,
            &format!(
                r#"#!/bin/sh
request=$(cat)
printf '%s\n' "$request" >> '{}'
case "$request" in
  *'"action":"ensure"'*) printf '%s' '{{"version":2,"ssh_host":"ssh://zellij@127.0.0.1:2201","cwd":"/workspace"}}' ;;
  *'"action":"list"'*) printf '%s' '{{"version":2,"vms":[{{"session":"work","name":"review","state":"running"}}],"recoveries":[{{"path":"/state/recovery/abc-1-sync-failed","reason":"sync-failed","retained_at":1,"session":"work","name":"review","worktree":"/repo"}}]}}' ;;
  *'"action":"sync"'*) printf '%s' '{{"version":2,"warnings":["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest"]}}' ;;
  *) printf '%s' '{{"version":2}}' ;;
esac
"#,
                log.display()
            ),
        );
        let provider = Provider::new(path, Duration::from_secs(1)).unwrap();

        let first = provider
            .ensure("work", "review", Some("rust"), Some(Path::new("/repo")))
            .unwrap();
        let second = provider
            .ensure("work", "review", Some("rust"), Some(Path::new("/repo")))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.ssh_host, "ssh://zellij@127.0.0.1:2201");
        assert_eq!(first.cwd, Some(PathBuf::from("/workspace")));
        assert_eq!(first.ssh_identity_file, None);
        assert_eq!(first.ssh_known_hosts_file, None);
        assert_eq!(
            provider.list(None).unwrap(),
            Listing {
                vms: vec![VmEntry {
                    session: "work".to_owned(),
                    name: "review".to_owned(),
                    state: "running".to_owned(),
                }],
                recoveries: vec![RecoveryEntry {
                    path: PathBuf::from("/state/recovery/abc-1-sync-failed"),
                    reason: "sync-failed".to_owned(),
                    retained_at: 1,
                    session: Some("work".to_owned()),
                    name: Some("review".to_owned()),
                    worktree: Some(PathBuf::from("/repo")),
                }],
            }
        );
        // A response without the additive fields is the older contract, still accepted.
        assert_eq!(
            provider.stop("work", "review").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            provider.sync("work", "review").unwrap(),
            ["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest"]
        );
        assert_eq!(provider.stop_session("work").unwrap(), Vec::<String>::new());

        let requests: Vec<serde_json::Value> = fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            requests,
            vec![
                json!({"version": 2, "action": "ensure", "session": "work", "name": "review", "image": "rust", "worktree": "/repo"}),
                json!({"version": 2, "action": "ensure", "session": "work", "name": "review", "image": "rust", "worktree": "/repo"}),
                json!({"version": 2, "action": "list", "session": null}),
                json!({"version": 2, "action": "stop", "session": "work", "name": "review"}),
                json!({"version": 2, "action": "sync", "session": "work", "name": "review"}),
                json!({"version": 2, "action": "stop_session", "session": "work"}),
            ]
        );
    }

    #[test]
    fn malformed_provider_output_is_rejected() {
        let (_dir, provider) = provider_script("printf 'not json'", Duration::from_secs(1));
        let error = provider
            .ensure("work", "review", None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid JSON"), "{error}");
    }

    #[test]
    fn provider_exit_failures_include_stderr() {
        let (_dir, provider) =
            provider_script("printf 'broken' >&2; exit 7", Duration::from_secs(1));
        let error = provider
            .ensure("work", "review", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("exit status: 7") && error.contains("broken"),
            "{error}"
        );
    }

    #[test]
    fn provider_timeout_is_enforced() {
        let (_dir, provider) =
            provider_script("sleep 2 & while :; do :; done", Duration::from_millis(200));
        let started = Instant::now();
        let error = provider
            .ensure("work", "review", None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("silent for"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cloned_provider_invocations_are_serialized_before_their_timeout_starts() {
        let (dir, provider) = provider_script(
            r#"lock="$(dirname "$0")/lock"
if ! mkdir "$lock"; then touch "$(dirname "$0")/overlap"; fi
sleep 0.6
rmdir "$lock"
printf '{"version":2}'"#,
            Duration::from_secs(1),
        );
        let other = provider.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let started = Instant::now();
        let threads: Vec<_> = [provider, other]
            .into_iter()
            .map(|provider| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    provider.stop_session("work")
                })
            })
            .collect();

        barrier.wait();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert!(started.elapsed() > Duration::from_secs(1));
        assert!(!dir.path().join("overlap").exists());
    }

    /// Booting a guest and copying a worktree into it take as long as they take.
    /// A provider that keeps saying what it is doing is not timed out, however
    /// long the whole request runs.
    #[test]
    fn progress_output_keeps_a_working_provider_alive() {
        let (_dir, provider) = provider_script(
            "for i in 1 2 3 4; do echo 'progress: copying' >&2; sleep 0.3; done\n\
             printf '{\"version\":2}'",
            Duration::from_millis(500),
        );
        provider.stop_session("work").unwrap();
    }

    #[test]
    fn a_silent_provider_is_reported_with_its_last_phase() {
        let (_dir, provider) = provider_script(
            "echo 'progress: booting the guest' >&2\n\
             echo 'progress: copying the worktree into the guest' >&2\n\
             sleep 5",
            Duration::from_millis(300),
        );
        let error = provider.stop_session("work").unwrap_err().to_string();
        assert!(error.contains("silent for 300ms"), "{error}");
        assert!(
            error.ends_with("(last: copying the worktree into the guest)"),
            "{error}"
        );
    }

    /// A provider that warns and still succeeds - a VM started without host resource
    /// limits, say - keeps its response. The warning goes to the log; the caller sees
    /// only the VM it asked for.
    #[test]
    fn a_warning_on_stderr_does_not_spoil_a_successful_response() {
        let (_dir, provider) = provider_script(
            "echo 'progress: booting the guest' >&2\n\
             echo 'warning: this VM runs without host-side limits' >&2\n\
             printf '{\"version\":2,\"ssh_host\":\"ssh://zellij@127.0.0.1:2201\"}'",
            Duration::from_secs(1),
        );
        let response = provider.ensure("work", "review", None, None).unwrap();
        assert_eq!(response.ssh_host, "ssh://zellij@127.0.0.1:2201");
    }

    /// A guest that failed to boot can leave a console log bigger than the output cap; the
    /// failure it ends with is still the message, not the size of what preceded it.
    #[test]
    fn a_failing_provider_with_oversize_output_reports_its_stderr() {
        let (_dir, provider) = provider_script(
            &format!(
                "head -c {} /dev/zero | tr '\\0' 'x' >&2\necho >&2\necho 'the guest never answered' >&2\nexit 1",
                MAX_OUTPUT + 10
            ),
            Duration::from_secs(5),
        );
        let error = provider.stop_session("work").unwrap_err().to_string();
        assert!(error.contains("the guest never answered"), "{error}");
        assert!(error.contains("truncated"), "{error}");
        assert!(!error.contains("output exceeded"), "{error}");
    }

    #[test]
    fn progress_lines_are_kept_out_of_provider_errors() {
        let (_dir, provider) = provider_script(
            "echo 'progress: booting the guest' >&2\necho 'no free TAP devices' >&2\nexit 1",
            Duration::from_secs(1),
        );
        let error = provider.stop_session("work").unwrap_err().to_string();
        assert!(error.ends_with(": no free TAP devices"), "{error}");
        assert!(!error.contains("progress"), "{error}");
    }

    /// The helpers a provider runs - rsync, ssh, git - must not outlive it: a
    /// copy still running after the kill would race the attempt that resumes it.
    #[test]
    fn a_timed_out_provider_takes_its_children_with_it() {
        let (dir, provider) = provider_script(
            r#"sleep 60 & echo $! > "$(dirname "$0")/child"; wait"#,
            Duration::from_millis(200),
        );
        let error = provider.stop_session("work").unwrap_err().to_string();
        assert!(error.contains("silent for"), "{error}");
        let pid: i32 = fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "the provider's child survived the timeout"
        );
    }

    #[test]
    fn provider_does_not_wait_for_pipes_inherited_by_a_background_child() {
        let (_dir, provider) = provider_script(
            "sleep 2 & printf '{\"version\":2,\"ssh_host\":\"build-box\"}'",
            Duration::from_secs(1),
        );
        let started = Instant::now();

        assert_eq!(
            provider
                .ensure("work", "review", None, None)
                .unwrap()
                .ssh_host,
            "build-box"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn oversized_provider_output_is_rejected() {
        let (_dir, provider) = provider_script(
            "dd if=/dev/zero bs=1048577 count=1 2>/dev/null",
            Duration::from_secs(1),
        );
        let error = provider
            .ensure("work", "review", None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("output exceeded"), "{error}");
    }
}
