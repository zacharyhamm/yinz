//! Scheme cases and mocked plugin instances run inside the existing disposable test worker.
use super::*;
use crate::permissions::{Completion, Permissions, Request as PermissionRequest};
use std::sync::atomic::{AtomicI64, AtomicU64};
use std::sync::{Arc, Mutex};
use yinz_steel::Runtime;
use yinz_types::data::{PermissionStatus, PluginCommand};

pub(super) struct Sources {
    pub(super) entry: PathBuf,
    pub(super) root: PathBuf,
}
impl TestPaths for Sources {
    fn entry_path(&self) -> Result<PathBuf> {
        Ok(self.entry.clone())
    }
    fn resolve_test_path(&self, path: &str) -> Result<String> {
        anyhow::ensure!(
            path.starts_with("file:") || path.starts_with("./") || path.starts_with("../"),
            "test plugin path must be a file URL or explicit relative Scheme path"
        );
        anyhow::ensure!(!path.contains('\\'), "test path contains a backslash");
        let base =
            url::Url::from_file_path(&self.entry).map_err(|_| anyhow!("invalid test path"))?;
        let url = base.join(path).context("resolve test plugin path")?;
        anyhow::ensure!(
            url.scheme() == "file" && url.query().is_none() && url.fragment().is_none(),
            "test plugin must be a local path without query or fragment"
        );
        let path = url
            .to_file_path()
            .map_err(|_| anyhow!("invalid test file URL"))?;
        anyhow::ensure!(
            yinz_plugin_api::authoring::is_script_plugin(&path),
            "test target must be Scheme"
        );
        let root = self
            .root
            .canonicalize()
            .context("resolve Scheme test root")?;
        // Normalize only through the root so symlinks beneath it remain visible to the resolver.
        let mut prefix = PathBuf::new();
        let mut relative = path.components();
        loop {
            prefix.push(relative.next().context("test target escapes its root")?);
            if prefix.canonicalize().context("resolve test root alias")? == root {
                break;
            }
        }
        // Keep URL decoding here; the source resolver uses filesystem path characters.
        let confined_root = yinz_steel::modules::SourceRoot::file(&self.entry, Some(&root))?;
        confined_root.resolve_from_root(relative.as_path())?;
        let path = path.canonicalize().context("resolve Scheme test target")?;
        anyhow::ensure!(
            path.starts_with(&root) && path.is_file(),
            "test target escapes its root or is not a regular file"
        );
        anyhow::ensure!(
            yinz_plugin_api::authoring::is_script_plugin(&path),
            "test target must be Scheme"
        );
        Ok(url::Url::from_file_path(path)
            .map_err(|_| anyhow!("invalid canonical test path"))?
            .to_string())
    }
}

#[derive(Default)]
struct Capture {
    replies: BTreeMap<String, Value>,
    failures: BTreeMap<String, String>,
    commands: Vec<Value>,
    command_bytes: usize,
    logs: Vec<String>,
    renders: Vec<String>,
    output: String,
    journal: PathBuf,
    permissions: Permissions,
    completions: Vec<Completion>,
    events: Vec<Value>,
    callbacks: BTreeMap<String, yinz_steel::JsonCallback>,
}
impl Capture {
    fn log(&mut self, text: &str) -> Result<()> {
        append_bounded(&mut self.output, text);
        std::fs::write(&self.journal, &self.output).context("record test output")?;
        if self.logs.len() < 1000 {
            self.logs.push(text.chars().take(4096).collect());
        }
        Ok(())
    }
    fn command(shared: &Arc<Mutex<Self>>, command: Value) -> Result<Value> {
        let envelope = command.get("__yinz_runtime");
        let actual = envelope.map(|value| &value["command"]).unwrap_or(&command);
        if let Some(line) = actual.get("__yinz_test_log") {
            shared
                .lock()
                .unwrap()
                .log(line.as_str().context("test log must be text")?)?;
            return Ok(Value::Null);
        }
        let decoded: PluginCommand = yinz_steel::binary::decode_command(actual.clone())
            .context("invalid plugin command in test")?;
        if let Some(envelope) = envelope.filter(|value| value["permission_id"].is_string()) {
            let PluginCommand::RequestPluginPermissions(permissions) = decoded else {
                bail!("permission callback requires RequestPluginPermissions");
            };
            let id = envelope["permission_id"].as_str().unwrap().to_owned();
            let client = serde_json::from_value(envelope["client"].clone())
                .context("mock permission client")?;
            shared
                .lock()
                .unwrap()
                .permissions
                .enqueue(PermissionRequest::new(0, client, permissions, id.clone()))?;
            Self::dispatch_permissions(shared, Some(&id))?;
            return Ok(Value::Null);
        }
        let reply = Self::reply(
            shared,
            yinz_steel::binary::expand_command_bytes(actual.clone())
                .context("encode mocked command")?,
        )?;
        if let PluginCommand::Pick(yinz_types::pick::PickCommand::Cancel {
            request_id,
            timeout,
        }) = decoded
        {
            shared.lock().unwrap().events.push(json!({"PickResult": {"request_id":request_id, "result":{"Err":if timeout {"TimeoutError"} else {"AbortError"}}}}));
        }
        Ok(reply)
    }

    fn reply(shared: &Arc<Mutex<Self>>, command: Value) -> Result<Value> {
        let name = command
            .as_str()
            .or_else(|| command.as_object()?.keys().next().map(String::as_str))
            .context("invalid mocked command")?;
        let (callback, reply) = {
            let mut capture = shared.lock().unwrap();
            capture.capture(command.clone())?;
            if let Some(message) = capture.failures.get(name) {
                bail!("{message}");
            }
            let callback = capture.callbacks.get(name).cloned();
            let reply = capture.replies.get(name).cloned().or(match name {
                "Pick" | "Subscribe" | "Unsubscribe" | "SetTimeout" | "SetSelectable"
                | "ShowCursor" => Some(Value::Null),
                _ => None,
            });
            (callback, reply)
        };
        // A callback can inspect commands or change its next reply without holding this lock.
        if let Some(callback) = callback {
            callback.call(command.get(name).cloned().unwrap_or(Value::Null))
        } else {
            reply.with_context(|| {
                format!("unstubbed host operation {name}; supply replies or test-stub")
            })
        }
    }

    fn dispatch_permissions(shared: &Arc<Mutex<Self>>, immediate: Option<&str>) -> Result<()> {
        let mut failure = None;
        loop {
            let request = shared.lock().unwrap().permissions.next();
            let Some(request) = request else { break };
            let command =
                serde_json::to_value(PluginCommand::RequestPluginPermissions(request.permissions))?;
            match Self::reply(shared, command) {
                Err(error) => {
                    let mut capture = shared.lock().unwrap();
                    let completion = capture.permissions.finish(Err(format!("{error:#}")));
                    if immediate == Some(request.id.as_str()) {
                        failure = Some(error);
                    } else if let Some(completion) = completion {
                        capture.completions.push(completion);
                    }
                },
                _ => {
                    break;
                },
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn capture(&mut self, command: Value) -> Result<()> {
        anyhow::ensure!(
            self.commands.len() < 10000,
            "test command capture exceeds 10000 commands"
        );
        let bytes = serde_json::to_vec(&command)
            .context("measure captured command")?
            .len();
        anyhow::ensure!(
            bytes <= 1_048_576 - self.command_bytes,
            "test command capture exceeds 1 MiB"
        );
        self.command_bytes += bytes;
        self.commands.push(command);
        Ok(())
    }
}

struct State {
    sources: Sources,
    capture: Arc<Mutex<Capture>>,
    plugin: Option<Runtime>,
    session: Option<session::Controller>,
    started: Instant,
    pending: String,
    now: Arc<AtomicU64>,
    wall_clock: Arc<AtomicI64>,
}

thread_local! {
    // The session's terminal grid and both interpreters stay on this worker thread.
    static CALLBACK_CAPTURE: std::cell::RefCell<Option<Arc<Mutex<Capture>>>> = const { std::cell::RefCell::new(None) };
    static HOST: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

fn callback_control(operation: &str, argument: &Value) -> Result<Option<Value>> {
    if !matches!(
        operation,
        "commands" | "logs" | "log" | "stub" | "stub-error"
    ) {
        return Ok(None);
    }
    let result = CALLBACK_CAPTURE.with(|slot| {
        let slot = slot.borrow();
        let mut capture = slot
            .as_ref()
            .context("test worker is not active")?
            .lock()
            .unwrap();
        Ok(Some(match operation {
            "commands" => json!(capture.commands),
            "logs" => json!(capture.logs),
            "log" => {
                capture.log(&format!(
                    "{}\n",
                    argument.as_str().context("log must be text")?
                ))?;
                Value::Null
            },
            "stub" => {
                let replies: BTreeMap<String, Value> = serde_json::from_value(argument.clone())?;
                for name in replies.keys() {
                    capture.failures.remove(name);
                    capture.callbacks.remove(name);
                }
                capture.replies.extend(replies);
                Value::Null
            },
            "stub-error" => {
                capture.failures.insert(
                    argument["name"].as_str().context("stub name")?.into(),
                    argument["message"].as_str().context("stub error")?.into(),
                );
                Value::Null
            },
            _ => unreachable!(),
        }))
    });
    if operation == "log" {
        // A test may loop forever immediately after logging. Persist disposable-session
        // diagnostics now; nested mock logs are flushed when the outer harness call returns.
        HOST.with(|host| {
            if let Ok(state) = host.try_borrow()
                && let Some(state) = state.as_ref()
                && let Some(session) = &state.session
            {
                session.diagnostics(&state.capture.lock().unwrap().output, &state.pending);
            }
        });
    }
    result
}

struct ResetHost;
impl Drop for ResetHost {
    fn drop(&mut self) {
        HOST.with(|host| host.borrow_mut().take());
        CALLBACK_CAPTURE.with(|capture| capture.borrow_mut().take());
    }
}
impl State {
    fn plugin(&mut self) -> Result<&mut Runtime> {
        self.plugin
            .as_mut()
            .context("load a plugin with test-load-plugin first")
    }
    fn call(&mut self, operation: &str, argument: Value) -> Result<Value> {
        let result = match operation {
            "location" => Ok(json!(url::Url::from_file_path(&self.sources.entry)
                .map_err(|_| anyhow!("invalid test location"))?
                .to_string())),
            "resolve" => Ok(json!(self.sources.resolve_test_path(
                argument
                    .as_str()
                    .context("test-resolve requires a relative path")?
            )?)),
            "copy-plugin" => {
                let source = self.sources.resolve_test_path(
                    argument["source"]
                        .as_str()
                        .context("test-copy-plugin source must be text")?,
                )?;
                let path = url::Url::parse(&source)
                    .context("parse copy source URL")?
                    .to_file_path()
                    .map_err(|_| anyhow!("invalid copy source path"))?;
                let mut sources =
                    yinz_steel::modules::SourceRoot::file(&path, Some(&self.sources.root))?;
                let location = self
                    .session
                    .as_mut()
                    .context("test-copy-plugin requires an integration test")?
                    .copy_plugin(
                        &mut sources,
                        argument["destination"]
                            .as_str()
                            .context("test-copy-plugin destination must be text")?,
                    )?;
                Ok(json!(location))
            },
            "load" => {
                anyhow::ensure!(self.plugin.is_none(), "one plugin harness per test");
                let url = self.sources.resolve_test_path(
                    argument["path"]
                        .as_str()
                        .context("plugin path must be text")?,
                )?;
                let path = url::Url::parse(&url)
                    .context("parse resolved plugin URL")?
                    .to_file_path()
                    .map_err(|_| anyhow!("invalid plugin path"))?;
                let replies = argument["options"]
                    .get("replies")
                    .cloned()
                    .unwrap_or(json!({}));
                self.capture.lock().unwrap().replies.extend(
                    serde_json::from_value::<BTreeMap<String, Value>>(replies)
                        .context("mock replies must be an object")?,
                );
                let capture = self.capture.clone();
                let clock = self.now.clone();
                let wall_clock = self.wall_clock.clone();
                let mut plugin = Runtime::new_with_clocks(
                    move |command| Capture::command(&capture, command),
                    move || clock.load(Ordering::Relaxed),
                    move || wall_clock.load(Ordering::Relaxed),
                )?;
                set_test_identity(&mut plugin)?;
                plugin.eval(r#"
                    (define (yinz-log value) (yinz-command (hash "__yinz_test_log" (string-append (if (string? value) value (to-string value)) "\n"))))
                "#)?;
                let mut root =
                    yinz_steel::modules::SourceRoot::file(&path, Some(&self.sources.root))?;
                let source = root.read_entry()?;
                let evaluated = plugin.eval_with_source_root(&source, &mut root)?;
                if !evaluated.output.is_empty() {
                    self.capture.lock().unwrap().log(&evaluated.output)?;
                }
                plugin.invoke("yinz-async-enter", vec![json!(1)])?;
                plugin.load(
                    argument["options"]
                        .get("configuration")
                        .cloned()
                        .unwrap_or(json!({})),
                )?;
                self.plugin = Some(plugin);
                Ok(Value::Null)
            },
            "stub" => {
                let replies = serde_json::from_value::<BTreeMap<String, Value>>(argument)
                    .context("mock replies must be an object")?;
                let mut capture = self.capture.lock().unwrap();
                for name in replies.keys() {
                    capture.failures.remove(name);
                    capture.callbacks.remove(name);
                }
                capture.replies.extend(replies);
                Ok(Value::Null)
            },
            "stub-error" => {
                let name = argument["name"]
                    .as_str()
                    .context("stub error operation name")?;
                let message = argument["message"].as_str().context("stub error message")?;
                self.capture
                    .lock()
                    .unwrap()
                    .failures
                    .insert(name.into(), message.into());
                Ok(Value::Null)
            },
            "commands" => Ok(json!(self.capture.lock().unwrap().commands)),
            "logs" => Ok(json!(self.capture.lock().unwrap().logs)),
            "renders" => Ok(json!(self.capture.lock().unwrap().renders)),
            "event" => {
                if let Some(status) = argument.get("PermissionRequestResult") {
                    let status: PermissionStatus = serde_json::from_value(status.clone())?;
                    let mut capture = self.capture.lock().unwrap();
                    if let Some(requested) = capture.permissions.requested() {
                        capture.permissions.answered(&requested, status);
                    }
                }
                self.plugin()?.invoke("yinz-async-enter", vec![json!(1)])?;
                let updated = self.plugin()?.update(argument.clone());
                {
                    let mut capture = self.capture.lock().unwrap();
                    if let Some(completion) = capture.permissions.take_answer() {
                        capture.completions.push(completion);
                    }
                }
                Capture::dispatch_permissions(&self.capture, None)?;
                updated.map(Value::Bool)
            },
            "pipe" => {
                let mut message = json!({"source":{"Plugin":2}, "args":{}, "payload":null, "is_private":true, "name":""});
                message.as_object_mut().unwrap().extend(
                    argument
                        .as_object()
                        .context("pipe must be an object")?
                        .clone(),
                );
                let message = serde_json::from_value::<yinz_types::data::PipeMessage>(message)
                    .context("invalid pipe message")?;
                self.plugin()?.invoke("yinz-async-enter", vec![json!(1)])?;
                self.plugin()?
                    .pipe(serde_json::to_value(message).context("encode pipe")?)
                    .map(Value::Bool)
            },
            "palette" => {
                let client = argument["client"]
                    .as_u64()
                    .context("palette requires a client ID")?;
                anyhow::ensure!(u32::try_from(client).is_ok(), "invalid palette client ID");
                let message = argument["message"].clone();
                if let Some(context) = message.get("context") {
                    anyhow::ensure!(
                        context["clientId"] == client,
                        "palette context clientId must match client ID"
                    );
                }
                self.plugin()?
                    .invoke("yinz-async-enter", vec![json!(client)])?;
                self.plugin()?.palette(message).map(Value::Bool)
            },
            "step-wall-clock" => {
                let milliseconds = argument
                    .as_i64()
                    .context("wall clock step requires signed integer milliseconds")?;
                let wall_clock = self
                    .wall_clock
                    .load(Ordering::Relaxed)
                    .checked_add(milliseconds)
                    .context("test wall clock overflow")?;
                self.wall_clock.store(wall_clock, Ordering::Relaxed);
                Ok(Value::Null)
            },
            "advance" => {
                let milliseconds = argument
                    .as_u64()
                    .context("advance requires nonnegative integer milliseconds")?;
                let now = self
                    .now
                    .load(Ordering::Relaxed)
                    .checked_add(milliseconds)
                    .context("test clock overflow")?;
                let wall_clock = self
                    .wall_clock
                    .load(Ordering::Relaxed)
                    .checked_add(i64::try_from(milliseconds).context("test wall clock overflow")?)
                    .context("test wall clock overflow")?;
                self.now.store(now, Ordering::Relaxed);
                self.wall_clock.store(wall_clock, Ordering::Relaxed);
                self.plugin()?.invoke("yinz-async-enter", vec![json!(1)])?;
                self.plugin()?
                    .update(json!({"Timer": milliseconds as f64 / 1000.0}))
                    .map(Value::Bool)
            },
            "render" | "render-text" => {
                let rows = argument["rows"]
                    .as_u64()
                    .context("render rows must be an integer")? as usize;
                let columns = argument["columns"]
                    .as_u64()
                    .context("render columns must be an integer")?
                    as usize;
                anyhow::ensure!(
                    rows > 0 && rows <= 500 && columns > 0 && columns <= 1000,
                    "render dimensions exceed limits"
                );
                let raw = if operation == "render" {
                    self.plugin()?.invoke("yinz-async-enter", vec![json!(1)])?;
                    self.plugin()?.render(rows, columns)?;
                    self.plugin()?.invoke("yinz-async-poll", vec![])?;
                    self.plugin()?.render(rows, columns)?
                } else {
                    argument["text"]
                        .as_str()
                        .context("render must be text")?
                        .to_owned()
                };
                let text = session::render_text(&raw, rows, columns)?;
                let mut capture = self.capture.lock().unwrap();
                if capture.renders.len() < 1000 {
                    let mut bounded = String::new();
                    append_bounded(&mut bounded, &text);
                    capture.renders.push(bounded);
                }
                Ok(json!(text))
            },
            "log" => {
                self.capture.lock().unwrap().log(&format!(
                    "{}\n",
                    argument.as_str().context("log requires text")?
                ))?;
                Ok(Value::Null)
            },
            "clock" => Ok(json!(self.started.elapsed().as_millis() as u64)),
            "pending" => {
                self.pending = argument
                    .as_str()
                    .context("pending wait must be text")?
                    .chars()
                    .take(4096)
                    .collect();
                Ok(Value::Null)
            },
            op if op.starts_with("session:") => self
                .session
                .as_mut()
                .context("session controls require an integration test")?
                .call(&op[8..], argument, &self.sources),
            _ => bail!("unknown test operation {operation}"),
        };
        loop {
            let completions = std::mem::take(&mut self.capture.lock().unwrap().completions);
            let events = std::mem::take(&mut self.capture.lock().unwrap().events);
            if completions.is_empty() && events.is_empty() {
                break;
            }
            for event in events {
                self.plugin()?.invoke("yinz-async-enter", vec![json!(1)])?;
                self.plugin()?.update(event)?;
            }
            for completion in completions {
                self.plugin()?
                    .invoke("yinz-async-enter", vec![json!(completion.request.client)])?;
                self.plugin()?.invoke(
                    "yinz-async-permission",
                    vec![json!(completion.request.id), json!(completion.result)],
                )?;
                self.plugin()?.invoke("yinz-async-poll", vec![])?;
            }
        }
        if let Some(plugin) = &mut self.plugin {
            for output in [plugin.take_output()?, plugin.take_logs()?] {
                if !output.is_empty() {
                    self.capture.lock().unwrap().log(&output)?;
                }
            }
        }
        if let Some(session) = &self.session {
            session.diagnostics(&self.capture.lock().unwrap().output, &self.pending);
        }
        result
    }
}

fn set_test_identity(runtime: &mut Runtime) -> Result<()> {
    runtime.set_server_metadata(yinz_steel::ServerMetadata {
        binary: "yinz".into(),
        session: Some("test".into()),
        incarnation: Some("test".into()),
    })?;
    runtime.eval("(set! %rpc-prefix \"test\")")?;
    Ok(())
}

pub(super) fn execute(request: Request) -> Result<Value> {
    let sources = Sources {
        entry: request
            .path
            .canonicalize()
            .context("resolve Scheme test entry")?,
        root: request
            .root
            .canonicalize()
            .context("resolve Scheme test root")?,
    };
    let mut root = yinz_steel::modules::SourceRoot::file(&sources.entry, Some(&sources.root))?;
    let source = root.read_entry()?;
    let capture = Arc::new(Mutex::new(Capture {
        journal: request.journal.context("missing test output journal")?,
        ..Default::default()
    }));
    let state = State {
        sources,
        capture: capture.clone(),
        plugin: None,
        now: Arc::new(AtomicU64::new(0)),
        wall_clock: Arc::new(AtomicI64::new(0)),
        started: Instant::now(),
        pending: String::new(),
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
    };
    let clock = state.now.clone();
    let wall_clock = state.wall_clock.clone();
    CALLBACK_CAPTURE.with(|slot| *slot.borrow_mut() = Some(capture.clone()));
    HOST.with(|host| *host.borrow_mut() = Some(state));
    let _reset = ResetHost;
    let mut runtime = Runtime::new_with_clocks(
        |_| bail!("host commands are unavailable in test cases; use a mocked plugin harness"),
        move || clock.load(Ordering::Relaxed),
        move || wall_clock.load(Ordering::Relaxed),
    )?;
    runtime.register_json_function("%test-control", |request| {
        let operation = request["op"]
            .as_str()
            .context("test operation must be text")?;
        if let Some(value) = callback_control(operation, &request["argument"])? {
            return Ok(value);
        }
        HOST.with(|host| {
            host.try_borrow_mut()
                .context("nested plugin harness operations are unavailable in mock callbacks")?
                .as_mut()
                .context("test worker is not active")?
                .call(operation, request["argument"].clone())
        })
    });
    let callbacks = capture.clone();
    runtime.register_mock_callbacks(move |name, callback| {
        let mut capture = callbacks.lock().unwrap();
        anyhow::ensure!(
            capture.callbacks.len() < 1000 || capture.callbacks.contains_key(&name),
            "at most 1000 mock callbacks"
        );
        capture.failures.remove(&name);
        capture.replies.remove(&name);
        capture.callbacks.insert(name, callback);
        Ok(())
    });
    set_test_identity(&mut runtime)?;
    runtime.eval(include_str!("steel/bootstrap.scm"))?;
    let result = runtime
        .eval_with_source_root(&source, &mut root)
        .and_then(|_| runtime.invoke("%test-run", vec![json!(request.index), json!(request.case)]));
    if let Some(case) = request.case {
        let mut output = capture.lock().unwrap().output.clone();
        append_bounded(&mut output, &runtime.take_output()?);
        append_bounded(&mut output, &runtime.take_logs()?);
        let duration_ms = HOST.with(|host| {
            let host = host.borrow();
            let state = host.as_ref().context("test worker is not active")?;
            append_bounded(&mut output, &state.pending);
            Ok::<_, anyhow::Error>(state.started.elapsed().as_millis() as u64)
        })?;
        Ok(json!(PluginTestResult {
            name: case.name,
            kind: PluginTestKind::Unit,
            status: if result.is_ok() { "passed" } else { "failed" }.into(),
            duration_ms,
            error: result.err().map(|error| format!("{error:#}")),
            output,
        }))
    } else {
        result
    }
}

#[cfg(test)]
#[path = "steel/unit.rs"]
mod tests;

#[cfg(test)]
#[path = "steel/picker_unit.rs"]
mod picker_tests;

#[cfg(test)]
#[path = "steel/mocks_unit.rs"]
mod mocks_tests;

#[cfg(test)]
#[path = "steel/assertions_unit.rs"]
mod assertions_tests;
