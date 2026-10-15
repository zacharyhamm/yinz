//! Disposable, resource-bounded authoring and provider inspection.
use std::io::{Read, Write};
pub(crate) use yinz_plugin_authoring::worker::exchange_cancel;
use yinz_plugin_authoring::worker::{Operation, Output, Request, Response, ARG, MAX};
use yinz_runtime::errors::prelude::*;
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
        disable_core_dumps()?;
        let mut input = Vec::new();
        std::io::stdin()
            .take((MAX + 1) as u64)
            .read_to_end(&mut input)
            .context("read request")?;
        if input.len() > MAX {
            bail!("worker request exceeds frame limit");
        }
        let request: Request = serde_json::from_slice(&input).context("decode request")?;
        yinz_plugin_authoring::limits::check_source(&request.path, &request.source)?;
        let response: Response = match request.operation {
            Operation::Check(mode) => {
                yinz_steel::worker::confine_process()
                    .context("limit Scheme authoring resources")?;
                Ok(Output::Analysis(crate::steel::authoring::check(
                    &request.path,
                    &request.source,
                    mode,
                    request.root.as_deref(),
                )))
            },
            Operation::Analyze => {
                yinz_steel::worker::confine_process()
                    .context("limit Scheme authoring resources")?;
                Ok(Output::Analysis(crate::steel::authoring::check(
                    &request.path,
                    &request.source,
                    yinz_types::authoring::PluginCheckMode::Syntax,
                    request.root.as_deref(),
                )))
            },
            Operation::AgentProvider => {
                let result = yinz_steel::worker::confine_process()
                    .context("limit Scheme provider inspection resources")
                    .and_then(|()| {
                        crate::steel::agent_provider::inspect(&request.path, &request.source)
                    });
                result
                    .map(Output::AgentProvider)
                    .map_err(|error| format!("{error:#}"))
            },
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

pub(crate) fn disable_core_dumps() -> Result<()> {
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
    Ok(())
}
