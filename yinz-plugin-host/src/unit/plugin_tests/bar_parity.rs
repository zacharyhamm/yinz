use super::reference_trace::Trace;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use yinz_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter};
use yinz_types::{
    data::{Event, InputMode, ModeInfo, PluginCommand},
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
        steel.eval(&format!("(define parity-clock (vector {NOW})) (define (yinz-wall-clock-ms) (vector-ref parity-clock 0))
            (define parity-random (vector 0)) (define (yinz-random-integer n) (vector-ref parity-random 0))
            (define (parity-choose-tip chosen cache)
              (vector-set! parity-random 0 (let loop ((names (sort (hash-keys->list (hash-ref cache \"cached_data\")) string<?)) (index 0))
                (if (equal? (car names) chosen) index (loop (cdr names) (+ index 1))))))")).unwrap();
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
        let mut source_root =
            yinz_steel::modules::SourceRoot::embedded(&format!("{name}.scm"), sources).unwrap();
        steel
            .eval_with_source_root(
                yinz_steel_builtins::builtin_steel_plugin(name).unwrap(),
                &mut source_root,
            )
            .unwrap();
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
        let steel = self
            .steel_commands
            .lock()
            .unwrap()
            .drain(..)
            .filter(filter)
            .collect();
        assert_eq!(
            normalize(steel),
            normalize(expected),
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
        let steel = self
            .steel
            .update(serde_json::to_value(&event).unwrap())
            .unwrap();
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
        let steel = self.steel.render(rows, cols).unwrap().replace('\n', "\n\r");
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
        for (runtime, right) in [("Deno", &right), ("Steel", &steel_cells)] {
            if left != *right {
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
                panic!("{:?} {rows}x{cols}:\nWasm: {:?}\n{runtime}: {:?}\nfirst different cell (position, characters and styles): {first:?}", self.mode.mode, plain(&left), plain(right));
            }
        }
        left
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
    info.keybinds = yinz_config::config::Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
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
