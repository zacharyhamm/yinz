//! The compilation worker's protocol, and the client side of it.
//!
//! Authoring and provider inspection execute in a disposable, bounded process.
//! The server binary re-executes with [`ARG`]; this client supervises its lifetime
//! and exchanges framed JSON without linking the guest interpreter.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::authoring::{PluginCheckMode, PluginCheckReport};
mod generation_identity;

/// Call while holding the installation's shared data lock, before snapshotting
/// builtin sources or pinning workers. Explicit test executables are independent
/// of the native test launcher and are validated by their fixture instead.
pub fn verify_server_generation() -> Result<()> {
    if std::env::var_os("YINZ_COMPILATION_WORKER").is_some() {
        return Ok(());
    }
    generation_identity::verify(&unpinned_server_executable()?)
}

/// The first argument that turns the server binary into a compilation worker.
pub const ARG: &str = "--plugin-compilation-worker";
/// The first argument of a `yinz plugins test` worker, which `yinz-plugin-host` spawns and
/// dispatches. Compilation workers started inside one stay in its process group.
pub const PLUGIN_TEST_WORKER_ARG: &str = "--plugin-test-worker";
/// The bound on every request and response, in bytes: an IPC frame.
pub const MAX: usize = yinz_runtime::consts::MAX_IPC_FRAME_LEN;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize)]
pub enum Operation {
    Analyze,
    Check(PluginCheckMode),
    AgentProvider,
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub operation: Operation,
    pub path: PathBuf,
    pub source: String,
    #[serde(default)]
    pub root: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Output {
    Analysis(PluginCheckReport),
    AgentProvider(yinz_types::agent::provider::AgentProvider),
}

pub type Response = std::result::Result<Output, String>;

/// Evaluate a Scheme plugin's provider descriptor in the confined, bounded worker.
/// Module initialization cannot invoke host commands; its load callback is never run.
pub fn agent_provider(
    path: &Path,
    source: &str,
) -> Result<yinz_types::agent::provider::AgentProvider> {
    match request(path, source, Operation::AgentProvider, None)? {
        Output::AgentProvider(provider) => Ok(provider),
        _ => bail!("agent provider worker returned the wrong operation"),
    }
}

pub fn analyze(path: &Path, source: &str) -> Result<PluginCheckReport> {
    match request(path, source, Operation::Analyze, None)? {
        Output::Analysis(report) => Ok(report),
        _ => bail!(
            "analysis compilation worker for {} returned the wrong operation",
            path.display()
        ),
    }
}

/// Fails when the worker does; [`crate::analysis::check`] reports that as a diagnostic instead.
pub(crate) fn check(path: &Path, source: &str, mode: PluginCheckMode) -> Result<PluginCheckReport> {
    check_with_root(path, source, mode, None)
}

pub(crate) fn check_with_root(
    path: &Path,
    source: &str,
    mode: PluginCheckMode,
    root: Option<&Path>,
) -> Result<PluginCheckReport> {
    match request(path, source, Operation::Check(mode), root)? {
        Output::Analysis(report) => Ok(report),
        _ => bail!("authoring worker returned the wrong operation"),
    }
}

fn inside_worker() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(ARG))
}

/// The production executable used by compiler and plugin runtime workers.
pub fn server_executable() -> Result<PathBuf> {
    if let Some(pinned) = PINNED_EXECUTABLE
        .lock()
        .to_anyhow()
        .context("read worker executable generation")?
        .upgrade()
    {
        return Ok(pinned.path.clone());
    }
    unpinned_server_executable()
}

fn unpinned_server_executable() -> Result<PathBuf> {
    let binary = std::env::var_os("YINZ_COMPILATION_WORKER")
        .map(PathBuf::from)
        .or_else(|| {
            if inside_worker() {
                std::env::current_exe().ok()
            } else {
                yinz_runtime::own_binary::server_binary().cloned()
            }
        })
        .context("resolve server executable")?;
    // Direct cargo/libtest launches must opt in with the freshly built executable.
    if binary.parent().is_some_and(|dir| dir.ends_with("deps")) {
        bail!("test launcher must supply YINZ_COMPILATION_WORKER; use cargo x test");
    }
    Ok(binary)
}

static PINNED_EXECUTABLE: std::sync::Mutex<std::sync::Weak<PinnedExecutable>> =
    std::sync::Mutex::new(std::sync::Weak::new());

/// Keeps future worker launches on the live server's build after installation
/// publication. The plugin thread retains this guard through worker shutdown.
pub struct PinnedExecutable {
    path: PathBuf,
    _directory: Option<tempfile::TempDir>,
}

pub fn pin_server_executable() -> Result<std::sync::Arc<PinnedExecutable>> {
    let mut existing = PINNED_EXECUTABLE
        .lock()
        .to_anyhow()
        .context("lock worker executable generation")?;
    if let Some(pinned) = existing.upgrade() {
        return Ok(pinned);
    }
    let binary = unpinned_server_executable()?;
    #[cfg(target_os = "linux")]
    let binary = if std::env::var_os("YINZ_COMPILATION_WORKER").is_none() {
        PathBuf::from("/proc/self/exe")
    } else {
        binary
    };
    let pinned = std::sync::Arc::new(pin_executable(&binary)?);
    #[cfg(target_os = "macos")]
    if std::env::var_os("YINZ_COMPILATION_WORKER").is_none() {
        // Validate the copied inode too: another installation sharing this
        // executable may publish while locking a different builtin directory.
        generation_identity::verify(&pinned.path)?;
    }
    *existing = std::sync::Arc::downgrade(&pinned);
    Ok(pinned)
}

fn pin_executable(binary: &Path) -> Result<PinnedExecutable> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let source = std::fs::metadata(binary).context("inspect worker executable")?;
        let running =
            std::fs::metadata("/proc/self/exe").context("inspect running server executable")?;
        if source.dev() == running.dev() && source.ino() == running.ino() {
            // This resolves the running inode, even after its installed path is
            // replaced. The parent outlives every supervised worker.
            return Ok(PinnedExecutable {
                path: format!("/proc/{}/exe", std::process::id()).into(),
                _directory: None,
            });
        }
    }
    let directory = tempfile::Builder::new()
        .prefix("yinz-worker-generation-")
        .tempdir()
        .context("create private worker generation")?;
    let path = directory.path().join("yinz");
    std::fs::copy(binary, &path).context("retain worker executable generation")?;
    Ok(PinnedExecutable {
        path,
        _directory: Some(directory),
    })
}

fn request(path: &Path, source: &str, operation: Operation, root: Option<&Path>) -> Result<Output> {
    let context = format!("{operation:?} compilation worker for {}", path.display());
    (|| {
        crate::limits::check_source(path, source)?;
        let request = Request {
            operation,
            path: path.to_owned(),
            source: source.to_owned(),
            root: root.map(Path::to_owned),
        };
        let input = serde_json::to_vec(&request).context("encode request")?;
        let binary = server_executable()?;
        exchange(Command::new(binary).arg(ARG), &input, DEADLINE)
    })()
    .with_context(|| context)
}

/// How many workers this process has spawned. Tests assert deltas against it, never
/// absolutes: under `cargo test` every test in the binary shares the counter.
static SPAWNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn spawn_count() -> usize {
    SPAWNS.load(std::sync::atomic::Ordering::Relaxed)
}

struct Reap(Child, bool);
impl Drop for Reap {
    fn drop(&mut self) {
        // All worker descendants share this process group.
        if self.1 {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(fd: &impl AsRawFd) -> Result<()> {
    // These are our newly created pipes, with no other users of their flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error()).context("set worker pipe nonblocking");
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool> {
    let mut buffer = [0; 8192];
    // One chunk per turn keeps the deadline effective even for an endless writer.
    match pipe.read(&mut buffer) {
        Ok(0) => Ok(true),
        Ok(n) => {
            if n > MAX - bytes.len() {
                bail!("worker output exceeds frame limit");
            }
            bytes.extend_from_slice(&buffer[..n]);
            Ok(false)
        },
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        },
        Err(error) => Err(error).context("read worker output"),
    }
}

fn exchange(command: &mut Command, input: &[u8], timeout: Duration) -> Result<Output> {
    exchange_cancel(command, input, timeout, None)
}

pub fn exchange_cancel<T: serde::de::DeserializeOwned>(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<T> {
    let (status, output, errors) = supervise_cancel(command, input, timeout, cancel)?;
    if !status.success() {
        bail!(
            "worker exited with {status}: {}",
            String::from_utf8_lossy(&errors)
        );
    }
    let response: std::result::Result<T, String> =
        serde_json::from_slice(&output).context("invalid worker response")?;
    response.map_err(|error| anyhow!(error))
}

pub fn supervise(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    supervise_cancel(command, input, timeout, None)
}

pub fn supervise_cancel(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    if input.len() > MAX {
        bail!("worker request exceeds frame limit");
    }
    let deadline = Instant::now() + timeout;
    // Compilation inside a test stays in the test worker's group so cancellation
    // reaps the complete test tree, including any compiler descendants.
    let owns_group = !inside_worker()
        && std::env::args_os().nth(1).as_deref()
            != Some(std::ffi::OsStr::new(PLUGIN_TEST_WORKER_ARG));
    if owns_group {
        command.process_group(0);
    }
    let mut child = Reap(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn worker")?,
        owns_group,
    );
    SPAWNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stdin = child.0.stdin.take().context("worker stdin")?;
    let mut stdout = child.0.stdout.take().context("worker stdout")?;
    let mut stderr = child.0.stderr.take().context("worker stderr")?;
    nonblocking(&stdin)?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut remaining = input;
    let mut stdin = Some(stdin);
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let (mut stdout_done, mut stderr_done) = (false, false);
    loop {
        if cancel.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire)) {
            bail!("operation cancelled");
        }
        if Instant::now() >= deadline {
            bail!("worker exceeded {} second deadline", timeout.as_secs_f64());
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(remaining) {
                Ok(0) if !remaining.is_empty() => bail!("worker closed input early"),
                Ok(n) => remaining = &remaining[n..],
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {},
                // The worker stopped reading, usually by exiting. Its status and stderr say why,
                // so stop writing and collect them below rather than report the broken pipe.
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    stdin.take();
                },
                Err(error) => return Err(error).context("write worker request"),
            }
            if remaining.is_empty() {
                stdin.take();
            }
        }
        if !stdout_done {
            stdout_done = drain(&mut stdout, &mut output)?;
        }
        if !stderr_done {
            stderr_done = drain(&mut stderr, &mut errors)?;
        }
        if let Some(status) = child.0.try_wait().context("wait for worker")?
            && stdout_done
            && stderr_done
        {
            if !remaining.is_empty() {
                bail!(
                    "worker exited with {status} before reading its request: {}",
                    String::from_utf8_lossy(&errors)
                );
            }
            return Ok((status, output, errors));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
#[path = "unit/worker.rs"]
mod tests;
