//! SWC owns recursively allocated native ASTs. Only a disposable process may
//! parse, inspect, transpile, or drop them; even successful output needs a clean exit.
use super::{analysis, limits, loader};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use zellij_runtime::errors::prelude::*;
use zellij_types::authoring::PluginCheckReport;

const ARG: &str = "--deno-compilation-worker";
const MAX: usize = zellij_ipc::MAX_IPC_FRAME_LEN;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize)]
enum Operation {
    Transpile,
    Analyze,
}

#[derive(Serialize, Deserialize)]
struct Request {
    operation: Operation,
    path: PathBuf,
    source: String,
    declarations: bool,
}

#[derive(Debug, Serialize, Deserialize)]
enum Output {
    Emitted {
        text: String,
        source_map: Option<String>,
    },
    Analysis(PluginCheckReport),
}

type Response = std::result::Result<Output, String>;

pub(super) fn transpile(
    path: &Path,
    source: &str,
    declarations: bool,
) -> Result<deno_ast::EmittedSourceText> {
    match request(path, source, declarations, Operation::Transpile)? {
        Output::Emitted { text, source_map } => {
            Ok(deno_ast::EmittedSourceText { text, source_map })
        },
        _ => bail!(
            "transpile compilation worker for {} returned the wrong operation",
            path.display()
        ),
    }
}

pub(super) fn analyze(path: &Path, source: &str) -> Result<PluginCheckReport> {
    match request(path, source, true, Operation::Analyze)? {
        Output::Analysis(report) => Ok(report),
        _ => bail!(
            "analysis compilation worker for {} returned the wrong operation",
            path.display()
        ),
    }
}

fn request(path: &Path, source: &str, declarations: bool, operation: Operation) -> Result<Output> {
    let context = format!("{operation:?} compilation worker for {}", path.display());
    (|| {
        limits::check_source(path, source)?;
        let request = Request {
            operation,
            path: path.to_owned(),
            source: source.to_owned(),
            declarations,
        };
        let input = serde_json::to_vec(&request).context("encode request")?;
        let binary = std::env::var_os("ZELLIJ_COMPILATION_WORKER")
            .map(PathBuf::from)
            .or_else(|| zellij_bus::os_input_output_unix::server_binary().cloned())
            .context("resolve server executable")?;
        // Direct cargo/libtest launches must opt in with the freshly built executable.
        if binary.parent().is_some_and(|dir| dir.ends_with("deps")) {
            bail!("test launcher must supply ZELLIJ_COMPILATION_WORKER; use cargo x test");
        }
        exchange(Command::new(binary).arg(ARG), &input, DEADLINE)
    })()
    .with_context(|| context)
}

/// How many workers this process has spawned. Tests assert deltas against it, never
/// absolutes: under `cargo test` every test in the binary shares the counter.
#[cfg(test)]
static SPAWNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn spawn_count() -> usize {
    SPAWNS.load(std::sync::atomic::Ordering::Relaxed)
}

struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
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
    if input.len() > MAX {
        bail!("worker request exceeds frame limit");
    }
    let deadline = Instant::now() + timeout;
    let mut child = Reap(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn worker")?,
    );
    #[cfg(test)]
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
        if let Some(status) = child.0.try_wait().context("wait for worker")? {
            if stdout_done && stderr_done {
                if !status.success() {
                    bail!(
                        "worker exited with {status}: {}",
                        String::from_utf8_lossy(&errors)
                    );
                }
                if !remaining.is_empty() {
                    bail!("worker exited before reading request");
                }
                let response: Response =
                    serde_json::from_slice(&output).context("invalid worker response")?;
                return response.map_err(|error| anyhow!(error));
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Dispatch before normal startup, logging, and any threads are created.
#[doc(hidden)]
pub fn run_compilation_worker() {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(ARG)) {
        return;
    }
    unsafe {
        close_fds::close_open_fds(3, &[]);
    }
    let result = (|| -> Result<()> {
        // Linux core_pattern pipe handlers can ignore RLIMIT_CORE.
        #[cfg(target_os = "linux")]
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error()).context("disable worker dumpability");
        }
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0 {
            return Err(std::io::Error::last_os_error()).context("disable worker core dumps");
        }
        let mut input = Vec::new();
        std::io::stdin()
            .take((MAX + 1) as u64)
            .read_to_end(&mut input)
            .context("read request")?;
        if input.len() > MAX {
            bail!("worker request exceeds frame limit");
        }
        let request: Request = serde_json::from_slice(&input).context("decode request")?;
        limits::check_source(&request.path, &request.source)?;
        let response: Response = match request.operation {
            Operation::Transpile => {
                loader::transpile_native(&request.path, &request.source, request.declarations)
                    .map(|emitted| Output::Emitted {
                        text: emitted.text,
                        source_map: emitted.source_map,
                    })
                    .map_err(|error| format!("{error:#}"))
            },
            Operation::Analyze => Ok(Output::Analysis(analysis::analyze_native(
                &request.path,
                &request.source,
            ))),
        };
        // All native ASTs have been destroyed before serialization and process exit.
        let output = serde_json::to_vec(&response).context("encode response")?;
        if output.len() > MAX {
            bail!("worker response exceeds frame limit");
        }
        std::io::stdout()
            .write_all(&output)
            .context("write response")
    })();
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

#[cfg(test)]
#[path = "unit/worker.rs"]
mod tests;
