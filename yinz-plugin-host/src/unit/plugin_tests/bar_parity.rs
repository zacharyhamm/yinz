use super::reference_trace::Trace;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use yinz_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter, AnsiCode};
use yinz_types::{
    data::{Event, InputMode, ModeInfo, PluginCommand},
    input::{actions::Action, layout::RunPluginOrAlias},
    units::FractionalSeconds,
};

pub(super) const NOW: u64 = 1_757_078_399_750; // Just before a UTC minute, with a deterministic offset.

pub(super) struct Pair {
    deno_commands: Vec<(Value, u32)>,
    trace: Trace,
    pub(super) steel: yinz_steel::Runtime,
    steel_commands: Arc<Mutex<Vec<Value>>>,
    steel_replies: Arc<Mutex<BTreeMap<String, Value>>>,
    pub(super) now: Arc<AtomicU64>,
    pub(super) offset: Arc<std::sync::atomic::AtomicI32>,
    pub(super) commands: Arc<Mutex<Vec<Value>>>,
    pub(super) mode: ModeInfo,
    pub(super) cache: tempfile::TempDir,
    status_bar: bool,
    tab_bar: bool,
    compact_bar: bool,
}

impl Pair {
    fn reference(&mut self, input: Value) -> (Value, Value) {
        let input = json!([
            input,
            self.now.load(Ordering::SeqCst),
            self.offset.load(Ordering::SeqCst)
        ]);
        let (result, _) = self.trace.replay(input, |_| {
            panic!("bar reference uses its deterministic JSON host")
        });
        *self.commands.lock().unwrap() =
            serde_json::from_value(result["commands"].clone()).unwrap();
        self.deno_commands.extend(
            serde_json::from_value::<Vec<(Value, u32)>>(result["deno_commands"].clone()).unwrap(),
        );
        if let Some(cache) = result["cache"].as_str() {
            std::fs::write(self.cache.path().join("status-bar-tips.cache"), cache).unwrap();
        }
        (result["wasm"].clone(), result["deno"].clone())
    }
    pub(super) fn set_offset(&mut self, offset: i32) {
        self.offset.store(offset, Ordering::SeqCst);
    }
    pub(super) fn reload_tip(
        &mut self,
        cache: Value,
        configuration: &BTreeMap<String, String>,
    ) -> String {
        std::fs::write(
            self.cache.path().join("status-bar-tips.cache"),
            cache.to_string(),
        )
        .unwrap();
        let (chosen, _) = self.reference(json!(["reload_tip", cache, configuration]));
        let chosen = chosen.as_str().unwrap().to_owned();
        self.reload_steel_tip(&chosen, configuration);
        self.compare_commands();
        chosen
    }
    pub(super) fn new(classic: bool) -> Self {
        Self::builtin(
            "status-bar",
            BTreeMap::from([("classic".to_owned(), classic.to_string())]),
            "",
        )
    }

    pub(super) fn builtin(
        name: &str,
        configuration: BTreeMap<String, String>,
        hostname: &str,
    ) -> Self {
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(
            cache.path().join("status-bar-tips.cache"),
            r#"{"yinz_version":"test","cached_data":{"quicknav":1}}"#,
        )
        .unwrap();
        let now = Arc::new(AtomicU64::new(NOW));
        let commands = Arc::new(Mutex::new(Vec::new()));
        let offset = Arc::new(std::sync::atomic::AtomicI32::new(19_800));
        let trace = Trace::new("bars", name, json!([configuration, hostname]), cache.path());
        let steel_commands = Arc::new(Mutex::new(Vec::new()));
        let steel_replies = Arc::new(Mutex::new(BTreeMap::from([
            (
                "GetPluginIds".to_owned(),
                json!({"hostname":hostname,"plugin_id":0,"client_id":0}),
            ),
            ("GetYinzVersion".to_owned(), json!("test")),
            (
                "GetSessionList".to_owned(),
                json!({"live_sessions":[{"is_current_session":true,"plugins":{"42":{"location":"yinz:agent-service"}}}]}),
            ),
        ])));
        let recorded = steel_commands.clone();
        let replies = steel_replies.clone();
        let zone = offset.clone();
        let mut steel = yinz_steel::Runtime::new(move |command: Value| {
            let typed = yinz_steel::binary::decode_command(command.clone()).unwrap();
            recorded.lock().unwrap().push(command.clone());
            if matches!(typed, PluginCommand::GetLocalUtcOffset { .. }) {
                return Ok(json!(zone.load(Ordering::SeqCst)));
            }
            let name = command
                .as_str()
                .or_else(|| {
                    command
                        .as_object()
                        .and_then(|c| c.keys().next().map(String::as_str))
                })
                .unwrap();
            Ok(replies
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or(Value::Null))
        })
        .unwrap();
        steel.eval(&format!("(define parity-clock (vector {NOW})) (define (yinz-now-ms) (vector-ref parity-clock 0))
            (define (yinz-wall-clock-ms) (vector-ref parity-clock 0))
            (define parity-random (vector 0)) (define (yinz-random-integer n) (vector-ref parity-random 0))
            (define (parity-choose-tip chosen cache)
              (vector-set! parity-random 0 (let loop ((names (sort (hash-keys->list (hash-ref cache \"cached_data\")) string<?)) (index 0))
                (if (equal? (car names) chosen) index (loop (cdr names) (+ index 1))))))")).unwrap();
        let mut root = yinz_steel::modules::SourceRoot::file(
            &yinz_steel_builtins::asset_dir().join(format!("{name}.scm")),
            None,
        )
        .unwrap();
        steel
            .eval_with_source_root(
                &yinz_steel_builtins::fixture_source(name).unwrap(),
                &mut root,
            )
            .unwrap();
        if name == "tab-bar" {
            // Replay the recorded 40-column cap; builtin tests cover the current 45-column cap.
            steel
                .eval(
                    "(define parity-tab-part tab-part)
                     (set! tab-part (lambda (tab alternate budget)
                       (parity-tab-part tab alternate (min 40 budget))))",
                )
                .unwrap();
        }
        steel.load(json!(configuration)).unwrap();
        let mut pair = Self {
            deno_commands: vec![],
            trace,
            steel,
            steel_commands,
            steel_replies,
            now,
            offset,
            commands,
            mode: ModeInfo::default(),
            cache,
            status_bar: name == "status-bar",
            tab_bar: name == "tab-bar",
            compact_bar: name == "compact-bar",
        };
        pair.reference(json!(["load"]));
        pair.compare_commands();
        pair
    }

    pub(super) fn compare_commands(&mut self) {
        // Cache IO is WASI in the reference and SDK commands in Deno; compare the shared host actions.
        let filter = |command: &Value| {
            let name = command
                .as_str()
                .or_else(|| {
                    command
                        .as_object()
                        .and_then(|c| c.keys().next().map(String::as_str))
                })
                .unwrap();
            !self.status_bar
                || !["FsMetadata", "FsRead", "FsWrite", "GetYinzVersion"].contains(&name)
        };
        let expected: Vec<_> = self
            .commands
            .lock()
            .unwrap()
            .drain(..)
            .filter(&filter)
            .collect();
        let actual: Vec<_> = self
            .deno_commands
            .drain(..)
            .map(|(c, _)| c)
            .filter(filter)
            .collect();
        // Subscription order is a set at the host boundary.
        let normalize = |commands: Vec<Value>| {
            commands
                .into_iter()
                .map(|mut c| {
                    if let Some(Value::Array(events)) = c.get_mut("Subscribe") {
                        if self.status_bar {
                            events
                                .retain(|event| event != "PaneUpdate" && event != "SessionUpdate");
                        }
                        events.sort_by_key(Value::to_string);
                    }
                    if let Some(timeout) = c.get_mut("SetTimeout") {
                        *timeout = json!(timeout.as_f64().unwrap());
                    }
                    c
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            normalize(actual),
            normalize(expected.clone()),
            "Deno/Wasm host commands differ"
        );
        // Steel tracks outstanding timers; its scheduling is covered by the status-bar tests.
        let shared_command = |command: &Value| {
            !self.status_bar
                || (command.get("SetTimeout").is_none()
                    && command != "GetSessionList"
                    && command.get("MessageToPlugin").is_none())
        };
        let steel = self
            .steel_commands
            .lock()
            .unwrap()
            .drain(..)
            .filter(filter)
            .filter(&shared_command)
            .collect();
        assert_eq!(
            normalize(steel),
            normalize(expected.into_iter().filter(shared_command).collect()),
            "Steel/Wasm host commands differ"
        );
    }

    pub(super) fn event(&mut self, event: Event) {
        if let Event::ModeUpdate(mode) = &event {
            self.mode = mode.clone();
        }
        let (expected, actual) = self.reference(json!(["update", event]));
        let expected = expected.as_bool().unwrap();
        assert_eq!(actual, json!(expected), "update decision for {event:?}");
        let timer_before = (self.status_bar && matches!(event, Event::Timer(_))).then(|| {
            (
                self.steel.value("state").unwrap()["clock"].clone(),
                self.steel
                    .invoke("hint-until", vec![])
                    .unwrap()
                    .as_u64()
                    .unwrap(),
            )
        });
        // Frozen traces supplied hook counts through PaneUpdate. Feed the same values through
        // the new pipe to retain their rendering assertions; service classification has its own tests.
        let steel = if let Event::PaneUpdate(manifest) = &event
            && self.status_bar
        {
            let (mut busy, mut waiting, mut idle) = (0, 0, 0);
            for agent in manifest
                .panes
                .values()
                .flatten()
                .filter_map(|pane| pane.agent.as_ref())
            {
                use yinz_types::agent::AgentState;
                match agent.state {
                    AgentState::Exited => {},
                    AgentState::Idle => idle += 1,
                    AgentState::Waiting if agent.wait_needs_a_person => waiting += 1,
                    _ => busy += 1,
                }
            }
            let seq = self.steel.value("service-seq").unwrap().as_u64().unwrap() + 1;
            self.steel.pipe(json!({"source":{"Plugin":42},"name":"agent_counts",
                "payload":json!({"total":busy+waiting+idle,"busy":busy,"waiting":waiting,"idle":idle,"held":0,"exited":0,"unknown":0}).to_string(),
                "args":{"seq":seq.to_string()},"is_private":true})).unwrap()
        } else {
            self.steel
                .update(current_bar_event(&event, self.status_bar))
                .unwrap()
        };
        // The frozen reference animates; Steel redraws timers only for clock or hint changes.
        let expected = if let Some((clock, hint)) = timer_before {
            clock != self.steel.value("state").unwrap()["clock"]
                || (hint != 0 && hint <= self.now.load(Ordering::SeqCst))
        } else {
            expected
        };
        assert_eq!(steel, expected, "Steel update decision for {event:?}");
        self.compare_commands();
    }

    pub(super) fn at(&mut self, now: u64) {
        self.now.store(now, Ordering::SeqCst);
        self.steel
            .eval(&format!("(vector-set! parity-clock 0 {now})"))
            .unwrap();
        self.event(Event::Timer(FractionalSeconds::new(0.0)));
    }

    pub(super) fn pipe(&mut self, message: yinz_types::data::PipeMessage) {
        let (expected, actual) = self.reference(json!(["pipe", message]));
        let expected = expected.as_bool().unwrap();
        assert_eq!(actual, json!(expected), "pipe decision for {message:?}");
        assert_eq!(
            self.steel.pipe(json!(message)).unwrap(),
            expected,
            "Steel pipe decision for {message:?}"
        );
        self.compare_commands();
    }

    pub(super) fn render(&mut self, rows: usize, cols: usize) -> String {
        self.render_cells(rows, cols)
            .iter()
            .flat_map(|row| row.iter().map(|c| c.character))
            .collect()
    }

    pub(super) fn render_cells(&mut self, rows: usize, cols: usize) -> Vec<Vec<TerminalCharacter>> {
        let (expected, actual) = self.reference(json!(["render", rows, cols]));
        let expected = expected.as_str().unwrap();
        let actual = actual.as_str().unwrap().replace('\n', "\n\r");
        // Recorded tab bars have one row; group rows have separate builtin and whole-app tests.
        let steel_rows = if self.tab_bar { rows.min(1) } else { rows };
        let steel = self
            .steel
            .render(steel_rows, cols)
            .unwrap()
            .replace('\n', "\n\r");
        self.compare_commands();
        let cells = |text: &str| -> Vec<Vec<TerminalCharacter>> {
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
            vte::Parser::new().advance(&mut grid, text.as_bytes());
            grid.as_character_lines()
        };
        let left = cells(expected);
        let right = cells(&actual);
        let steel_cells = cells(&steel);
        // The Agents recording contains the removed Move panel hint and labels the
        // picker launch "Other action" in the compact tooltip. Builtin bar tests
        // cover the current hints. Keep command/geometry comparisons above, and
        // retain cell comparisons for custom generic LaunchPlugin payload fixtures.
        let legacy_picker_hint = self.compact_bar
            && self.mode.keybinds.iter().any(|(mode, keys)| {
                *mode == InputMode::Agent
                    && keys.iter().flat_map(|(_, actions)| actions).any(|action| {
                        matches!(action, Action::LaunchOrFocusPlugin {
                            plugin: RunPluginOrAlias::Alias(alias), ..
                        } if alias.name == "agents")
                    })
            });
        if (self.status_bar || legacy_picker_hint) && self.mode.mode == InputMode::Agent {
            assert_eq!(left, right);
            return steel_cells;
        }
        let mut steel_expected = left.clone();
        if self.status_bar && rows == 1 && self.steel.value("state").unwrap()["classic"] == false {
            // Preserve historical layout/style coverage while replacing the retired animation.
            for row in &mut steel_expected {
                for i in 0..row.len() {
                    let glyph = row[i].character;
                    if "⣾⣽⣻⢿⡿⣟⣯⣷".contains(glyph) {
                        row[i].character = '●';
                    } else if "|/-\\".contains(glyph)
                        && i > 0
                        && row[i - 1].character.is_ascii_digit()
                    {
                        row[i].character = 'b';
                    }
                    let range = if glyph == '◆' {
                        let end = i
                            + 1
                            + row[i + 1..]
                                .iter()
                                .take_while(|c| c.character.is_ascii_digit())
                                .count();
                        i..end
                    } else if glyph == 'w' && i > 0 && row[i - 1].character.is_ascii_digit() {
                        let digits = row[..i]
                            .iter()
                            .rev()
                            .take_while(|c| c.character.is_ascii_digit())
                            .count();
                        i - digits..i + 1
                    } else {
                        continue;
                    };
                    for cell in &mut row[range] {
                        cell.styles.update(|style| {
                            style.foreground =
                                Some(self.mode.style.colors.exit_code_error.base.into());
                            style.bold = Some(AnsiCode::On);
                            style.dim = Some(AnsiCode::Reset);
                        });
                    }
                }
            }
        }
        if self.tab_bar && !self.mode.capabilities.arrow_fonts {
            let parts = self.steel.value("parts").unwrap();
            if parts
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part["tab"] == 0 && !part["part"].as_str().unwrap().contains("← +"))
            {
                // The frozen reference predates the first tab's flat edge; narrow bars can wrap.
                for (before, after) in steel_expected
                    .iter_mut()
                    .flatten()
                    .zip(steel_cells.iter().flatten())
                {
                    if before.character == ''
                        && after.character == ''
                        && before.styles.foreground != after.styles.foreground
                    {
                        before
                            .styles
                            .update(|style| style.foreground = style.background);
                        break;
                    }
                }
            }
        }
        for (runtime, left, right) in [
            ("Deno", &left, &right),
            ("Steel", &steel_expected, &steel_cells),
        ] {
            if left != right {
                let plain = |rows: &[Vec<TerminalCharacter>]| {
                    rows.iter()
                        .map(|row| row.iter().map(|c| c.character).collect::<String>())
                        .collect::<Vec<_>>()
                };
                let first = left
                    .iter()
                    .zip(right)
                    .enumerate()
                    .find_map(|(row, (a, b))| {
                        a.iter().zip(b).enumerate().find(|(_, (a, b))| a != b).map(
                            |(col, (a, b))| {
                                (row, col, a.character, &a.styles, b.character, &b.styles)
                            },
                        )
                    });
                panic!("{:?} {rows}x{cols}:\nWasm: {:?}\n{runtime}: {:?}\nfirst different cell (position, characters and styles): {first:?}", self.mode.mode, plain(left), plain(right));
            }
        }
        if self.status_bar {
            steel_cells
        } else {
            left
        }
    }

    pub(super) fn reload_steel_tip(
        &mut self,
        chosen: &str,
        configuration: &BTreeMap<String, String>,
    ) {
        // Random sequences may differ; steer the choice through the same real cache path.
        let cache = json!({"yinz_version":"test","cached_data":{"quicknav":21,chosen:21}});
        self.steel
            .invoke("parity-choose-tip", vec![json!(chosen), cache.clone()])
            .unwrap();
        let mut replies = self.steel_replies.lock().unwrap();
        replies.insert("FsMetadata".into(), json!({}));
        replies.insert("FsRead".into(), json!(cache.to_string().as_bytes()));
        drop(replies);
        self.steel.load(json!(configuration)).unwrap();
        assert_eq!(
            self.steel.value("state").unwrap()["tipName"],
            chosen,
            "controlled cache must choose the same tip"
        );
    }
}

pub(super) fn shipped(mode: InputMode) -> ModeInfo {
    let mut info = ModeInfo {
        mode,
        ..Default::default()
    };
    info.keybinds = super::reference_trace::legacy_keybinds();
    // Default keybindings originate in maps, whose iteration order changes
    // between test processes. Supply the same ordered input to every guest.
    info.keybinds
        .sort_by_key(|(mode, _)| serde_json::to_string(mode).unwrap());
    for (_, bindings) in &mut info.keybinds {
        // Replay the defaults captured before the latest-notification shortcut existed.
        bindings.retain(|(_, actions)| {
            !actions.contains(&yinz_types::input::actions::Action::ActivateLastNotification)
        });
        bindings.sort_by_key(|(key, _)| serde_json::to_string(key).unwrap());
    }
    info
}

pub(super) fn rgb_theme(info: &mut ModeInfo) {
    // Give every role a distinct RGB value so mismatched emphasis cannot hide.
    use yinz_types::{colors::PaletteColor, style::StyleDeclaration};
    for (index, declaration) in [
        &mut info.style.colors.text_unselected,
        &mut info.style.colors.ribbon_unselected,
        &mut info.style.colors.ribbon_selected,
        &mut info.style.colors.text_selected,
    ]
    .into_iter()
    .enumerate()
    {
        let n = (index * 40) as u8;
        *declaration = StyleDeclaration {
            base: PaletteColor::Rgb((n + 1, 2, 3)),
            background: PaletteColor::Rgb((n + 4, 5, 6)),
            emphasis_0: PaletteColor::Rgb((n + 7, 8, 9)),
            emphasis_1: PaletteColor::Rgb((n + 10, 11, 12)),
            emphasis_2: PaletteColor::Rgb((n + 13, 14, 15)),
            emphasis_3: PaletteColor::Rgb((n + 16, 17, 18)),
        };
    }
}

fn current_bar_event(event: &Event, status_bar: bool) -> Value {
    fn convert(value: &mut Value, status_bar: bool) {
        match value {
            Value::Array(values) => {
                for value in values {
                    convert(value, status_bar);
                }
            },
            Value::Object(object) => {
                if ["LaunchPlugin", "LaunchOrFocusPlugin"].iter().any(|kind| {
                    object
                        .get(*kind)
                        .and_then(|value| value.pointer("/plugin/Alias/name"))
                        .is_some_and(|name| name == "agents")
                }) {
                    if status_bar || object.contains_key("LaunchOrFocusPlugin") {
                        *value = json!("OpenAgents");
                    } else {
                        // The custom LaunchPlugin fixture checks generic payload grouping.
                        // Keep it on a retained modal; shipped LaunchOrFocusPlugin bindings
                        // must use OpenAgents so they do not merge into the New agent row.
                        for kind in ["LaunchPlugin", "LaunchOrFocusPlugin"] {
                            if let Some(name) = object
                                .get_mut(kind)
                                .and_then(|value| value.pointer_mut("/plugin/Alias/name"))
                            {
                                *name = json!("new-agent");
                            }
                        }
                    }
                } else {
                    for value in object.values_mut() {
                        convert(value, status_bar);
                    }
                }
            },
            _ => {},
        }
    }
    let mut value = serde_json::to_value(event).unwrap();
    convert(&mut value, status_bar);
    value
}
