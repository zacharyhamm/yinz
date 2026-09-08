//! Offline authoring checks. Only the bundled compiler executes in this isolate;
//! plugin source is passed to TypeScript as data, never loaded as an ES module.
use super::loader::{file_specifier, transpile, PluginModuleLoader, SDK_SPECIFIER};
use deno_core::{op2, JsRuntime, OpState, RuntimeOptions};
use deno_error::JsErrorBox;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;
use zellij_runtime::errors::prelude::*;

/// Parse and transpile an editor buffer, including an unsaved file.
/// This does not resolve imports or check types; `.d.ts` is accepted here.
pub fn check_syntax(path: &Path, source: &str) -> Result<()> {
    transpile(path, source, true).map(|_| ())
}

/// Check a local plugin and its imported modules against ES2022 and the SDK.
pub fn check_plugin(path: &Path) -> Result<()> {
    let loader = PluginModuleLoader::for_check(path, false)?;
    let entry = loader.local_path(loader.entrypoint(), true)?;
    let source = std::fs::read_to_string(&entry)
        .with_context(|| format!("read plugin {}", path.display()))?;
    check_semantic_source(path, &source)
}

/// Type-check an unsaved entrypoint buffer, resolving dependencies from disk.
/// Its parent directory must exist. No file is written.
pub fn check_semantic_source(path: &Path, source: &str) -> Result<()> {
    check(path, Some(source))
}

struct CompilerFiles {
    loader: PluginModuleLoader,
    source: Option<String>,
}

#[op2]
#[string]
fn op_zellij_check_read(
    state: &mut OpState,
    #[string] name: &str,
) -> std::result::Result<String, JsErrorBox> {
    let files = state.borrow::<CompilerFiles>();
    let read = || -> Result<String> {
        let url = file_specifier(Path::new(name))?;
        if url == *files.loader.entrypoint() {
            if let Some(source) = &files.source {
                return Ok(source.clone());
            }
        }
        let path = files.loader.local_path(&url, true)?;
        std::fs::read_to_string(&path)
            .with_context(|| format!("read check input {}", path.display()))
    };
    read()
        .and_then(|source| {
            check_syntax(Path::new(name), &source)?;
            Ok(source)
        })
        .map_err(|e| JsErrorBox::generic(format!("{e:#}")))
}

#[op2]
#[string]
fn op_zellij_check_resolve(
    state: &mut OpState,
    #[string] name: &str,
    #[string] referrer: &str,
) -> std::result::Result<String, JsErrorBox> {
    let resolve = || -> Result<String> {
        if name == SDK_SPECIFIER {
            return Ok("/$zellij/sdk.d.ts".to_owned());
        }
        let files = state.borrow::<CompilerFiles>();
        let referrer = file_specifier(Path::new(referrer))?;
        let url = files.loader.resolve_local(name, referrer.as_str(), true)?;
        let path = url
            .to_file_path()
            .map_err(|_| anyhow!("invalid compiler module path: {url}"))?;
        path.to_str()
            .map(str::to_owned)
            .context("compiler path is not UTF-8")
    };
    resolve().map_err(|e| JsErrorBox::generic(format!("{referrer}: import {name:?}: {e:#}")))
}

deno_core::extension!(
    zellij_checker,
    ops = [op_zellij_check_read, op_zellij_check_resolve]
);

fn check(path: &Path, source: Option<&str>) -> Result<()> {
    let loader = PluginModuleLoader::for_check(path, source.is_some())?;
    let entry = loader
        .entrypoint()
        .to_file_path()
        .map_err(|_| anyhow!("invalid checker entrypoint"))?;
    let entry = entry.to_str().context("compiler path is not UTF-8")?;
    let input = serde_json::json!({
        "entry": entry,
        "sdk": include_str!("sdk.d.ts"),
    });
    // A fresh runtime has no plugin extensions, filesystem/network runtime,
    // package resolver or module loader. The two ops enforce the loader root.
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        extensions: vec![zellij_checker::init()],
        ..Default::default()
    })
    .context("create isolated TypeScript checker")?;
    runtime.op_state().borrow_mut().put(CompilerFiles {
        loader,
        source: source.map(str::to_owned),
    });
    with_deadline(&mut runtime, Duration::from_secs(30), |runtime| {
        runtime
            .execute_script(
                "zellij:typescript-5.9.3",
                include_str!("compiler/typescript.js"),
            )
            .context("initialize offline TypeScript compiler")?;
        runtime
            .execute_script(
                "zellij:compiler-libs",
                format!(
                    "globalThis.__zellijLibs = {};",
                    include_str!("compiler/libs.json")
                ),
            )
            .context("load bundled ES2022 declarations")?;
        runtime
            .execute_script("zellij:compiler-host", include_str!("compiler/host.js"))
            .context("initialize restricted compiler host")?;
        runtime
            .execute_script("zellij:check", format!("zellijCheck({input});"))
            .with_context(|| format!("check plugin {}", path.display()))?;
        Ok(())
    })
}

// The isolate stays on the calling thread. Only V8's thread-safe interrupt
// handle crosses to the watchdog; completion wakes and joins it immediately.
fn with_deadline<T>(
    runtime: &mut JsRuntime,
    timeout: Duration,
    check: impl FnOnce(&mut JsRuntime) -> Result<T>,
) -> Result<T> {
    let interrupt = runtime.v8_isolate().thread_safe_handle();
    let (done, completion) = mpsc::channel();
    let timed_out = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let timed_out = &timed_out;
        scope.spawn(move || {
            if matches!(
                completion.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                timed_out.store(true, Ordering::Release);
                interrupt.terminate_execution();
            }
        });
        let result = check(runtime);
        let _ = done.send(());
        result
    });
    if timed_out.load(Ordering::Acquire) {
        Err(anyhow!(
            "TypeScript check exceeded its {} second time limit",
            timeout.as_secs_f64()
        ))
    } else {
        result
    }
}

#[cfg(test)]
#[path = "unit/checker.rs"]
mod tests;
