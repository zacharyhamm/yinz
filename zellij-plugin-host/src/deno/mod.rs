//! A Send plugin handle with its V8 isolate confined to the pinned executor thread.
mod checker;
mod event;
mod loader;

pub use checker::{check_plugin, check_syntax};

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
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
use zellij_runtime::errors::prelude::*;
use zellij_types::data::{CommandType, Event, PermissionStatus, PipeMessage, PluginCommand};

use super::plugin_map::{PluginEnv, ResponseSink, CANCELLED_BY_HOST};
use super::response::{decode_response, DecodedResponse};
use super::zellij_exports::{check_command_permission, run_plugin_command};
use super::{PluginId, PluginInstruction};
use zellij_bus::global_async_runtime::get_tokio_runtime;
use zellij_bus::{ClientId, ThreadSenders};

const SDK: &str = include_str!("sdk.js");
pub(crate) const SDK_MODULE: &str = "export const zellij = globalThis.zellij; export const definePlugin = zellij.definePlugin; export default zellij;";

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static RUNTIMES: RefCell<HashMap<u64, RuntimeInstance>> = RefCell::new(HashMap::new());
}

struct HostState {
    env: Option<PluginEnv>,
}

#[op2]
#[serde]
fn op_zellij_command(
    state: &mut OpState,
    #[serde] value: serde_json::Value,
    #[smi] client: u32,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    dispatch(state, value, client).map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}

fn dispatch(state: &mut OpState, value: Value, client: u32) -> Result<Value> {
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
        serde_json::from_value(value).context("invalid zellij command or arguments")?;
    if let (PermissionStatus::Denied, Some(permission)) = check_command_permission(env, &command) {
        bail!("command requires the '{permission}' permission; wait for PermissionRequestResult before calling it");
    }
    let command_type = CommandType::from(&command);
    let client = ClientId::try_from(client).context("invalid callback client")?;
    let previous_client = std::mem::replace(&mut env.client_id, client);
    let previous_sink = std::mem::replace(&mut env.response_sink, ResponseSink::Capture(None));
    let result = run_plugin_command(env, command);
    let captured = env.response_sink.take();
    env.response_sink = previous_sink;
    env.client_id = previous_client;
    result.context("zellij command failed")?;
    match captured {
        None => Ok(json!({ "json": null })),
        Some(reply) => match serde_json::from_str::<Vec<u8>>(&reply) {
            Ok(bytes) => match decode_response(command_type, &bytes).transpose()? {
                Some(DecodedResponse::Json(value)) => Ok(json!({ "json": value })),
                Some(DecodedResponse::Bytes(bytes)) => Ok(json!({ "bytes": bytes })),
                None => Ok(json!({ "bytes": bytes })),
            },
            Err(_) => Ok(json!({ "json": reply })),
        },
    }
}

#[op2(fast)]
fn op_zellij_log(state: &OpState, #[string] line: &str) -> std::result::Result<(), JsErrorBox> {
    let env = state.borrow::<HostState>().env.as_ref().ok_or_else(|| {
        JsErrorBox::generic("logging is unavailable during module initialization")
    })?;
    let name = env
        .resolved_source_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    log::info!("[deno:{}] {}", name, line);
    Ok(())
}

deno_core::extension!(zellij_host, ops = [op_zellij_command, op_zellij_log]);

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
        let generation = NEXT_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| anyhow!("Deno plugin generation exhausted"))?;
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
        self.lifecycle("update", event::event_to_json(event)?)
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
        self.with_runtime(|runtime, _| {
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
        let _entered = get_tokio_runtime().enter();
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
            call(&mut runtime, env)
        })
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.wake.stopped.store(true, Ordering::Release);
        self.terminate.terminate_execution();
        self.env().clear_deno_watches(self.generation);
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
        let loader = Rc::new(loader::PluginModuleLoader::new(path, source)?);
        let mut runtime = JsRuntime::try_new(RuntimeOptions {
            module_loader: Some(loader.clone()),
            extensions: vec![zellij_host::init()],
            ..Default::default()
        })
        .context("creating Deno runtime")?;
        runtime.op_state().borrow_mut().put(HostState { env: None });
        let bridge = runtime
            .execute_script("zellij:sdk-bootstrap", SDK)
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
        // ponytail: one 100 Hz cancellation watcher per isolate; use a shared
        // thread-safe isolate-handle registry if plugin counts make polling costly.
        let watcher = get_tokio_runtime().spawn(async move {
            loop {
                if alive.stopped.load(Ordering::Acquire) {
                    break;
                }
                interrupt_active(&active, &cancel, &client_cancel, &tripped, &interrupt);
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
        };
        let module = get_tokio_runtime()
            .block_on(instance.runtime.load_main_es_module(loader.entrypoint()))
            .context("loading Deno plugin modules")?;
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
        instance.call_value("bind", namespace, None)?;
        instance.park();
        Ok(instance)
    }

    fn poll(&mut self) -> Result<()> {
        let waker = Waker::from(self.wake.clone());
        let mut context = TaskContext::from_waker(&waker);
        match self
            .runtime
            .poll_event_loop(&mut context, Default::default())
        {
            Poll::Ready(result) => result.context("polling Deno plugin event loop"),
            Poll::Pending => Ok(()),
        }
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
        commands: Vec<(Value, u32)>,
        fail_permission_client: Option<u32>,
    }

    #[op2]
    #[serde]
    fn op_zellij_command(
        state: &mut OpState,
        #[serde] command: serde_json::Value,
        #[smi] client: u32,
    ) -> std::result::Result<serde_json::Value, JsErrorBox> {
        let host = state.borrow_mut::<TestHost>();
        host.commands.push((command.clone(), client));
        if command.get("RequestPluginPermissions").is_some()
            && host.fail_permission_client == Some(client)
        {
            return Err(JsErrorBox::generic("permission dispatch failed"));
        }
        if command == json!("GetPluginIds") {
            Ok(json!({ "json": { "client_id": client } }))
        } else {
            Ok(json!({ "json": null }))
        }
    }

    #[op2(fast)]
    fn op_zellij_log(#[string] _line: &str) {}

    deno_core::extension!(sdk_test_host, ops = [op_zellij_command, op_zellij_log]);

    fn sdk() -> JsRuntime {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![sdk_test_host::init()],
            ..Default::default()
        });
        runtime.op_state().borrow_mut().put(TestHost::default());
        run(&mut runtime, &format!("globalThis.bridge = {SDK}; globalThis.bind = (plugin) => bridge('bind', {{default: plugin}}); globalThis.check = (ok) => {{ if (!ok) throw new Error('SDK assertion failed'); }};"));
        runtime
    }

    fn run(runtime: &mut JsRuntime, source: &str) {
        runtime
            .execute_script("sdk-test.js", source.to_owned())
            .unwrap();
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

    #[test]
    fn permission_waits_serialize_and_resolve_only_their_own_answer() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            globalThis.answers = [];
            bind({ async load({permission}) {
                await zellij.waitForPermissions([permission]);
                answers.push(zellij.getPluginIds().client_id);
            } });
            bridge('load', {permission: 'ReadApplicationState'}, 1);
            bridge('load', {permission: 'ChangeApplicationState'}, 2);
        "#,
        );
        assert_eq!(
            permission_requests(&mut runtime),
            json!([[["ReadApplicationState"], 1]])
        );
        run(
            &mut runtime,
            "bridge('update', {PermissionRequestResult: 'Granted'}, 9)",
        );
        run(&mut runtime, "check(answers.join() === '1')");
        assert_eq!(
            permission_requests(&mut runtime),
            json!([
                [["ReadApplicationState"], 1],
                [["ReadApplicationState", "ChangeApplicationState"], 2]
            ])
        );
        run(
            &mut runtime,
            "bridge('update', {PermissionRequestResult: 'Granted'}, 9)",
        );
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
                    await zellij.waitForPermissions([permission]);
                    answers.push(`granted:${zellij.getPluginIds().client_id}`);
                } catch (error) { answers.push(`${error.message}:${zellij.getPluginIds().client_id}`); }
            } });
            bridge('load', {permission: 'ReadApplicationState'}, 1);
            bridge('load', {permission: 'ChangeApplicationState'}, 2);
            bridge('load', {permission: 'OpenFiles'}, 3);
            bridge('update', {PermissionRequestResult: 'Denied'}, 9);
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
        run(
            &mut runtime,
            "bridge('update', {PermissionRequestResult: 'Granted'}, 9)",
        );
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
                if (kind === 'helper') check(zellij.requestPermission(permissions) === null);
                if (kind === 'string') check(zellij.command('RequestPluginPermissions', permissions) === null);
                if (kind === 'object') check(zellij.command({RequestPluginPermissions: permissions}) === null);
                if (kind === 'wait') return zellij.waitForPermissions(permissions).then(() => {
                    check(zellij.getPluginIds().client_id === 5); done = true;
                });
                permissions[0] = 'WriteFiles';
            } });
            bridge('load', {kind: 'helper', permission: 'ReadApplicationState'}, 1);
            bridge('load', {kind: 'string', permission: 'ChangeApplicationState'}, 2);
            bridge('load', {kind: 'object', permission: 'RunCommands'}, 3);
            bridge('load', {kind: 'object', permission: 'OpenFiles'}, 4);
            bridge('load', {kind: 'wait', permission: 'WebAccess'}, 5);
            bridge('update', {PermissionRequestResult: 'Granted'}, 9);
            check(!done);
            bridge('update', {PermissionRequestResult: 'Granted'}, 9);
            let reported = false;
            try { bridge('status'); } catch (error) { reported = error.message.includes('permission dispatch failed'); }
            check(reported && !done);
            bridge('update', {PermissionRequestResult: 'Granted'}, 9);
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
        run(
            &mut runtime,
            "bridge('update', {PermissionRequestResult: 'Granted'}, 9)",
        );
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
                    off = zellij.on('Key', key => { calls.push(`listener:${key}`); });
                    calls.push(`load:${await zellij.waitForEvent('Key')}`);
                },
                async update(event) {
                    if (event.Key !== 'a') return;
                    calls.push('update');
                    off();
                    delete event.Key;
                    event.Visible = true;
                    zellij.on('Key', key => { calls.push(`new:${key}`); });
                    const key = await zellij.waitForEvent('Key');
                    calls.push(`update:${key}:${zellij.getPluginIds().client_id}`);
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
                const before = zellij.getPluginIds().client_id;
                starts.push(before); active++; maximum = Math.max(maximum, active);
                await zellij.sleep(0);
                active--;
                return `${before}:${zellij.getPluginIds().client_id}:${rows}x${cols}`;
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
                    await zellij.waitForEvent('Key');
                    calls.push(`load:${zellij.getPluginIds().client_id}`);
                },
                async update(event) {
                    if (!event.Key) return false;
                    calls.push('update');
                    await zellij.sleep(0);
                    calls.push(`update:${zellij.getPluginIds().client_id}`);
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
    fn module_initialization_has_no_commands_and_lifecycle_rejection_is_observed() {
        let mut runtime = sdk();
        run(
            &mut runtime,
            r#"
            let refused = false;
            try { zellij.command('GetPluginIds'); } catch (_) { refused = true; }
            check(refused && typeof Deno === 'undefined');
            check(typeof globalThis.__bootstrap === 'undefined');
            check(typeof console === 'undefined');
            bind({ async load() { throw new Error('rejected lifecycle'); } });
            bridge('load', {}, 1);
        "#,
        );
        let error = runtime
            .execute_script("assert-rejection.js", "bridge('status')")
            .unwrap_err();
        assert!(error.to_string().contains("rejected lifecycle"));
    }

    #[test]
    fn handle_is_send_without_moving_v8() {
        fn assert_send<T: Send>() {}
        assert_send::<DenoPlugin>();
    }
}
