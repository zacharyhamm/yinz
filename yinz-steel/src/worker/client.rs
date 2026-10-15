use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use super::{
    protocol::{self, Operation, Output, Request, Response},
    ARG, DEADLINE, MAX_ERRORS, MAX_FRAME,
};
use crate::{EvalOutput, InterruptHandle, InterruptTarget, MAX_SOURCE_BYTES};

type Host = Box<dyn FnMut(Value) -> Result<Value> + Send>;

pub(crate) struct Process(Mutex<Child>);

impl Process {
    pub(crate) fn terminate(&self) {
        let mut child = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if child.try_wait().ok().flatten().is_none() {
            // The unreaped leader keeps this process-group ID from being reused.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn status(&self) -> Result<Option<std::process::ExitStatus>> {
        self.0
            .lock()
            .map_err(|_| anyhow!("Steel worker process lock poisoned"))?
            .try_wait()
            .context("check Steel worker exit")
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub struct WorkerRuntime {
    process: Arc<Process>,
    input: ChildStdin,
    output: ChildStdout,
    errors: ChildStderr,
    received: Vec<u8>,
    diagnostics: Vec<u8>,
    host: Host,
    sources: Option<crate::modules::SourceRoot>,
}

impl WorkerRuntime {
    pub fn new(
        binary: &Path,
        host: impl FnMut(Value) -> Result<Value> + Send + 'static,
    ) -> Result<Self> {
        let mut command = Command::new(binary);
        command.arg(ARG);
        Self::spawn(command, Box::new(host))
    }

    fn spawn(mut command: Command, host: Host) -> Result<Self> {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .context("spawn Steel worker")?;
        let process = Arc::new(Process(Mutex::new(child)));
        let (input, output, errors) = {
            let mut child = process
                .0
                .lock()
                .map_err(|_| anyhow!("Steel worker process lock poisoned"))?;
            (
                child.stdin.take().context("Steel worker stdin")?,
                child.stdout.take().context("Steel worker stdout")?,
                child.stderr.take().context("Steel worker stderr")?,
            )
        };
        nonblocking(&input)?;
        nonblocking(&output)?;
        nonblocking(&errors)?;
        let mut runtime = Self {
            process,
            input,
            output,
            errors,
            received: Vec::new(),
            diagnostics: Vec::new(),
            host,
            sources: None,
        };
        match runtime.receive(Instant::now() + DEADLINE)? {
            Response::Finished(Ok(Output::Ready)) => Ok(runtime),
            Response::Finished(Err(error)) => bail!("initializing Steel worker: {error}"),
            _ => bail!("unexpected Steel worker startup response"),
        }
    }

    pub fn set_server_metadata(&mut self, metadata: crate::ServerMetadata) -> Result<()> {
        self.unit(Operation::Metadata(metadata))
    }

    pub fn interruption_handle(&self) -> InterruptHandle {
        InterruptHandle(InterruptTarget::Process(self.process.clone()))
    }

    pub fn set_source_root(&mut self, root: crate::modules::SourceRoot) {
        self.sources = Some(root);
    }

    #[cfg(test)]
    pub(super) fn process_id(&self) -> u32 {
        self.process.0.lock().unwrap().id()
    }

    pub fn eval(&mut self, source: &str) -> Result<EvalOutput> {
        self.eval_with_path(source, None)
    }

    pub fn eval_with_path(&mut self, source: &str, path: Option<&Path>) -> Result<EvalOutput> {
        anyhow::ensure!(
            source.len() <= MAX_SOURCE_BYTES,
            "Steel source exceeds 8 MiB"
        );
        let origin = self
            .sources
            .as_mut()
            .map(|root| {
                root.begin(source.len())?;
                Ok::<_, anyhow::Error>(root.entry_id().to_owned())
            })
            .transpose()?;
        match self.call(Operation::Eval {
            source: source.to_owned(),
            path: path.map(Path::to_owned),
            origin,
        })? {
            Output::Eval(result) => Ok(result),
            _ => bail!("unexpected Steel evaluation response"),
        }
    }

    pub fn value(&mut self, name: &str) -> Result<Value> {
        match self.call(Operation::Value(name.to_owned()))? {
            Output::Value(value) => Ok(value),
            _ => bail!("unexpected Steel value response"),
        }
    }

    pub fn invoke(&mut self, name: &str, arguments: Vec<Value>) -> Result<Value> {
        match self.call(Operation::Invoke(name.to_owned(), arguments))? {
            Output::Value(value) => Ok(value),
            _ => bail!("unexpected Steel invocation response"),
        }
    }

    pub fn load(&mut self, configuration: Value) -> Result<()> {
        self.unit(Operation::Load(configuration))
    }
    pub fn update(&mut self, event: Value) -> Result<bool> {
        self.changed(Operation::Update(event))
    }
    pub fn pipe(&mut self, message: Value) -> Result<bool> {
        self.changed(Operation::Pipe(message))
    }
    pub fn palette(&mut self, message: Value) -> Result<bool> {
        self.changed(Operation::Palette(message))
    }
    pub fn render(&mut self, rows: usize, columns: usize) -> Result<String> {
        self.text(Operation::Render(rows, columns))
    }
    pub fn unload(&mut self) -> Result<()> {
        self.unit(Operation::Unload)
    }
    pub fn take_output(&mut self) -> Result<String> {
        self.text(Operation::Output)
    }
    pub fn take_logs(&mut self) -> Result<String> {
        self.text(Operation::Logs)
    }

    fn unit(&mut self, operation: Operation) -> Result<()> {
        match self.call(operation)? {
            Output::Unit => Ok(()),
            _ => bail!("unexpected Steel lifecycle response"),
        }
    }
    fn changed(&mut self, operation: Operation) -> Result<bool> {
        match self.call(operation)? {
            Output::Changed(changed) => Ok(changed),
            _ => bail!("unexpected Steel event response"),
        }
    }
    fn text(&mut self, operation: Operation) -> Result<String> {
        match self.call(operation)? {
            Output::Text(text) => Ok(text),
            _ => bail!("unexpected Steel output response"),
        }
    }

    fn call(&mut self, operation: Operation) -> Result<Output> {
        self.call_before(operation, Instant::now() + DEADLINE)
    }

    pub(super) fn call_before(
        &mut self,
        operation: Operation,
        deadline: Instant,
    ) -> Result<Output> {
        // Encoding failed before any request bytes were sent; the worker is still synchronized.
        let frame = protocol::encode(&Request::Call(operation))?;
        match self.exchange(frame, deadline) {
            Ok(result) => result.map_err(|error| anyhow!(error)),
            Err(error) => {
                self.process.terminate();
                Err(error)
            },
        }
    }

    fn exchange(
        &mut self,
        frame: Vec<u8>,
        deadline: Instant,
    ) -> Result<std::result::Result<Output, String>> {
        self.send_frame(&frame, deadline)?;
        loop {
            match self.receive(deadline)? {
                Response::Finished(result) => return Ok(result),
                Response::HostCall(command) => {
                    let reply = (self.host)(command).map_err(|error| format!("{error:#}"));
                    let frame = protocol::encode(&Request::HostReply(reply)).or_else(|error| {
                        protocol::encode(&Request::HostReply(Err(format!(
                            "host reply exceeds worker transport limit: {error:#}"
                        ))))
                    })?;
                    self.send_frame(&frame, deadline)?;
                },
                Response::SourceRequest { origin, specifier } => {
                    let source = self
                        .sources
                        .as_mut()
                        .context("Steel plugin has no source root")
                        .and_then(|root| root.load(&origin, &specifier))
                        .map_err(|error| format!("{error:#}"));
                    self.send(&Request::SourceReply(source), deadline)?;
                },
            }
        }
    }

    fn send(&mut self, request: &Request, deadline: Instant) -> Result<()> {
        let frame = protocol::encode(request)?;
        self.send_frame(&frame, deadline)
    }

    fn send_frame(&mut self, frame: &[u8], deadline: Instant) -> Result<()> {
        let mut remaining = frame;
        while !remaining.is_empty() {
            self.check_alive(deadline)?;
            match self.input.write(remaining) {
                Ok(0) => bail!("Steel worker closed input"),
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.poll(deadline, true)?
                },
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(error).context("write Steel worker request"),
            }
            self.drain_errors()?;
        }
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<Response> {
        loop {
            if self.received.len() >= 4 {
                let size = u32::from_be_bytes(
                    self.received[..4]
                        .try_into()
                        .context("Steel frame prefix")?,
                ) as usize;
                if size > MAX_FRAME {
                    self.process.terminate();
                    bail!("Steel worker frame exceeds 96 MiB");
                }
                if self.received.len() >= size + 4 {
                    let response = serde_json::from_slice(&self.received[4..size + 4])
                        .context("decode Steel worker response");
                    self.received.drain(..size + 4);
                    return response;
                }
            }
            let mut buffer = [0; 8192];
            match self.output.read(&mut buffer) {
                Ok(0) => {
                    self.drain_errors()?;
                    bail!("Steel worker exited: {}", self.diagnostic());
                },
                Ok(count) => self.received.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.check_alive(deadline)?;
                    self.poll(deadline, false)?;
                },
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(error).context("read Steel worker response"),
            }
            self.drain_errors()?;
            if Instant::now() >= deadline {
                self.process.terminate();
                bail!("Steel worker exceeded 30 second deadline");
            }
        }
    }

    fn check_alive(&mut self, deadline: Instant) -> Result<()> {
        self.drain_errors()?;
        if Instant::now() >= deadline {
            self.process.terminate();
            bail!("Steel worker exceeded 30 second deadline");
        }
        if let Some(status) = self.process.status()? {
            bail!("Steel worker exited with {status}: {}", self.diagnostic());
        }
        Ok(())
    }

    fn diagnostic(&self) -> String {
        String::from_utf8_lossy(&self.diagnostics).into_owned()
    }

    fn drain_errors(&mut self) -> Result<()> {
        let mut buffer = [0; 4096];
        // A bounded drain prevents a noisy child from starving the deadline check.
        for _ in 0..16 {
            match self.errors.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let keep = count.min(MAX_ERRORS.saturating_sub(self.diagnostics.len()));
                    self.diagnostics.extend_from_slice(&buffer[..keep]);
                },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(error).context("read Steel worker diagnostics"),
            }
        }
        Ok(())
    }

    fn poll(&self, deadline: Instant, writing: bool) -> Result<()> {
        let mut descriptors = [
            libc::pollfd {
                fd: if writing { self.input.as_raw_fd() } else { -1 },
                events: libc::POLLOUT,
                revents: 0,
            },
            libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.errors.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(10));
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                wait.as_millis() as libc::c_int,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error).context("poll Steel worker pipes");
            }
        }
        Ok(())
    }
}

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        self.process.terminate();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error()).context("set Steel worker pipe nonblocking");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../unit/worker_transport.rs"]
mod tests;
