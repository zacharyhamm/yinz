//! A Send plugin handle with its V8 isolate confined to the pinned executor thread.
mod agent_provider;
mod analysis;
pub use agent_provider::inspect_agent_provider;
mod authoring;
pub(crate) mod background_worker;
mod checker;
mod event;
mod image;
mod limits;
mod loader;
mod permissions;
mod transport;
mod ui;
mod worker;
pub use worker::run_compilation_worker;
#[cfg(test)]
pub(crate) use worker::spawn_count as compilation_worker_spawn_count;

#[cfg(test)]
pub use checker::check_plugin;
pub use checker::check_syntax;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Wake, Waker};
use std::thread::{self, ThreadId};
use std::time::Duration;

use deno_core::futures::FutureExt;
use deno_core::{op2, serde_v8, v8, JsRuntime, OpState, RuntimeOptions};
use deno_error::JsErrorBox;
use serde_json::{json, Value};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{CommandType, Event, PermissionStatus, PipeMessage, PluginCommand};

use super::plugin_map::{PluginEnv, ResponseSink, CANCELLED_BY_HOST};
use super::response::{decode_response, DecodedResponse};
use super::yinz_exports::{
    check_command_permission, permission_answer_pending, run_plugin_command,
};
use super::{PluginId, PluginInstruction};
use image::op_yinz_image;
use limits::op_yinz_encode;
use ui::{op_yinz_ui_call, op_yinz_ui_create, op_yinz_ui_pure};
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::{thread_bus::ThreadSenders, ClientId};

pub mod testing;

const SDK: &str = include_str!("sdk.js");
pub(crate) const SDK_MODULE: &str = "export const yinz = globalThis.yinz; export const definePlugin = yinz.definePlugin; export default yinz;";

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
#[op2]
#[string]
fn op_yinz_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[op2(fast)]
fn op_yinz_monotonic_ms() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0
}
thread_local! {
    static RUNTIMES: RefCell<HashMap<u64, RuntimeInstance>> = RefCell::new(HashMap::new());
}

struct HostState {
    env: Option<PluginEnv>,
    /// The `[deno:<name>]` prefix of this plugin's log lines, known before `env` is: a
    /// `console.log` at module top level is the first thing many authors write.
    log_tag: String,
}

// CheckPlugin's semantic check enters a separate compiler isolate. Its ops cannot
// reach this plugin or its borrowed host state, but Deno's op guard is thread-local.
#[op2(reentrant)]
fn op_yinz_command<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &mut OpState,
    value: v8::Local<'s, v8::Value>,
    #[smi] client: u32,
    #[string] completion: Option<String>,
) -> std::result::Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    let limits = state.borrow::<Arc<limits::Limits>>().clone();
    limits
        .check()
        .and_then(|_| transport::from_v8(scope, value))
        .and_then(|value| dispatch(state, value, client, completion))
        .and_then(|reply| reply.into_v8(scope, &limits))
        .map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}

/// What a host command hands back to the guest: `{json}` for a decoded reply and
/// `{bytes}` for a byte reply, the latter a real `Uint8Array` rather than a JSON array of
/// numbers walked element by element on both sides of the boundary.
enum Reply {
    Json(Value),
    Bytes(Vec<u8>),
}

impl Reply {
    fn into_v8<'s>(
        self,
        scope: &mut v8::PinScope<'s, '_>,
        limits: &Arc<limits::Limits>,
    ) -> Result<v8::Local<'s, v8::Value>> {
        let (key, value): (&str, v8::Local<v8::Value>) = match self {
            Reply::Json(value) => (
                "json",
                serde_v8::to_v8(scope, value).context("serializing Deno command reply")?,
            ),
            Reply::Bytes(bytes) => (
                "bytes",
                limits::counted_bytes(scope, limits, &bytes)?.into(),
            ),
        };
        Ok(object_with(scope, key, value).into())
    }
}

fn object_with<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    key: &str,
    value: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Object> {
    let object = v8::Object::new(scope);
    let key = v8::String::new(scope, key).expect("short ASCII key");
    object.set(scope, key.into(), value);
    object
}

// Validate before the SDK inspects permission/unsubscribe payloads. Otherwise
// its bookkeeping could invoke an accessor or proxy before the command op.
#[op2(fast)]
fn op_yinz_validate<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> std::result::Result<(), JsErrorBox> {
    transport::from_v8(scope, value)
        .map(|_| ())
        .map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}

fn dispatch(
    state: &mut OpState,
    value: Value,
    client: u32,
    completion: Option<String>,
) -> Result<Reply> {
    let env = state
        .borrow_mut::<HostState>()
        .env
        .as_mut()
        .context("host commands are unavailable during module initialization")?;
    if env.cancel.load(Ordering::Acquire) {
        env.cancel_tripped.store(true, Ordering::Release);
        bail!(CANCELLED_BY_HOST);
    }
    let command: PluginCommand =
        serde_json::from_value(value).context("invalid yinz command or arguments")?;
    let command_type = CommandType::from(&command);
    if let (PermissionStatus::Denied, Some(permission)) = check_command_permission(env, &command) {
        // Two failures wear the same denial and have opposite fixes: a plugin that asked and
        // has not been *answered* - every plugin that asks in `load`, for the whole of `load`,
        // because the grant arrives as an event - needs to wait, and a plugin that never asked
        // needs to ask. See `permission_answer_pending`.
        if permission_answer_pending(env) {
            bail!(
                "'{command_type}' requires the '{permission}' permission, and no permission has \
                 been answered for this plugin yet - a grant arrives as the \
                 PermissionRequestResult event, so nothing gated works synchronously during \
                 load. Ask in load with yinz.requestPermission(['{permission}']) and await \
                 yinz.waitForPermissions(['{permission}']) before calling it"
            );
        }
        bail!(
            "'{command_type}' requires the '{permission}' permission, which this plugin has not \
             been granted - ask for it in load with yinz.requestPermission(['{permission}']) \
             and await yinz.waitForPermissions(['{permission}'])"
        );
    }
    let client = ClientId::try_from(client).context("invalid callback client")?;
    if let PluginCommand::RequestPluginPermissions(permissions) = command {
        let workers = env
            .deno_workers
            .clone()
            .context("missing Deno coordinator")?;
        workers.request_permissions(
            env,
            permissions::Request::new(
                env.deno_generation.context("missing Deno generation")?,
                client,
                permissions,
                completion.context("missing permission completion ID")?,
            ),
        )?;
        return Ok(Reply::Json(Value::Null));
    }
    let previous_client = std::mem::replace(&mut env.client_id, client);
    let previous_sink = std::mem::replace(&mut env.response_sink, ResponseSink::Capture(None));
    let result = run_plugin_command(env, command);
    let captured = env.response_sink.take();
    env.response_sink = previous_sink;
    env.client_id = previous_client;
    result.context("yinz command failed")?;
    match captured {
        None => Ok(Reply::Json(Value::Null)),
        Some(reply) => match serde_json::from_str::<Vec<u8>>(&reply) {
            Ok(bytes) => match decode_response(command_type, &bytes).transpose()? {
                Some(DecodedResponse::Json(value)) => Ok(Reply::Json(value)),
                Some(DecodedResponse::Bytes(bytes)) => Ok(Reply::Bytes(bytes)),
                None => Ok(Reply::Bytes(bytes)),
            },
            Err(_) => Ok(Reply::Json(Value::String(reply))),
        },
    }
}

#[op2(fast)]
fn op_yinz_log(state: &OpState, #[string] line: &str) {
    for line in line.split('\n') {
        log::info!("{} {}", state.borrow::<HostState>().log_tag, line);
    }
}

#[op2(fast)]
fn op_yinz_worker_started(state: &mut OpState) {
    // Charge jobs waiting in the JavaScript FIFO until their callbacks actually start.
    state
        .borrow_mut::<VecDeque<background_worker::PostReservation>>()
        .pop_front();
}

fn log_tag(env: &PluginEnv) -> String {
    format!(
        "{} [deno:{}]",
        env.log_identity(),
        env.resolved_source_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().escape_default().to_string())
            .unwrap_or_else(|| env
                .plugin_config
                .path
                .to_string_lossy()
                .escape_default()
                .to_string())
    )
}

deno_core::extension!(
    yinz_host,
    ops = [
        op_yinz_command,
        op_yinz_monotonic_ms,
        op_yinz_validate,
        op_yinz_encode,
        op_yinz_log,
        op_yinz_worker_started,
        op_yinz_ui_create,
        op_yinz_ui_call,
        op_yinz_ui_pure,
        op_yinz_image,
        op_yinz_uuid
    ],
    state = |state| state.put(VecDeque::<background_worker::PostReservation>::new())
);

struct TickWake {
    senders: ThreadSenders,
    plugin_id: PluginId,
    owner_client_id: ClientId,
    generation: u64,
    queued: AtomicBool,
    stopped: AtomicBool,
}

impl Wake for TickWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if self.stopped.load(Ordering::Acquire) || self.queued.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(error) = self.senders.send_to_plugin(PluginInstruction::DenoTick(
            self.plugin_id,
            self.owner_client_id,
            self.generation,
        )) {
            self.queued.store(false, Ordering::Release);
            Err::<(), _>(error)
                .context("failed to wake Deno plugin")
                .non_fatal();
        }
    }
}

struct RuntimeInstance {
    // Drop persistent handles before the isolate they belong to.
    bridge: v8::Global<v8::Function>,
    runtime: JsRuntime,
    watcher: tokio::task::JoinHandle<()>,
    terminate: v8::IsolateHandle,
    wake: Arc<TickWake>,
    parked: bool,
    limits: Arc<limits::Limits>,
}

/// The watcher and invocation boundaries share this lock, so an interrupt cannot
/// race with the end of one call and poison an idle isolate or a later cleanup call.
struct ExecutionScope(Arc<Mutex<bool>>);

impl ExecutionScope {
    fn enter(
        active: &Arc<Mutex<bool>>,
        cancel: &AtomicBool,
        client_cancel: &AtomicBool,
    ) -> Result<Option<Self>> {
        let mut executing = active.lock().unwrap_or_else(|error| error.into_inner());
        if client_cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        if cancel.load(Ordering::Acquire) {
            // Nothing entered the guest, so cancel_tripped must remain false.
            bail!(CANCELLED_BY_HOST);
        }
        assert!(!*executing, "Deno invocation is already active");
        *executing = true;
        Ok(Some(Self(active.clone())))
    }
}

impl Drop for ExecutionScope {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = false;
    }
}

fn interrupt_active(
    active: &Mutex<bool>,
    cancel: &AtomicBool,
    client_cancel: &AtomicBool,
    tripped: &AtomicBool,
    terminate: &v8::IsolateHandle,
) {
    let executing = active.lock().unwrap_or_else(|error| error.into_inner());
    if *executing && (cancel.load(Ordering::Acquire) || client_cancel.load(Ordering::Acquire)) {
        tripped.store(true, Ordering::Release);
        terminate.terminate_execution();
    }
}

impl Drop for RuntimeInstance {
    fn drop(&mut self) {
        self.wake.stopped.store(true, Ordering::Release);
        self.watcher.abort();
        self.terminate.terminate_execution();
        if self.parked {
            // SAFETY: RuntimeInstance is !Send and stays on its creating thread.
            // Parking exited the constructor's entry. JsRuntime's destructor needs
            // its isolate current, and OwnedIsolate::drop consumes this entry.
            unsafe { self.runtime.v8_isolate().enter() };
        }
    }
}

/// Balance entry even when a callback errors or unwinds. No V8 scopes or local
/// handles may escape the call; persistent handles remain in RuntimeInstance.
struct RuntimeEntry<'a>(&'a mut RuntimeInstance);

impl std::ops::Deref for RuntimeEntry<'_> {
    type Target = RuntimeInstance;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl std::ops::DerefMut for RuntimeEntry<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

impl Drop for RuntimeEntry<'_> {
    fn drop(&mut self) {
        self.0.park();
    }
}

/// Moves authority into OpState only while host-controlled JavaScript is running.
/// Drop restores it on errors and unwinding as well as on success.
struct EnvScope<'a> {
    env: &'a mut Option<PluginEnv>,
    state: Rc<RefCell<OpState>>,
}

impl Drop for EnvScope<'_> {
    fn drop(&mut self) {
        *self.env = self.state.borrow_mut().borrow_mut::<HostState>().env.take();
    }
}

pub struct DenoPlugin {
    replies: Option<yinz_bus::thread_bus::DenoReplies>,
    env: Option<PluginEnv>,
    generation: u64,
    thread: ThreadId,
    wake: Arc<TickWake>,
    terminate: v8::IsolateHandle,
    disposed: bool,
    client_cancel: Arc<AtomicBool>,
    active: Arc<Mutex<bool>>,
}

impl DenoPlugin {
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
        let replies = yinz_bus::thread_bus::DenoReplies::new(generation);
        env.deno_generation = Some(generation);
        env.deno_workers
            .get_or_insert_with(|| Arc::new(background_worker::Workers::new(generation)));
        let wake = Arc::new(TickWake {
            // Event-loop wakers may outlive the isolate. They only route back to
            // the plugin thread, so must not retain the screen or other services.
            senders: ThreadSenders {
                to_plugin: env.senders.to_plugin.clone(),
                should_silently_fail: env.senders.should_silently_fail,
                ..Default::default()
            },
            plugin_id: env.plugin_id,
            owner_client_id: env.owner_client_id,
            generation,
            queued: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let active = Arc::new(Mutex::new(false));
        let instance =
            RuntimeInstance::new(source, path, &env, wake.clone(), &client_cancel, &active)?;
        let terminate = instance.terminate.clone();
        RUNTIMES.with(|instances| instances.borrow_mut().insert(generation, instance));
        Ok(Self {
            replies: Some(replies),
            env: Some(env),
            generation,
            thread: thread::current().id(),
            wake,
            terminate,
            disposed: false,
            client_cancel,
            active,
        })
    }

    pub fn env(&self) -> &PluginEnv {
        self.env
            .as_ref()
            .expect("Deno environment is only absent during a scoped call")
    }

    pub fn env_mut(&mut self) -> &mut PluginEnv {
        self.env
            .as_mut()
            .expect("Deno environment is only absent during a scoped call")
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed || self.client_cancel.load(Ordering::Acquire)
    }

    pub fn client_cancel(&self) -> Arc<AtomicBool> {
        self.client_cancel.clone()
    }

    pub fn load(&mut self, configuration: &BTreeMap<String, String>) -> Result<()> {
        if self.is_disposed() {
            return Ok(());
        }
        self.with_runtime(|runtime, client| {
            runtime.call("load", serde_json::to_value(configuration)?, Some(client))?;
            runtime.poll()?;
            // A load has no boolean return slot; preserve its render request for a tick.
            runtime.wake.wake_by_ref();
            Ok(())
        })
    }

    pub fn update(&mut self, event: &Event) -> Result<bool> {
        if self.is_disposed() {
            return Ok(false);
        }
        let completions = if let Event::PermissionRequestResult(_) = event {
            if let Some(workers) = &self.env().deno_workers {
                if workers.parent_generation == self.generation {
                    workers.permissions(self.env().client_id, event);
                }
            }
            // Process the recorded answer in this event's render/resize replay. Ordinary
            // observers cannot supply an answer, and still run before awaiters resume.
            self.permission_completions()?
        } else {
            vec![]
        };
        self.with_runtime(|runtime, client| {
            runtime.call_event(event, Some(client))?;
            for completion in completions {
                runtime.permission_result(completion)?;
            }
            runtime.poll()?;
            runtime.needs_render()
        })
    }

    pub fn palette(&mut self, message: &yinz_types::command_palette::Message) -> Result<bool> {
        self.lifecycle(
            "palette",
            serde_json::to_value(message).context("encoding private palette message")?,
        )
    }

    pub fn pipe(&mut self, message: &PipeMessage) -> Result<bool> {
        if self.is_disposed() {
            return Ok(false);
        }
        self.lifecycle(
            "pipe",
            serde_json::to_value(message).context("serializing Deno pipe message")?,
        )
    }

    fn lifecycle(&mut self, name: &str, arg: Value) -> Result<bool> {
        self.with_runtime(|runtime, client| {
            runtime.call(name, arg, Some(client))?;
            runtime.poll()?;
            runtime.needs_render()
        })
    }

    pub fn render(&mut self, rows: usize, columns: usize) -> Result<Vec<u8>> {
        if self.is_disposed() {
            return Ok(Vec::new());
        }
        self.with_runtime(|runtime, client| {
            let frame = runtime.call("render", json!([rows, columns]), Some(client))?;
            runtime.poll()?;
            if runtime.call("peek", Value::Null, None)?.as_bool() == Some(true) {
                runtime.wake.wake_by_ref();
            }
            let text = frame
                .as_str()
                .context("Deno render did not return a string")?;
            Ok(text.replace('\n', "\n\r").into_bytes())
        })
    }

    pub fn tick(&mut self) -> Result<bool> {
        if self.is_disposed() {
            return Ok(false);
        }
        // Clear only when consuming a tick. Clearing from render/update would admit duplicates.
        self.wake.queued.store(false, Ordering::Release);
        let completions = self.permission_completions()?;
        self.with_runtime(|runtime, _| {
            for completion in completions {
                runtime.permission_result(completion)?;
            }
            runtime.poll()?;
            runtime.needs_render()
        })
    }

    fn with_runtime<T: Default>(
        &mut self,
        call: impl FnOnce(&mut RuntimeInstance, ClientId) -> Result<T>,
    ) -> Result<T> {
        if self.is_disposed() {
            return Ok(T::default());
        }
        if self.thread != thread::current().id() {
            bail!("Deno plugin must run on its pinned executor thread");
        }
        let env = self.env().client_id;
        let Some(_execution) =
            ExecutionScope::enter(&self.active, &self.env().cancel, &self.client_cancel)?
        else {
            return Ok(T::default());
        };
        // Host operations run synchronously on the pinned executor.
        RUNTIMES.with(|instances| {
            let mut instances = instances
                .try_borrow_mut()
                .context("reentrant Deno execution")?;
            let runtime = instances
                .get_mut(&self.generation)
                .context("Deno runtime is no longer available")?;
            let mut runtime = runtime.enter();
            let state = runtime.runtime.op_state();
            state.borrow_mut().borrow_mut::<HostState>().env = self.env.take();
            let _scope = EnvScope {
                env: &mut self.env,
                state,
            };
            runtime.limits.check()?;
            let result = call(&mut runtime, env);
            runtime.limits.check()?;
            result
        })
    }

    fn unload(&mut self) {
        // A terminated or failed isolate cannot safely run guest code again. Workers
        // bind an empty lifecycle object, so they have no pane cleanup to invoke.
        if self.thread != thread::current().id()
            || self.is_disposed()
            || self.env().cancel_tripped.load(Ordering::Acquire)
        {
            return;
        }
        self.env().cancel.store(false, Ordering::Release);
        let replies = self
            .replies
            .as_ref()
            .expect("live isolate reply mailbox")
            .begin_cleanup();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let terminate = self.terminate.clone();
        let (finished, completion) = std::sync::mpsc::channel();
        // Interrupt synchronous loops as well as promises that never settle. Unlike
        // the shared cancellation token, this cannot poison a replacement instance.
        let watchdog = thread::spawn(move || {
            if matches!(
                completion
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                terminate.terminate_execution();
                true
            } else {
                false
            }
        });
        let result = self.with_runtime(|runtime, client| {
            runtime
                .call("unload", Value::Null, Some(client))
                .context("starting unload hook")?;
            loop {
                runtime.poll().context("polling unload cleanup")?;
                if runtime
                    .call("unloaded", Value::Null, None)
                    .context("checking unload completion")?
                    .as_bool()
                    == Some(true)
                {
                    runtime
                        .call("cancelPending", Value::Null, Some(client))
                        .context("cancelling pending SDK operations")?;
                    runtime.poll().context("polling unload cleanup")?;
                    return Ok(());
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    bail!("unload exceeded its two-second deadline");
                }
                if let Ok((client, event)) =
                    replies.recv_timeout(remaining.min(Duration::from_millis(10)))
                {
                    runtime
                        .call_event(&event, Some(client))
                        .context("delivering unload reply")?;
                }
            }
        });
        let _ = finished.send(());
        let timed_out = watchdog.join().unwrap_or(false);
        if timed_out || std::time::Instant::now() >= deadline {
            log::warn!(
                "plugin {} unload exceeded its two-second deadline; forcing teardown",
                self.env().plugin_id
            );
        } else if let Err(error) = result {
            log::error!("plugin {} unload failed: {error:#}", self.env().plugin_id);
        }
    }

    pub fn dispose(&mut self) {
        self.dispose_with_cleanup(true);
    }

    pub(crate) fn dispose_with_cleanup(&mut self, run_unload: bool) {
        if self.disposed {
            return;
        }
        if run_unload {
            self.unload();
        }
        self.env_mut().filesystem_watcher.take();
        testing::cancel_instance(self.env());
        self.disposed = true;
        self.replies.take();
        if let Some(workers) = &self.env().deno_workers {
            if workers.parent_generation == self.generation {
                workers.stop();
            } else {
                workers
                    .permissions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .forget(self.generation);
            }
        }
        self.env()
            .senders
            .send_to_screen(yinz_bus::screen::ScreenInstruction::CommandPalette(
                yinz_bus::command_palette::Instruction::Dispose {
                    plugin_id: self.env().plugin_id,
                    generation: self.generation,
                },
            ))
            .context("removing disposed palette providers")
            .non_fatal();
        self.env().clear_agent_providers();
        self.wake.stopped.store(true, Ordering::Release);
        self.terminate.terminate_execution();
        self.env().clear_deno_watches(self.generation);
        self.env()
            .senders
            .send_to_screen(yinz_bus::screen::ScreenInstruction::UnsubscribePaneOutput(
                self.env().plugin_id,
                Some(self.generation),
            ))
            .context("removing disposed Deno pane-output subscriptions")
            .non_fatal();
        if self.thread == thread::current().id() {
            dispose_generation(self.generation);
        } else {
            self.wake
                .senders
                .send_to_plugin(PluginInstruction::DenoDispose(
                    self.wake.plugin_id,
                    self.generation,
                ))
                .context("failed to queue Deno disposal on its pinned thread")
                .non_fatal();
        }
    }
}

fn next_generation() -> Result<u64> {
    NEXT_GENERATION
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| anyhow!("Deno plugin generation exhausted"))
}

impl Drop for DenoPlugin {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Called by the executor, including when the Send handle was dropped on another thread.
pub(crate) fn dispose_generation(generation: u64) {
    RUNTIMES.with(|instances| {
        instances.borrow_mut().remove(&generation);
    });
}

impl RuntimeInstance {
    fn enter(&mut self) -> RuntimeEntry<'_> {
        assert!(self.parked, "Deno runtime is already entered");
        // SAFETY: This !Send runtime is only accessed on its creating executor
        // thread. RuntimeEntry exits after all callback scopes have been dropped.
        unsafe { self.runtime.v8_isolate().enter() };
        self.parked = false;
        RuntimeEntry(self)
    }

    fn park(&mut self) {
        assert!(!self.parked, "Deno runtime is already parked");
        // SAFETY: Construction or RuntimeEntry made this isolate current; all V8
        // handle scopes have ended. No parked instance remains on V8's TLS stack.
        unsafe { self.runtime.v8_isolate().exit() };
        self.parked = true;
    }

    fn new(
        source: &str,
        path: &Path,
        env: &PluginEnv,
        wake: Arc<TickWake>,
        client_cancel: &Arc<AtomicBool>,
        active: &Arc<Mutex<bool>>,
    ) -> Result<Self> {
        let Some(_execution) = ExecutionScope::enter(active, &env.cancel, client_cancel)? else {
            bail!(CANCELLED_BY_HOST);
        };
        let _entered = get_tokio_runtime().enter();
        let loader = Rc::new(match &env.plugin_config.location {
            yinz_types::input::layout::RunPluginLocation::Yinz(tag) => {
                loader::PluginModuleLoader::embedded(&tag.to_string(), source)?
            },
            _ if env.plugin_config.repository_root.is_none() => {
                loader::PluginModuleLoader::new(path, source)?
            },
            _ => loader::PluginModuleLoader::new_with_root(
                path,
                source,
                env.plugin_config.repository_root.as_deref(),
            )?,
        });
        let limits = limits::Limits::new(path.display().to_string());
        let mut runtime = limits
            .runtime(RuntimeOptions {
                module_loader: Some(loader.clone()),
                extensions: vec![yinz_host::init()],
                ..Default::default()
            })
            .context("creating Deno runtime")?;
        runtime.op_state().borrow_mut().put(HostState {
            env: None,
            log_tag: log_tag(env),
        });
        let bridge = runtime
            .execute_script("yinz:sdk-bootstrap", SDK)
            .map_err(|e| anyhow!("{e}"))?;
        let bridge = {
            deno_core::scope!(scope, runtime);
            let value = v8::Local::new(scope, bridge);
            let function = v8::Local::<v8::Function>::try_from(value)
                .context("SDK bridge must be a function")?;
            v8::Global::new(scope, function)
        };
        let terminate = runtime.v8_isolate().thread_safe_handle();
        let cancel = env.cancel.clone();
        let tripped = env.cancel_tripped.clone();
        let interrupt = terminate.clone();
        let alive = wake.clone();
        let client_cancel = client_cancel.clone();
        let active = active.clone();
        let allocation_limits = limits.clone();
        // ponytail: one 100 Hz cancellation watcher per isolate; use a shared
        // thread-safe isolate-handle registry if plugin counts make polling costly.
        let watcher = get_tokio_runtime().spawn(async move {
            loop {
                if alive.stopped.load(Ordering::Acquire) {
                    break;
                }
                interrupt_active(&active, &cancel, &client_cancel, &tripped, &interrupt);
                if allocation_limits.exhausted() {
                    interrupt.terminate_execution();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        // Construct before evaluating user code so every error aborts the watcher.
        let mut instance = Self {
            runtime,
            bridge,
            watcher,
            terminate,
            wake,
            parked: false,
            limits,
        };
        instance.call(
            "initialize",
            json!({
                "instanceId": uuid::Uuid::new_v4().to_string(),
                "identity": super::host_bridge::server_identity(&env.server_session)?,
            }),
            None,
        )?;
        let module = get_tokio_runtime()
            .block_on(instance.runtime.load_main_es_module(loader.entrypoint()))
            .context("loading Deno plugin modules");
        instance.limits.check()?;
        let module = module?;
        let evaluation = instance.runtime.mod_evaluate(module);
        instance.poll()?;
        evaluation
            .now_or_never()
            .context("plugin module initialization must complete without pending top-level await")?
            .context("evaluating Deno plugin module")?;
        let namespace = instance
            .runtime
            .get_module_namespace(module)
            .context("reading Deno plugin exports")?;
        let namespace = {
            deno_core::scope!(scope, instance.runtime);
            let value = v8::Local::new(scope, namespace);
            let value: v8::Local<v8::Value> = value.into();
            v8::Global::new(scope, value)
        };
        let bound = instance.call_value("bind", namespace, None);
        instance.limits.check()?;
        bound?;
        instance.park();
        Ok(instance)
    }

    fn poll(&mut self) -> Result<()> {
        let waker = Waker::from(self.wake.clone());
        let mut context = TaskContext::from_waker(&waker);
        let result = match self
            .runtime
            .poll_event_loop(&mut context, Default::default())
        {
            Poll::Ready(result) => result.context("polling Deno plugin event loop"),
            Poll::Pending => Ok(()),
        };
        self.limits.check()?;
        result
    }

    fn call(&mut self, name: &str, arg: Value, client: Option<ClientId>) -> Result<Value> {
        let arg = {
            deno_core::scope!(scope, self.runtime);
            let value =
                serde_v8::to_v8(scope, arg).context("serializing Deno callback argument")?;
            v8::Global::new(scope, value)
        };
        self.call_value(name, arg, client)
    }

    /// Deliver an event, with live output as bytes.
    ///
    /// `RunCommandOutput` and `PaneOutput` carry a chunk of a stream that arrives at every
    /// render. Serialized as JSON their `data` is a number per byte, walked once into a
    /// `serde_json::Value`, once into a JS array and once more by the SDK into the
    /// `Uint8Array` the guest wanted all along; here the chunk is copied once, into a buffer
    /// charged to the isolate's budget. Every other event keeps its JSON shape.
    fn call_event(&mut self, event: &Event, client: Option<ClientId>) -> Result<Value> {
        let arg = (|| {
            deno_core::scope!(scope, self.runtime);
            let (tag, data, fields) = match event {
                Event::RunCommandOutput {
                    data,
                    stderr,
                    context,
                } => (
                    "RunCommandOutput",
                    data,
                    json!({ "stderr": stderr, "context": context }),
                ),
                Event::PaneOutput {
                    data,
                    pane_id,
                    context,
                } => (
                    "PaneOutput",
                    data,
                    json!({ "pane_id": pane_id, "context": context }),
                ),
                _ => {
                    let value = serde_v8::to_v8(scope, event::event_to_json(event)?)
                        .context("serializing Deno event")?;
                    return Ok(v8::Global::new(scope, value));
                },
            };
            let payload: v8::Local<v8::Object> = serde_v8::to_v8(scope, fields)
                .context("serializing Deno event")?
                .try_into()
                .context("event payload is an object")?;
            let bytes = limits::counted_bytes(scope, &self.limits, data)?;
            let key = v8::String::new(scope, "data").expect("short ASCII key");
            payload.set(scope, key.into(), bytes.into());
            let value: v8::Local<v8::Value> = object_with(scope, tag, payload.into()).into();
            Ok::<_, anyhow::Error>(v8::Global::new(scope, value))
        })()?;
        self.call_value("update", arg, client)
    }

    fn call_value(
        &mut self,
        name: &str,
        arg: v8::Global<v8::Value>,
        client: Option<ClientId>,
    ) -> Result<Value> {
        let args = {
            deno_core::scope!(scope, self.runtime);
            let name = serde_v8::to_v8(scope, name).context("serializing callback name")?;
            let name = v8::Global::new(scope, name);
            let client = serde_v8::to_v8(scope, client).context("serializing callback client")?;
            vec![name, arg, v8::Global::new(scope, client)]
        };
        let value = self
            .runtime
            .call_with_args(&self.bridge, &args)
            .now_or_never()
            .context("SDK bridge unexpectedly returned a promise")?
            .map_err(|error| anyhow!("{error}"))?;
        deno_core::scope!(scope, self.runtime);
        let value = v8::Local::new(scope, value);
        serde_v8::from_v8(scope, value).context("reading Deno callback result")
    }

    fn needs_render(&mut self) -> Result<bool> {
        self.call("status", Value::Null, None)?
            .as_bool()
            .context("invalid SDK render status")
    }
}

#[cfg(test)]
mod sdk_tests {
    use super::*;

    #[derive(Default)]
    struct TestHost {
        permissions: permissions::Permissions,
        completions: Vec<permissions::Completion>,
        commands: Vec<(Value, u32)>,
        logs: Vec<String>,
        fail_permission_client: Option<u32>,
        fail_command: Option<&'static str>,
        session_list: Value,
        pane_info: Value,
        replies: BTreeMap<String, Value>,
    }

    #[op2]
    #[serde]
    fn op_yinz_command<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        state: &mut OpState,
        command: v8::Local<'s, v8::Value>,
        #[smi] client: u32,
        #[string] completion: Option<String>,
    ) -> std::result::Result<serde_json::Value, JsErrorBox> {
        let command = transport::from_v8(scope, command)
            .map_err(|error| JsErrorBox::generic(format!("{error:#}")))?;
        let host = state.borrow_mut::<TestHost>();
        if let Some(values) = command.get("RequestPluginPermissions") {
            let permissions = serde_json::from_value(values.clone())
                .map_err(|error| JsErrorBox::generic(error.to_string()))?;
            let id = completion.expect("SDK permission completion ID");
            host.permissions
                .enqueue(permissions::Request::new(
                    1,
                    ClientId::try_from(client).unwrap(),
                    permissions,
                    id.clone(),
                ))
                .map_err(|error| JsErrorBox::generic(error.to_string()))?;
            host.dispatch_permissions(Some(&id))?;
            return Ok(json!({"json": null}));
        }
        host.commands.push((command.clone(), client));
        if let Some(name) = host.fail_command {
            if command.get(name).is_some() || command == json!(name) {
                return Err(JsErrorBox::generic(format!("{name} dispatch failed")));
            }
        }
        let name = command.as_str().or_else(|| {
            command
                .as_object()
                .and_then(|c| c.keys().next().map(String::as_str))
        });
        if let Some(reply) = name.and_then(|name| host.replies.get(name)) {
            return Ok(if name == Some("FsRead") {
                json!({"bytes": reply})
            } else {
                json!({"json": reply})
            });
        }
        if command == json!("GetPluginIds") {
            Ok(json!({ "json": { "client_id": client } }))
        } else if command == json!("GetSessionList") {
            Ok(json!({ "json": host.session_list }))
        } else if command.get("DataWatch").is_some() {
            Ok(json!({"json":{"watch_id":1,"rows":{"columns":[],"rows":[]}}}))
        } else if command.get("GetPaneInfo").is_some() {
            Ok(json!({"json":host.pane_info}))
        } else if command.get("OpenCommandPane").is_some()
            || command.get("OpenCommandPaneFloating").is_some()
            || command.get("OpenCommandPaneBackground").is_some()
        {
            Ok(json!({"json":{"Terminal":7}}))
        } else if command.get("SpawnAgent").is_some() {
            Ok(json!({"json":{"pane_id":{"Terminal":7},"branch":null,"cwd":null}}))
        } else {
            Ok(json!({ "json": null }))
        }
    }

    impl TestHost {
        fn dispatch_permissions(
            &mut self,
            immediate: Option<&str>,
        ) -> std::result::Result<(), JsErrorBox> {
            while let Some(request) = self.permissions.next() {
                self.commands.push((
                    json!({"RequestPluginPermissions": request.permissions}),
                    u32::from(request.client.get()),
                ));
                if self.fail_permission_client != Some(u32::from(request.client.get())) {
                    break;
                }
                let completion = self
                    .permissions
                    .finish(Err("permission dispatch failed".into()))
                    .unwrap();
                if immediate == Some(request.id.as_str()) {
                    return Err(JsErrorBox::generic("permission dispatch failed"));
                }
                self.completions.push(completion);
            }
            Ok(())
        }
    }

    #[op2]
    #[serde]
    fn op_yinz_test_permission_answer(
        state: &mut OpState,
        #[serde] status: PermissionStatus,
    ) -> serde_json::Value {
        let host = state.borrow_mut::<TestHost>();
        let requested = host.permissions.requested().unwrap();
        if host.permissions.answered(&requested, status) {
            let completion = host.permissions.take_answer().unwrap();
            host.completions.push(completion);
            host.dispatch_permissions(None).unwrap();
        }
        json!(host
            .completions
            .drain(..)
            .map(|completion| {
                json!([
                    completion.request.id,
                    completion.result,
                    completion.request.client
                ])
            })
            .collect::<Vec<_>>())
    }

    #[op2(fast)]
    fn op_yinz_log(state: &mut OpState, #[string] line: &str) {
        state.borrow_mut::<TestHost>().logs.push(line.to_owned());
    }

    deno_core::extension!(
        sdk_test_host,
        ops = [
            op_yinz_command,
            op_yinz_test_permission_answer,
            op_yinz_validate,
            op_yinz_encode,
            op_yinz_log,
            op_yinz_worker_started,
            op_yinz_ui_create,
            op_yinz_ui_call,
            op_yinz_ui_pure,
            op_yinz_image,
            op_yinz_uuid
        ],
        state = |state| state.put(VecDeque::<background_worker::PostReservation>::new())
    );

    fn sdk() -> JsRuntime {
        let mut runtime = limits::Limits::new("sdk-test".into())
            .runtime(RuntimeOptions {
                extensions: vec![sdk_test_host::init()],
                ..Default::default()
            })
            .unwrap();
        runtime.op_state().borrow_mut().put(TestHost::default());
        run(&mut runtime, &format!("globalThis.answerPermissions = ((answer) => status => {{ for (const [id, result, client] of answer(status)) bridge('permissionResult', [id, result], client); }})(Deno.core.ops.op_yinz_test_permission_answer); globalThis.bridge = {SDK}; globalThis.bind = (plugin) => bridge('bind', {{default: plugin}}); globalThis.check = (ok) => {{ if (!ok) throw new Error('SDK assertion failed'); }};"));
        run(
            &mut runtime,
            &format!(
                "bridge('initialize', {{instanceId: '{}', identity: {{}}}})",
                uuid::Uuid::new_v4()
            ),
        );
        runtime
    }

    fn run(runtime: &mut JsRuntime, source: &str) {
        runtime
            .execute_script("sdk-test.js", source.to_owned())
            .unwrap_or_else(|error| panic!("{error}\nScript:\n{source}"));
        get_tokio_runtime()
            .block_on(runtime.run_event_loop(Default::default()))
            .unwrap();
    }

    fn permission_requests(runtime: &mut JsRuntime) -> Value {
        let state = runtime.op_state();
        let state = state.borrow();
        json!(state
            .borrow::<TestHost>()
            .commands
            .iter()
            .filter_map(|(command, client)| command
                .get("RequestPluginPermissions")
                .map(|permissions| (permissions, client)))
            .collect::<Vec<_>>())
    }

    fn capture_commands(runtime: &mut JsRuntime) {
        let commands = json!(runtime.op_state().borrow().borrow::<TestHost>().commands);
        run(runtime, &format!("globalThis.sent = {commands}"));
    }

    #[test]
    fn command_output_callbacks_correlate_clients_and_release_listeners() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
          globalThis.chunks=[]; globalThis.results=[]; globalThis.errors=[]; globalThis.operations=[]; globalThis.raw=[];
          bind({load({tag}) {
            operations.push(yinz.runCommand({command:['true'], context:{tag}, onOutput(output) {
              check(output.data instanceof Uint8Array && !('__yinz_async' in output.context));
              chunks.push([tag, yinz.getPluginIds().client_id, [...output.data], output.stderr]);
              if(tag==='throws') throw Error('callback failed');
              return true;
            }}));
            operations.at(-1).then(r=>results.push(r),e=>errors.push(e.message));
          }});
          yinz.on('RunCommandOutput', payload=>{raw.push(payload);});
          for(const [i,tag] of ['one','two','cancel','throws'].entries()) bridge('load',{tag},i+1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
          globalThis.commands=sent.filter(([c])=>c.RunCommand).map(([c])=>c.RunCommand);
          check(commands.length===4 && commands.every(c=>c.length===6 && c[5]===true));
          globalThis.emit=(index,stderr=false)=>bridge('update',{RunCommandOutput:{data:[0,128,255],stderr,context:index===null?{}:commands[index][3]}},9);
          globalThis.finish=index=>bridge('update',{RunCommandResult:[7,[1],[2],commands[index][3]]},9);
          emit(null); emit(1,true); emit(0); operations[2].cancel(); emit(2); emit(3); emit(3);
          check(chunks.length===3 && chunks[0][0]==='two' && chunks[0][1]===2 && chunks[0][3]);
          check(chunks[1][0]==='one' && chunks[1][1]===1 && chunks[1][2].join()==='0,128,255');
          check(raw.length===6 && raw[1].context.__yinz_async===commands[1][3].__yinz_async);
          bind({load(){yinz.unsubscribe(['RunCommandOutput','RunCommandResult']);}}); bridge('load',{},1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
          check(sent.at(-1)[0].Unsubscribe.length===0);
          finish(1); finish(0); finish(2); finish(3); emit(0); emit(1);
          bridge('load',{},1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
          check(chunks.length===3 && results.length===2 && errors.join()==='operation cancelled,callback failed');
          check(results[0].exitCode===7 && results[0].stdout[0]===1 && results[0].stderr[0]===2);
          check(results[0].context.tag==='two' && !('__yinz_async' in results[0].context));
          check(sent.at(-1)[0].Unsubscribe.join()==='RunCommandOutput,RunCommandResult');
        "#,
        );
    }

    #[test]
    fn raw_command_output_accepts_legacy_and_explicit_streaming_forms() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
          bind({load() {
            const legacy=Object.freeze([['true'],{},'/host',{},null]);
            yinz.command('RunCommand',legacy); yinz.command({RunCommand:legacy});
            check(legacy.length===5);
            let rejected=false;
            try {yinz.command({RunCommand:legacy, extra:NaN});} catch {rejected=true;}
            check(rejected);
            yinz.command('RunCommand',[...legacy,true]); yinz.command({RunCommand:[...legacy,false]});
            const buffered=yinz.runCommand({command:['true']}); buffered.catch(()=>{}); buffered.cancel();
          }}); bridge('load',{},1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
          const commands=sent.filter(([c])=>c.RunCommand).map(([c])=>c.RunCommand);
          check(commands.length===5 && commands.every(c=>c.length===6));
          check(commands.map(c=>c[5]).join()===',,true,false,');
        "#,
        );
    }

    #[test]
    fn async_requests_correlate_out_of_order_replies_and_preserve_wire_payloads() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.results = []; globalThis.operations = []; globalThis.raw = [];
            globalThis.context = Object.freeze({tag: 'caller'});
            bind({ load({kind, streamId}) {
                yinz.on(kind === 'command' ? 'RunCommandResult' : 'WebRequestResult', payload => { raw.push(payload); });
                const promise = kind === 'command'
                    ? yinz.runCommand({command: ['echo', 'hello'], context})
                    : yinz.webRequest({url: 'https://example.com', body: 'hé🙂', context, streamId});
                check(promise instanceof Promise && typeof promise.cancel === 'function');
                operations.push(promise);
                const chained = promise.then(result => { results.push([yinz.getPluginIds().client_id, result]); });
                check(chained.cancel === undefined);
            } });
            bridge('load', {kind: 'command'}, 1);
            bridge('load', {kind: 'command'}, 2);
            bridge('load', {kind: 'web'}, 3);
            bridge('load', {kind: 'web', streamId: 'errors'}, 4);
            bridge('update', {RunCommandResult: [0, [], [], {tag: 'unrelated'}]}, 9);
            bridge('update', {WebRequestResult: [200, {}, [], {}]}, 9);
            check(results.length === 0 && !Object.hasOwn(context, '__yinz_async'));
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
            const commands = sent.filter(([cmd]) => cmd.RunCommand).map(([cmd]) => cmd.RunCommand);
            const requests = sent.filter(([cmd]) => cmd.WebRequest).map(([cmd]) => cmd.WebRequest);
            check(commands.length === 2 && requests.length === 2);
            check(JSON.stringify(commands[0].slice(0, 3)) === '[[' + '"echo","hello"],{},"/host"]');
            check(commands[0][4] === null);
            check(requests[1][5] === 'errors');
            check(requests[0][1] === 'Get' && requests[0][5] === null && requests[0][6] === null);
            check(yinz.decodeUtf8(new Uint8Array(requests[0][3])) === 'hé🙂');
            const contexts = [...commands.map(args => args[3]), ...requests.map(args => args[4])];
            check(new Set(contexts.map(ctx => ctx.__yinz_async)).size === 4);
            bridge('update', {WebRequestResult: [503, {server: 'test'}, [0, 255, 128], contexts[3]]}, 9);
            bridge('update', {RunCommandResult: [7, [104, 105], [255], contexts[1]]}, 9);
            bridge('update', {RunCommandResult: [null, [], [], contexts[0]]}, 9);
            bridge('update', {WebRequestResult: [200, {}, requests[0][3], contexts[2]]}, 9);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(results.map(([id]) => id).join() === '4,2,1,3');
            const [web, cmd, killed, unicode] = results.map(([,result]) => result);
            check(web.status === 503 && web.body instanceof Uint8Array && web.body.join() === '0,255,128');
            check(cmd.exitCode === 7 && yinz.decodeUtf8(cmd.stdout) === 'hi' && cmd.stderr[0] === 255);
            check(killed.exitCode === null && yinz.decodeUtf8(unicode.body) === 'hé🙂');
            for (const [,result] of results) check(JSON.stringify(result.context) === '{"tag":"caller"}');
            check(raw.length === 12 && raw.slice(4).every(payload => payload[3].__yinz_async));
            cmd.stdout[0] = 0; web.headers.server = 'edited';
            check(raw[4][1].server === 'test' && raw[6][1][0] === 104);
            for (const operation of operations) { operation.cancel(); operation.cancel(); }
            check(results.length === 4);
        "#,
        );
    }

    #[test]
    fn streaming_requests_correlate_responses_and_cancel_only_the_wait() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.results = []; globalThis.errors = []; globalThis.chunks = [];
            globalThis.requests = [];
            bind({load() {
                yinz.subscribe(['WebRequestStreamChunk']);
                yinz.on('WebRequestStreamChunk', event => {
                    if (event.stream_id === 'cancelled') chunks.push(event);
                });
                for (const streamId of ['first', 'second', 'cancelled']) {
                    const request = yinz.webRequest({url: 'https://example.com', streamId, context: {streamId}});
                    request.then(result => { results.push(result); }, error => { errors.push(error.name); });
                    requests.push(request);
                }
                requests[2].cancel(); requests[2].cancel();
            }}); bridge('load', {}, 1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
            const wires = sent.filter(([cmd]) => cmd.WebRequest).map(([cmd]) => cmd.WebRequest);
            check(wires.map(args => args[5]).join() === 'first,second,cancelled');
            check(!sent.some(([cmd]) => cmd.CancelWebRequest));
            bridge('update', {WebRequestResult: [200, {}, [], {}]}, 1);
            for (const i of [2, 1, 0]) {
                bridge('update', {WebRequestResult: [200, {}, [], wires[i][4]]}, 1);
            }
            bridge('update', {WebRequestStreamChunk: {stream_id: 'other', chunk: [1], done: false, error: null}}, 1);
            bridge('update', {WebRequestStreamChunk: {stream_id: 'cancelled', chunk: [0, 255], done: false, error: null}}, 1);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(results.map(result => result.context.streamId).join() === 'second,first');
            check(results.every(result => result.status === 200 && result.body instanceof Uint8Array && !result.body.length));
            check(errors.join() === 'AbortError' && chunks.length === 1 && chunks[0].chunk.join() === '0,255');
            for (const request of requests) request.cancel();
            bind({load() { yinz.command('CancelWebRequest', 'cancelled'); }}); bridge('load', {}, 1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            "check(sent.at(-1)[0].CancelWebRequest === 'cancelled')",
        );
    }

    #[test]
    fn cancellation_removes_waiters_and_guards_all_unsubscribe_entrypoints() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.errors = []; globalThis.matched = 0; globalThis.seen = 0;
            globalThis.operations = [];
            bind({ load() {
                const wait = yinz.waitForEvent('Key', () => { matched++; return true; });
                const second = yinz.waitForEvent('Key');
                const pipe = yinz.waitForPipe('reply', () => { matched++; return true; });
                operations = [wait, second, pipe, yinz.sleep(0),
                    yinz.runCommand({command: ['true']}), yinz.webRequest({url: 'https://example.com'})];
                for (const promise of operations) promise.catch(error => { errors.push(error.name); });
                yinz.on('RunCommandResult', () => { seen++; });
                const events = Object.freeze(['Key', 'Timer', 'RunCommandResult', 'WebRequestResult', 'Visible']);
                yinz.unsubscribe(events);
                yinz.command('Unsubscribe', events);
                yinz.command({Unsubscribe: events});
                wait.cancel();
                yinz.unsubscribe(['Key']); // The second waiter still needs Key.
                for (const promise of operations) { promise.cancel(); promise.cancel(); }
                yinz.command({Unsubscribe: events});
            } });
            bridge('load', {}, 1);
            bridge('update', {Key: 'late'}, 2);
            bridge('pipe', {name: 'reply'}, 2);
            bridge('update', {Timer: 1}, 2);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
            const unsubscribes = sent.filter(([cmd]) => cmd.Unsubscribe).map(([cmd]) => cmd.Unsubscribe);
            check(JSON.stringify(unsubscribes) === '[["Visible"],["Visible"],["Visible"],[],["Key","Timer","RunCommandResult","WebRequestResult","Visible"]]');
            const command = sent.find(([cmd]) => cmd.RunCommand)[0].RunCommand;
            const web = sent.find(([cmd]) => cmd.WebRequest)[0].WebRequest;
            bridge('update', {RunCommandResult: [0, [], [], command[3]]}, 2);
            bridge('update', {WebRequestResult: [200, {}, [], web[4]]}, 2);
            check(errors.length === 6 && errors.every(name => name === 'AbortError'));
            check(matched === 0 && seen === 1);
        "#,
        );
    }

    #[test]
    fn pipe_waits_snapshot_before_callback_and_reject_predicate_errors() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.calls = [];
            bind({
                load() {
                    yinz.waitForPipe('reply', msg => msg.payload === 'a').then(msg => { calls.push(`load:${msg.payload}`); });
                    yinz.waitForPipe('reply', () => { throw Error('predicate'); }).catch(error => { calls.push(error.message); });
                },
                async pipe(msg) {
                    calls.push(`pipe:${msg.payload}`);
                    if (msg.payload === 'a') {
                        const next = await yinz.waitForPipe('reply');
                        calls.push(`next:${next.payload}:${yinz.getPluginIds().client_id}`);
                    }
                }
            });
            bridge('load', {}, 1);
            bridge('pipe', {name: 'other', payload: 'ignored'}, 9);
            bridge('pipe', {name: 'reply', payload: 'a'}, 2);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(calls.join() === 'pipe:ignored,pipe:a,load:a,predicate');
            bridge('pipe', {name: 'reply', payload: 'b'}, 3);
        "#,
        );
        run(
            &mut runtime,
            "check(calls.join() === 'pipe:ignored,pipe:a,load:a,predicate,pipe:b,next:b:2')",
        );
    }

    #[test]
    fn dispatch_failures_remove_async_listeners_and_timers() {
        for command in ["Subscribe", "RunCommand", "WebRequest", "SetTimeout"] {
            let mut runtime = sdk();
            runtime
                .op_state()
                .borrow_mut()
                .borrow_mut::<TestHost>()
                .fail_command = Some(command);
            run(
                &mut runtime,
                r#"
                globalThis.errors = []; globalThis.operations = [];
                bind({ load() {
                    operations = [yinz.runCommand({command: ['true'], onOutput() {throw Error('failed dispatch emitted output');}}), yinz.webRequest({url: 'https://example.com'}), yinz.sleep(0)];
                    for (const promise of operations) promise.catch(error => { errors.push(error.message); });
                } });
                bridge('load', {}, 1);
            "#,
            );
            run(&mut runtime, &format!("check(errors.length === {} && errors.every(error => error === '{command} dispatch failed'))", if command == "Subscribe" { 3 } else { 1 }));
            run(
                &mut runtime,
                r#"
                bind({load() {
                    yinz.unsubscribe(['RunCommandOutput', 'RunCommandResult', 'WebRequestResult', 'Timer']);
                    for (const promise of operations) promise.cancel();
                    yinz.unsubscribe(['RunCommandOutput', 'RunCommandResult', 'WebRequestResult', 'Timer']);
                }});
                bridge('load', {}, 1);
            "#,
            );
            capture_commands(&mut runtime);
            let expected = match command {
                "Subscribe" => json!([
                    "RunCommandOutput",
                    "RunCommandResult",
                    "WebRequestResult",
                    "Timer"
                ]),
                "RunCommand" => json!(["RunCommandOutput", "RunCommandResult"]),
                "WebRequest" => json!(["WebRequestResult"]),
                "SetTimeout" => json!(["Timer"]),
                _ => unreachable!(),
            };
            run(
                &mut runtime,
                &format!(
                    r#"
                const unsubscribes = sent.filter(([cmd]) => cmd.Unsubscribe).map(([cmd]) => cmd.Unsubscribe);
                check(JSON.stringify(unsubscribes[0]) === JSON.stringify({expected}));
                check(unsubscribes[1].length === 4);
            "#
                ),
            );
        }
    }

    #[test]
    fn callback_timers_keep_clients_do_not_overlap_and_rearm_stale_events() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.now = 1000; Date.now = () => now;
            globalThis.calls = []; globalThis.release = null; globalThis.interval = null;
            bind({ load() {
                const cancelled = yinz.after(1, () => { throw Error('cancelled callback ran'); });
                cancelled.cancel(); cancelled.cancel();
                yinz.after(5, () => { calls.push(`after:${yinz.getPluginIds().client_id}`); return true; });
                interval = yinz.every(10, async () => {
                    calls.push(`start:${yinz.getPluginIds().client_id}`);
                    await new Promise(resolve => { release = resolve; });
                    calls.push(`end:${yinz.getPluginIds().client_id}`);
                    return true;
                });
            } });
            bridge('load', {}, 1);
            bridge('status');
            now = 1002; bridge('update', {Timer: 0}, 9);
            check(calls.length === 0);
            now = 1005; bridge('update', {Timer: 0}, 9);
            check(calls.join() === 'after:1' && bridge('status') === true);
            now = 1010; bridge('update', {Timer: 0}, 9);
            now = 1100; bridge('update', {Timer: 0}, 9);
            check(calls.join() === 'after:1,start:1');
            release();
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(calls.join() === 'after:1,start:1,end:1' && bridge('status') === true);
            now = 1109; bridge('update', {Timer: 0}, 9);
            check(calls.length === 3);
            now = 1110; bridge('update', {Timer: 0}, 9);
            check(calls.join() === 'after:1,start:1,end:1,start:1');
            interval.cancel(); interval.cancel(); release();
        "#,
        );
        run(
            &mut runtime,
            r#"
            now = 1200; bridge('update', {Timer: 0}, 9);
            check(calls.join() === 'after:1,start:1,end:1,start:1,end:1');
            bind({load() { yinz.unsubscribe(['Timer']); }}); bridge('load', {}, 1);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
            const scheduled = sent.filter(([cmd]) => cmd.SetTimeout);
            check(scheduled.length === 5 && scheduled.every(([,client]) => client === 1));
            check(JSON.stringify(sent.at(-1)[0]) === '{"Unsubscribe":["Timer"]}');
        "#,
        );
    }

    #[test]
    fn timer_callback_failures_use_lifecycle_error_reporting_and_release_subscriptions() {
        for callback in [
            "() => { throw Error('callback failed'); }",
            "async () => { throw Error('callback failed'); }",
        ] {
            let mut runtime = sdk();
            run(
                &mut runtime,
                &format!(
                    r#"
                bind({{load() {{ yinz.every(0, {callback}); }}}});
                bridge('load', {{}}, 1);
                bridge('update', {{Timer: 0}}, 9);
            "#
                ),
            );
            let error = runtime
                .execute_script("timer-error.js", "bridge('status')")
                .unwrap_err();
            assert!(error.to_string().contains("callback failed"));
            run(
                &mut runtime,
                r#"
                bind({load() { yinz.unsubscribe(['Timer']); }}); bridge('load', {}, 1);
                bridge('update', {Timer: 0}, 9);
            "#,
            );
            capture_commands(&mut runtime);
            run(
                &mut runtime,
                "check(JSON.stringify(sent.at(-1)[0]) === '{\"Unsubscribe\":[\"Timer\"]}')",
            );
        }
    }

    #[test]
    fn stale_timer_dispatch_failure_rejects_sleep_and_cleans_up() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.now = 1000; Date.now = () => now;
            globalThis.errors = [];
            bind({load() {
                yinz.sleep(100).catch(error => { errors.push(error.message); });
                yinz.sleep(200).catch(error => { errors.push(error.message); });
            }}); bridge('load', {}, 1);
        "#,
        );
        runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<TestHost>()
            .fail_command = Some("SetTimeout");
        run(&mut runtime, "bridge('update', {Timer: 0}, 9)");
        run(
            &mut runtime,
            r#"
            check(errors.join() === 'SetTimeout dispatch failed,SetTimeout dispatch failed');
            bind({load() { yinz.unsubscribe(['Timer']); }}); bridge('load', {}, 1);
            now = 1300; bridge('update', {Timer: 0}, 9);
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            "check(JSON.stringify(sent.at(-1)[0]) === '{\"Unsubscribe\":[\"Timer\"]}')",
        );
    }

    #[test]
    fn async_options_copy_binary_bodies_and_keep_permissions_subscribed() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.done = false;
            bind({load() {
                const bytes = new Uint8Array([42, 0, 255, 128, 42]);
                for (const verb of ['Post', 'Put', 'Delete', 'Patch']) {
                    const request = yinz.webRequest({url: 'https://example.com', verb,
                        headers: {'content-type': 'application/octet-stream'}, body: bytes.subarray(1, 4),
                        context: {verb}, proxy: 'socks5://localhost:1080'});
                    request.catch(() => {}); request.cancel();
                }
                bytes.fill(0);
                const command = yinz.runCommand({command: ['echo', 'hello'],
                    envVariables: {LANG: 'C'}, cwd: '/host/project', maxBytesReturned: 0});
                command.catch(() => {}); command.cancel();
                const permission = yinz.waitForPermissions(['RunCommands']);
                yinz.command({Unsubscribe: ['PermissionRequestResult']});
                permission.then(() => { done = true; });
            }}); bridge('load', {}, 1);
            answerPermissions('Granted');
        "#,
        );
        capture_commands(&mut runtime);
        run(
            &mut runtime,
            r#"
            check(done);
            for (const [cmd] of sent.filter(([cmd]) => cmd.WebRequest)) {
                check(cmd.WebRequest[3].join() === '0,255,128');
                check(cmd.WebRequest[2]['content-type'] === 'application/octet-stream');
                check(cmd.WebRequest[4].verb === cmd.WebRequest[1]);
                check(cmd.WebRequest[5] === null && cmd.WebRequest[6] === 'socks5://localhost:1080');
            }
            const command = sent.find(([cmd]) => cmd.RunCommand)[0].RunCommand;
            check(command[1].LANG === 'C' && command[2] === '/host/project' && command[4] === 0);
            check(JSON.stringify(sent.at(-1)[0]) === '{"Unsubscribe":[]}');
            bind({load() { yinz.unsubscribe(['PermissionRequestResult']); }}); bridge('load', {}, 1);
        "#,
        );
        capture_commands(&mut runtime);
        run(&mut runtime, "check(JSON.stringify(sent.at(-1)[0]) === '{\"Unsubscribe\":[\"PermissionRequestResult\"]}')");
    }

    #[test]
    fn defaults_options_and_utf8_validate_without_mutating_inputs() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            const defaults = Object.freeze({greeting: 'hello', fallback: 'default'});
            const config = Object.freeze({greeting: 'override', extra: 'supplied'});
            bind({defaults, load(values) {
                check(JSON.stringify(values) === '{"greeting":"override","fallback":"default","extra":"supplied"}');
                values.fallback = 'changed';
            }});
            bridge('load', config, 1);
            check(defaults.fallback === 'default' && config.fallback === undefined);
            for (const defaults of [null, [], 'bad', {value: 1}]) {
                let rejected = false;
                try { bind({defaults}); } catch (error) { rejected = error instanceof TypeError; }
                check(rejected);
            }
            check(yinz.decodeUtf8(yinz.encodeUtf8('hé\0🙂')) === 'hé\0🙂');
            check(yinz.decodeUtf8(new Uint8Array([0, 104, 105, 0]).subarray(1, 3)) === 'hi');
            check(yinz.decodeUtf8(new Uint8Array([255])) === '\uFFFD');
            globalThis.errors = [];
            const invalid = [
                () => yinz.runCommand({command: ['true'], onOutput: true}),
                () => yinz.runCommand(null), () => yinz.runCommand({command: []}),
                () => yinz.runCommand({command: ['', 'arg']}), () => yinz.runCommand({command: [3]}),
                () => yinz.runCommand({command: ['true'], envVariables: {A: 1}}),
                () => yinz.runCommand({command: ['true'], cwd: null}),
                () => yinz.runCommand({command: ['true'], maxBytesReturned: -1}),
                () => yinz.runCommand({command: ['true'], maxBytesReturned: 1.5}),
                () => yinz.runCommand({command: ['true'], context: {__yinz_async: 'collision'}}),
                () => yinz.webRequest({url: ''}), () => yinz.webRequest({url: 'x', verb: 'Head'}),
                () => yinz.webRequest({url: 'x', body: [1]}), () => yinz.webRequest({url: 'x', headers: []}),
                () => yinz.webRequest({url: 'x', proxy: 1}),
                ...[null, 1, true, [], {}].map(streamId => () => yinz.webRequest({url: 'x', streamId})),
                () => yinz.webRequest({url: 'x', context: {__yinz_async: 'collision'}}),
                () => yinz.waitForEvent('Key', null), () => yinz.waitForPipe('pipe', null),
                () => yinz.sleep(NaN), () => yinz.sleep(-1), () => yinz.sleep(2147483648),
            ];
            bind({load() {
                for (const call of invalid) {
                    const promise = call(); check(typeof promise.cancel === 'function');
                    promise.catch(error => { errors.push(error instanceof TypeError); });
                }
                for (const call of [() => yinz.after(-1, () => {}), () => yinz.every(1, null),
                    () => yinz.encodeUtf8(1), () => yinz.decodeUtf8([1])]) {
                    let rejected = false;
                    try { call(); } catch (error) { rejected = error instanceof TypeError; }
                    check(rejected);
                }
            }}); bridge('load', {}, 1);
            globalThis.expectedErrors = invalid.length;
        "#,
        );
        run(
            &mut runtime,
            "check(errors.length === expectedErrors && errors.every(Boolean))",
        );
        assert!(runtime
            .op_state()
            .borrow()
            .borrow::<TestHost>()
            .commands
            .is_empty());
    }

    #[test]
    fn permission_overflow_uses_the_sdk_error_path_and_recovers_after_delivery() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.rejected = false;
            bind({load() {
                for (let i = 0; i < 1024; i++) yinz.requestPermission(['RunCommands']);
                yinz.waitForPermissions(['ReadFiles']).catch(error => {
                    rejected = error.message.includes('1024 outstanding');
                });
            }});
            bridge('load', {}, 1);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(rejected);
            answerPermissions('Granted');
            bind({load() { yinz.requestPermission(['ReadFiles']); }});
            bridge('load', {}, 1);
        "#,
        );
        assert_eq!(
            runtime
                .op_state()
                .borrow()
                .borrow::<TestHost>()
                .permissions
                .requested(),
            Some(vec![yinz_types::data::PermissionType::RunCommands])
        );
    }

    #[test]
    fn permission_waits_serialize_and_resolve_only_their_own_answer() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.answers = [];
            bind({ async load({permission}) {
                await yinz.waitForPermissions([permission]);
                answers.push(yinz.getPluginIds().client_id);
            } });
            bridge('load', {permission: 'ReadApplicationState'}, 1);
            bridge('load', {permission: 'ChangeApplicationState'}, 2);
            bridge('update', {PermissionRequestResult: 'Granted'}, 9);
            bridge('update', {PermissionRequestResult: 'Denied'}, 9);
        "#,
        );
        assert_eq!(
            permission_requests(&mut runtime),
            json!([[["ReadApplicationState"], 1]])
        );
        run(&mut runtime, "check(answers.length === 0)");
        run(&mut runtime, "answerPermissions('Granted')");
        run(&mut runtime, "check(answers.join() === '1')");
        assert_eq!(
            permission_requests(&mut runtime),
            json!([
                [["ReadApplicationState"], 1],
                [["ReadApplicationState", "ChangeApplicationState"], 2]
            ])
        );
        run(&mut runtime, "answerPermissions('Granted')");
        run(&mut runtime, "check(answers.join() === '1,2')");
    }

    #[test]
    fn permission_denial_and_dispatch_failure_release_the_queue() {
        let mut runtime = sdk();
        runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<TestHost>()
            .fail_permission_client = Some(2);
        run(
            &mut runtime,
            r#"
            globalThis.answers = [];
            bind({ async load({permission}) {
                try {
                    await yinz.waitForPermissions([permission]);
                    answers.push(`granted:${yinz.getPluginIds().client_id}`);
                } catch (error) { answers.push(`${error.message}:${yinz.getPluginIds().client_id}`); }
            } });
            bridge('load', {permission: 'ReadApplicationState'}, 1);
            bridge('load', {permission: 'ChangeApplicationState'}, 2);
            bridge('load', {permission: 'OpenFiles'}, 3);
            answerPermissions('Denied');
        "#,
        );
        run(
            &mut runtime,
            "check(answers.join() === 'plugin permissions denied:1,permission dispatch failed:2')",
        );
        assert_eq!(
            permission_requests(&mut runtime),
            json!([
                [["ReadApplicationState"], 1],
                [["ChangeApplicationState"], 2],
                [["OpenFiles"], 3]
            ])
        );
        run(&mut runtime, "answerPermissions('Granted')");
        run(&mut runtime, "check(answers.join() === 'plugin permissions denied:1,permission dispatch failed:2,granted:3'); check(bridge('status') === true)");
    }

    #[test]
    fn mixed_permission_commands_copy_inputs_keep_grants_and_report_deferred_errors() {
        let mut runtime = sdk();
        runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<TestHost>()
            .fail_permission_client = Some(3);
        run(
            &mut runtime,
            r#"
            globalThis.done = false;
            bind({ load({kind, permission}) {
                const permissions = [permission];
                if (kind === 'helper') check(yinz.requestPermission(permissions) === null);
                if (kind === 'string') check(yinz.command('RequestPluginPermissions', permissions) === null);
                if (kind === 'object') check(yinz.command({RequestPluginPermissions: permissions}) === null);
                if (kind === 'wait') return yinz.waitForPermissions(permissions).then(() => {
                    check(yinz.getPluginIds().client_id === 5); done = true;
                });
                permissions[0] = 'WriteFiles';
            } });
            bridge('load', {kind: 'helper', permission: 'ReadApplicationState'}, 1);
            bridge('load', {kind: 'string', permission: 'ChangeApplicationState'}, 2);
            bridge('load', {kind: 'object', permission: 'RunCommands'}, 3);
            bridge('load', {kind: 'object', permission: 'OpenFiles'}, 4);
            bridge('load', {kind: 'wait', permission: 'WebAccess'}, 5);
            answerPermissions('Granted');
            check(!done);
            answerPermissions('Granted');
            let reported = false;
            try { bridge('status'); } catch (error) { reported = error.message.includes('permission dispatch failed'); }
            check(reported && !done);
            answerPermissions('Granted');
            check(!done);
        "#,
        );
        assert_eq!(
            permission_requests(&mut runtime),
            json!([
                [["ReadApplicationState"], 1],
                [["ReadApplicationState", "ChangeApplicationState"], 2],
                [
                    [
                        "ReadApplicationState",
                        "ChangeApplicationState",
                        "RunCommands"
                    ],
                    3
                ],
                [
                    [
                        "ReadApplicationState",
                        "ChangeApplicationState",
                        "OpenFiles"
                    ],
                    4
                ],
                [
                    [
                        "ReadApplicationState",
                        "ChangeApplicationState",
                        "OpenFiles",
                        "WebAccess"
                    ],
                    5
                ]
            ])
        );
        run(&mut runtime, "answerPermissions('Granted')");
        run(
            &mut runtime,
            "check(done); check(bridge('status') === true)",
        );
    }

    #[test]
    fn update_snapshots_listeners_and_payload_before_starting_new_event_waits() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.calls = [];
            let off;
            bind({
                async load() {
                    off = yinz.on('Key', key => { calls.push(`listener:${key}`); });
                    calls.push(`load:${await yinz.waitForEvent('Key')}`);
                },
                async update(event) {
                    if (event.Key !== 'a') return;
                    calls.push('update');
                    off();
                    delete event.Key;
                    event.Visible = true;
                    yinz.on('Key', key => { calls.push(`new:${key}`); });
                    const key = await yinz.waitForEvent('Key');
                    calls.push(`update:${key}:${yinz.getPluginIds().client_id}`);
                }
            });
            bridge('load', {}, 1);
            bridge('update', {Key: 'a'}, 2);
            check(calls.join() === 'update,listener:a');
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(calls.join() === 'update,listener:a,load:a');
            bridge('update', {Key: 'b'}, 3);
            check(calls.join() === 'update,listener:a,load:a,new:b');
        "#,
        );
        run(
            &mut runtime,
            "check(calls.join() === 'update,listener:a,load:a,new:b,update:b:2')",
        );
    }

    #[test]
    fn idle_cancellation_preserves_cleanup_but_active_client_cancellation_interrupts() {
        let mut runtime = sdk();
        let terminate = runtime.v8_isolate().thread_safe_handle();
        let active = Arc::new(Mutex::new(false));
        let cancel = AtomicBool::new(true);
        let client_cancel = AtomicBool::new(false);
        let tripped = AtomicBool::new(false);

        interrupt_active(&active, &cancel, &client_cancel, &tripped, &terminate);
        assert!(!tripped.load(Ordering::Acquire));
        assert!(ExecutionScope::enter(&active, &cancel, &client_cancel).is_err());
        // Shutdown clears the shared cancellation flag before an idle BeforeClose.
        cancel.store(false, Ordering::Release);
        {
            let _execution = ExecutionScope::enter(&active, &cancel, &client_cancel)
                .unwrap()
                .unwrap();
            run(&mut runtime, "check(true)");
        }
        assert!(!tripped.load(Ordering::Acquire));

        let execution = ExecutionScope::enter(&active, &cancel, &client_cancel)
            .unwrap()
            .unwrap();
        client_cancel.store(true, Ordering::Release);
        interrupt_active(&active, &cancel, &client_cancel, &tripped, &terminate);
        assert!(tripped.load(Ordering::Acquire));
        assert!(runtime.execute_script("cancelled.js", "1 + 1").is_err());
        drop(execution);
        assert!(ExecutionScope::enter(&active, &cancel, &client_cancel)
            .unwrap()
            .is_none());
    }

    #[test]
    fn parked_isolates_can_alternate_and_drop_in_creation_order() {
        fn parked(identity: u64) -> RuntimeInstance {
            let mut runtime = sdk();
            run(&mut runtime, &format!("globalThis.identity = {identity}"));
            let bridge = runtime.execute_script("bridge.js", "bridge").unwrap();
            let bridge = {
                deno_core::scope!(scope, runtime);
                let value = v8::Local::new(scope, bridge);
                let function = v8::Local::<v8::Function>::try_from(value).unwrap();
                v8::Global::new(scope, function)
            };
            let terminate = runtime.v8_isolate().thread_safe_handle();
            let mut instance = RuntimeInstance {
                limits: limits::Limits::new("sdk-test".into()),
                bridge,
                runtime,
                watcher: get_tokio_runtime().spawn(std::future::pending()),
                terminate,
                wake: Arc::new(TickWake {
                    senders: ThreadSenders::default(),
                    plugin_id: PluginId::new(1),
                    owner_client_id: ClientId::new(1),
                    generation: identity,
                    queued: AtomicBool::new(false),
                    stopped: AtomicBool::new(false),
                }),
                parked: false,
            };
            instance.park();
            instance
        }

        let mut older = parked(1);
        let mut newer = parked(2);
        for _ in 0..3 {
            run(&mut older.enter().runtime, "check(identity === 1)");
            run(&mut newer.enter().runtime, "check(identity === 2)");
        }
        // An error must park just like success before the next isolate is used.
        assert!(older
            .enter()
            .runtime
            .execute_script("error.js", "throw Error('expected')")
            .is_err());
        drop(older);
        run(&mut newer.enter().runtime, "check(identity === 2)");
        let mut replacement = parked(3);
        drop(newer);
        run(&mut replacement.enter().runtime, "check(identity === 3)");
    }

    #[test]
    fn async_render_sleep_has_one_flight_and_keeps_each_clients_frame() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.starts = []; globalThis.active = 0; globalThis.maximum = 0;
            bind({ async render(rows, cols) {
                const before = yinz.getPluginIds().client_id;
                starts.push(before); active++; maximum = Math.max(maximum, active);
                await yinz.sleep(0);
                active--;
                return `${before}:${yinz.getPluginIds().client_id}:${rows}x${cols}`;
            } });
            check(bridge('render', [10, 80], 1) === '');
            check(bridge('render', [20, 90], 2) === '');
            check(starts.join() === '1');
            bridge('update', {Timer: 0}, 9);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(bridge('status') === true);
            check(bridge('render', [10, 80], 1) === '1:1:10x80');
            check(bridge('render', [20, 90], 2) === '');
            check(starts.join() === '1,2');
            bridge('update', {Timer: 0}, 9);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(bridge('status') === true);
            check(bridge('render', [10, 80], 1) === '1:1:10x80');
            check(bridge('render', [20, 90], 2) === '2:2:20x90');
            check(bridge('status') === false);
            check(maximum === 1 && starts.join() === '1,2');
        "#,
        );
    }

    #[test]
    fn callback_starts_are_ordered_and_await_restores_the_originating_client() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.calls = [];
            bind({
                async load() {
                    calls.push('load');
                    await yinz.waitForEvent('Key');
                    calls.push(`load:${yinz.getPluginIds().client_id}`);
                },
                async update(event) {
                    if (!event.Key) return false;
                    calls.push('update');
                    await yinz.sleep(0);
                    calls.push(`update:${yinz.getPluginIds().client_id}`);
                    return true;
                }
            });
            bridge('load', {}, 1);
            bridge('update', {Key: 'a'}, 2);
            check(calls.join() === 'load,update');
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(calls.join() === 'load,update,load:1');
            check(bridge('status') === true);
            bridge('update', {Timer: 0}, 3);
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(calls.join() === 'load,update,load:1,update:2');
            check(bridge('status') === true);
            bridge('update', {Visible: true}, 4);
        "#,
        );
        run(&mut runtime, "check(bridge('status') === false)");
    }

    #[test]
    fn resize_and_state_changes_discard_pending_frames_but_keep_the_last_frame() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.release = null; globalThis.delayed = false;
            bind({ render(rows, columns) {
                if (!delayed) return `${rows}x${columns}`;
                return new Promise(resolve => { release = () => resolve(`${rows}x${columns}`); });
            }, update() { return true; } });
            check(bridge('render', [1, 2], 1) === '1x2');
            delayed = true;
            bridge('update', 'InputReceived', 1);
            check(bridge('render', [3, 4], 1) === '1x2');
            check(bridge('render', [5, 6], 1) === '1x2');
            release();
        "#,
        );
        run(
            &mut runtime,
            r#"
            check(bridge('render', [5, 6], 1) === '1x2');
            release();
        "#,
        );
        run(&mut runtime, "check(bridge('render', [5, 6], 1) === '5x6')");
    }

    #[test]
    fn module_initialization_has_no_commands_console_logs_and_lifecycle_rejection_is_observed() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            let refused = false;
            try { yinz.command('GetPluginIds'); } catch (_) { refused = true; }
            check(refused && typeof Deno === 'undefined');
            check(typeof globalThis.__bootstrap === 'undefined');
            // Core's console would write to the host's stdout; the familiar names go to the
            // plugin's log instead, and are available before `load`.
            console.log('from console', 1);
            console.error('from error', {toString() { return 'stringified'; }});
            yinz.log('from yinz', 2);
            check(Object.isFrozen(console));
            const before = console;
            try { globalThis.console = null; } catch (_) {}
            check(globalThis.console === before);
            for (const name of ['log', 'info', 'warn', 'error', 'debug', 'trace']) check(typeof console[name] === 'function');
            bind({ async load() { throw new Error('rejected lifecycle'); } });
            bridge('load', {}, 1);
        "#,
        );
        let error = runtime
            .execute_script("assert-rejection.js", "bridge('status')")
            .unwrap_err();
        assert!(error.to_string().contains("rejected lifecycle"));
        let logs = runtime
            .op_state()
            .borrow()
            .borrow::<TestHost>()
            .logs
            .clone();
        assert_eq!(
            logs,
            ["from console\t1", "from error\tstringified", "from yinz\t2"]
        );
    }

    #[test]
    fn load_merges_user_configuration_over_the_plugins_defaults() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            bind({ defaults: {greeting: 'hello', name: 'world'}, load(config) { globalThis.config = config; } });
            bridge('load', {name: 'yinz'}, 1);
            check(config.greeting === 'hello' && config.name === 'yinz' && Object.keys(config).length === 2);
        "#,
        );
    }

    #[test]
    fn handle_is_send_without_moving_v8() {
        fn assert_send<T: Send>() {}
        assert_send::<DenoPlugin>();
    }
    include!("unit/trace_sdk.rs");
    include!("unit/ui_sdk.rs");
    include!("unit/image_sdk.rs");
    include!("unit/rpc_sdk.rs");
    include!("unit/agent_sdk.rs");
    include!("unit/pane_sdk.rs");
    include!("unit/completion_sdk.rs");
    include!("unit/consumer_sdk.rs");
    include!("unit/host_sdk.rs");
    include!("unit/palette_sdk.rs");
    mod bar_parity {
        include!("unit/bar_parity.rs");
    }
    mod tab_bar {
        include!("unit/tab_bar.rs");
    }
    mod compact_bar {
        include!("unit/compact_bar.rs");
    }
    mod status_bar {
        include!("unit/status_bar.rs");
    }
}

#[cfg(test)]
#[path = "unit/builtins.rs"]
mod builtin_tests;
