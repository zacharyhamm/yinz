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
use yinz_types::internal_ids::PluginId;
use yinz_types::own_pane::own_pane_command;

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
        PluginCommand::GetLocalTime { epoch_ms } => ProtobufGetLocalTimeResponse {
            text: chrono::DateTime::from_timestamp_millis(epoch_ms.get())
                .map(|at| at.format("%b %-d, %Y, %-I:%M %p UTC").to_string())
                .unwrap_or_default(),
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
    fn instance(
        name: &str,
        configuration: BTreeMap<String, String>,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
        worker: Option<&str>,
    ) -> Self {
        let source = yinz_steel_builtins::fixture_source(name).unwrap();
        Self::instance_of_source(name, &source, configuration, responder, clock, worker)
    }
    /// A builtin's runtime evaluating `source` in place of the shipped entry file, resolving its
    /// imports as the shipped one does.
    pub(crate) fn from_source(
        name: &str,
        source: &str,
        configuration: BTreeMap<String, String>,
        responder: Responder,
    ) -> Self {
        Self::instance_of_source(name, source, configuration, responder, None, None)
    }
    fn instance_of_source(
        name: &str,
        source: &str,
        configuration: BTreeMap<String, String>,
        responder: Responder,
        clock: Option<Arc<AtomicU64>>,
        worker: Option<&str>,
    ) -> Self {
        // As the host's gate does: a plugin that declares no pane for this configuration runs
        // headless, and every command that needs its own pane is refused. The plugin's own id is
        // the one its `GetPluginIds` reply gives it.
        let headless = !yinz_steel::manifest::pane_declaration(source)
            .unwrap()
            .applies(&configuration);
        let me = responder(&PluginCommand::GetPluginIds)
            .and_then(|reply| ProtobufPluginIds::decode(reply.as_slice()).ok())
            .and_then(|ids| u32::try_from(ids.plugin_id).ok())
            .map_or(PluginId::new(u32::MAX), PluginId::new);
        let plugin = name.to_owned();
        let commands = Commands::default();
        let recorded = commands.clone();
        let host = move |value: Value| {
            let value = value
                .get("__yinz_runtime")
                .map(|envelope| envelope["command"].clone())
                .unwrap_or(value);
            let command = yinz_steel::binary::decode_command(value)?;
            if headless && let Some(refused) = own_pane_command(&command, me) {
                panic!(
                    "{plugin} declares no pane, so the host refuses its '{refused}': {command:?}"
                );
            }
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
        runtime.eval_with_source_root(source, &mut root).unwrap();
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
    service: bool,
    unrecorded_hook_publications: u64,
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
        let mut pair = Self {
            service: name == "agent-service",
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
            unrecorded_hook_publications: 0,
            _root: root,
        };
        pair.commands();
        pair
    }
    pub(crate) fn commands(&mut self) -> Vec<Value> {
        self.commands_for_event(false)
    }
    fn commands_for_event(&mut self, hook_event: bool) -> Vec<Value> {
        self.compare_commands_for_event(hook_event, |_, _| {})
    }
    pub(crate) fn compare_commands_for_event(
        &mut self,
        hook_event: bool,
        adapt: impl FnOnce(&mut Vec<Value>, &mut Vec<Value>),
    ) -> Vec<Value> {
        let mut actual = self.steel.take_commands();
        let expected = self.wasm.take_commands();
        let is_snapshot = |command: &Value| {
            command.pointer("/MessageToPlugin/message_name")
                == Some(&serde_json::json!("agent_snapshot"))
        };
        // Receipt timestamps now publish even when a hook leaves the legacy snapshot unchanged.
        // Keep comparing every recorded publication, accounting for these extra sequence values.
        if hook_event && !expected.iter().any(is_snapshot) {
            let mut publication = None;
            actual.retain(|command| {
                if !is_snapshot(command) {
                    return true;
                }
                let seq = command
                    .pointer("/MessageToPlugin/message_args/seq")
                    .unwrap();
                if let Some(previous) = &publication {
                    assert_eq!(
                        seq, previous,
                        "one hook publication shared by all subscribers"
                    );
                }
                publication = Some(seq.clone());
                false
            });
            self.unrecorded_hook_publications += u64::from(publication.is_some());
        }
        let normalize = |commands: Vec<Value>, sequence_offset: u64, current: bool| {
            let commands = if self.service {
                normalize_run_persistence(commands, current)
            } else {
                commands
            };
            commands
                .into_iter()
                // Provider-chain and cache tests cover these new service effects.
                .filter(|command| {
                    command != "GetProjectProviders"
                        && command.get("PublishProjectLocations").is_none()
                        && command.get("Subscribe")
                            != Some(&serde_json::json!(["ProjectProvidersChanged"]))
                })
                .map(|mut command| {
                    if let Some(Value::Array(events)) = command.get_mut("Subscribe") {
                        events.sort_by_key(Value::to_string);
                    }
                    if let Some(preset) = command.pointer_mut("/Reconfigure/0")
                        && let Some(source) = preset.as_str()
                    {
                        *preset = Value::String(super::reference_trace::legacy_preset(source));
                    }
                    // Live hook receipt times postdate the recordings. The builtin service's
                    // timestamp test checks their publication and preservation separately.
                    if command.pointer("/MessageToPlugin/message_name")
                        == Some(&serde_json::json!("agent_snapshot"))
                        && let Some(Value::String(payload)) =
                            command.pointer_mut("/MessageToPlugin/message_payload")
                    {
                        let mut snapshot: Value = serde_json::from_str(payload).unwrap();
                        for field in ["panes", "suppressed_panes"] {
                            if let Some(panes) =
                                snapshot.get_mut(field).and_then(Value::as_array_mut)
                            {
                                for pane in panes {
                                    pane.as_object_mut().unwrap().remove("last_hook_at_ms");
                                }
                            }
                        }
                        *payload = snapshot.to_string();
                        let seq = command
                            .pointer_mut("/MessageToPlugin/message_args/seq")
                            .unwrap();
                        let number = seq.as_str().unwrap().parse::<u64>().unwrap();
                        *seq =
                            Value::String(number.checked_sub(sequence_offset).unwrap().to_string());
                    }
                    normalize_command_settings(&mut command);
                    command
                })
                .collect::<Vec<_>>()
        };
        let mut expected = normalize(expected, 0, false);
        let mut actual = normalize(actual, self.unrecorded_hook_publications, true);
        adapt(&mut actual, &mut expected);
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
        let actual = self.steel.update(&event);
        let expected = self.wasm.update(&event);
        assert_eq!(actual, expected, "update decision for {event:?}");
        self.commands_for_event(matches!(event, Event::AgentEvent(_)))
    }
    pub(crate) fn key(&mut self, key: &str) -> Vec<Value> {
        self.event(Event::Key(key.parse().unwrap()))
    }
    pub(crate) fn pipe(&mut self, message: PipeMessage) {
        let actual = self.steel.pipe(&message);
        let expected = self.wasm.pipe(&message);
        assert_eq!(actual, expected);
        self.commands();
    }
    pub(crate) fn render(&mut self, rows: usize, cols: usize) {
        let expected = self.wasm.render(rows, cols);
        let actual = self.steel.render(rows, cols);
        self.commands();
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

// Compare the original ordered mutations while checking the new revision contract separately.
fn normalize_run_persistence(commands: Vec<Value>, current: bool) -> Vec<Value> {
    let mut normalized = Vec::new();
    for mut command in commands {
        if current
            && command.pointer("/DataFetch/request/entity") == Some(&serde_json::json!("agent_run"))
        {
            assert_eq!(
                command["DataFetch"]["request"]["select"],
                serde_json::json!(["uid", "rev", "payload", "host_id", "yinz_session_name"])
            );
            command["DataFetch"]["request"]["select"] = serde_json::json!(["payload"]);
        }
        let Some(mutations) = command
            .pointer_mut("/DataApply/mutations")
            .and_then(Value::as_array_mut)
        else {
            normalized.push(command);
            continue;
        };
        let run_batch = mutations.iter().any(|mutation| {
            mutation
                .pointer("/Put/entity")
                .or_else(|| mutation.pointer("/Update/entity"))
                == Some(&serde_json::json!("agent_run"))
        });
        if !run_batch {
            normalized.push(command);
            continue;
        }
        for mutation in mutations.iter_mut() {
            let entity = mutation
                .pointer("/Put/entity")
                .or_else(|| mutation.pointer("/Update/entity"));
            assert!(
                entity == Some(&serde_json::json!("agent_run"))
                    || entity == Some(&serde_json::json!("agent_run_entry")),
                "run batches contain only metadata and timeline writes"
            );
            if entity == Some(&serde_json::json!("agent_run")) {
                let operation = if mutation.get("Put").is_some() {
                    "Put"
                } else {
                    "Update"
                };
                let body = &mut mutation[operation];
                if current {
                    assert!(
                        body["expect_rev"].as_u64().is_some(),
                        "every service run write carries its author's revision"
                    );
                }
                body["expect_rev"] = Value::Null;
            }
        }
        let writes = std::mem::take(mutations);
        for mutation in writes {
            let mut single = command.clone();
            single["DataApply"]["mutations"] = serde_json::json!([mutation]);
            normalized.push(single);
        }
    }
    normalized
}

#[test]
fn run_persistence_comparison_preserves_writes_and_requires_current_guards() {
    use serde_json::json;
    let first = json!({"Put":{"entity":"agent_run","uid":"one","values":[["payload",{"Text":"first"}]],"expect_rev":null}});
    let entry = json!({"Put":{"entity":"agent_run_entry","uid":"one#1","values":[["text",{"Text":"entry"}]],"expect_rev":null}});
    let second = json!({"Put":{"entity":"agent_run","uid":"two","values":[["payload",{"Text":"second"}]],"expect_rev":null}});
    let batch = |mutations: Vec<Value>| json!({"DataApply":{"namespace":"shared:yinz","mutations":mutations}});
    let legacy = vec![
        batch(vec![first.clone(), entry.clone(), second.clone()]),
        json!("GetPluginIds"),
    ];
    let mut guarded_first = first.clone();
    guarded_first["Put"]["expect_rev"] = json!(0);
    let mut guarded_second = second.clone();
    guarded_second["Put"]["expect_rev"] = json!(4);
    let current = vec![
        batch(vec![guarded_first, entry]),
        batch(vec![guarded_second]),
        json!("GetPluginIds"),
    ];
    assert_eq!(
        normalize_run_persistence(current, true),
        normalize_run_persistence(legacy, false)
    );
    assert!(
        std::panic::catch_unwind(|| normalize_run_persistence(vec![batch(vec![first])], true))
            .is_err()
    );
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
                && let Some(Value::Array(args)) = object.get_mut("args")
            {
                for index in 1..args.len() {
                    if args[index - 1].as_str() == Some("--settings")
                        && let Some(raw) = args[index].as_str()
                        && let Ok(mut settings) = serde_json::from_str::<Value>(raw)
                    {
                        settings.sort_all_objects();
                        args[index] = Value::String(settings.to_string());
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
