//! Explicitly invoked installed TypeScript tests. Guest code never runs in the caller process.
mod session;

use super::{checker, limits, loader::PluginModuleLoader, worker};
use deno_core::{op2, serde_v8, v8, OpState, RuntimeOptions};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
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
        let kind = if path.to_string_lossy().ends_with(".integration.test.ts") {
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
    // A shipped top-level module: a builtin, or a shared module.
    if yinz_deno_builtins::builtin_deno_plugin(name).is_some() {
        let root = tempfile::tempdir().context("unpack builtin tests")?;
        yinz_deno_builtins::BUILTIN_DENO_FILES
            .extract(root.path())
            .context("extract builtin test sources")?;
        return run(
            &root.path().join(format!("{name}.ts")),
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
    } else if path.to_string_lossy().ends_with(".test.ts") {
        out.push(path.to_owned());
    } else if path.parent() == Some(root) && yinz_plugin_api::authoring::is_deno_plugin(path) {
        let stem = path.file_stem().context("plugin stem")?.to_string_lossy();
        for suffix in ["test.ts", "integration.test.ts"] {
            let adjacent = path.with_file_name(format!("{stem}.{suffix}"));
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

struct Host {
    loader: Rc<PluginModuleLoader>,
    replies: BTreeMap<String, Value>,
    command_bytes: usize,
    commands: Vec<Value>,
    logs: Vec<String>,
    renders: Vec<String>,
    output: String,
    journal: PathBuf,
    pending: String,
    session: Option<session::Controller>,
    started: Instant,
}

#[op2]
#[serde]
fn op_yinz_command<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &mut OpState,
    command: v8::Local<'s, v8::Value>,
    #[smi] _client: u32,
    #[string] _completion: Option<String>,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    // SDK filesystem writes carry Uint8Array, just as they do in a live plugin.
    let command = super::transport::from_v8(scope, command)
        .map_err(|error| JsErrorBox::generic(format!("{error:#}")))?;
    let host = state.borrow_mut::<Host>();
    let name = command
        .as_str()
        .or_else(|| {
            command
                .as_object()
                .and_then(|object| object.keys().next().map(String::as_str))
        })
        .unwrap_or("invalid command")
        .to_owned();
    if host.commands.len() == 10000 {
        return Err(JsErrorBox::generic(
            "test command capture exceeds 10000 commands",
        ));
    }
    let bytes = serde_json::to_vec(&command)
        .map_err(|error| JsErrorBox::generic(error.to_string()))?
        .len();
    if bytes > 1_048_576 - host.command_bytes {
        return Err(JsErrorBox::generic("test command capture exceeds 1 MiB"));
    }
    host.command_bytes += bytes;
    host.commands.push(command);
    let reply = host
        .replies
        .get(&name)
        .cloned()
        .or_else(|| match name.as_str() {
            "Pick" | "Subscribe" | "Unsubscribe" | "SetTimeout" | "SetSelectable"
            | "ShowCursor" => Some(Value::Null),
            _ => None,
        })
        .ok_or_else(|| {
            JsErrorBox::generic(format!(
                "unstubbed host operation {name}; supply replies or harness.stub"
            ))
        })?;
    Ok(json!({"json": reply}))
}
#[op2(fast)]
fn op_yinz_log(state: &mut OpState, #[string] line: &str) {
    let host = state.borrow_mut::<Host>();
    append_bounded(&mut host.output, &format!("{line}\n"));
    let _ = std::fs::write(&host.journal, &host.output);
    if let Some(session) = &host.session {
        session.diagnostics(&host.output, &host.pending);
    }
    if host.logs.len() < 1000 {
        host.logs.push(line.chars().take(4096).collect());
    }
}
#[op2]
#[serde]
fn op_yinz_test(
    state: &mut OpState,
    #[string] op: &str,
    #[serde] arg: serde_json::Value,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    let run = || -> Result<Value> {
        let host = state.borrow_mut::<Host>();
        let result = match op {
            "mock" => {
                host.replies.extend(
                    serde_json::from_value::<BTreeMap<String, Value>>(arg)
                        .context("mock replies must be an object")?,
                );
                Ok(Value::Null)
            },
            "resolve" => {
                let path = arg.as_str().context("plugin path must be a string")?;
                let url =
                    host.loader
                        .resolve_local(path, host.loader.entrypoint().as_str(), false)?;
                Ok(json!(url.as_str()))
            },
            "commands" => Ok(json!(host.commands)),
            "logs" => Ok(json!(host.logs)),
            "renders" => Ok(json!(host.renders)),
            "render" => {
                let text = session::render_text(
                    arg["text"].as_str().context("render must be text")?,
                    arg["rows"].as_u64().unwrap_or(24) as usize,
                    arg["columns"].as_u64().unwrap_or(80) as usize,
                )?;
                if host.renders.len() < 1000 {
                    let mut captured = String::new();
                    append_bounded(&mut captured, &text);
                    host.renders.push(captured);
                }
                Ok(json!(text))
            },
            "clock" => Ok(json!(host.started.elapsed().as_millis() as u64)),
            "pending" => {
                host.pending = arg
                    .as_str()
                    .context("pending wait must be text")?
                    .chars()
                    .take(4096)
                    .collect();
                Ok(Value::Null)
            },
            op if op.starts_with("session:") => host
                .session
                .as_mut()
                .context("session controls require an integration test")?
                .call(&op[8..], arg, &host.loader),
            _ => bail!("unknown test operation {op}"),
        };
        if let Some(session) = &host.session {
            session.diagnostics(&host.output, &host.pending);
        }
        result
    };
    run().map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}

#[op2(async(deferred), fast)]
async fn op_yinz_test_tick() {
    tokio::task::yield_now().await;
}

deno_core::extension!(
    test_host,
    ops = [
        op_yinz_test_tick,
        op_yinz_command,
        op_yinz_log,
        op_yinz_test,
        super::op_yinz_validate,
        super::op_yinz_monotonic_ms,
        super::op_yinz_worker_started,
        super::limits::op_yinz_encode,
        super::ui::op_yinz_ui_create,
        super::ui::op_yinz_ui_call,
        super::ui::op_yinz_ui_pure,
        super::image::op_yinz_image,
        super::op_yinz_uuid
    ],
    state = |state| state.put(std::collections::VecDeque::<
        super::background_worker::PostReservation,
    >::new())
);

fn execute(request: Request) -> Result<Value> {
    if request.index.is_none() {
        checker::check_tests(&request.path, &request.root)?;
    }
    let loader = Rc::new(PluginModuleLoader::for_tests(&request.path, &request.root)?);
    let limits = limits::Limits::new(request.path.display().to_string());
    let mut runtime = limits.runtime(RuntimeOptions {
        module_loader: Some(loader.clone()),
        extensions: vec![test_host::init()],
        ..Default::default()
    })?;
    runtime.op_state().borrow_mut().put(Host {
        loader: loader.clone(),
        replies: BTreeMap::new(),
        command_bytes: 0,
        commands: vec![],
        logs: vec![],
        renders: vec![],
        output: String::new(),
        journal: request.journal.context("missing test output journal")?,
        pending: String::new(),
        started: Instant::now(),
        session: request.session.map(|root| {
            session::Controller::new(
                root,
                request
                    .case
                    .as_ref()
                    .map(|case| case.options.grants.clone())
                    .unwrap_or_default(),
            )
        }),
    });
    runtime.execute_script("yinz:testing-bootstrap", include_str!("bootstrap.js"))?;
    runtime.execute_script("yinz:sdk-bootstrap", format!("globalThis.__yinzTestBridge = {}; __yinzTestBridge('initialize', {{instanceId:'test', identity:{{serverSession:'test',serverIncarnation:'test',serverBinary:'yinz'}}}});", super::SDK))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create test event loop")?;
    let result = rt.block_on(async {
        let module = runtime
            .load_main_es_module(loader.entrypoint())
            .await
            .context("load test modules")?;
        let evaluate = runtime.mod_evaluate(module);
        runtime
            .run_event_loop(Default::default())
            .await
            .context("evaluate test module")?;
        evaluate.await.context("register tests")?;
        let value = runtime.execute_script(
            "yinz:test-case",
            format!(
                "__yinzTestRun({}, {})",
                json!(request.index),
                json!(request.case)
            ),
        )?;
        let value = runtime.resolve(value);
        let value = runtime
            .with_event_loop_promise(value, Default::default())
            .await
            .context("run test")?;
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, value);
        serde_v8::from_v8::<Value>(scope, local).context("read test result")
    });
    limits.check()?;
    if let Some(case) = request.case {
        let host = runtime.op_state();
        let host = host.borrow();
        let host = host.borrow::<Host>();
        let mut output = host.output.clone();
        append_bounded(&mut output, &host.pending);
        Ok(json!(PluginTestResult {
            name: case.name,
            kind: PluginTestKind::Unit,
            status: if result.is_ok() { "passed" } else { "failed" }.into(),
            duration_ms: host.started.elapsed().as_millis() as u64,
            error: result.err().map(|error| format!("{error:#}")),
            output
        }))
    } else {
        result
    }
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
        if (*plugin_id, *client_id, *generation)
            == (env.plugin_id, env.client_id, env.deno_generation)
        {
            cancel.store(true, Ordering::Release);
        }
    }
}

pub(crate) fn cancel(env: &crate::plugin_map::PluginEnv, id: &str) {
    if let Some(cancel) = RUNS.lock().unwrap().get(&(
        env.plugin_id,
        env.client_id,
        env.deno_generation,
        id.to_owned(),
    )) {
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
                env.deno_generation,
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
        env.deno_generation,
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

#[cfg(test)]
mod unit;
