use crate::{
    plugin_tests::reference_trace::{Scenario, Trace},
    PaneId,
};
use serde_json::{json, Value};
use std::sync::{atomic::AtomicU64, Mutex};
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc},
};
use yinz_types::data::Event;
use yinz_types::data::PluginCommand;

struct Pair {
    reference: Reference,
    steel: yinz_steel::Runtime,
    commands: Arc<Mutex<Vec<(Value, u32)>>>,
    replies: Arc<Mutex<BTreeMap<String, Value>>>,
    tokens: [BTreeMap<String, String>; 2],
    last: [Vec<Value>; 2],
    now: Arc<AtomicU64>,
}

struct Reference {
    trace: Trace,
    replies: Arc<Mutex<BTreeMap<String, Value>>>,
    commands: Vec<(Value, u32)>,
    logs: Vec<String>,
}

impl Reference {
    fn new(name: &str, configuration: Value, replies: Arc<Mutex<BTreeMap<String, Value>>>) -> Self {
        let trace = Trace::new("deno", name, configuration, std::path::Path::new(""));
        Self {
            trace,
            replies,
            commands: vec![],
            logs: vec![],
        }
    }

    fn step(&mut self, operation: Value, now: u64) -> Value {
        // Recorded replies guard every assumption made by the original fixed-reply host.
        let input =
            json!({"operation":operation,"now":now,"replies":*self.replies.lock().unwrap()});
        let result = self
            .trace
            .replay(input, |_| {
                panic!("fixed JSON reference has no protobuf host calls")
            })
            .0;
        self.commands.extend(
            serde_json::from_value::<Vec<(Value, u32)>>(result["commands"].clone()).unwrap(),
        );
        self.logs
            .extend(serde_json::from_value::<Vec<String>>(result["logs"].clone()).unwrap());
        result["value"].clone()
    }
}

impl Pair {
    fn new(name: &str, configuration: Value) -> Self {
        Self::with_replies(name, configuration, BTreeMap::new())
    }

    fn with_replies(name: &str, configuration: Value, extra: BTreeMap<String, Value>) -> Self {
        let mut replies = BTreeMap::from([
            (
                "GetPluginIds".into(),
                json!({"plugin_id":0,"client_id":1,"max_session_name_bytes":80}),
            ),
            ("GetPaneInfo".into(), Value::Null),
        ]);
        replies.extend(extra);
        let replies = Arc::new(Mutex::new(replies));
        let mut reference = Reference::new(name, configuration.clone(), replies.clone());
        reference.step(json!({"load":configuration}), 0);
        let commands = Arc::new(Mutex::new(Vec::new()));
        let recorded = commands.clone();
        let answers = replies.clone();
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let mut steel = yinz_steel::Runtime::new_with_clock(
            move |command: Value| {
                let (command, client) = match command.get("__yinz_runtime") {
                    Some(envelope) => (
                        envelope["command"].clone(),
                        envelope["client"].as_u64().unwrap() as u32,
                    ),
                    None => (command, 1),
                };
                recorded.lock().unwrap().push((command.clone(), client));
                let name = command
                    .as_str()
                    .or_else(|| command.as_object()?.keys().next().map(String::as_str))
                    .unwrap();
                Ok(answers
                    .lock()
                    .unwrap()
                    .get(name)
                    .cloned()
                    .unwrap_or(Value::Null))
            },
            move || clock.load(Ordering::SeqCst),
        )
        .unwrap();
        let mut directories = vec![&yinz_steel_builtins::BUILTIN_STEEL_FILES];
        let mut sources = BTreeMap::new();
        while let Some(directory) = directories.pop() {
            directories.extend(directory.dirs());
            for file in directory.files() {
                sources.insert(
                    file.path().to_str().unwrap().to_owned(),
                    file.contents_utf8().unwrap().to_owned(),
                );
            }
        }
        let mut root =
            yinz_steel::modules::SourceRoot::embedded(&format!("{name}.scm"), sources).unwrap();
        steel
            .eval_with_source_root(
                yinz_steel_builtins::builtin_steel_plugin(name).unwrap(),
                &mut root,
            )
            .unwrap();
        steel.invoke("yinz-async-enter", vec![json!(1)]).unwrap();
        steel.load(configuration).unwrap();
        let mut pair = Self {
            reference,
            steel,
            commands,
            replies,
            tokens: Default::default(),
            last: Default::default(),
            now,
        };
        pair.compare_commands();
        pair
    }

    fn stub(&mut self, name: &str, reply: Value) {
        self.replies.lock().unwrap().insert(name.into(), reply);
    }

    fn compare_commands(&mut self) {
        let normalize = |commands: Vec<(Value, u32)>, tokens: &mut BTreeMap<String, String>| {
            commands
                .into_iter()
                .map(|(command, client)| {
                    let typed: PluginCommand = yinz_steel::binary::decode_command(command.clone())
                        .unwrap_or_else(|error| panic!("invalid host command {command}: {error}"));
                    let mut command = json!(typed);
                    if let Some(Value::Array(events)) = command.get_mut("Subscribe") {
                        events.sort_by_key(Value::to_string);
                        events.dedup();
                    }
                    if let Some(palette) = command.get_mut("CommandPalette") {
                        normalize_palette_tokens(palette, tokens);
                    }
                    if let Some(pick) = command.get_mut("Pick") {
                        for kind in ["Start", "Cancel"] {
                            if let Some(request) = pick.get_mut(kind) {
                                let next = format!("pick:{}", tokens.len());
                                let mapped = tokens
                                    .entry(format!("pick:{}", request["request_id"]))
                                    .or_insert(next);
                                request["request_id"] = json!(mapped);
                            }
                        }
                    }
                    if let Some(run) = command.get_mut("RunCommand") {
                        let context = run[3].as_object_mut().unwrap();
                        if let Some(token) = context
                            .remove("__yinz_async")
                            .or_else(|| context.remove("agent-exit"))
                        {
                            let next = format!("request:{}", tokens.len());
                            let mapped = tokens.entry(format!("request:{token}")).or_insert(next);
                            context.insert("request".into(), json!(mapped));
                        }
                    }
                    (command, client)
                })
                .collect::<Vec<_>>()
        };
        let reference = std::mem::take(&mut self.reference.commands);
        let actual = std::mem::take(&mut *self.commands.lock().unwrap());
        self.last = [
            reference
                .iter()
                .map(|(command, _)| command.clone())
                .collect(),
            actual.iter().map(|(command, _)| command.clone()).collect(),
        ];
        assert_eq!(
            normalize(actual, &mut self.tokens[1]),
            normalize(reference, &mut self.tokens[0]),
            "Steel/Deno host actions differ"
        );
    }

    fn event(&mut self, event: Value) {
        self.events([event.clone(), event]);
    }

    fn events(&mut self, events: [Value; 2]) {
        let [reference_event, event] = events;
        let reference = self.reference.step(
            json!({"update":reference_event}),
            self.now.load(Ordering::SeqCst),
        );
        let actual = self.steel.update(event.clone()).unwrap();
        assert_eq!(
            json!(actual),
            reference,
            "Steel/Deno update decision for {event}"
        );
        self.compare_commands();
    }

    fn at(&mut self, milliseconds: u64) {
        self.now.store(milliseconds, Ordering::SeqCst);
        self.event(json!({"Timer":0.0}));
    }

    fn command_reply(&mut self, code: i32, stdout: &str, stderr: &str) {
        let events = self.last.each_ref().map(|commands| {
            let run = commands
                .iter()
                .rev()
                .find_map(|command| command.get("RunCommand"))
                .unwrap();
            json!({"RunCommandResult":[code,stdout.as_bytes(),stderr.as_bytes(),run[3]]})
        });
        self.events(events);
    }

    fn palette_dispatch(&mut self, messages: [Value; 2]) {
        self.reference.step(
            json!({"palette":messages[0]}),
            self.now.load(Ordering::SeqCst),
        );
        self.steel.palette(messages[1].clone()).unwrap();
        self.compare_commands();
    }

    fn palette(&mut self, messages: [Value; 2]) -> [Value; 2] {
        self.palette_dispatch(messages);
        self.last.each_ref().map(|commands| {
            commands
                .iter()
                .rev()
                .find_map(|command| {
                    let palette = command.get("CommandPalette")?;
                    (palette["type"] == "reply").then(|| palette["result"].clone())
                })
                .expect("palette replies")
        })
    }

    fn render(&mut self, rows: usize, columns: usize) {
        let reference = self.reference.step(
            json!({"render":[rows,columns]}),
            self.now.load(Ordering::SeqCst),
        );
        let actual = self.steel.render(rows, columns).unwrap();
        let cells = |text: &str| {
            let mut grid = yinz_terminal::panes::grid::Grid::new(
                rows.max(1),
                columns.max(1),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                false,
                false,
                true,
                true,
                false,
            );
            vte::Parser::new().advance(&mut grid, text.replace('\n', "\n\r").as_bytes());
            grid.as_character_lines()
        };
        assert_eq!(
            cells(&actual),
            cells(reference.as_str().unwrap()),
            "Steel/Deno terminal cells at {rows}x{columns}\nSteel: {actual:?}\nDeno: {reference}"
        );
        self.compare_commands();
    }
}

// Only provider/callback capabilities are opaque; all user-visible fields and actions compare.
fn normalize_palette_tokens(value: &mut Value, tokens: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if ["registration", "handle", "source"].contains(&key.as_str()) && value.is_string()
                {
                    let kind = if key == "registration" {
                        "provider"
                    } else {
                        "callback"
                    };
                    let next = format!("{kind}:{}", tokens.len());
                    let mapped = tokens.entry(format!("{kind}:{value}")).or_insert(next);
                    *value = json!(mapped);
                } else {
                    normalize_palette_tokens(value, tokens);
                }
            }
        },
        Value::Array(items) => {
            for item in items {
                normalize_palette_tokens(item, tokens);
            }
        },
        _ => (),
    }
}

#[test]
fn agent_policy_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("agent_policy_deno_steel_runtime_parity");
    let mut pair = Pair::new("agent-policy", json!({}));
    for (point, payload) in [
        (
            "session_start",
            json!({"session_name":"test","client_id":1}),
        ),
        (
            "agent_waiting",
            json!({"pane_id":7,"agent":{"kind":"codex","detail":"needs an answer"}}),
        ),
        (
            "agent_waiting",
            json!({"pane_id":9,"agent":{"kind":"","detail":""}}),
        ),
    ] {
        pair.event(json!({"Hook":{"point":point,"payload":payload}}));
        let logs = std::mem::take(&mut pair.reference.logs);
        let actual = pair.steel.take_logs().unwrap();
        let prefix = format!("hook {point} ");
        let expected: Value =
            serde_json::from_str(logs.last().unwrap().strip_prefix(&prefix).unwrap()).unwrap();
        let actual: Value =
            serde_json::from_str(actual.trim().strip_prefix(&prefix).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }
    pair.render(1, 80);
}

#[test]
fn opencode_provider_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("opencode_provider_deno_steel_runtime_parity");
    let mut pair = Pair::new("agent-opencode", json!({}));
    pair.render(1, 80);
}

#[test]
fn context_menu_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("context_menu_deno_steel_runtime_parity");
    use yinz_types::{
        data::{ContextMenuContext, ContextMenuItem, ContextMenuLink},
        internal_ids::{ContextMenuItemId, ContextMenuRequestId},
    };
    let mut pair = Pair::new("context-menu", json!({}));
    for pane in [
        Value::Null,
        json!({"agent":null}),
        json!({"agent":{"kind":"codex","source":"Reported"}}),
    ] {
        pair.stub("GetPaneInfo", pane);
        for selection in [
            None,
            Some("hello".into()),
            Some(format!("  {}\n", "🙂".repeat(25))),
        ] {
            pair.event(json!(Event::ContextMenuRequested(Box::new(
                ContextMenuContext {
                    request_id: ContextMenuRequestId::new(73),
                    pane_id: PaneId::Terminal(crate::TerminalId::new(7)),
                    items: vec![
                        ContextMenuItem::separator(),
                        ContextMenuItem::new("Copy", vec![]).with_id(ContextMenuItemId::new(1)),
                        ContextMenuItem::separator(),
                        ContextMenuItem::new("Close pane", vec![])
                            .with_id(ContextMenuItemId::new(2)),
                        ContextMenuItem::separator(),
                    ],
                    selected_text: selection,
                    link: Some(ContextMenuLink {
                        matched: "https://example.test".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            ))));
        }
    }
    pair.event(json!({"RunCommandResult":[0,[],[],{"probe":"xdg-open"}]}));
    pair.event(json!({"ContextMenuItemSelected":{"item_context":{"action":"open_link","url":"https://example.test"}}}));
    pair.render(1, 80);
}

#[test]
fn agent_exit_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("agent_exit_deno_steel_runtime_parity");
    for configuration in [
        json!({}),
        json!({"agent_pane_id":"7","agent_cwd":"/work","agent_target":"ssh"}),
    ] {
        let mut pair = Pair::new("agent-exit", configuration);
        for (rows, columns) in [(10, 80), (5, 30), (1, 10)] {
            pair.render(rows, columns);
        }
        for key in ["Enter", "Esc"] {
            pair.event(json!({"Key":{"bare_key":key,"key_modifiers":[]}}));
        }
    }
}

#[test]
fn agent_exit_checkout_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("agent_exit_checkout_deno_steel_runtime_parity");
    for outcome in [
        "dirty", "unmerged", "merged", "changed", "partial", "timeout",
    ] {
        let mut pair = Pair::with_replies(
            "agent-exit",
            json!({
                "agent_pane_id":"7", "agent_cwd":"/repo/topic", "agent_target":"local",
            }),
            BTreeMap::from([(
                "ProjectConfigRead".into(),
                json!({"config":{"default_branch":"main"},"error":null}),
            )]),
        );
        let head = "0123456789012345678901234567890123456789";
        let listing = format!("worktree /repo\0HEAD {head}\0branch refs/heads/main\0\0worktree /repo/topic\0HEAD {head}\0branch refs/heads/topic\0\0");
        pair.command_reply(0, "/repo/topic\n", "");
        pair.command_reply(0, &listing, "");
        pair.render(24, 100);
        pair.event(json!({"Key":{"bare_key":"Delete","key_modifiers":[]}}));
        pair.command_reply(0, &listing, "");
        pair.render(24, 100);
        pair.event(json!({"Key":{"bare_key":"Delete","key_modifiers":[]}}));
        if outcome == "timeout" {
            pair.at(10_000);
        } else {
            pair.command_reply(128, "", "origin unavailable");
        }
        if outcome == "changed" {
            pair.command_reply(0, &listing.replace(head, &"1".repeat(40)), "");
        } else {
            pair.command_reply(0, &listing, "");
            pair.command_reply(
                0,
                if outcome == "dirty" {
                    " M tracked.txt\n"
                } else {
                    ""
                },
                "",
            );
            if outcome != "dirty" {
                pair.command_reply(
                    0,
                    if outcome == "unmerged" || outcome == "timeout" {
                        head
                    } else {
                        ""
                    },
                    "",
                );
                if outcome == "merged" || outcome == "partial" {
                    pair.command_reply(0, "", "");
                    pair.command_reply(
                        if outcome == "partial" { 128 } else { 0 },
                        "",
                        "branch changed concurrently",
                    );
                }
            }
        }
        pair.render(24, 100);
    }
}

fn palette_message(
    registrations: &[Value; 2],
    revision: u64,
    view: u64,
    messages: [Value; 2],
) -> [Value; 2] {
    std::array::from_fn(|index| {
        let mut message = messages[index].clone();
        message["scope"] = json!({"registration":registrations[index],"opening":1,"view":view,"revision":revision});
        message["context"] =
            json!({"clientId":1,"paneId":{"Terminal":4},"tabId":7,"currentProject":null});
        message
    })
}

#[test]
fn session_palette_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("session_palette_deno_steel_runtime_parity");
    for count in [2, 205] {
        let mut pair = Pair::new("session-palette", json!({}));
        let registrations = pair.last.each_ref().map(|commands| {
            commands
                .iter()
                .find_map(|command| {
                    let palette = command.get("CommandPalette")?;
                    (palette["type"] == "register").then(|| palette["registration"].clone())
                })
                .unwrap()
        });
        let live: Vec<_> = (0..count).map(|index| json!({
            "name":format!("session-{index:03}"), "is_current_session":index == 0, "connected_clients":1,
            "creation_time":{"secs":12,"nanos":0},
            "tabs":[{"tab_id":7,"position":0,"name":"shell"}],
            "panes":{"panes":{"0":[{"id":4,"is_plugin":false,"is_selectable":true,"is_suppressed":false,"title":"shell","exited":false,"exit_status":null}]}},
            "available_layouts":[{"BuiltIn":"default"}],
        })).collect();
        pair.stub("GetSessionList", json!({"live_sessions":live,"resurrectable_sessions":[["saved",{"secs":20,"nanos":0}]]}));
        let roots = pair.palette(palette_message(
            &registrations,
            2,
            0,
            std::array::from_fn(|_| json!({"type":"discover","source":null,"query":""})),
        ));
        for (index, root) in [
            "switch-session",
            "find-pane",
            "new-session",
            "rename",
            "details",
        ]
        .into_iter()
        .enumerate()
        {
            let messages = roots.each_ref().map(|reply| {
                let action = reply["Ok"]["actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|action| action["id"] == root)
                    .unwrap();
                json!({"type":"execute","handle":action["handle"],"values":null})
            });
            let views = pair.palette(palette_message(
                &registrations,
                3 + index as u64 * 3,
                0,
                messages,
            ));
            if views[0]["Ok"]["view"]["type"] == "actions" {
                let messages = views.each_ref().map(|reply| json!({"type":"discover","source":reply["Ok"]["view"]["source"],"query":""}));
                pair.palette(palette_message(
                    &registrations,
                    4 + index as u64 * 3,
                    1,
                    messages,
                ));
                let messages = views.each_ref().map(|reply| json!({"type":"discover","source":reply["Ok"]["view"]["source"],"query":"session-204"}));
                pair.palette(palette_message(
                    &registrations,
                    5 + index as u64 * 3,
                    1,
                    messages,
                ));
            }
        }
        pair.event(json!("BeforeClose"));
        let replies = pair.palette(palette_message(
            &registrations,
            100,
            0,
            std::array::from_fn(|_| json!({"type":"discover","source":null,"query":""})),
        ));
        assert_eq!(replies[0]["Err"], "Palette callback is no longer available");
    }
}

#[test]
fn configuration_palette_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("configuration_palette_deno_steel_runtime_parity");
    let setting = json!({"name":"mouse_mode","arguments":[true],"properties":{},"children":[]});
    let snapshot = json!({
        "action_properties":{},"action_children":{},"section_timing":{},"sections":[],"templates":{},
        "revision":"revision-one","notice":"Applied to this session","overrides":[],"actions":[],
        "catalog":[{"name":"mouse_mode","label":"mouse mode","kind":"boolean","choices":["Enabled","Disabled"],"minimum":null,"maximum":null,"help":"Mouse","timing":"Applies to this session"}],
        "saved":[setting],"clients":{"1":[setting]},
    });
    let mut pair = Pair::with_replies(
        "configuration-palette",
        json!({}),
        BTreeMap::from([
            ("InspectConfiguration".into(), snapshot.clone()),
            ("EditConfiguration".into(), snapshot),
        ]),
    );
    let registrations = pair.last.each_ref().map(|commands| {
        commands
            .iter()
            .find_map(|command| {
                let palette = command.get("CommandPalette")?;
                (palette["type"] == "register").then(|| palette["registration"].clone())
            })
            .unwrap()
    });
    // Main's configuration submenu and combined scalar form extend the original reference flow.
    let main = pair.palette(palette_message(
        &registrations,
        1,
        0,
        std::array::from_fn(|_| json!({"type":"discover","source":null,"query":""})),
    ));
    assert_eq!(main[0]["Ok"]["actions"][0]["label"], "Configuration…");
    let messages = main.each_ref().map(|reply| {
        json!({"type":"execute","handle":reply["Ok"]["actions"][0]["handle"],"values":null})
    });
    let submenu = pair.palette(palette_message(&registrations, 2, 0, messages));
    let messages = submenu.each_ref().map(|reply| {
        json!({"type":"discover","source":reply["Ok"]["view"]["source"],"query":"mouse_mode"})
    });
    let roots = pair.palette(palette_message(&registrations, 3, 1, messages));
    let messages = roots.each_ref().map(|reply| {
        let action = reply["Ok"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["id"] == "mouse_mode")
            .unwrap();
        json!({"type":"execute","handle":action["handle"],"values":null})
    });
    let forms = pair.palette(palette_message(&registrations, 4, 1, messages));
    let messages = forms.each_ref().map(|reply| json!({"type":"execute","handle":reply["Ok"]["view"]["handle"],"values":{"value":"Disabled","destination":"Apply to session"}}));
    let results = pair.palette(palette_message(&registrations, 5, 2, messages));
    assert_eq!(results[0]["Ok"]["type"], "complete");
}

#[test]
fn session_picker_deno_steel_runtime_parity() {
    let _scenario = Scenario::new("session_picker_deno_steel_runtime_parity");
    for selected in [json!("/chosen"), Value::Null, json!("close while picking")] {
        let mut pair = Pair::new("session-palette", json!({}));
        let registrations = pair.last.each_ref().map(|commands| {
            commands
                .iter()
                .find_map(|command| {
                    let palette = command.get("CommandPalette")?;
                    (palette["type"] == "register").then(|| palette["registration"].clone())
                })
                .unwrap()
        });
        pair.stub("GetSessionList", json!({"live_sessions":[{"is_current_session":true,"available_layouts":[{"BuiltIn":"default"}]}],"resurrectable_sessions":[]}));
        let roots = pair.palette(palette_message(
            &registrations,
            1,
            0,
            std::array::from_fn(|_| json!({"type":"discover","source":null,"query":""})),
        ));
        let messages = roots.each_ref().map(|reply| {
            let action = reply["Ok"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|action| action["id"] == "new-session")
                .unwrap();
            json!({"type":"execute","handle":action["handle"],"values":null})
        });
        let forms = pair.palette(palette_message(&registrations, 2, 0, messages));
        let messages = forms.each_ref().map(|reply| json!({"type":"execute","handle":reply["Ok"]["view"]["actions"][0]["handle"],"values":{"name":"kept"}}));
        let configured = pair.palette(palette_message(&registrations, 3, 1, messages));
        let messages = configured.each_ref().map(|reply| {
            let browse = reply["Ok"]["view"]["actions"].as_array().unwrap().iter().find(|action| action["id"] == "browse:directory").unwrap();
            json!({"type":"execute","handle":browse["handle"],"values":{"name":"edited","layout":"Default","directory":"/original"}})
        });
        pair.palette_dispatch(palette_message(&registrations, 4, 2, messages));
        if selected == "close while picking" {
            pair.event(json!("BeforeClose"));
        } else {
            let events = pair.last.each_ref().map(|commands| {
                let request = commands
                    .iter()
                    .find_map(|command| command.get("Pick")?.get("Start"))
                    .unwrap();
                json!({"PickResult":{"request_id":request["request_id"],"result":{"Ok":selected}}})
            });
            pair.events(events);
        }
    }
}
