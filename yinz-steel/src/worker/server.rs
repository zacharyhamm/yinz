use anyhow::{anyhow, Context, Result};
use std::io::{self, Write};

use super::protocol::{self, Operation, Output, Request, Response};
use crate::Runtime;

pub fn run_worker() -> Result<()> {
    confine_process().context("limit Steel worker resources")?;
    let mut runtime = Runtime::new(|command| {
        protocol::write(&mut io::stdout().lock(), &Response::HostCall(command))?;
        match protocol::read(&mut io::stdin().lock())? {
            Request::HostReply(reply) => reply.map_err(|error| anyhow!(error)),
            _ => Err(anyhow!("expected Steel host reply")),
        }
    })?;
    runtime.set_plugin_resolver(|specifier| {
        protocol::write(
            &mut io::stdout().lock(),
            &Response::ResolverRequest(specifier),
        )?;
        match protocol::read(&mut io::stdin().lock())? {
            Request::ResolverReply(reply) => reply.map_err(|error| anyhow!(error)),
            _ => Err(anyhow!("expected Steel plugin location reply")),
        }
    })?;
    protocol::write(
        &mut io::stdout().lock(),
        &Response::Finished(Ok(Output::Ready)),
    )?;
    loop {
        let request = protocol::read(&mut io::stdin().lock())?;
        let Request::Call(operation) = request else {
            anyhow::bail!("unexpected Steel host reply")
        };
        let result = execute(&mut runtime, operation).map_err(|error| format!("{error:#}"));
        let frame = protocol::encode(&Response::Finished(result)).or_else(|error| {
            protocol::encode(&Response::Finished(Err(format!(
                "result exceeds worker transport limit: {error:#}"
            ))))
        })?;
        let mut output = io::stdout().lock();
        output
            .write_all(&frame)
            .context("write Steel worker response")?;
        output.flush().context("flush Steel worker response")?;
    }
}

fn execute(runtime: &mut Runtime, operation: Operation) -> Result<Output> {
    Ok(match operation {
        Operation::Eval {
            source,
            path,
            origin,
        } => {
            let diagnostic_path = path.or_else(|| origin.as_ref().map(std::path::PathBuf::from));
            let source = if let Some(origin) = origin {
                runtime.prepare_modules(&source, &origin, &mut |origin, specifier| {
                    protocol::write(
                        &mut io::stdout().lock(),
                        &Response::SourceRequest {
                            origin: origin.to_owned(),
                            specifier: specifier.to_owned(),
                        },
                    )?;
                    match protocol::read(&mut io::stdin().lock())? {
                        Request::SourceReply(reply) => reply.map_err(|error| anyhow!(error)),
                        _ => Err(anyhow!("expected Steel module source reply")),
                    }
                })?
            } else {
                source
            };
            Output::Eval(runtime.eval_with_path(&source, diagnostic_path.as_deref())?)
        },
        Operation::Metadata(metadata) => {
            runtime.set_server_metadata(metadata)?;
            Output::Unit
        },
        Operation::Value(name) => Output::Value(runtime.value(&name)?),
        Operation::Invoke(name, arguments) => Output::Value(runtime.invoke(&name, arguments)?),
        Operation::Load(configuration) => {
            runtime.load(configuration)?;
            Output::Unit
        },
        Operation::Update(event) => Output::Changed(runtime.update(event)?),
        Operation::Pipe(message) => Output::Changed(runtime.pipe(message)?),
        Operation::Palette(message) => Output::Changed(runtime.palette(message)?),
        Operation::Render(rows, columns) => Output::Text(runtime.render(rows, columns)?),
        Operation::Unload => {
            runtime.unload()?;
            Output::Unit
        },
        Operation::Output => Output::Text(runtime.take_output()?),
        Operation::Logs => Output::Text(runtime.take_logs()?),
    })
}

#[doc(hidden)]
pub fn confine_process() -> Result<()> {
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } != 0 {
        return Err(io::Error::last_os_error()).context("disable Steel worker core dumps");
    }
    // No server descriptors survive into the interpreter, including sockets without CLOEXEC.
    #[cfg(target_os = "linux")]
    let closed = unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } == 0;
    #[cfg(not(target_os = "linux"))]
    let closed = false;
    if !closed {
        let maximum = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        anyhow::ensure!(maximum >= 0, "cannot determine worker descriptor limit");
        for fd in 3..maximum {
            unsafe {
                libc::close(fd as libc::c_int);
            }
        }
    }
    for (resource, amount) in [
        // macOS rejects this cap when existing mappings already exceed it.
        #[cfg(not(target_os = "macos"))]
        (libc::RLIMIT_AS, 1024 * 1024 * 1024),
        (libc::RLIMIT_STACK, 64 * 1024 * 1024),
        (libc::RLIMIT_CORE, 0),
    ] {
        let mut previous = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(resource, &mut previous) } != 0 {
            return Err(io::Error::last_os_error()).context("read Steel worker resource limit");
        }
        let amount = amount.min(previous.rlim_max);
        let limit = libc::rlimit {
            rlim_cur: amount,
            rlim_max: amount,
        };
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(io::Error::last_os_error()).context("set Steel worker resource limit");
        }
    }
    Ok(())
}
