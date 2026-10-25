//! Checked transcripts captured from the original guest engines before their removal.
//! Replay executes typed host calls against the test's independent backend.
use crate::response::{decode_response, DecodedResponse};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use yinz_types::data::{CommandType, PluginCommand};

pub(super) fn legacy_keybinds() -> yinz_types::data::KeybindsVec {
    let source = std::str::from_utf8(yinz_config::assets::DEFAULT_CONFIG).unwrap();
    yinz_config::config::Config::from_kdl(&legacy_preset(source), None)
        .unwrap()
        .keybinds
        .to_keybinds_vec()
}

pub(super) fn legacy_preset(source: &str) -> String {
    // Replay the recorded keymap, including the former manager shortcuts.
    let mut source = source
        .split_inclusive('\n')
        .filter(|line| !line.contains("TabGroup"))
        .collect::<String>()
        .replace("GoToTabInGroup", "GoToTab");
    // The recordings bound duplicate-agent with `LaunchOrFocusPlugin`; see
    // `legacy_duplicate_binding`.
    source = source.replace(
        "LaunchPlugin \"duplicate-agent\" {\n                floating true\n                overlay true\n                modal true\n            };",
        "LaunchOrFocusPlugin \"duplicate-agent\" {\n                floating true\n                overlay true\n                modal true\n                move_to_focused_tab true\n            };",
    );
    for mode in ["Normal", "Locked"] {
        for (key, plugin) in [
            ("w", "session-manager"),
            ("p", "plugin-manager"),
            ("l", "yinz:layout-manager"),
        ] {
            source = source.replace(
                &format!("bind \"{key}\" {{\n            SwitchToMode \"{mode}\";\n            ToggleCommandPalette\n        }}"),
                &format!("bind \"{key}\" {{\n            LaunchOrFocusPlugin \"{plugin}\" {{\n                floating true\n                move_to_focused_tab true\n            }};\n            SwitchToMode \"{mode}\"\n        }}"),
            );
        }
    }
    source
}

thread_local! {
    static SCENARIO: RefCell<Option<(String, BTreeMap<String, usize>)>> = const { RefCell::new(None) };
}

/// Explicit test identity; instance numbers count construction within this scope,
/// so nextest scheduling and thread names cannot affect fixture paths.
pub(crate) struct Scenario(Option<(String, BTreeMap<String, usize>)>);
impl Scenario {
    pub(crate) fn new(name: &str) -> Self {
        assert!(name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'));
        Self(SCENARIO.with(|slot| slot.replace(Some((name.into(), BTreeMap::new())))))
    }
}
impl Drop for Scenario {
    fn drop(&mut self) {
        SCENARIO.with(|slot| slot.replace(self.0.take()));
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HostCall {
    pub command: Value,
    pub reply: Value,
}
impl HostCall {
    pub(crate) fn new(command: &PluginCommand, reply: Option<&[u8]>) -> Self {
        let mut command_json = serde_json::to_value(command).unwrap();
        for name in ["Subscribe", "Unsubscribe"] {
            if let Some(events) = command_json.get_mut(name).and_then(Value::as_array_mut) {
                events.sort_by_key(Value::to_string);
            }
        }
        Self {
            command: command_json,
            reply: reply_value(command, reply),
        }
    }
}
fn reply_value(command: &PluginCommand, bytes: Option<&[u8]>) -> Value {
    let Some(bytes) = bytes else {
        return json!({"absent":true});
    };
    match decode_response(CommandType::from(command), bytes).transpose() {
        Ok(Some(DecodedResponse::Json(mut value))) => {
            // The original runtime recorded these typed fields as byte arrays. Compare their
            // full contents without making the current compact transport part of the fixture.
            if matches!(command, PluginCommand::GetPaneGitProbe { .. }) && value.is_object() {
                for field in ["metadata", "config"] {
                    value[field] = json!(yinz_steel::binary::decode(&value[field])
                        .expect("valid Git probe bytes in reference reply"));
                }
            }
            json!({"json":value})
        },
        Ok(Some(DecodedResponse::Bytes(value))) => json!({"bytes":value}),
        Ok(None) => json!({"protobuf":bytes}),
        Err(error) => json!({"error":error.to_string()}),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Step {
    input: Value,
    result: Value,
    calls: Vec<HostCall>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Transcript {
    version: u32,
    engine: String,
    plugin: String,
    configuration: Value,
    source_sha256: String,
    steps: Vec<Step>,
}
pub(crate) struct Trace {
    path: PathBuf,
    root: String,
    transcript: Transcript,
    cursor: usize,
}
impl Trace {
    pub(crate) fn new(engine: &str, plugin: &str, configuration: Value, root: &Path) -> Self {
        let root = root.to_str().unwrap().to_owned();
        let identity = SCENARIO.with(|slot| {
            let mut slot = slot.borrow_mut();
            let (scenario, instances) = slot
                .as_mut()
                .expect("reference replay requires an explicit Scenario guard");
            let instance = instances.entry(format!("{engine}-{plugin}")).or_default();
            let identity = format!("{scenario}.{plugin}.{instance}.json.gz");
            *instance += 1;
            identity
        });
        let directory = std::env::var_os("YINZ_REFERENCE_TRACE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/unit/plugin_tests/fixtures/reference")
            });
        let path = directory.join(engine).join(identity);
        let transcript: Transcript = serde_json::from_reader(GzDecoder::new(
            std::fs::File::open(&path)
                .unwrap_or_else(|error| panic!("read reference trace {}: {error}", path.display())),
        ))
        .unwrap();
        assert_eq!(transcript.version, 1);
        assert_eq!(transcript.engine, engine);
        assert_eq!(transcript.plugin, plugin);
        assert_eq!(
            transcript.configuration,
            replace(configuration, &root, "<fixture-root>"),
            "reference configuration {}:",
            path.display()
        );
        assert_eq!(
            transcript.source_sha256.len(),
            64,
            "reference source fingerprint"
        );
        assert!(
            transcript
                .source_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
            "reference source fingerprint must be SHA-256 hex"
        );
        Self {
            path,
            root,
            transcript,
            cursor: 0,
        }
    }
    pub(crate) fn replay(
        &mut self,
        input: Value,
        mut responder: impl FnMut(&PluginCommand) -> Option<Vec<u8>>,
    ) -> (Value, Vec<Value>) {
        let step = self.transcript.steps.get(self.cursor).unwrap_or_else(|| {
            panic!(
                "reference {} has no step {} for {input}",
                self.path.display(),
                self.cursor
            )
        });
        let mut input = replace(input, &self.root, "<fixture-root>");
        legacy_project_fields(&mut input);
        legacy_duplicate_binding(&mut input);
        assert!(
            input == step.input,
            "reference input at {} step {}: {}",
            self.path.display(),
            self.cursor,
            first_difference(&input, &step.input, "input")
        );
        let mut commands = vec![];
        for call in &step.calls {
            // The original agent service tinted shared-checkout frames. The screen's own 👯 title
            // marker replaced it, the host no longer has that command, and it never had a reply.
            if self.transcript.plugin == "agent-service"
                && call.command.get("SetRelatedPanes").is_some()
            {
                assert_eq!(call.reply, json!({"absent":true}));
                continue;
            }
            let recorded = match headless_adaptation(&self.transcript, &call.command) {
                Adapt::Keep => call.command.clone(),
                Adapt::Skip => {
                    assert_eq!(call.reply, json!({"absent":true}));
                    continue;
                },
                Adapt::Rewrite(command) => command,
            };
            let command_json = replace(recorded, "<fixture-root>", &self.root);
            let mut command: PluginCommand = serde_json::from_value(command_json.clone()).unwrap();
            if let PluginCommand::DataDefineEntity { def, .. } = &mut command
                && def.name == "project"
            {
                *def = project_data::store::project_def();
            }
            let reply = responder(&command);
            let mut actual = replace(
                reply_value(&command, reply.as_deref()),
                &self.root,
                "<fixture-root>",
            );
            legacy_project_fields(&mut actual);
            assert_eq!(
                actual,
                call.reply,
                "reference host reply at {} step {} for {command:?}",
                self.path.display(),
                self.cursor
            );
            commands.push(command_json);
        }
        self.cursor += 1;
        (
            replace(step.result.clone(), "<fixture-root>", &self.root),
            commands,
        )
    }
}
enum Adapt {
    Keep,
    Skip,
    Rewrite(Value),
}
/// Plugins that declare no pane run headless, so their ports dropped the calls that only
/// managed that pane. None of these calls had a reply.
fn headless_adaptation(transcript: &Transcript, command: &Value) -> Adapt {
    let named = |name: &str| command.get(name).is_some() || command == &json!(name);
    match transcript.plugin.as_str() {
        "file-finder" if named("HideSelf") => Adapt::Skip,
        "file-finder" if named("Subscribe") => {
            let mut command = command.clone();
            if let Some(events) = command.get_mut("Subscribe").and_then(Value::as_array_mut) {
                events.retain(|event| event != "PaneUpdate");
            }
            Adapt::Rewrite(command)
        },
        "new-agent" if named("HideSelf") => Adapt::Skip,
        "new-agent"
            if transcript.configuration.get("duplicate_focused") == Some(&json!("true"))
                && (named("SetSelectable") || named("RenamePluginPane")) =>
        {
            Adapt::Skip
        },
        _ => Adapt::Keep,
    }
}
fn first_difference(left: &Value, right: &Value, path: &str) -> String {
    match (left, right) {
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                if a != b {
                    return first_difference(a, b, &format!("{path}[{index}]"));
                }
            }
        },
        (Value::Object(a), Value::Object(b)) if a.len() == b.len() => {
            for (key, a) in a {
                if let Some(b) = b.get(key) {
                    if a != b {
                        return first_difference(a, b, &format!("{path}.{key}"));
                    }
                } else {
                    return format!("{path}: missing key {key}");
                }
            }
        },
        _ => {},
    }
    let short = |value: &Value| value.to_string().chars().take(240).collect::<String>();
    format!(
        "{path}: actual {} != recorded {}",
        short(left),
        short(right)
    )
}
fn replace(value: Value, from: &str, to: &str) -> Value {
    match value {
        Value::String(text) if !from.is_empty() => Value::String(text.replace(from, to)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| replace(item, from, to))
                .collect(),
        ),
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(key, value)| (key, replace(value, from, to)))
                .collect(),
        ),
        other => other,
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        assert_eq!(
            self.cursor,
            self.transcript.steps.len(),
            "unconsumed reference steps in {}",
            self.path.display()
        );
    }
}

fn fixture_for_test() -> Trace {
    Trace {
        path: PathBuf::from("test-reference.json.gz"),
        root: "/temporary/test-root".into(),
        transcript: Transcript {
            version: 1,
            engine: "wasm".into(),
            plugin: "fixture".into(),
            configuration: json!({}),
            source_sha256: "a".repeat(64),
            steps: vec![Step {
                input: json!(["update", "Visible"]),
                result: json!(true),
                calls: vec![HostCall::new(&PluginCommand::SetSelectable(true), None)],
            }],
        },
        cursor: 0,
    }
}

#[test]
fn reference_trace_replays_typed_host_effects_and_checks_replies() {
    let mut trace = fixture_for_test();
    let mut selectable = false;
    let (result, commands) = trace.replay(json!(["update", "Visible"]), |command| {
        if let PluginCommand::SetSelectable(value) = command {
            selectable = *value;
        }
        None
    });
    assert!(selectable);
    assert_eq!(result, json!(true));
    assert_eq!(commands, vec![json!({"SetSelectable":true})]);
    assert!(std::panic::catch_unwind(|| {
        let mut trace = fixture_for_test();
        trace.replay(json!(["update", "Visible"]), |_| Some(vec![]));
    })
    .is_err());
}

#[test]
fn reference_trace_rejects_changed_inputs_and_unconsumed_steps() {
    assert!(std::panic::catch_unwind(|| {
        let mut trace = fixture_for_test();
        trace.replay(json!(["update", "Other"]), |_| None);
    })
    .is_err());
    assert!(std::panic::catch_unwind(|| drop(fixture_for_test())).is_err());
    assert_eq!(
        first_difference(&json!({"nested":[1,2]}), &json!({"nested":[1,3]}), "input"),
        "input.nested[1]: actual 2 != recorded 3"
    );
}

#[test]
fn reference_trace_drops_only_the_pane_calls_of_headless_ports() {
    let transcript = |plugin: &str, configuration: Value| Transcript {
        version: 1,
        engine: "wasm".into(),
        plugin: plugin.into(),
        configuration,
        source_sha256: "a".repeat(64),
        steps: vec![],
    };
    let finder = transcript("file-finder", json!({}));
    assert!(matches!(
        headless_adaptation(&finder, &json!("HideSelf")),
        Adapt::Skip
    ));
    let Adapt::Rewrite(subscribe) = headless_adaptation(
        &finder,
        &json!({"Subscribe":["PaneClosed","PaneUpdate","PluginConfigurationChanged"]}),
    ) else {
        panic!("file-finder subscriptions are rewritten");
    };
    assert_eq!(
        subscribe,
        json!({"Subscribe":["PaneClosed","PluginConfigurationChanged"]})
    );
    let rename = json!({"RenamePluginPane":[1,"new agent"]});
    let selectable = json!({"SetSelectable":false});
    let duplicate = transcript("new-agent", json!({"duplicate_focused":"true"}));
    for command in [json!("HideSelf"), selectable.clone(), rename.clone()] {
        assert!(matches!(
            headless_adaptation(&duplicate, &command),
            Adapt::Skip
        ));
    }
    let modal = transcript("new-agent", json!({}));
    assert!(matches!(
        headless_adaptation(&modal, &json!("HideSelf")),
        Adapt::Skip
    ));
    for command in [selectable.clone(), rename] {
        assert!(matches!(headless_adaptation(&modal, &command), Adapt::Keep));
    }
    for plugin in ["strider", "agent-service"] {
        assert!(matches!(
            headless_adaptation(&transcript(plugin, json!({})), &json!("HideSelf")),
            Adapt::Keep
        ));
    }
    // The checked fixtures this adaptation touches: every file-finder trace and five
    // new-agent traces. A new or missing match means the adaptation drifted.
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/unit/plugin_tests/fixtures/reference/wasm");
    let mut adapted = vec![];
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        let transcript: Transcript =
            serde_json::from_reader(GzDecoder::new(std::fs::File::open(&path).unwrap())).unwrap();
        let changes = |command: &Value| match headless_adaptation(&transcript, command) {
            Adapt::Keep => false,
            Adapt::Skip => true,
            Adapt::Rewrite(rewritten) => rewritten != *command,
        };
        if transcript
            .steps
            .iter()
            .any(|step| step.calls.iter().any(|call| changes(&call.command)))
        {
            adapted.push(transcript.plugin);
        }
    }
    assert_eq!(adapted.iter().filter(|p| *p == "file-finder").count(), 20);
    assert_eq!(adapted.iter().filter(|p| *p == "new-agent").count(), 5);
    assert_eq!(adapted.len(), 25);
}

#[test]
fn reference_trace_compression_preserves_full_transcript_and_root_placeholder() {
    use flate2::{Compression, GzBuilder};
    let mut fixture = fixture_for_test();
    fixture.cursor = fixture.transcript.steps.len();
    let source = replace(
        serde_json::to_value(&fixture.transcript).unwrap(),
        "/temporary/test-root",
        "<fixture-root>",
    );
    let mut source: Transcript = serde_json::from_value(source).unwrap();
    source.steps[0] = Step {
        input: json!(["render", 2, 80]),
        result: json!("<fixture-root>/file 日本語"),
        calls: vec![],
    };
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    serde_json::to_writer(&mut encoder, &source).unwrap();
    let bytes = encoder.finish().unwrap();
    let transcript: Transcript = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    assert_eq!(
        transcript.steps[0].result,
        json!("<fixture-root>/file 日本語")
    );
    assert_eq!(transcript.source_sha256, "a".repeat(64));
    assert_eq!(transcript.steps[0].input, json!(["render", 2, 80]));
    assert_eq!(
        replace(
            transcript.steps[0].result.clone(),
            "<fixture-root>",
            "/temporary/test-root"
        ),
        json!("/temporary/test-root/file 日本語")
    );
}

/// The recordings were made while the shipped config bound duplicate-agent with
/// `LaunchOrFocusPlugin`. It is a `LaunchPlugin` now - every press duplicates, because the headless
/// instance a press starts closes itself and must not be taken for the next press - so a keymap
/// handed to a guest is compared in the recorded spelling. The bars find the key either way.
pub(crate) fn legacy_duplicate_binding(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let duplicate = object.get("LaunchPlugin").is_some_and(|launch| {
                launch.pointer("/plugin/Alias/name") == Some(&json!("duplicate-agent"))
            });
            if duplicate && let Some(Value::Object(launch)) = object.remove("LaunchPlugin") {
                let field = |name: &str| launch.get(name).cloned().unwrap_or(Value::Null);
                object.insert(
                    "LaunchOrFocusPlugin".into(),
                    json!({
                        "plugin": field("plugin"),
                        "should_float": field("should_float"),
                        "move_to_focused_tab": true,
                        "should_open_in_place": field("should_open_in_place"),
                        "close_replaced_pane": field("close_replaced_pane"),
                        "skip_cache": field("skip_cache"),
                        "tab_id": field("tab_id"),
                    }),
                );
            }
            for child in object.values_mut() {
                legacy_duplicate_binding(child);
            }
        },
        Value::Array(items) => items.iter_mut().for_each(legacy_duplicate_binding),
        _ => {},
    }
}

/// Original Git-only recordings predate the nullable provider column.
/// Provider tests check populated values; never erase a populated identity here.
pub(crate) fn legacy_project_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(index) =
                object
                    .get("columns")
                    .and_then(Value::as_array)
                    .and_then(|columns| {
                        columns
                            .iter()
                            .position(|column| column == "provider_identity")
                    })
            {
                if let Some(rows) = object.get_mut("rows").and_then(Value::as_array_mut) {
                    for row in rows {
                        let cells = row.as_array_mut().unwrap();
                        assert!(cells[index].is_null() || cells[index] == "Null");
                        cells.remove(index);
                    }
                }
                object
                    .get_mut("columns")
                    .unwrap()
                    .as_array_mut()
                    .unwrap()
                    .remove(index);
            }
            if object
                .get("provider_identity")
                .is_some_and(|v| v.is_null() || v == "Null")
            {
                object.remove("provider_identity");
            }
            if object
                .get("entity")
                .is_some_and(|entity| entity == "project")
                && let Some(values) = object.get_mut("values").and_then(Value::as_array_mut)
            {
                values.retain(|pair| {
                    !(pair[0] == "provider_identity" && (pair[1].is_null() || pair[1] == "Null"))
                });
            }
            for key in ["attrs", "attrs_added", "select"] {
                if let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) {
                    items.retain(|item| {
                        item != "provider_identity"
                            && item
                                .get("name")
                                .is_none_or(|name| name != "provider_identity")
                    });
                }
            }
            for child in object.values_mut() {
                legacy_project_fields(child);
            }
        },
        Value::Array(items) => {
            for item in items {
                legacy_project_fields(item);
            }
        },
        _ => {},
    }
}
