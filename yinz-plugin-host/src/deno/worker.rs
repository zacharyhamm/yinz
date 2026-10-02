//! The compilation worker's entry point. SWC owns recursively allocated native ASTs, so only
//! this disposable process may parse, inspect, transpile, or drop them; even successful output
//! needs a clean exit. The protocol and the client that spawns and supervises the process live
//! in `yinz_plugin_authoring::worker`, which links neither V8 nor SWC.
use super::{analysis, limits, loader};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(test)]
pub(crate) use yinz_plugin_authoring::worker::spawn_count;
pub(super) use yinz_plugin_authoring::worker::{analyze, exchange_cancel, supervise};
use yinz_plugin_authoring::worker::{Operation, Output, Request, Response, ARG, MAX};
use yinz_runtime::errors::prelude::*;

pub(super) fn transpile(
    path: &Path,
    source: &str,
    declarations: bool,
) -> Result<deno_ast::EmittedSourceText> {
    let (text, source_map) = yinz_plugin_authoring::worker::transpile(path, source, declarations)?;
    Ok(deno_ast::EmittedSourceText { text, source_map })
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
            Operation::Check(mode) => Ok(Output::Analysis(analysis::check_authoring(
                &request.path,
                &request.source,
                mode,
                request.root.as_deref(),
            ))),
            Operation::Analyze => Ok(Output::Analysis(analysis::analyze_native(
                &request.path,
                &request.source,
            ))),
            Operation::AgentProvider => {
                super::agent_provider::inspect(&request.path, &request.source)
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

pub(super) fn disable_core_dumps() -> Result<()> {
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

#[cfg(test)]
#[path = "unit/worker.rs"]
mod tests;
