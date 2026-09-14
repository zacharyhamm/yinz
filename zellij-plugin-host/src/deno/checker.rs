//! Offline authoring checks. Only the bundled compiler executes in this isolate;
//! plugin source is passed to TypeScript as data, never loaded as an ES module.
use super::limits::Limits;
use super::loader::{
    file_specifier, transpile, PluginModuleLoader, SDK_SPECIFIER, WORKFLOW_PATH, WORKFLOW_SOURCE,
    WORKFLOW_SPECIFIER,
};
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

/// Check a local plugin and its imported modules against ES2022 and the SDK. The CLI and Hocus
/// go through `plugin_authoring::check_plugin_report`, which also carries the warnings; this is
/// the tests' shorthand.
#[cfg(test)]
pub fn check_plugin(path: &Path) -> Result<()> {
    let loader = PluginModuleLoader::for_check(path, false)?;
    let entry = loader.local_path(loader.entrypoint(), true)?;
    let source = super::limits::read_source(&entry)?;
    check_semantic_source(path, &source)
}

/// Type-check an unsaved entrypoint buffer, resolving dependencies from disk.
/// Its parent directory must exist. No file is written.
pub fn check_semantic_source(path: &Path, source: &str) -> Result<()> {
    check(path, Some(source))
}

struct CompilerFiles {
    loader: PluginModuleLoader,
}

#[op2]
#[string]
fn op_zellij_check_read(
    state: &mut OpState,
    #[string] name: &str,
) -> std::result::Result<String, JsErrorBox> {
    let files = state.borrow::<CompilerFiles>();
    let read = || -> Result<String> {
        if name == WORKFLOW_PATH {
            return Ok(WORKFLOW_SOURCE.to_owned());
        }
        let url = file_specifier(Path::new(name))?;
        files.loader.read_source(&url, true)
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
        if name == WORKFLOW_SPECIFIER {
            return Ok(WORKFLOW_PATH.to_owned());
        }
        if name == SDK_SPECIFIER {
            return Ok("/$zellij/sdk.d.ts".to_owned());
        }
        if referrer == WORKFLOW_PATH {
            bail!("embedded modules may import only registered modules");
        }
        let files = state.borrow::<CompilerFiles>();
        if name == "zellij:testing" && files.loader.testing() {
            return Ok("/$zellij/testing.d.ts".to_owned());
        }
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
    check_with_limits(
        path,
        source,
        Limits::new(format!("{} (checker)", path.display())),
    )
}

pub(super) fn check_with_limits(
    path: &Path,
    source: Option<&str>,
    limits: std::sync::Arc<Limits>,
) -> Result<()> {
    let mut loader = PluginModuleLoader::for_check(path, source.is_some())?;
    if let Some(source) = source {
        loader.set_entry_source(source)?;
    }
    check_loader(path, loader, limits)
}

pub(super) fn check_tests(path: &Path, root: &Path) -> Result<()> {
    check_loader(
        path,
        PluginModuleLoader::for_tests(path, root)?,
        Limits::new("test checker".into()),
    )
}

fn check_loader(
    path: &Path,
    loader: PluginModuleLoader,
    limits: std::sync::Arc<Limits>,
) -> Result<()> {
    let entry = loader
        .entrypoint()
        .to_file_path()
        .map_err(|_| anyhow!("invalid checker entrypoint"))?;
    let entry = entry.to_str().context("compiler path is not UTF-8")?;
    let input = serde_json::json!({
        "entry": entry,
        "sdk": include_str!("sdk.d.ts"),
        "testing": loader.testing().then_some(include_str!("testing/testing.d.ts")),
    });
    // A fresh runtime has no plugin extensions, filesystem/network runtime,
    // package resolver or module loader. The two ops enforce the loader root.
    let mut runtime = limits
        .runtime(RuntimeOptions {
            extensions: vec![zellij_checker::init()],
            ..Default::default()
        })
        .context("create isolated TypeScript checker")?;
    runtime
        .op_state()
        .borrow_mut()
        .put(CompilerFiles { loader });
    let result = with_deadline(&mut runtime, Duration::from_secs(30), |runtime| {
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
    });
    limits.check()?;
    result
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
    let limits = runtime
        .op_state()
        .borrow()
        .try_borrow::<std::sync::Arc<Limits>>()
        .cloned();
    let result = std::thread::scope(|scope| {
        let timed_out = &timed_out;
        scope.spawn(move || {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match completion.recv_timeout(remaining.min(Duration::from_millis(10))) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                }
                if limits.as_ref().is_some_and(|limits| limits.exhausted()) {
                    interrupt.terminate_execution();
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    timed_out.store(true, Ordering::Release);
                    interrupt.terminate_execution();
                    break;
                }
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
