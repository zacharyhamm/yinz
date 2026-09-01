use nix::fcntl::{fcntl, FcntlArg, OFlag};
use serde::{Deserialize, Serialize};
use std::{
    io::{ErrorKind, Read, Write},
    os::fd::AsFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

#[derive(Clone, Debug)]
pub struct Provider {
    path: PathBuf,
    timeout: Duration,
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

#[derive(Debug, Deserialize)]
struct ListResponse {
    version: u32,
    #[serde(alias = "entries")]
    vms: Vec<VmEntry>,
}

#[derive(Debug, Deserialize)]
struct EmptyResponse {
    version: u32,
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
        Ok(Self { path, timeout })
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

    pub fn stop(&self, session: &str, name: &str) -> Result<()> {
        validate_firecracker_vm_name(name).map_err(|e| anyhow!(e))?;
        let response: EmptyResponse = self.invoke(Request::Stop {
            version: PROTOCOL_VERSION,
            session,
            name,
        })?;
        self.check_version(response.version)
    }

    pub fn stop_session(&self, session: &str) -> Result<()> {
        let response: EmptyResponse = self.invoke(Request::StopSession {
            version: PROTOCOL_VERSION,
            session,
        })?;
        self.check_version(response.version)
    }

    pub fn list(&self, session: Option<&str>) -> Result<Vec<VmEntry>> {
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
        Ok(response.vms)
    }

    pub fn sync(&self, session: &str, name: &str) -> Result<()> {
        validate_firecracker_vm_name(name).map_err(|e| anyhow!(e))?;
        let response: EmptyResponse = self.invoke(Request::Sync {
            version: PROTOCOL_VERSION,
            session,
            name,
        })?;
        self.check_version(response.version)
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
        let mut child = Command::new(&self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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

        let deadline = Instant::now() + self.timeout;
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
                        let _ = child.kill();
                        let _ = child.wait();
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
            if Instant::now() >= deadline {
                if status.is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(anyhow!(
                    "firecracker provider timed out after {:?}",
                    self.timeout
                ));
            }
            if !read_any {
                thread::sleep(POLL_INTERVAL);
            }
        }

        if stdout_oversized || stderr_oversized {
            return Err(anyhow!(
                "firecracker provider output exceeded {MAX_OUTPUT} bytes"
            ));
        }
        let status = status.context("the provider output loop ended before the provider exited")?;
        if !status.success() {
            let message = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
            return Err(anyhow!(
                "firecracker provider exited with {status}: {}",
                if message.is_empty() {
                    "no error message".to_owned()
                } else {
                    message
                }
            ));
        }
        serde_json::from_slice(&stdout_bytes).with_context(|| {
            format!(
                "firecracker provider returned invalid JSON: {}",
                String::from_utf8_lossy(&stdout_bytes).trim()
            )
        })
    }
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
                let remaining = MAX_OUTPUT.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(remaining)]);
                *oversized |= read > remaining;
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
    use std::{fs, os::unix::fs::PermissionsExt};
    use tempfile::TempDir;

    fn provider_script(body: &str, timeout: Duration) -> (TempDir, Provider) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let provider = Provider::new(path, timeout).unwrap();
        (dir, provider)
    }

    #[test]
    fn provider_default_timeout_leaves_room_after_boot() {
        let provider = provider_from_options(Some(Path::new("/provider")), None).unwrap();
        assert_eq!(provider.timeout, Duration::from_secs(90));
    }

    #[test]
    fn provider_requests_and_responses_follow_the_contract() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        let log = dir.path().join("requests");
        fs::write(
            &path,
            format!(
                r#"#!/bin/sh
request=$(cat)
printf '%s\n' "$request" >> '{}'
case "$request" in
  *'"action":"ensure"'*) printf '%s' '{{"version":2,"ssh_host":"ssh://zellij@127.0.0.1:2201","cwd":"/workspace"}}' ;;
  *'"action":"list"'*) printf '%s' '{{"version":2,"vms":[{{"session":"work","name":"review","state":"running"}}]}}' ;;
  *) printf '%s' '{{"version":2}}' ;;
esac
"#,
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
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
            vec![VmEntry {
                session: "work".to_owned(),
                name: "review".to_owned(),
                state: "running".to_owned(),
            }]
        );
        provider.stop("work", "review").unwrap();
        provider.sync("work", "review").unwrap();
        provider.stop_session("work").unwrap();

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
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
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
