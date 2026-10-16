//! Steel execution shared by plugins and the Yinz REPL.

mod agent_model;
pub mod analysis;
pub mod binary;
mod bridge;
mod callback;
pub mod format;
pub mod modules;
pub mod repl;
mod sandbox;
mod text;
pub mod ui;
pub mod worker;

use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use steel::parser::parser::{ParseError, Parser};
use steel::parser::{lexer::TokenStream, tokens::TokenType};
use steel::rerrs::ErrorKind;
use steel::rvals::{IntoSteelVal, SteelVal};
use steel::steel_vm::{engine::Engine, register_fn::RegisterFn};

pub use bridge::{from_scheme, to_scheme};
pub use callback::JsonCallback;
#[derive(Clone)]
pub struct InterruptHandle(InterruptTarget);

#[derive(Clone)]
enum InterruptTarget {
    Thread(steel::steel_vm::ThreadStateController),
    Process(Arc<worker::Process>),
}

impl InterruptHandle {
    pub fn interrupt(&self) {
        match &self.0 {
            InterruptTarget::Thread(thread) => thread.interrupt(),
            InterruptTarget::Process(process) => process.terminate(),
        }
    }

    pub fn resume(&self) {
        if let InterruptTarget::Thread(thread) = &self.0 {
            thread.resume();
        }
    }
}

pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RENDER_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVAL_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvalOutput {
    pub values: Vec<String>,
    pub output: String,
}

#[derive(Clone)]
struct Output(Arc<Mutex<Vec<u8>>>, usize, &'static str);

impl Output {
    fn new(limit: usize, diagnostic: &'static str) -> Self {
        Self(Arc::new(Mutex::new(Vec::new())), limit, diagnostic)
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut output = self
            .0
            .lock()
            .map_err(|_| io::Error::other("output lock poisoned"))?;
        if output.len().saturating_add(bytes.len()) > self.1 {
            return Err(io::Error::other(self.2));
        }
        output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ServerMetadata {
    pub binary: String,
    pub session: Option<String>,
    pub incarnation: Option<String>,
}

pub struct Runtime {
    engine: Engine,
    output: Output,
    logs: Output,
    metadata: Arc<OnceLock<ServerMetadata>>,
    plugin_resolver: Arc<OnceLock<PluginResolver>>,
}

type PluginResolver = Box<dyn Fn(Option<String>) -> Result<String> + Send + Sync>;

impl Runtime {
    /// Add an explicit, reentrant JSON operation to a host-owned interpreter.
    pub fn register_json_function(
        &mut self,
        name: &'static str,
        function: impl Fn(Value) -> Result<Value> + Send + Sync + 'static,
    ) {
        self.engine.register_fn(
            name,
            move |value: SteelVal| -> steel::rvals::Result<SteelVal> {
                let value = from_scheme(&value).map_err(scheme_error)?;
                to_scheme(&function(value).map_err(scheme_error)?).map_err(scheme_error)
            },
        );
    }

    /// Install rooted Scheme mock callbacks only in an explicitly configured test interpreter.
    pub fn register_mock_callbacks(
        &mut self,
        receive: impl Fn(String, JsonCallback) -> Result<()> + Send + Sync + 'static,
    ) {
        callback::register(&mut self.engine, receive);
    }

    pub fn new(host: impl FnMut(Value) -> Result<Value> + Send + 'static) -> Result<Self> {
        let started = std::time::Instant::now();
        Self::new_with_clock(host, move || {
            started.elapsed().as_millis().min(u64::MAX as u128) as u64
        })
    }

    /// Inject a monotonic clock in milliseconds before compiling the SDK.
    pub fn new_with_clock(
        host: impl FnMut(Value) -> Result<Value> + Send + 'static,
        clock: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::new_with_clocks(host, clock, || chrono::Utc::now().timestamp_millis())
    }

    /// Inject both clocks before SDK compilation for deterministic lifecycle replay.
    pub fn new_with_clocks(
        host: impl FnMut(Value) -> Result<Value> + Send + 'static,
        clock: impl Fn() -> u64 + Send + Sync + 'static,
        wall_clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut engine = sandbox::engine().context("creating confined Steel engine")?;
        let output = Output::new(MAX_RENDER_BYTES, "Steel render output exceeds 16 MiB");
        let logs = Output::new(MAX_OUTPUT_BYTES, "Steel diagnostic output exceeds 4 MiB");
        let metadata = Arc::new(OnceLock::<ServerMetadata>::new());
        let plugin_resolver = Arc::new(OnceLock::<PluginResolver>::new());
        let resolver = plugin_resolver.clone();
        engine.register_fn(
            "yinz-plugin-location",
            move || -> steel::rvals::Result<String> {
                resolver
                    .get()
                    .ok_or_else(|| scheme_error("plugin source is unknown"))?(None)
                .map_err(scheme_error)
            },
        );
        let resolver = plugin_resolver.clone();
        engine.register_fn(
            "yinz-plugin-resolve",
            move |path: String| -> steel::rvals::Result<String> {
                resolver
                    .get()
                    .ok_or_else(|| scheme_error("plugin source is unknown"))?(Some(
                    path,
                ))
                .map_err(scheme_error)
            },
        );
        for (name, field) in [
            ("yinz-server-binary", 0),
            ("yinz-server-session", 1),
            ("yinz-server-incarnation", 2),
        ] {
            let metadata = metadata.clone();
            engine.register_fn(name, move || -> steel::rvals::Result<SteelVal> {
                let value = metadata.get().and_then(|metadata| match field {
                    0 => Some(metadata.binary.as_str()),
                    1 => metadata.session.as_deref(),
                    _ => metadata.incarnation.as_deref(),
                });
                to_scheme(&value.map(Value::from).unwrap_or(Value::Null)).map_err(scheme_error)
            });
        }
        let port = SteelVal::new_dyn_writer_port(output.clone());
        engine.register_value("yinz-output-port", port);
        engine.register_value("yinz-log-port", SteelVal::new_dyn_writer_port(logs.clone()));
        let host = Mutex::new(host);
        engine.register_fn(
            "yinz-command",
            move |command: SteelVal| -> steel::rvals::Result<SteelVal> {
                let command = bridge::from_command(&command).map_err(scheme_error)?;
                let reply =
                    host.lock().map_err(scheme_error)?(command.clone()).map_err(scheme_error)?;
                binary::reply(&command, &reply).map_err(scheme_error)
            },
        );
        let command = engine
            .extract_value("yinz-command")
            .map_err(|error| anyhow!("registering host command bridge: {error}"))?;
        engine.register_value("%async-native-command", command);
        engine.register_fn(
            "%yinz-validate-command",
            |value: SteelVal| -> steel::rvals::Result<()> {
                bridge::from_command(&value)
                    .map(|_| ())
                    .map_err(scheme_error)
            },
        );
        engine.register_value(
            "%rpc-prefix",
            SteelVal::StringV(format!("{:032x}", rand::random::<u128>()).into()),
        );
        engine.register_fn(
            "yinz-json-string",
            |value: SteelVal| -> steel::rvals::Result<String> {
                let mut value = from_scheme(&value).map_err(scheme_error)?;
                value.sort_all_objects();
                serde_json::to_string(&value).map_err(scheme_error)
            },
        );
        engine.register_fn(
            "yinz-json-parse",
            |source: String| -> steel::rvals::Result<SteelVal> {
                if source.len() > bridge::MAX_INPUT_BYTES {
                    return Err(scheme_error("JSON source exceeds 64 MiB"));
                }
                let value: Value = serde_json::from_str(&source).map_err(scheme_error)?;
                to_scheme(&value).map_err(scheme_error)
            },
        );
        engine.register_fn("yinz-text-width", |text: String| {
            unicode_width::UnicodeWidthStr::width(text.as_str())
        });
        engine.register_fn("char-alphanumeric?", char::is_alphanumeric);
        engine.register_fn(
            "yinz-key-name",
            |value: SteelVal| -> steel::rvals::Result<String> {
                let key: yinz_types::keys::BareKey =
                    serde_json::from_value(from_scheme(&value).map_err(scheme_error)?)
                        .map_err(scheme_error)?;
                Ok(match key {
                    yinz_types::keys::BareKey::Char(character) => character.to_string(),
                    yinz_types::keys::BareKey::F(number) => format!("F{number}"),
                    key => format!("{key:?}"),
                })
            },
        );
        engine.register_fn("yinz-now-ms", move || {
            clock().min(isize::MAX as u64) as isize
        });
        engine.register_fn(
            "yinz-random-integer",
            |upper: i64| -> steel::rvals::Result<SteelVal> {
                use rand::RngExt;
                if upper <= 0 {
                    return Err(scheme_error("random upper bound must be positive"));
                }
                to_scheme(&Value::from(rand::rng().random_range(0..upper))).map_err(scheme_error)
            },
        );
        engine.register_fn(
            "yinz-wall-clock-ms",
            move || -> steel::rvals::Result<SteelVal> {
                to_scheme(&Value::from(wall_clock())).map_err(scheme_error)
            },
        );
        engine.register_fn(
            "yinz-utc-date",
            |milliseconds: i64| -> steel::rvals::Result<SteelVal> {
                use chrono::{Datelike, Timelike};
                let date = chrono::DateTime::from_timestamp_millis(milliseconds)
                    .ok_or_else(|| scheme_error("date is outside the supported UTC range"))?;
                to_scheme(&serde_json::json!({
                    "year": date.year(), "month": date.month(), "day": date.day(),
                    "weekday": date.weekday().num_days_from_sunday(),
                    "hour": date.hour(), "minute": date.minute(), "second": date.second(),
                }))
                .map_err(scheme_error)
            },
        );
        engine.register_fn(
            "yinz-decode-utf8",
            |value: SteelVal| -> steel::rvals::Result<String> {
                binary::decode_utf8(value).map_err(scheme_error)
            },
        );
        engine.register_fn("yinz-text-sanitize", |text: String| -> String {
            String::from_utf8_lossy(&strip_ansi_escapes::strip(text.replace('\t', "    ")))
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .collect()
        });
        engine.register_fn(
            "yinz-text-truncate",
            |text: String, columns: usize| -> String {
                let mut width = 0;
                text.chars()
                    .take_while(|c| {
                        width += unicode_width::UnicodeWidthChar::width(*c).unwrap_or_default();
                        width <= columns
                    })
                    .collect()
            },
        );
        ui::register(&mut engine);
        agent_model::register(&mut engine);
        engine
            .run(include_str!("sdk.scm"))
            .map_err(|error| anyhow!("initializing Steel SDK: {error}"))?;
        engine
            .run(include_str!("trace.scm"))
            .map_err(|error| anyhow!("initializing Steel trace SDK: {error}"))?;
        engine
            .run(include_str!("async.scm"))
            .map_err(|error| anyhow!("initializing Steel async SDK: {error}"))?;
        engine
            .run(include_str!("data.scm"))
            .map_err(|error| anyhow!("initializing Steel data SDK: {error}"))?;
        engine
            .run(include_str!("rpc.scm"))
            .map_err(|error| anyhow!("initializing Steel RPC SDK: {error}"))?;
        engine
            .run(include_str!("services.scm"))
            .map_err(|error| anyhow!("initializing Steel services SDK: {error}"))?;
        engine
            .run(include_str!("consumer.scm"))
            .map_err(|error| anyhow!("initializing Steel consumer SDK: {error}"))?;
        engine
            .run(include_str!("panes.scm"))
            .map_err(|error| anyhow!("initializing Steel pane SDK: {error}"))?;
        engine
            .run(include_str!("agents.scm"))
            .map_err(|error| anyhow!("initializing Steel agent SDK: {error}"))?;
        engine
            .run(include_str!("ui.scm"))
            .map_err(|error| anyhow!("initializing Steel UI SDK: {error}"))?;
        engine
            .run(include_str!("palette.scm"))
            .map_err(|error| anyhow!("initializing Steel palette SDK: {error}"))?;
        engine
            .run(include_str!("workers.scm"))
            .map_err(|error| anyhow!("initializing Steel workers SDK: {error}"))?;
        Ok(Self {
            engine,
            output,
            logs,
            metadata,
            plugin_resolver,
        })
    }

    /// Supply read-only host identity before evaluating plugin source.
    pub fn set_server_metadata(&mut self, metadata: ServerMetadata) -> Result<()> {
        self.metadata
            .set(metadata)
            .map_err(|_| anyhow!("Steel server metadata is already initialized"))
    }

    /// Install immutable, confined source identity supplied by the host.
    pub fn set_plugin_resolver(
        &mut self,
        resolver: impl Fn(Option<String>) -> Result<String> + Send + Sync + 'static,
    ) -> Result<()> {
        self.plugin_resolver
            .set(Box::new(resolver))
            .map_err(|_| anyhow!("Steel plugin resolver is already initialized"))
    }

    fn set_source_identity(&mut self, root: &modules::SourceRoot) -> Result<()> {
        if self.plugin_resolver.get().is_none() {
            let root = root.try_clone()?;
            self.set_plugin_resolver(move |path| match path {
                Some(path) => root.resolve(&path),
                None => root.location(),
            })?;
        }
        Ok(())
    }

    /// Interrupt running bytecode with `interrupt()`; call `resume()` before reuse.
    pub fn interruption_handle(&self) -> InterruptHandle {
        InterruptHandle(InterruptTarget::Thread(
            self.engine.get_thread_state_controller(),
        ))
    }

    pub fn eval(&mut self, source: &str) -> Result<EvalOutput> {
        self.eval_with_path(source, None)
    }

    pub fn check(&mut self, source: &str) -> Result<()> {
        self.check_with_path(source, None)
    }

    fn check_with_path(&mut self, source: &str, path: Option<&std::path::Path>) -> Result<()> {
        validate_source(source)?;
        let program = match path {
            Some(path) => self
                .engine
                .emit_raw_program_with_source_name(source.to_owned(), path.to_owned()),
            None => self.engine.emit_raw_program_no_path(source.to_owned()),
        }
        .map_err(|error| {
            let diagnostic = self
                .engine
                .raise_error_to_string(error.clone())
                .unwrap_or_else(|| error.to_string());
            anyhow!("compiling Steel: {diagnostic}")
        })?;
        self.engine
            .raw_program_to_executable(program)
            .map(|_| ())
            .map_err(|error| {
                let diagnostic = self
                    .engine
                    .raise_error_to_string(error.clone())
                    .unwrap_or_else(|| error.to_string());
                anyhow!("checking Steel: {diagnostic}")
            })
    }

    pub fn check_with_source_root(
        &mut self,
        source: &str,
        root: &mut modules::SourceRoot,
    ) -> Result<()> {
        self.set_source_identity(root)?;
        root.begin(source.len())?;
        let entry = root.entry_id().to_owned();
        let rewritten =
            self.prepare_modules(source, &entry, &mut |origin, path| root.load(origin, path))?;
        self.check_with_path(&rewritten, Some(std::path::Path::new(&entry)))
    }

    pub fn eval_with_source_root(
        &mut self,
        source: &str,
        root: &mut modules::SourceRoot,
    ) -> Result<EvalOutput> {
        self.set_source_identity(root)?;
        root.begin(source.len())?;
        let entry = root.entry_id().to_owned();
        let rewritten =
            self.prepare_modules(source, &entry, &mut |origin, path| root.load(origin, path))?;
        self.eval_with_path(&rewritten, Some(std::path::Path::new(&entry)))
    }

    pub(crate) fn prepare_modules(
        &mut self,
        source: &str,
        entry: &str,
        loader: &mut impl FnMut(&str, &str) -> Result<modules::ModuleSource>,
    ) -> Result<String> {
        let (source, modules) = modules::prepare(source, entry, loader)?;
        for (name, source) in modules {
            self.engine.register_steel_module(name, source);
        }
        Ok(source)
    }

    pub fn invoke(&mut self, name: &str, arguments: Vec<Value>) -> Result<Value> {
        let function = self
            .engine
            .extract_value(name)
            .map_err(|error| anyhow!("finding Steel function {name}: {error}"))?;
        let arguments = arguments
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if name == "yinz-worker-event" && index == 0 {
                    binary::event(value)
                } else {
                    to_scheme(value)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let result = self
            .engine
            .call_function_with_args(function, arguments)
            .map_err(|error| {
                anyhow!("calling Steel function {name}: {}", self.diagnostic(error))
            })?;
        if matches!(result, SteelVal::Void) {
            Ok(Value::Null)
        } else {
            from_scheme(&result)
        }
    }

    pub fn value(&self, name: &str) -> Result<Value> {
        let value = self
            .engine
            .extract_value(name)
            .map_err(|error| anyhow!("reading Steel value {name}: {error}"))?;
        from_scheme(&value)
    }

    pub fn eval_with_path(
        &mut self,
        source: &str,
        path: Option<&std::path::Path>,
    ) -> Result<EvalOutput> {
        validate_source(source)?;
        // The name is diagnostic metadata only; all imports still use the confined resolver.
        let program = match path {
            Some(path) => self
                .engine
                .emit_raw_program_with_source_name(source.to_owned(), path.to_owned()),
            None => self.engine.emit_raw_program_no_path(source.to_owned()),
        };
        let result = program
            .and_then(|program| self.engine.run_raw_program(program))
            .map_err(|error| {
                let diagnostic = self
                    .engine
                    .raise_error_to_string(error.clone())
                    .unwrap_or_else(|| error.to_string());
                match path {
                    Some(path) => anyhow!("evaluating Steel {}: {diagnostic}", path.display()),
                    None => anyhow!("evaluating Steel: {diagnostic}"),
                }
            })?;
        let mut values = Vec::new();
        let mut size = 0;
        for value in result
            .into_iter()
            .filter(|value| !matches!(value, SteelVal::Void))
        {
            let mut rendered = Output::new(MAX_EVAL_BYTES, "Steel values exceed 16 MiB");
            write!(rendered, "{value}").context("format Steel value within output limit")?;
            let bytes = std::mem::take(
                &mut *rendered
                    .0
                    .lock()
                    .map_err(|_| anyhow!("Steel value output lock poisoned"))?,
            );
            size += bytes.len();
            anyhow::ensure!(size <= MAX_EVAL_BYTES, "Steel values exceed 16 MiB");
            values.push(String::from_utf8_lossy(&bytes).into_owned());
        }
        Ok(EvalOutput {
            values,
            output: self.take_output()?,
        })
    }

    /// Output produced before an error remains available here.
    pub fn take_output(&mut self) -> Result<String> {
        Self::drain_output(&self.output)
    }

    /// Diagnostic messages never become part of a rendered frame.
    pub fn take_logs(&mut self) -> Result<String> {
        Self::drain_output(&self.logs)
    }

    fn drain_output(output: &Output) -> Result<String> {
        let bytes = std::mem::take(
            &mut *output
                .0
                .lock()
                .map_err(|_| anyhow!("Steel output lock poisoned"))?,
        );
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn load(&mut self, configuration: Value) -> Result<()> {
        self.lifecycle("load", vec![to_scheme(&configuration)?])?;
        Ok(())
    }

    pub fn update(&mut self, event: Value) -> Result<bool> {
        let event = binary::event(&event)?;
        self.call_optional("yinz-agent-cache-event", vec![event.clone()])?;
        self.call_optional("yinz-palette-update", vec![event.clone()])?;
        Ok(matches!(
            self.lifecycle("update", vec![event])?,
            Some(SteelVal::BoolV(true))
        ))
    }

    pub fn pipe(&mut self, message: Value) -> Result<bool> {
        self.call_optional(
            "yinz-trace-input",
            vec![
                to_scheme(&Value::String("pipe".into()))?,
                to_scheme(&message)?,
            ],
        )?;
        for dispatch in [
            "yinz-consumer-pipe",
            "yinz-rpc-dispatch",
            "yinz-keybinding-dispatch",
        ] {
            if matches!(
                self.call_optional(dispatch, vec![to_scheme(&message)?])?,
                Some(SteelVal::BoolV(true))
            ) {
                return Ok(matches!(
                    self.call_optional("yinz-async-poll", Vec::new())?,
                    Some(SteelVal::BoolV(true))
                ));
            }
        }
        self.changed("pipe", message)
    }

    pub fn palette(&mut self, message: Value) -> Result<bool> {
        self.changed("palette", message)
    }

    fn changed(&mut self, name: &str, value: Value) -> Result<bool> {
        Ok(matches!(
            self.lifecycle(name, vec![to_scheme(&value)?])?,
            Some(SteelVal::BoolV(true))
        ))
    }

    pub fn render(&mut self, rows: usize, columns: usize) -> Result<String> {
        self.take_output()?;
        let result = self.lifecycle(
            "render",
            vec![
                rows.into_steelval().map_err(scheme_error)?,
                columns.into_steelval().map_err(scheme_error)?,
            ],
        )?;
        let mut output = self.take_output()?;
        if let Some(SteelVal::StringV(text)) = result {
            anyhow::ensure!(
                output.len().saturating_add(text.len()) <= MAX_RENDER_BYTES,
                "Steel render output exceeds 16 MiB"
            );
            output.push_str(text.as_str());
        }
        Ok(output)
    }

    pub fn unload(&mut self) -> Result<()> {
        self.lifecycle("unload", Vec::new())?;
        Ok(())
    }

    fn lifecycle(&mut self, name: &str, arguments: Vec<SteelVal>) -> Result<Option<SteelVal>> {
        let hook = self
            .optional_function(name)?
            .unwrap_or(SteelVal::BoolV(false));
        let cleanup = if name == "unload" {
            self.optional_function("yinz-palette-unload")?
                .unwrap_or(SteelVal::BoolV(false))
        } else {
            SteelVal::BoolV(false)
        };
        self.call_optional(
            "yinz-async-lifecycle",
            vec![
                SteelVal::StringV(name.into()),
                hook,
                SteelVal::ListV(arguments.into()),
                cleanup,
            ],
        )
    }

    fn optional_function(&self, name: &str) -> Result<Option<SteelVal>> {
        if !self.engine.global_exists(name) {
            return Ok(None);
        }
        match self.engine.extract_value(name) {
            Ok(function) => Ok(Some(function)),
            Err(error) if error.kind() == ErrorKind::FreeIdentifier => Ok(None),
            Err(error) => Err(anyhow!("finding Steel hook {name}: {error}")),
        }
    }

    fn call_optional(&mut self, name: &str, arguments: Vec<SteelVal>) -> Result<Option<SteelVal>> {
        let Some(function) = self.optional_function(name)? else {
            return Ok(None);
        };
        self.engine
            .call_function_with_args(function, arguments)
            .map(Some)
            .map_err(|error| anyhow!("Steel hook {name}: {}", self.diagnostic(error)))
    }

    fn diagnostic(&self, error: steel::SteelErr) -> String {
        self.engine
            .raise_error_to_string(error.clone())
            .unwrap_or_else(|| error.to_string())
    }
}

fn scheme_error(error: impl std::fmt::Display) -> steel::SteelErr {
    steel::SteelErr::new(ErrorKind::Generic, error.to_string())
}

/// Invalid complete input is evaluated immediately so the REPL can report its error.
pub fn source_complete(source: &str) -> bool {
    if validate_source(source).is_err() {
        return true;
    }
    !Parser::new(source, None)
        .any(|expression| matches!(expression, Err(ParseError::UnexpectedEOF(..))))
}

fn validate_source(source: &str) -> Result<()> {
    anyhow::ensure!(
        source.len() <= MAX_SOURCE_BYTES,
        "Steel source exceeds 8 MiB"
    );
    let mut nesting = Vec::new();
    let mut prefixes = 0;
    let mut depth = 0;
    for (count, token) in TokenStream::new(source, false, None).enumerate() {
        anyhow::ensure!(count < 500_000, "Steel source exceeds token limit");
        let Ok(token) = token else { continue };
        match token.ty {
            TokenType::Comment => {},
            TokenType::OpenParen(..) => {
                nesting.push(prefixes + 1);
                depth += prefixes + 1;
                prefixes = 0;
            },
            TokenType::CloseParen(..) => {
                depth -= nesting.pop().unwrap_or_default();
                prefixes = 0;
            },
            TokenType::QuoteTick
            | TokenType::QuasiQuote
            | TokenType::Unquote
            | TokenType::UnquoteSplice
            | TokenType::QuoteSyntax
            | TokenType::QuasiQuoteSyntax
            | TokenType::UnquoteSyntax
            | TokenType::UnquoteSpliceSyntax
            | TokenType::DatumComment => prefixes += 1,
            _ => prefixes = 0,
        }
        anyhow::ensure!(depth + prefixes <= 128, "Steel source nesting exceeds 128");
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/runtime.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/async.rs"]
mod async_tests;

#[cfg(test)]
#[path = "unit/history_model.rs"]
mod history_model_tests;

#[cfg(test)]
#[path = "unit/rpc.rs"]
mod rpc_tests;

#[cfg(test)]
#[path = "unit/services.rs"]
mod services_tests;

#[cfg(test)]
#[path = "unit/consumer.rs"]
mod consumer_tests;

#[cfg(test)]
#[path = "unit/panes.rs"]
mod panes_tests;

#[cfg(test)]
#[path = "unit/agents.rs"]
mod agents_tests;

#[cfg(test)]
#[path = "unit/palette_async.rs"]
mod palette_async_tests;

#[cfg(test)]
#[path = "unit/trace.rs"]
mod trace_tests;

#[cfg(test)]
#[path = "unit/binary.rs"]
mod binary_tests;
