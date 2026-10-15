//! Explicitly invoked installed Scheme tests. Guest code never runs in the caller process.
mod session;
mod steel;

use crate::compilation_worker as worker;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use yinz_plugin_authoring::worker::PLUGIN_TEST_WORKER_ARG as ARG;
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    authoring::{PluginTestKind, PluginTestOptions, PluginTestReport, PluginTestResult},
    data::PermissionType,
};

const OUTPUT_LIMIT: usize = 32 * 1024;

trait TestPaths {
    fn entry_path(&self) -> Result<PathBuf>;
    fn resolve_test_path(&self, path: &str) -> Result<String>;
}

pub fn binary() -> Result<PathBuf> {
    std::env::var_os("YINZ_COMPILATION_WORKER")
        .map(PathBuf::from)
        .or_else(|| yinz_runtime::own_binary::server_binary().cloned())
        .or_else(|| std::env::current_exe().ok())
        .context("locate installed yinz")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    options: CaseOptions,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseOptions {
    #[serde(default)]
    grants: Vec<PermissionType>,
}
#[derive(Serialize, Deserialize)]
struct Request {
    path: PathBuf,
    root: PathBuf,
    index: Option<usize>,
    case: Option<Case>,
    session: Option<PathBuf>,
    journal: Option<PathBuf>,
}

/// Sequential by design. A subprocess bounds registration, type checking, and each case.
/// `allowed` is the API caller's authority; the CLI supplies `None`.
pub fn run(
    path: &Path,
    options: &PluginTestOptions,
    allowed: Option<&[PermissionType]>,
    cancel: &AtomicBool,
) -> Result<PluginTestReport> {
    if options.timeout_ms == 0 || options.timeout_ms > 3_600_000 {
        bail!("timeout_ms must be between 1 and 3600000");
    }
    if path.is_symlink() {
        bail!("test selection refuses symlinks");
    }
    let path = path.canonicalize().context("resolve test selection")?;
    let root = if path.is_dir() {
        path.clone()
    } else {
        path.parent().context("test root")?.to_owned()
    };
    let mut files = Vec::new();
    discover(&path, &root, &mut files)?;
    files.sort();
    files.dedup();
    let mut report = PluginTestReport::default();
    let timeout = Duration::from_millis(options.timeout_ms);
    for path in files {
        if cancel.load(Ordering::Acquire) {
            report.tests.push(failed(
                "cancelled".into(),
                options.kind,
                anyhow!("test run cancelled"),
            ));
            return Ok(report);
        }
        let kind = if path
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().ends_with(".integration.test"))
        {
            PluginTestKind::Integration
        } else {
            PluginTestKind::Unit
        };
        if options.kind != PluginTestKind::All && options.kind != kind {
            continue;
        }
        let mut request = Request {
            path: path.clone(),
            root: root.clone(),
            index: None,
            case: None,
            session: None,
            journal: None,
        };
        let found = invoke(&request, timeout, cancel);
        let cases: Vec<Case> = match found
            .and_then(|value| serde_json::from_value(value).context("decode discovered tests"))
        {
            Ok(cases) => cases,
            Err(error) => {
                report
                    .tests
                    .push(failed(path.display().to_string(), kind, error));
                continue;
            },
        };
        if report.tests.len() + cases.len() > 1000 {
            bail!("test selection exceeds 1000 cases");
        }
        for (index, case) in cases.into_iter().enumerate() {
            if options
                .filter
                .as_ref()
                .is_some_and(|filter| !case.name.contains(filter))
            {
                continue;
            }
            if cancel.load(Ordering::Acquire) {
                report
                    .tests
                    .push(failed(case.name, kind, anyhow!("test run cancelled")));
                return Ok(report);
            }
            if kind == PluginTestKind::Integration
                && allowed
                    .is_some_and(|allowed| !allowed.contains(&PermissionType::RunActionsAsUser))
            {
                report.tests.push(failed(case.name, kind, anyhow!("integration session controls require RunActionsAsUser on the calling plugin")));
                continue;
            }
            if allowed.is_some_and(|allowed| {
                case.options
                    .grants
                    .iter()
                    .any(|grant| !allowed.contains(grant))
            }) {
                report.tests.push(failed(
                    case.name,
                    kind,
                    anyhow!("test grants exceed the calling plugin's permissions"),
                ));
                continue;
            }
            let started = Instant::now();
            let owned = if kind == PluginTestKind::Integration {
                Some(session::OwnedSession::new()?)
            } else {
                None
            };
            request.session = owned.as_ref().map(|session| session.root().to_owned());
            request.index = Some(index);
            request.case = Some(case.clone());
            let result = invoke(&request, timeout, cancel);
            let mut test = match result {
                Ok(value) => serde_json::from_value(value).context("decode test result")?,
                Err(error) => failed(case.name, kind, error),
            };
            if let Some(mut owned) = owned {
                if let Err(error) = owned.shutdown() {
                    test.status = "failed".into();
                    test.error = Some(format!(
                        "{}\ncleanup: {error:#}",
                        test.error.unwrap_or_default()
                    ));
                }
                if test.status != "passed" {
                    append_bounded(&mut test.output, &owned.diagnostics());
                }
            }
            test.kind = kind;
            test.duration_ms = started.elapsed().as_millis() as u64;
            if let Some(error) = &mut test.error {
                let text = std::mem::take(error);
                append_bounded(error, &text);
            }
            let text = std::mem::take(&mut test.output);
            append_bounded(&mut test.output, &text);
            report.tests.push(test);
        }
    }
    if report.tests.is_empty() {
        report.tests.push(failed(
            "discovery".into(),
            options.kind,
            anyhow!("no matching tests"),
        ));
    }
    Ok(report)
}

pub fn run_named(
    dir: &Path,
    name: &str,
    options: &PluginTestOptions,
    allowed: Option<&[PermissionType]>,
    cancel: &AtomicBool,
) -> Result<PluginTestReport> {
    if yinz_steel_builtins::builtin_steel_plugin(name).is_some() {
        let root = tempfile::tempdir().context("unpack builtin Scheme tests")?;
        yinz_steel_builtins::BUILTIN_STEEL_FILES
            .extract(root.path())
            .context("extract builtin Scheme test sources")?;
        return run(
            &root.path().join(format!("{name}.scm")),
            options,
            allowed,
            cancel,
        );
    }
    if dir.as_os_str().is_empty() {
        bail!("no plugin folder is configured; set user_plugin_dir or pass --config-dir");
    }
    let path = yinz_plugin_api::authoring::resolve_plugin_file(dir, name)
        .map_err(|error| anyhow!(error))?;
    run(&path, options, allowed, cancel)
}

fn failed(name: String, kind: PluginTestKind, error: anyhow::Error) -> PluginTestResult {
    PluginTestResult {
        name,
        kind,
        status: "failed".into(),
        duration_ms: 0,
        error: Some(format!("{error:#}").chars().take(8192).collect()),
        output: String::new(),
    }
}

fn discover(path: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if out.len() >= 1000 {
        bail!("test selection exceeds 1000 files");
    }
    if path.is_symlink() {
        bail!("test discovery refuses symlinks: {}", path.display());
    }
    if path.is_dir() {
        for entry in std::fs::read_dir(path).context("list test directory")? {
            let entry = entry.context("read test directory entry")?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            discover(&entry.path(), root, out)?;
        }
    } else if yinz_plugin_api::authoring::is_script_plugin(path)
        && path
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().ends_with(".test"))
    {
        out.push(path.to_owned());
    } else if path.parent() == Some(root) && yinz_plugin_api::authoring::is_script_plugin(path) {
        let stem = path.file_stem().context("plugin stem")?.to_string_lossy();
        let extension = path
            .extension()
            .context("plugin extension")?
            .to_string_lossy();
        for suffix in ["test", "integration.test"] {
            let adjacent = path.with_file_name(format!("{stem}.{suffix}.{extension}"));
            if adjacent.is_file() {
                out.push(adjacent);
            }
        }
    }
    Ok(())
}

fn invoke(request: &Request, timeout: Duration, cancel: &AtomicBool) -> Result<Value> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(binary()?);
    command
        .process_group(0)
        .arg(ARG)
        .arg(std::process::id().to_string());
    if let Some(root) = &request.session {
        session::environment(&mut command, root);
    }
    let journal = tempfile::NamedTempFile::new().context("create bounded test output journal")?;
    let mut input = serde_json::to_value(request).context("encode test request")?;
    input["journal"] = json!(journal.path());
    let result = worker::exchange_cancel(
        &mut command,
        &serde_json::to_vec(&input).context("encode test request")?,
        timeout,
        Some(cancel),
    );
    match (result, &request.case) {
        (Err(error), Some(case)) => {
            let mut test = failed(case.name.clone(), PluginTestKind::Unit, error);
            append_bounded(
                &mut test.output,
                &std::fs::read_to_string(journal.path()).unwrap_or_default(),
            );
            Ok(json!(test))
        },
        (result, _) => result,
    }
}

fn append_bounded(output: &mut String, text: &str) {
    output.push_str(text);
    if output.len() > OUTPUT_LIMIT {
        let mut start = output.len() - OUTPUT_LIMIT;
        while !output.is_char_boundary(start) {
            start += 1;
        }
        output.drain(..start);
    }
}

fn execute(request: Request) -> Result<Value> {
    yinz_steel::worker::confine_process().context("limit Scheme test worker resources")?;
    steel::execute(request)
}

#[doc(hidden)]
pub fn run_worker() {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(ARG)) {
        return;
    }
    unsafe {
        close_fds::close_open_fds(3, &[]);
    }
    // The CLI's signal handlers cancel normally; also stop this process group if its
    // owner crashes. Compilation descendants belong to the same private group.
    let owner = std::env::args()
        .nth(2)
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        .unwrap_or_else(|| unsafe { libc::getppid() });
    std::thread::spawn(move || loop {
        if unsafe { libc::getppid() } != owner {
            unsafe {
                if libc::getpgrp() == libc::getpid() {
                    libc::kill(-libc::getpgrp(), libc::SIGKILL);
                }
            }
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    });
    let result = (|| -> Result<Value> {
        worker::disable_core_dumps()?;
        let mut input = Vec::new();
        std::io::stdin()
            .take((yinz_ipc::MAX_IPC_FRAME_LEN + 1) as u64)
            .read_to_end(&mut input)
            .context("read test request")?;
        if input.len() > yinz_ipc::MAX_IPC_FRAME_LEN {
            bail!("test request exceeds frame limit");
        }
        execute(serde_json::from_slice(&input).context("decode test request")?)
    })()
    .map_err(|error| format!("{error:#}"));
    let output = serde_json::to_vec(&result).expect("serialize test response");
    if std::io::stdout().write_all(&output).is_err() {
        std::process::exit(1);
    }
    std::process::exit(0);
}

type RunKey = (crate::PluginId, crate::ClientId, Option<u64>, String);
static RUNS: std::sync::LazyLock<std::sync::Mutex<BTreeMap<RunKey, std::sync::Arc<AtomicBool>>>> =
    std::sync::LazyLock::new(Default::default);

pub(crate) fn cancel_instance(env: &crate::plugin_map::PluginEnv) {
    for ((plugin_id, client_id, generation, _), cancel) in RUNS.lock().unwrap().iter() {
        if (*plugin_id, *client_id, *generation) == (env.plugin_id, env.client_id, env.generation) {
            cancel.store(true, Ordering::Release);
        }
    }
}

pub(crate) fn cancel(env: &crate::plugin_map::PluginEnv, id: &str) {
    if let Some(cancel) =
        RUNS.lock()
            .unwrap()
            .get(&(env.plugin_id, env.client_id, env.generation, id.to_owned()))
    {
        cancel.store(true, Ordering::Release);
    }
}

pub(crate) fn start(
    env: &crate::plugin_map::PluginEnv,
    name: String,
    options: PluginTestOptions,
    request_id: String,
) -> Result<()> {
    if let Err(error) = start_run(env, name, options, request_id.clone()) {
        env.senders
            .send_to_plugin(crate::PluginInstruction::for_plugin_event(
                env.plugin_id,
                env.client_id,
                env.generation,
                yinz_types::data::Event::CustomMessage(
                    format!("__yinz_plugin_test:{request_id}"),
                    serde_json::to_string(&Result::<PluginTestReport, String>::Err(format!(
                        "{error:#}"
                    )))?,
                ),
            ))
            .context("deliver test startup failure")?;
    }
    Ok(())
}

fn start_run(
    env: &crate::plugin_map::PluginEnv,
    name: String,
    options: PluginTestOptions,
    request_id: String,
) -> Result<()> {
    if request_id.is_empty() || request_id.len() > 256 {
        bail!("test request ID must contain 1 to 256 bytes");
    }
    let dir = env
        .user_plugin_dir
        .clone()
        .or_else(yinz_runtime::home::default_user_plugin_dir)
        .unwrap_or_default();
    let key = (
        env.plugin_id,
        env.client_id,
        env.generation,
        request_id.clone(),
    );
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    {
        let mut runs = RUNS.lock().unwrap();
        if runs
            .keys()
            .any(|key| key.0 == env.plugin_id && key.1 == env.client_id)
        {
            bail!("a plugin test run is already active for this client");
        }
        runs.insert(key.clone(), cancel.clone());
    }
    let allowed = (!env.plugin_config.is_builtin()).then(|| {
        env.permissions
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>()
    });
    let parent_cancel = env.cancel.clone();
    let sender = env.senders.clone();
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let monitor_done = done.clone();
    let monitor_cancel = cancel.clone();
    yinz_bus::global_async_runtime::get_tokio_runtime().spawn(async move {
        while !monitor_done.load(Ordering::Acquire) {
            if parent_cancel.load(Ordering::Acquire) {
                monitor_cancel.store(true, Ordering::Release);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    std::thread::spawn(move || {
        let result = run_named(&dir, &name, &options, allowed.as_deref(), &cancel)
            .map_err(|error| format!("{error:#}"));
        done.store(true, Ordering::Release);
        RUNS.lock().unwrap().remove(&key);
        sender
            .send_to_plugin(crate::PluginInstruction::for_plugin_event(
                key.0,
                key.1,
                key.2,
                yinz_types::data::Event::CustomMessage(
                    format!("__yinz_plugin_test:{request_id}"),
                    serde_json::to_string(&result).unwrap_or_default(),
                ),
            ))
            .context("deliver plugin test result")
            .non_fatal();
    });
    Ok(())
}
