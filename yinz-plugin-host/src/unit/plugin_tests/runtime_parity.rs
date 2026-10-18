//! Compare real Scheme guests with recordings captured from the original Wasm runtimes.
//! Recorded commands run against independent host services, preserving database and filesystem checks.
use super::reference_trace::Trace;
use crate::response::{decode_response, DecodedResponse};
use prost::Message;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use yinz_plugin_api::{
    plugin_command::*,
    plugin_ids::{ProtobufPluginIds, ProtobufYinzVersion},
};
use yinz_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter};
use yinz_types::data::{CommandType, Event, InputMode, ModeInfo, PipeMessage, PluginCommand};

pub(crate) type Responder = Arc<dyn Fn(&PluginCommand) -> Option<Vec<u8>> + Send + Sync>;
type Commands = Arc<Mutex<Vec<Value>>>;

fn record(commands: &Commands, command: &PluginCommand) {
    let mut value = serde_json::to_value(command).unwrap();
    // These payloads are sets. Preserve order for every ordered argument and event.
    for name in ["Subscribe", "Unsubscribe"] {
        if let Some(values) = value.get_mut(name).and_then(Value::as_array_mut) {
            values.sort_by_key(Value::to_string);
        }
    }
    super::reference_trace::legacy_project_fields(&mut value);
    commands.lock().unwrap().push(value);
}

pub(crate) fn default_reply(command: &PluginCommand) -> Option<Vec<u8>> {
    Some(match command {
        PluginCommand::GetProjectProviders => ProtobufAgentProvidersResponse {
            json: "{\"Ok\":[]}".into(),
        }
        .encode_to_vec(),
        PluginCommand::PublishProjectLocations(_) => ProtobufAgentProvidersResponse {
            json: "{\"Ok\":null}".into(),
        }
        .encode_to_vec(),
        PluginCommand::GetPluginIds => ProtobufPluginIds {
            plugin_id: 1,
            client_id: 1,
            yinz_pid: 1234,
            initial_cwd: "/work".into(),
            hostname: "parity-host".into(),
            host_id: "parity-host-id".into(),
            max_session_name_bytes: Some(80),
        }
        .encode_to_vec(),
        PluginCommand::GetYinzVersion => ProtobufYinzVersion {
            version: "test".into(),
        }
        .encode_to_vec(),
        PluginCommand::GetLocalUtcOffset { .. } => {
            ProtobufGetLocalUtcOffsetResponse { offset_seconds: 0 }.encode_to_vec()
        },
        PluginCommand::OpenFile(..) => ProtobufOpenFileResponse::default().encode_to_vec(),
        PluginCommand::OpenTerminal(..) => ProtobufOpenTerminalResponse::default().encode_to_vec(),
        PluginCommand::OpenFileInPlaceOfPlugin(..) => {
            ProtobufOpenFileInPlaceOfPluginResponse::default().encode_to_vec()
        },
        PluginCommand::OpenTerminalInPlaceOfPlugin(..) => {
            ProtobufOpenTerminalInPlaceOfPluginResponse::default().encode_to_vec()
        },
        PluginCommand::BreakPanesToNewTab(..) => {
            ProtobufBreakPanesToNewTabResponse::default().encode_to_vec()
        },
        PluginCommand::BreakPanesToTabWithIndex(..) => {
            ProtobufBreakPanesToTabWithIndexResponse::default().encode_to_vec()
        },
        _ => return None,
    })
}

pub(crate) struct WasmReference {
    commands: Commands,
    responder: Responder,
    trace: Trace,
}
impl WasmReference {
    pub(crate) fn new(
        name: &str,
        configuration: BTreeMap<String, String>,
        filesystem_root: &Path,
        responder: Responder,
    ) -> Self {
        Self::with_clock(name, configuration, filesystem_root, responder, None)
    }
    pub(crate) fn with_clock(
        name: &str,
        configuration: BTreeMap<String, String>,
        filesystem_root: &Path,
        responder: Responder,
        _clock: Option<Arc<AtomicU64>>,
    ) -> Self {
        let mut trace = Trace::new(
            "wasm",
            name,
            serde_json::to_value(configuration).unwrap(),
            filesystem_root,
        );
        let (_, commands) = trace.replay(serde_json::json!(["load"]), |command| responder(command));
        Self {
            commands: Arc::new(Mutex::new(commands)),
            responder,
            trace,
        }
    }
    pub(crate) fn update(&mut self, event: &Event) -> bool {
        self.replay(serde_json::json!(["update", event]))
            .as_bool()
            .unwrap()
    }
    pub(crate) fn pipe(&mut self, message: &PipeMessage) -> Option<bool> {
        serde_json::from_value(self.replay(serde_json::json!(["pipe", message]))).unwrap()
    }
    pub(crate) fn render(&mut self, rows: usize, columns: usize) -> String {
        let result = self.replay(serde_json::json!(["render", rows, columns]));
        if let Some(trap) = result.get("trap") {
            panic!("recorded original runtime render trap: {trap}");
        }
        result.as_str().unwrap().to_owned()
    }
    fn replay(&mut self, input: Value) -> Value {
        let rendering = input[0] == "render";
        let (result, mut commands) = self
            .trace
            .replay(input, |command| (self.responder)(command));
        if rendering {
            // Steel publishes only the final cursor with the accepted frame.
            let mut cursor = None;
            commands.retain(|command| {
                if command.get("ShowCursor").is_some() {
                    cursor = Some(command.clone());
                    false
                } else {
                    true
                }
            });
            commands.extend(cursor);
        }
        self.commands.lock().unwrap().extend(commands);
        result
    }
    pub(crate) fn take_commands(&self) -> Vec<Value> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
}

pub(crate) struct SteelReference {
    pub(crate) runtime: yinz_steel::Runtime,
    commands: Commands,
}
impl SteelReference {
    pub(crate) fn new(
        name: &str,
        configuration: BTreeMap<String, String>,
        filesystem_root: &Path,
        responder: Responder,
    ) -> Self {
        Self::with_clock(name, configuration, filesystem_root, responder, None)
    }
    pub(crate) fn with_clock(
        name: &str,
        configuration: BTreeMap<String, String>,
        _filesystem_root: &Path,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
    ) -> Self {
        Self::instance(name, configuration, responder, clock, None)
    }
    pub(crate) fn worker(
        name: &str,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
        worker: &str,
    ) -> Self {
        Self::instance(name, BTreeMap::new(), responder, clock, Some(worker))
    }
    fn instance(
        name: &str,
        configuration: BTreeMap<String, String>,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
        worker: Option<&str>,
    ) -> Self {
        let commands = Commands::default();
        let recorded = commands.clone();
        let host = move |value: Value| {
            let value = value
                .get("__yinz_runtime")
                .map(|envelope| envelope["command"].clone())
                .unwrap_or(value);
            let command = yinz_steel::binary::decode_command(value)?;
            record(&recorded, &command);
            let Some(response) = responder(&command) else {
                return Ok(Value::Null);
            };
            Ok(
                match decode_response(CommandType::from(&command), &response).transpose()? {
                    Some(DecodedResponse::Json(value)) => value,
                    Some(DecodedResponse::Bytes(bytes)) => serde_json::to_value(bytes)?,
                    None => serde_json::to_value(response)?,
                },
            )
        };
        let mut runtime = match clock {
            Some(clock) => {
                let wall_clock = clock.clone();
                yinz_steel::Runtime::new_with_clocks(
                    host,
                    move || clock.load(Ordering::SeqCst),
                    move || wall_clock.load(Ordering::SeqCst) as i64,
                )
            },
            None => yinz_steel::Runtime::new(host),
        }
        .unwrap();
        let mut root = yinz_steel::modules::SourceRoot::file(
            &yinz_steel_builtins::asset_dir().join(format!("{name}.scm")),
            None,
        )
        .unwrap();
        runtime
            .eval_with_source_root(
                &yinz_steel_builtins::fixture_source(name).unwrap(),
                &mut root,
            )
            .unwrap();
        if let Some(worker) = worker {
            runtime
                .invoke("yinz-worker-bind", vec![Value::String(worker.into())])
                .unwrap();
        } else {
            runtime
                .load(serde_json::to_value(configuration).unwrap())
                .unwrap();
        }
        Self { runtime, commands }
    }
    pub(crate) fn update(&mut self, event: &Event) -> bool {
        self.runtime
            .update(crate::event::event_to_json(event).unwrap())
            .unwrap()
    }
    pub(crate) fn pipe(&mut self, message: &PipeMessage) -> Option<bool> {
        Some(
            self.runtime
                .pipe(serde_json::to_value(message).unwrap())
                .unwrap(),
        )
    }
    pub(crate) fn render(&mut self, rows: usize, columns: usize) -> String {
        self.runtime
            .render(rows, columns)
            .unwrap()
            .replace("\n\r", "\n")
    }
    pub(crate) fn grant_permissions(&mut self) {
        self.runtime
            .eval(
                "(for-each (lambda (id) (yinz-async-permission id (hash \"Ok\" \"Granted\"))) (hash-keys->list %async-permissions))",
            )
            .unwrap();
    }
    pub(crate) fn take_commands(&self) -> Vec<Value> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
}

pub(crate) struct Pair {
    pub(crate) wasm: WasmReference,
    pub(crate) steel: SteelReference,
    pub(crate) mode: ModeInfo,
    history_worker: Option<SteelReference>,
    _root: tempfile::TempDir,
}
impl Pair {
    pub(crate) fn new(name: &str, configuration: BTreeMap<String, String>) -> Self {
        Self::with_host(name, configuration, Arc::new(default_reply), None)
    }
    pub(crate) fn with_host(
        name: &str,
        configuration: BTreeMap<String, String>,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
    ) -> Self {
        Self::with_hosts(name, configuration, responder.clone(), responder, clock)
    }
    pub(crate) fn with_hosts(
        name: &str,
        configuration: BTreeMap<String, String>,
        wasm_responder: Responder,
        steel_responder: Responder,
        clock: Option<Arc<AtomicU64>>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let history_worker = (name == "agents").then(|| {
            SteelReference::worker(name, steel_responder.clone(), clock.clone(), "history")
        });
        let mut pair = Self {
            history_worker,
            wasm: WasmReference::with_clock(
                name,
                configuration.clone(),
                root.path(),
                wasm_responder,
                clock.clone(),
            ),
            steel: SteelReference::with_clock(
                name,
                configuration,
                root.path(),
                steel_responder,
                clock,
            ),
            mode: ModeInfo::default(),
            _root: root,
        };
        pair.commands();
        pair
    }
    pub(crate) fn commands(&mut self) -> Vec<Value> {
        let mut actual = self.steel.take_commands();
        if let Some(worker) = &mut self.history_worker {
            // Compare the recorded cells after explicitly delivering every history result.
            let mut requests: Vec<_> = actual
                .iter()
                .filter_map(|c| c.get("PostMessageTo"))
                .cloned()
                .collect();
            while let Some(request) = requests.pop() {
                worker
                    .runtime
                    .invoke(
                        "yinz-worker-message",
                        vec![request["name"].clone(), request["payload"].clone()],
                    )
                    .unwrap();
                for command in worker.take_commands() {
                    if let Some(reply) = command.get("PostMessageToPlugin") {
                        self.steel.update(&Event::CustomMessage(
                            reply["name"].as_str().unwrap().into(),
                            reply["payload"].as_str().unwrap().into(),
                        ));
                    }
                }
                let commands = self.steel.take_commands();
                requests.extend(
                    commands
                        .iter()
                        .filter_map(|c| c.get("PostMessageTo"))
                        .cloned(),
                );
                actual.extend(commands);
            }
        }
        let agents = self.history_worker.is_some();
        let normalize = |commands: Vec<Value>| {
            commands
                .into_iter()
                // Provider-chain and cache tests cover these new service effects.
                .filter(|command| {
                    command != "GetProjectProviders"
                        && command.get("PublishProjectLocations").is_none()
                        && command.get("Subscribe")
                            != Some(&serde_json::json!(["ProjectProvidersChanged"]))
                })
                // History I/O now belongs to a worker and unchanged snapshots omit it entirely.
                // Dedicated archive tests assert requests, ordering and failures separately.
                // The screen now creates and moves the overlay container.
                .filter(|command| {
                    !agents
                        || (command.pointer("/PaneTabs/Mount").is_none()
                            && command.get("BreakPanesToTabWithIndex").is_none())
                })
                .filter(|command| {
                    !agents
                        || !(command.get("PostMessageTo").is_some()
                            || command
                                .pointer("/DataFetch/request/entity")
                                .is_some_and(|entity| entity == "agent_run_entry")
                            || command.pointer("/DataFetch/request/select")
                                == Some(&serde_json::json!(["payload"]))
                            || command.get("DataCount").is_some()
                            || command.get("GetLocalUtcOffset").is_some())
                })
                .map(|mut command| {
                    if let Some(Value::Array(events)) = command.get_mut("Subscribe") {
                        if agents {
                            // Worker replies and theme repainting were added after the recordings.
                            events
                                .retain(|event| event != "CustomMessage" && event != "ModeUpdate");
                        }
                        events.sort_by_key(Value::to_string);
                    }
                    if let Some(preset) = command.pointer_mut("/Reconfigure/0") {
                        if let Some(source) = preset.as_str() {
                            *preset = Value::String(super::reference_trace::legacy_preset(source));
                        }
                    }
                    normalize_command_settings(&mut command);
                    command
                })
                .collect::<Vec<_>>()
        };
        let expected = normalize(self.wasm.take_commands());
        let actual = normalize(actual);
        assert_eq!(
            actual, expected,
            "actual runtimes emitted different host commands"
        );
        expected
    }
    pub(crate) fn event(&mut self, event: Event) -> Vec<Value> {
        if let Event::ModeUpdate(mode) = &event {
            self.mode = mode.clone();
        }
        if self.history_worker.is_some()
            && matches!(event, Event::Key(_))
            && self
                .steel
                .runtime
                .invoke("a-get", vec![Value::String("history_hidden".into())])
                .unwrap()
                == true
        {
            // The original traces omit the visibility event between panel visits.
            self.steel.update(&Event::Visible(true));
        }
        let actual = self.steel.update(&event);
        let expected = self.wasm.update(&event);
        if self.history_worker.is_none() {
            assert_eq!(actual, expected, "update decision for {event:?}");
        }
        self.commands()
    }
    pub(crate) fn key(&mut self, key: &str) -> Vec<Value> {
        self.event(Event::Key(key.parse().unwrap()))
    }
    pub(crate) fn pipe(&mut self, message: PipeMessage) {
        let actual = self.steel.pipe(&message);
        let expected = self.wasm.pipe(&message);
        if self.history_worker.is_none() {
            assert_eq!(actual, expected);
        }
        self.commands();
    }
    pub(crate) fn render(&mut self, rows: usize, cols: usize) {
        let expected = self.wasm.render(rows, cols);
        let actual = self.steel.render(rows, cols);
        self.commands();
        // Agents now uses a draggable column layout; agents_render checks its cells and mouse geometry.
        // Keep replaying these recordings for the host-command contract.
        if self.history_worker.is_some() {
            return;
        }
        let cells = |text: &str| {
            let mut grid = Grid::new(
                rows.max(1),
                cols.max(1),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                self.mode.style,
                false,
                !self.mode.capabilities.arrow_fonts,
                true,
                true,
                false,
            );
            vte::Parser::new().advance(
                &mut grid,
                text.replace("\n\r", "\n").replace('\n', "\n\r").as_bytes(),
            );
            grid.as_character_lines()
        };
        let left = cells(&expected);
        let right = cells(&actual);
        let first = left
            .iter()
            .zip(&right)
            .enumerate()
            .find_map(|(row, (a, b))| {
                a.iter()
                    .zip(b)
                    .enumerate()
                    .find(|(_, (a, b))| a != b)
                    .map(|(col, (a, b))| {
                        (
                            row,
                            col,
                            a.character,
                            a.styles.clone(),
                            b.character,
                            b.styles.clone(),
                        )
                    })
            });
        let plain = |lines: &[Vec<TerminalCharacter>]| {
            lines
                .iter()
                .map(|row| row.iter().map(|cell| cell.character).collect::<String>())
                .collect::<Vec<_>>()
        };
        assert!(
            left == right,
            "{rows}x{cols} first difference {first:?}\nWasm: {:?}\nSteel: {:?}",
            plain(&left),
            plain(&right)
        );
    }
    pub(crate) fn viewports(&mut self) {
        for (rows, cols) in [(30, 120), (30, 100), (20, 60), (12, 34), (6, 20)] {
            self.render(rows, cols);
        }
    }
    pub(crate) fn mode(&mut self, locked: bool) {
        let mut mode = ModeInfo {
            base_mode: Some(if locked {
                InputMode::Locked
            } else {
                InputMode::Normal
            }),
            ..Default::default()
        };
        mode.keybinds = super::reference_trace::legacy_keybinds();
        // The source is a map. Fix its iteration order before either guest sees
        // the event so captures replay the identical input in another process.
        mode.keybinds
            .sort_by_key(|(mode, _)| serde_json::to_string(mode).unwrap());
        for (_, bindings) in &mut mode.keybinds {
            // Replay the defaults captured before the latest-notification shortcut existed.
            bindings.retain(|(_, actions)| {
                !actions.contains(&yinz_types::input::actions::Action::ActivateLastNotification)
            });
            bindings.sort_by_key(|(key, _)| serde_json::to_string(key).unwrap());
        }
        self.event(Event::ModeUpdate(mode));
    }
}

// Native and Wasm builds use different serde_json object-map features. Claude
// interprets --settings as JSON, so object key order is not an argv difference.
// Every other argument remains byte-for-byte comparable.
fn normalize_command_settings(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("path")
                && object.contains_key("args")
                && object.contains_key("target")
                && serde_json::from_value::<yinz_types::command::CommandToRun>(Value::Object(
                    object.clone(),
                ))
                .is_ok()
            {
                if let Some(Value::Array(args)) = object.get_mut("args") {
                    for index in 1..args.len() {
                        if args[index - 1].as_str() == Some("--settings") {
                            if let Some(raw) = args[index].as_str() {
                                if let Ok(mut settings) = serde_json::from_str::<Value>(raw) {
                                    settings.sort_all_objects();
                                    args[index] = Value::String(settings.to_string());
                                }
                            }
                        }
                    }
                }
            }
            for child in object.values_mut() {
                normalize_command_settings(child);
            }
        },
        Value::Array(values) => {
            for child in values {
                normalize_command_settings(child);
            }
        },
        _ => {},
    }
}

#[test]
fn settings_normalization_only_changes_valid_settings_json_in_commands() {
    let mut command = serde_json::to_value(yinz_types::command::CommandToRun {
        path: "claude".into(),
        args: vec![
            "--settings".into(),
            r#"{ "z": [{"b":2,"a":1}], "a": true }"#.into(),
            "--prompt".into(),
            r#"{ "z": 2, "a": 1 }"#.into(),
            "--settings".into(),
            "{broken JSON".into(),
            "--settings".into(),
        ],
        ..Default::default()
    })
    .unwrap();
    normalize_command_settings(&mut command);
    assert_eq!(command["args"][1], r#"{"a":true,"z":[{"a":1,"b":2}]}"#);
    assert_eq!(command["args"][3], r#"{ "z": 2, "a": 1 }"#);
    assert_eq!(command["args"][5], "{broken JSON");
    assert_eq!(command["args"][6], "--settings");
    let mut unrelated = serde_json::json!({"args":["--settings", "{ \"z\": 2, \"a\": 1 }"]});
    let expected = unrelated.clone();
    normalize_command_settings(&mut unrelated);
    assert_eq!(unrelated, expected);
}
