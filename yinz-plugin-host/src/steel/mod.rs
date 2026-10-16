//! Steel engines stay on their pinned executor; handles retain only host-owned state.

pub(crate) mod agent_provider;
pub(crate) mod authoring;
mod repl;
pub(crate) mod workers;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use serde_json::Value;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::thread_bus::PluginReplies;
use yinz_runtime::errors::prelude::*;
use yinz_steel::worker::WorkerRuntime as Runtime;
use yinz_steel::InterruptHandle;
use yinz_types::data::{CommandType, Event, PermissionStatus, PipeMessage, PluginCommand};

use crate::plugin_map::{PluginEnv, CANCELLED_BY_HOST};
use crate::response::{decode_response, DecodedResponse};
use crate::yinz_exports::{
    check_command_permission, permission_answer_pending, run_plugin_command,
};
use crate::PluginInstruction;

pub(crate) const EMBEDDED_ROOT: &str = "/__yinz_builtin__";

pub(crate) fn source_root(
    path: &Path,
    repository_root: Option<&Path>,
) -> Result<yinz_steel::modules::SourceRoot> {
    if let Ok(entry) = path.strip_prefix(EMBEDDED_ROOT) {
        let mut directories = vec![&yinz_steel_builtins::BUILTIN_STEEL_FILES];
        let mut sources = BTreeMap::new();
        while let Some(directory) = directories.pop() {
            directories.extend(directory.dirs());
            for file in directory.files() {
                sources.insert(
                    file.path()
                        .to_str()
                        .context("builtin path is not UTF-8")?
                        .to_owned(),
                    file.contents_utf8()
                        .context("builtin source is not UTF-8")?
                        .to_owned(),
                );
            }
        }
        yinz_steel::modules::SourceRoot::embedded(
            entry.to_str().context("builtin entrypoint is not UTF-8")?,
            sources,
        )
    } else {
        yinz_steel::modules::SourceRoot::file(path, repository_root)
    }
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_generation() -> Result<u64> {
    NEXT_GENERATION
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| anyhow!("plugin generation exhausted"))
}

thread_local! {
    static RUNTIMES: RefCell<HashMap<u64, Runtime>> = RefCell::new(HashMap::new());
}

#[cfg(test)]
pub(crate) fn runtime_count() -> usize {
    RUNTIMES.with(|runtimes| runtimes.borrow().len())
}

type HostEnv = Arc<Mutex<Option<PluginEnv>>>;

struct EnvScope<'a> {
    env: &'a mut Option<PluginEnv>,
    host: HostEnv,
    active: Arc<Mutex<bool>>,
}

impl Drop for EnvScope<'_> {
    fn drop(&mut self) {
        *self.env = self
            .host
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        *self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
    }
}

pub struct SteelPlugin {
    env: Option<PluginEnv>,
    host: HostEnv,
    generation: u64,
    thread: ThreadId,
    replies: Option<PluginReplies>,
    interrupt: InterruptHandle,
    watcher: tokio::task::JoinHandle<()>,
    active: Arc<Mutex<bool>>,
    client_cancel: Arc<AtomicBool>,
    repl_pipe: Option<String>,
    disposed: bool,
    first_render_started: Option<Instant>,
}

impl SteelPlugin {
    pub fn new(
        source: &str,
        path: &Path,
        env: PluginEnv,
        client_cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        Self::with_generation(source, path, env, client_cancel, next_generation()?)
    }

    fn with_generation(
        source: &str,
        path: &Path,
        mut env: PluginEnv,
        client_cancel: Arc<AtomicBool>,
        generation: u64,
    ) -> Result<Self> {
        let started = Instant::now();
        if env.cancel.load(Ordering::Acquire) || client_cancel.load(Ordering::Acquire) {
            bail!(CANCELLED_BY_HOST);
        }
        let source_path = match &env.plugin_config.location {
            yinz_types::input::layout::RunPluginLocation::Yinz(tag) => {
                Path::new(EMBEDDED_ROOT).join(format!("{tag}.scm"))
            },
            _ => env
                .resolved_source_path
                .clone()
                .unwrap_or_else(|| path.to_owned()),
        };
        let mut location = env.plugin_config.location.clone();
        if let yinz_types::input::layout::RunPluginLocation::Git(repository) = &mut location {
            if let Some(commit) = &env.plugin_config.installed_commit {
                if !repository.is_local() {
                    repository.revision = Some(yinz_types::input::git_plugin::GitRevision::Commit(
                        commit.clone(),
                    ));
                }
            }
        }
        let sources = source_root(&source_path, env.plugin_config.repository_root.as_deref())?
            .with_location(location);
        env.generation = Some(generation);
        env.workers
            .get_or_insert_with(|| Arc::new(crate::background_worker::Workers::new(generation)));
        let host: HostEnv = Arc::default();
        let commands = host.clone();
        let binary = crate::plugin_authoring::worker::server_executable()?;
        let worker_started = Instant::now();
        let runtime = Runtime::new(&binary, move |command| {
            let mut host = commands
                .lock()
                .map_err(|_| anyhow!("Steel host lock poisoned"))?;
            let env = host
                .as_mut()
                .context("host commands are unavailable during module initialization")?;
            dispatch(env, command)
        });
        log::info!(
            "startup: {} plugin={} client={} generation={generation} phase=worker_ready elapsed={:?} ok={}",
            env.log_identity(), env.plugin_id, env.client_id, worker_started.elapsed(), runtime.is_ok()
        );
        let mut runtime = runtime?;
        let identity = crate::host_bridge::server_identity(&env.server_session)?;
        runtime.set_server_metadata(yinz_steel::ServerMetadata {
            binary: identity.server_binary,
            session: Some(identity.server_session),
            incarnation: Some(identity.server_incarnation.to_owned()),
        })?;
        runtime.set_source_root(sources);
        let interrupt = runtime.interruption_handle();
        let active = Arc::new(Mutex::new(false));
        let watcher = {
            let interrupt = interrupt.clone();
            let active = active.clone();
            let cancel = env.cancel.clone();
            let tripped = env.cancel_tripped.clone();
            let client_cancel = client_cancel.clone();
            get_tokio_runtime().spawn(async move {
                loop {
                    {
                        let active = active.lock().unwrap_or_else(|error| error.into_inner());
                        if *active
                            && (cancel.load(Ordering::Acquire)
                                || client_cancel.load(Ordering::Acquire))
                        {
                            tripped.store(true, Ordering::Release);
                            interrupt.interrupt();
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
        };
        RUNTIMES.with(|runtimes| runtimes.borrow_mut().insert(generation, runtime));
        let mut plugin = Self {
            env: Some(env),
            host,
            generation,
            thread: thread::current().id(),
            replies: Some(PluginReplies::new(generation)),
            interrupt,
            watcher,
            active,
            client_cancel,
            repl_pipe: None,
            disposed: false,
            first_render_started: Some(started),
        };
        let log_identity = plugin.env().log_identity();
        let evaluation_started = Instant::now();
        let initialized = plugin.with_runtime(false, |runtime| {
            let evaluated = runtime
                .eval_with_path(source, Some(&source_path))
                .with_context(|| format!("evaluating Steel plugin {}", path.display()))?;
            for line in evaluated.output.lines() {
                log::info!("{log_identity} [steel:{}] {line}", path.display());
            }
            Ok(())
        });
        log::info!(
            "startup: {log_identity} plugin={} client={} generation={generation} phase=module_eval elapsed={:?} ok={}",
            plugin.env().plugin_id, plugin.env().client_id, evaluation_started.elapsed(), initialized.is_ok()
        );
        if let Err(error) = initialized {
            plugin.dispose_with_cleanup(false);
            return Err(error);
        }
        Ok(plugin)
    }

    pub fn env(&self) -> &PluginEnv {
        self.env
            .as_ref()
            .expect("Steel environment is absent only during a scoped call")
    }

    pub fn env_mut(&mut self) -> &mut PluginEnv {
        self.env
            .as_mut()
            .expect("Steel environment is absent only during a scoped call")
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn client_cancel(&self) -> Arc<AtomicBool> {
        self.client_cancel.clone()
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed || self.client_cancel.load(Ordering::Acquire)
    }

    fn with_runtime<T: Default>(
        &mut self,
        allow_host: bool,
        call: impl FnOnce(&mut Runtime) -> Result<T>,
    ) -> Result<T> {
        if self.is_disposed() {
            return Ok(T::default());
        }
        if self.thread != thread::current().id() {
            bail!("Steel plugin must run on its pinned executor thread");
        }
        let log_tag = format!(
            "{} [steel:{}]",
            self.env().log_identity(),
            self.env().plugin_config.path.display()
        );
        let client = self.env().client_id;
        {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if self.env().cancel.load(Ordering::Acquire) {
                bail!(CANCELLED_BY_HOST);
            }
            *active = true;
        }
        let mut local_env = None;
        let env = if allow_host {
            &mut self.env
        } else {
            &mut local_env
        };
        *self.host.lock().unwrap_or_else(|error| error.into_inner()) = env.take();
        let _scope = EnvScope {
            env,
            host: self.host.clone(),
            active: self.active.clone(),
        };
        RUNTIMES.with(|runtimes| {
            let mut runtimes = runtimes
                .try_borrow_mut()
                .context("reentrant Steel execution")?;
            let runtime = runtimes
                .get_mut(&self.generation)
                .context("Steel runtime is no longer available")?;
            if allow_host {
                runtime.invoke("yinz-async-enter", vec![serde_json::to_value(client)?])?;
            }
            let result = call(runtime);
            let output = runtime.take_output();
            let logs = runtime.take_logs();
            for captured in [&output, &logs].into_iter().flatten() {
                for line in captured.lines() {
                    log::info!("{log_tag} {line}");
                }
            }
            let result = result?;
            output.context("reading Steel plugin output")?;
            logs.context("reading Steel plugin diagnostics")?;
            Ok(result)
        })
    }

    pub fn load(&mut self, configuration: &BTreeMap<String, String>) -> Result<()> {
        let configuration =
            serde_json::to_value(configuration).context("encoding Steel configuration")?;
        let started = Instant::now();
        let loaded = self.with_runtime(true, |runtime| {
            runtime.load(configuration)?;
            // The load API has no boolean return slot, so re-arm its initial render for the host.
            runtime.invoke("yinz-request-render", vec![])?;
            Ok(())
        });
        log::info!(
            "startup: {} plugin={} client={} generation={} phase=load elapsed={:?} ok={}",
            self.env().log_identity(),
            self.env().plugin_id,
            self.env().client_id,
            self.generation,
            started.elapsed(),
            loaded.is_ok()
        );
        loaded?;
        // Permission prompts can hold resize events, so load's render request needs its own tick.
        self.env()
            .senders
            .send_to_plugin(PluginInstruction::PluginTick(
                self.env().plugin_id,
                self.env().client_id,
                self.generation,
            ))
            .context("scheduling initial Steel render")
    }

    pub fn update(&mut self, event: &Event) -> Result<bool> {
        let completions = if matches!(event, Event::PermissionRequestResult(_)) {
            if let Some(workers) = &self.env().workers {
                if workers.parent_generation == self.generation {
                    workers.permissions(self.env().client_id, event);
                }
            }
            self.permission_completions()?
        } else {
            vec![]
        };
        let event = crate::event::event_to_json(event)?;
        self.with_runtime(true, |runtime| {
            let mut changed = runtime.update(event)?;
            for completion in completions {
                changed |= deliver_permission(runtime, completion)?;
            }
            Ok(changed)
        })
    }

    fn permission_completions(&mut self) -> Result<Vec<crate::permissions::Completion>> {
        if self.is_disposed() {
            return Ok(vec![]);
        }
        if self.env().cancel.load(Ordering::Acquire) {
            bail!(CANCELLED_BY_HOST);
        }
        if let Some(workers) = self.env().workers.clone() {
            if workers.parent_generation == self.generation {
                return workers.take_permission_completions(self.env_mut());
            }
        }
        Ok(vec![])
    }

    pub fn tick(&mut self) -> Result<bool> {
        let completions = self.permission_completions()?;
        self.with_runtime(true, |runtime| {
            let mut changed = false;
            for completion in completions {
                changed |= deliver_permission(runtime, completion)?;
            }
            changed |= runtime
                .invoke("yinz-async-poll", vec![])?
                .as_bool()
                .unwrap_or(false);
            Ok(changed)
        })
    }

    fn permission_completion(
        &mut self,
        completion: crate::permissions::Completion,
    ) -> Result<bool> {
        if completion.request.generation != self.generation {
            return Ok(false);
        }
        self.with_runtime(true, |runtime| deliver_permission(runtime, completion))
    }

    pub fn pipe(&mut self, message: &PipeMessage) -> Result<bool> {
        if let Some(changed) = self.repl_pipe(message)? {
            return Ok(changed);
        }
        let message = serde_json::to_value(message).context("encoding Steel pipe message")?;
        self.with_runtime(true, |runtime| runtime.pipe(message))
    }

    pub fn palette(&mut self, message: &yinz_types::command_palette::Message) -> Result<bool> {
        let message = serde_json::to_value(message).context("encoding Steel palette message")?;
        self.with_runtime(true, |runtime| runtime.palette(message))
    }

    pub fn render(&mut self, rows: usize, columns: usize) -> Result<Vec<u8>> {
        let started = Instant::now();
        let frame = self.with_runtime(true, |runtime| {
            runtime
                .render(rows, columns)
                .map(|frame| frame.replace('\n', "\n\r").into_bytes())
        })?;
        if !self.is_disposed() {
            if let Some(startup) = self.first_render_started.take() {
                log::info!(
                    "startup: {} plugin={} client={} generation={} phase=first_render elapsed={:?} total={:?}",
                    self.env().log_identity(), self.env().plugin_id, self.env().client_id,
                    self.generation, started.elapsed(), startup.elapsed()
                );
            }
        }
        Ok(frame)
    }

    pub fn dispose_with_cleanup(&mut self, run_unload: bool) {
        if self.disposed {
            return;
        }
        if run_unload
            && self.thread == thread::current().id()
            && !self.is_disposed()
            && !self.env().cancel_tripped.load(Ordering::Acquire)
            && self
                .env()
                .workers
                .as_ref()
                .is_none_or(|workers| workers.parent_generation == self.generation)
        {
            self.env().cancel.store(false, Ordering::Release);
            let replies = self
                .replies
                .as_ref()
                .expect("live Steel reply mailbox")
                .begin_cleanup();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let interrupt = self.interrupt.clone();
            let (finished, completion) = std::sync::mpsc::channel();
            let watchdog = thread::spawn(move || {
                if matches!(
                    completion.recv_timeout(Duration::from_secs(2)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    interrupt.interrupt();
                }
            });
            self.with_runtime(true, |runtime| {
                runtime.unload()?;
                loop {
                    if runtime.invoke("yinz-async-unloaded", vec![])?.as_bool() == Some(true) {
                        runtime.invoke("yinz-async-cancel", vec![])?;
                        return Ok(());
                    }
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        bail!("Steel unload exceeded its two-second deadline");
                    }
                    if let Ok((client, event)) =
                        replies.recv_timeout(remaining.min(Duration::from_millis(10)))
                    {
                        runtime.invoke("yinz-async-enter", vec![serde_json::to_value(client)?])?;
                        runtime.update(crate::event::event_to_json(&event)?)?;
                    }
                }
            })
            .context("unloading Steel plugin")
            .non_fatal();
            let _ = finished.send(());
            let _ = watchdog.join();
        }
        crate::testing::cancel_instance(self.env());
        self.disposed = true;
        self.replies.take();
        if let Some(workers) = &self.env().workers {
            if workers.parent_generation == self.generation {
                workers.stop();
            } else {
                workers.forget_permission_requests(self.generation);
            }
        }
        self.watcher.abort();
        self.interrupt.interrupt();
        self.env_mut().filesystem_watcher.take();
        self.env().clear_agent_providers();
        self.env().clear_data_watches(self.generation);
        self.env()
            .senders
            .send_to_screen(yinz_bus::screen::ScreenInstruction::UnsubscribePaneOutput(
                self.env().plugin_id,
                Some(self.generation),
            ))
            .context("removing disposed Steel pane-output subscriptions")
            .non_fatal();
        self.env()
            .senders
            .send_to_screen(yinz_bus::screen::ScreenInstruction::CommandPalette(
                yinz_bus::command_palette::Instruction::Dispose {
                    plugin_id: self.env().plugin_id,
                    generation: self.generation,
                },
            ))
            .context("removing disposed Steel palette providers")
            .non_fatal();
        if self.thread == thread::current().id() {
            dispose_generation(self.generation);
        } else {
            self.env()
                .senders
                .send_to_plugin(PluginInstruction::PluginDispose(
                    self.env().plugin_id,
                    self.generation,
                ))
                .context("queuing Steel disposal on its pinned executor")
                .non_fatal();
        }
    }
}

impl Drop for SteelPlugin {
    fn drop(&mut self) {
        self.dispose_with_cleanup(true);
    }
}

pub(crate) fn dispose_generation(generation: u64) {
    RUNTIMES.with(|runtimes| runtimes.borrow_mut().remove(&generation));
}

fn dispatch(env: &mut PluginEnv, value: Value) -> Result<Value> {
    if let Some(envelope) = value.get("__yinz_runtime") {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Command {
            client: yinz_types::internal_ids::ClientId,
            command: Value,
            permission_id: Option<String>,
        }
        anyhow::ensure!(
            value.as_object().is_some_and(|value| value.len() == 1),
            "invalid Steel command envelope"
        );
        let command: Command =
            serde_json::from_value(envelope.clone()).context("invalid Steel callback context")?;
        let previous = std::mem::replace(&mut env.client_id, command.client);
        let result = dispatch_command(env, command.command, command.permission_id);
        env.client_id = previous;
        return result;
    }
    dispatch_command(env, value, None)
}

fn dispatch_command(
    env: &mut PluginEnv,
    value: Value,
    permission_id: Option<String>,
) -> Result<Value> {
    if env.cancel.load(Ordering::Acquire) {
        env.cancel_tripped.store(true, Ordering::Release);
        bail!(CANCELLED_BY_HOST);
    }
    let command =
        yinz_steel::binary::decode_command(value).context("invalid yinz command or arguments")?;
    let command_type = CommandType::from(&command);
    if let PluginCommand::RequestPluginPermissions(permissions) = command {
        let workers = env.workers.clone().context("missing Steel coordinator")?;
        workers.request_permissions(
            env,
            crate::permissions::Request::new(
                env.generation.context("missing Steel generation")?,
                env.client_id,
                permissions,
                permission_id.unwrap_or_default(),
            ),
        )?;
        return Ok(Value::Null);
    }
    if let (PermissionStatus::Denied, Some(permission)) = check_command_permission(env, &command) {
        if permission_answer_pending(env) {
            bail!("'{command_type}' requires '{permission}'; wait for PermissionRequestResult before calling it");
        }
        bail!("'{command_type}' requires '{permission}'; request it with RequestPluginPermissions");
    }
    let previous_sink = env.response_sink.take();
    let result = run_plugin_command(env, command);
    let captured = env.response_sink.take();
    env.response_sink = previous_sink;
    result.context("yinz command failed")?;
    match captured {
        None => Ok(Value::Null),
        Some(bytes) => match decode_response(command_type, &bytes).transpose()? {
            Some(DecodedResponse::Json(value)) => Ok(value),
            Some(DecodedResponse::Bytes(bytes)) => yinz_steel::binary::encode(&bytes),
            None => Ok(serde_json::to_value(bytes)?),
        },
    }
}

fn deliver_permission(
    runtime: &mut Runtime,
    completion: crate::permissions::Completion,
) -> Result<bool> {
    Ok(runtime
        .invoke(
            "yinz-async-permission",
            vec![
                Value::String(completion.request.id),
                serde_json::to_value(completion.result)?,
            ],
        )?
        .as_bool()
        .unwrap_or(false))
}

#[cfg(test)]
#[path = "unit/host.rs"]
mod tests;
