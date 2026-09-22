use super::*;
use crate::{
    plugin_map::{RunningPlugin, VecDequeInputStream, WriteOutputStream},
    test_env::test_env,
    zellij_exports::{wasi_read_bytes, wasi_write_object},
};
use prost::Message;
use std::sync::atomic::AtomicU64;
use wasmi::{Caller, Engine, Linker, Module, Store};
use wasmi_wasi::{
    sync::WasiCtxBuilder,
    wasi_common::pipe::{ReadPipe, WritePipe},
    Dir,
};
use zellij_plugin_api::plugin_command::{ProtobufGetLocalUtcOffsetResponse, ProtobufPluginCommand};
use zellij_plugin_api::plugin_ids::ProtobufZellijVersion;
use zellij_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter};
use zellij_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    data::{
        CopyDestination, InputMode, ModeInfo, Mouse, PaneInfo, PaneManifest, PluginCommand, TabInfo,
    },
    input::{actions::Action, layout::PluginUserConfiguration},
    keys::KeyModifier,
    units::FractionalSeconds,
};

const NOW: u64 = 1_757_078_399_750; // Just before a UTC minute, with a deterministic offset.

struct Pair {
    wasm: RunningPlugin,
    deno: JsRuntime,
    now: Arc<AtomicU64>,
    offset: Arc<std::sync::atomic::AtomicI32>,
    commands: Arc<Mutex<Vec<Value>>>,
    mode: ModeInfo,
    cache: tempfile::TempDir,
}

fn eval(runtime: &mut JsRuntime, source: String) -> Value {
    let result = runtime
        .execute_script("status-bar-parity.js", source)
        .unwrap();
    deno_core::scope!(scope, runtime);
    let value = v8::Local::new(scope, result);
    serde_v8::from_v8(scope, value).unwrap()
}

impl Pair {
    fn new(classic: bool) -> Self {
        let engine = Engine::default();
        let artifact = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/wasm32-wasip1/debug/status-bar.wasm");
        let module = Module::new(
            &engine,
            std::fs::read(&artifact).expect("cargo x test builds the Wasm reference"),
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(
            cache.path().join("status-bar-tips.cache"),
            r#"{"zellij_version":"test","cached_data":{"quicknav":1}}"#,
        )
        .unwrap();
        let mut env = test_env();
        let mut wasi = WasiCtxBuilder::new();
        wasi.preopened_dir(
            Dir::from_std_file(std::fs::File::open(cache.path()).unwrap()),
            "/tmp",
        )
        .unwrap();
        wasi.stdin(Box::new(ReadPipe::new(VecDequeInputStream(
            env.stdin_pipe.clone(),
        ))));
        wasi.stdout(Box::new(WritePipe::new(WriteOutputStream(
            env.stdout_pipe.clone(),
        ))));
        wasi.inherit_stderr();
        env.wasi_ctx = Some(wasi.build());
        let mut linker = Linker::new(&engine);
        wasmi_wasi::add_to_linker(&mut linker, |env: &mut PluginEnv| {
            env.wasi_ctx.as_mut().unwrap()
        })
        .unwrap();
        let now = Arc::new(AtomicU64::new(NOW));
        let clock = now.clone();
        linker.allow_shadowing(true);
        linker
            .func_wrap(
                "wasi_snapshot_preview1",
                "clock_time_get",
                move |mut caller: Caller<'_, PluginEnv>,
                      _clock: i32,
                      _precision: i64,
                      pointer: i32|
                      -> i32 {
                    let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
                    memory
                        .write(
                            &mut caller,
                            pointer as usize,
                            &(clock.load(Ordering::SeqCst) * 1_000_000).to_le_bytes(),
                        )
                        .unwrap();
                    0
                },
            )
            .unwrap();
        // Deterministic entropy for HashMap seeds and random tip selection, without
        // changing the production plugin or depending on its particular sequence.
        let entropy = AtomicU64::new(1);
        linker
            .func_wrap(
                "wasi_snapshot_preview1",
                "random_get",
                move |mut caller: Caller<'_, PluginEnv>, pointer: i32, length: i32| -> i32 {
                    let bytes: Vec<_> = (0..length)
                        .map(|_| {
                            let mut n = entropy.fetch_add(0x9e3779b97f4a7c15, Ordering::SeqCst);
                            n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                            n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
                            (n ^ (n >> 31)) as u8
                        })
                        .collect();
                    caller
                        .get_export("memory")
                        .unwrap()
                        .into_memory()
                        .unwrap()
                        .write(&mut caller, pointer as usize, &bytes)
                        .unwrap();
                    0
                },
            )
            .unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let recorded = commands.clone();
        let offset = Arc::new(std::sync::atomic::AtomicI32::new(19_800));
        let zone = offset.clone();
        linker
            .func_wrap(
                "zellij",
                "host_run_plugin_command",
                move |mut caller: Caller<'_, PluginEnv>| {
                    let env = caller.data_mut();
                    let bytes = wasi_read_bytes(env).unwrap();
                    let wire = ProtobufPluginCommand::decode(bytes.as_slice()).unwrap();
                    let command: PluginCommand = wire.try_into().unwrap();
                    recorded.lock().unwrap().push(json!(command));
                    match command {
                        PluginCommand::GetZellijVersion => wasi_write_object(
                            env,
                            &ProtobufZellijVersion {
                                version: "test".into(),
                            }
                            .encode_to_vec(),
                        )
                        .unwrap(),
                        PluginCommand::GetLocalUtcOffset { .. } => wasi_write_object(
                            env,
                            &ProtobufGetLocalUtcOffsetResponse {
                                offset_seconds: zone.load(Ordering::SeqCst),
                            }
                            .encode_to_vec(),
                        )
                        .unwrap(),
                        _ => (),
                    }
                },
            )
            .unwrap();
        let mut store = Store::new(&engine, env);
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        let mut wasm = RunningPlugin::new(store, instance, 1, 200);
        wasm.start().unwrap();
        let configuration = BTreeMap::from([("classic".to_owned(), classic.to_string())]);
        wasm.load(&PluginUserConfiguration::new(configuration.clone()))
            .unwrap();

        let loader = Rc::new(
            loader::PluginModuleLoader::embedded(
                "status-bar",
                zellij_config::consts::builtin_deno_plugin("status-bar").unwrap(),
            )
            .unwrap(),
        );
        let mut deno = limits::Limits::new("status-bar parity".into())
            .runtime(RuntimeOptions {
                module_loader: Some(loader.clone()),
                extensions: vec![sdk_test_host::init()],
                ..Default::default()
            })
            .unwrap();
        deno.op_state().borrow_mut().put(TestHost {
            replies: BTreeMap::from([
                ("GetLocalUtcOffset".into(), json!(19_800)),
                ("GetZellijVersion".into(), json!("test")),
            ]),
            ..Default::default()
        });
        run(&mut deno, &format!("globalThis.now = {NOW}; Date.now = () => now; globalThis.bridge = {SDK}; bridge('initialize', {{instanceId:'parity', identity:{{}}}});"));
        let module = get_tokio_runtime()
            .block_on(deno.load_main_es_module(loader.entrypoint()))
            .unwrap();
        let evaluation = deno.mod_evaluate(module);
        get_tokio_runtime()
            .block_on(deno.run_event_loop(Default::default()))
            .unwrap();
        get_tokio_runtime().block_on(evaluation).unwrap();
        let namespace = deno.get_module_namespace(module).unwrap();
        {
            deno_core::scope!(scope, deno);
            let namespace = v8::Local::new(scope, namespace);
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "plugin").unwrap();
            global.set(scope, key.into(), namespace.into());
        }
        run(
            &mut deno,
            &format!(
                "bridge('bind', plugin); bridge('load', {}, 1); bridge('status');",
                json!(configuration)
            ),
        );
        let mut pair = Self {
            wasm,
            deno,
            now,
            offset,
            commands,
            mode: ModeInfo::default(),
            cache,
        };
        pair.compare_commands();
        pair
    }

    fn compare_commands(&mut self) {
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
            !["FsMetadata", "FsRead", "FsWrite", "GetZellijVersion"].contains(&name)
        };
        let expected: Vec<_> = self
            .commands
            .lock()
            .unwrap()
            .drain(..)
            .filter(&filter)
            .collect();
        let actual: Vec<_> = self
            .deno
            .op_state()
            .borrow_mut()
            .borrow_mut::<TestHost>()
            .commands
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
            normalize(expected),
            "host commands differ"
        );
    }

    fn event(&mut self, event: Event) {
        if let Event::ModeUpdate(mode) = &event {
            self.mode = mode.clone();
        }
        let expected = self.wasm.update(&event).unwrap();
        let actual = eval(
            &mut self.deno,
            format!(
                "bridge('update', {}, 1); bridge('status')",
                event::event_to_json(&event).unwrap()
            ),
        );
        assert_eq!(actual, json!(expected), "update decision for {event:?}");
        self.compare_commands();
    }

    fn at(&mut self, now: u64) {
        self.now.store(now, Ordering::SeqCst);
        eval(&mut self.deno, format!("globalThis.now = {now}"));
        self.event(Event::Timer(FractionalSeconds::new(0.0)));
    }

    fn render(&mut self, rows: usize, cols: usize) -> String {
        let expected = String::from_utf8(self.wasm.render(rows, cols).unwrap()).unwrap();
        let actual = eval(
            &mut self.deno,
            format!("bridge('render', [{rows}, {cols}], 1)"),
        )
        .as_str()
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
        let left = cells(&expected);
        let right = cells(&actual);
        if left != right {
            let plain = |rows: &[Vec<TerminalCharacter>]| {
                rows.iter()
                    .map(|row| row.iter().map(|c| c.character).collect::<String>())
                    .collect::<Vec<_>>()
            };
            let first =
                left.iter()
                    .zip(&right)
                    .enumerate()
                    .find_map(|(row, (a, b))| {
                        a.iter().zip(b).enumerate().find(|(_, (a, b))| a != b).map(
                            |(col, (a, b))| {
                                (row, col, a.character, &a.styles, b.character, &b.styles)
                            },
                        )
                    });
            panic!("{:?} {rows}x{cols}:\nwasm: {:?}\ndeno: {:?}\nfirst different cell (position, characters and styles): {first:?}", self.mode.mode, plain(&left), plain(&right));
        }
        left.iter()
            .flat_map(|row| row.iter().map(|c| c.character))
            .collect()
    }
}

fn shipped(mode: InputMode) -> ModeInfo {
    let mut info = ModeInfo {
        mode,
        ..Default::default()
    };
    info.keybinds = zellij_config::config::Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    info
}

fn visual_parity(classic: bool, rows: usize) {
    use strum::IntoEnumIterator;
    {
        let mut pair = Pair::new(classic);
        pair.event(Event::TabUpdate(vec![TabInfo {
            active: true,
            active_swap_layout_name: Some("vertical".into()),
            selectable_tiled_panes_count: 3,
            ..Default::default()
        }]));
        for mode in InputMode::iter() {
            for simple in [false, true] {
                let mut info = shipped(mode);
                info.capabilities.arrow_fonts = simple;
                pair.event(Event::ModeUpdate(info));
                {
                    for cols in [
                        0, 1, 4, 5, 15, 29, 40, 59, 60, 79, 80, 99, 100, 119, 120, 121, 150, 200,
                        400,
                    ] {
                        pair.render(rows, cols);
                    }
                }
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_one_line_visual_parity() {
    visual_parity(false, 1);
}
#[test]
fn status_bar_wasm_deno_classic_one_line_visual_parity() {
    visual_parity(true, 1);
}
#[test]
fn status_bar_wasm_deno_two_line_visual_parity() {
    visual_parity(false, 2);
}

#[test]
fn status_bar_wasm_deno_events_and_timing_parity() {
    let mut pair = Pair::new(false);
    pair.event(Event::InitialKeybinds(shipped(InputMode::Normal).keybinds));
    pair.event(Event::ModeUpdate(ModeInfo::default()));
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        selectable_tiled_panes_count: 2,
        ..Default::default()
    }]));
    pair.render(1, 500);
    for event in [
        Event::CopyToClipboard(CopyDestination::System),
        Event::CopyToClipboard(CopyDestination::System),
        Event::CopyToClipboard(CopyDestination::Primary),
        Event::CopyToClipboard(CopyDestination::Command),
        Event::InputReceived,
        Event::InputReceived,
        Event::SystemClipboardFailure,
        Event::InputReceived,
    ] {
        pair.event(event);
        pair.render(1, 500);
    }
    for (modifiers, is_live) in [
        (vec![KeyModifier::Alt], false),
        (vec![KeyModifier::Alt], true),
        (vec![KeyModifier::Alt], false),
        (vec![], false),
        (vec![], true),
    ] {
        pair.event(Event::ModifiersHeld {
            modifiers: modifiers.into_iter().collect(),
            is_live,
        });
        pair.render(1, 500);
    }
    pair.event(Event::Mouse(Mouse::Hover(0, 0)));
    let text = pair.render(1, 500);
    for label in ["New Pane", "Floating"] {
        let col = text
            .chars()
            .collect::<Vec<_>>()
            .windows(label.chars().count())
            .position(|s| s.iter().collect::<String>() == label)
            .unwrap_or_else(|| panic!("missing {label}: {text}"));
        pair.event(Event::Mouse(Mouse::Hover(0, col)));
        pair.render(1, 500);
        pair.event(Event::Mouse(Mouse::LeftClick(0, col)));
        pair.render(1, 500);
    }
    pair.event(Event::Mouse(Mouse::Hover(0, u16::MAX as usize)));
    pair.render(1, 500);
    let mut clock = NOW;
    for state in [
        AgentState::Busy,
        AgentState::Waiting,
        AgentState::Idle,
        AgentState::Exited,
    ] {
        let pane = PaneInfo {
            agent: Some(AgentInfo {
                state,
                wait_needs_a_person: true,
                kind: "codex".into(),
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                cwd: None,
                session_id: None,
                run_id: None,
            }),
            ..Default::default()
        };
        pair.event(Event::PaneUpdate(PaneManifest {
            panes: [(zellij_types::internal_ids::TabPosition::new(0), vec![pane])]
                .into_iter()
                .collect(),
        }));
        for _ in 0..9 {
            clock += 125;
            pair.at(clock);
            pair.render(1, 500);
        }
    }
    pair.event(Event::ModifiersHeld {
        modifiers: [KeyModifier::Alt].into_iter().collect(),
        is_live: false,
    });
    pair.at(clock + 4000);
    pair.render(1, 500);
    pair.offset.store(-18_000, Ordering::SeqCst);
    pair.deno
        .op_state()
        .borrow_mut()
        .borrow_mut::<TestHost>()
        .replies
        .insert("GetLocalUtcOffset".into(), json!(-18_000));
    pair.at((pair.now.load(Ordering::SeqCst) / 3_600_000 + 1) * 3_600_000);
    pair.render(1, 500);
    pair.render(2, 80);
    pair.render(1, 80);
    pair.event(Event::Mouse(Mouse::Hover(0, 0)));
    let deadline = pair.now.load(Ordering::SeqCst) + 30_000;
    pair.at(deadline - 1);
    pair.render(1, 500);
    pair.at(deadline);
    pair.render(1, 500);
    let midnight = (NOW / 86_400_000 + 2) * 86_400_000 + 18_000_000;
    pair.at(midnight - 1);
    pair.render(1, 500);
    pair.at(midnight);
    pair.render(1, 500);
}

fn rgb_theme(info: &mut ModeInfo) {
    // Give every role a distinct RGB value so mismatched emphasis cannot hide.
    use zellij_types::{colors::PaletteColor, style::StyleDeclaration};
    for (index, declaration) in [
        &mut info.style.colors.text_unselected,
        &mut info.style.colors.ribbon_unselected,
        &mut info.style.colors.ribbon_selected,
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

#[test]
fn status_bar_wasm_deno_locked_base_rgb_and_missing_bindings() {
    use strum::IntoEnumIterator;
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    for missing in [false, true] {
        for mode in InputMode::iter() {
            let mut info = shipped(mode);
            info.base_mode = Some(InputMode::Locked);
            if missing {
                for (_, bindings) in &mut info.keybinds {
                    bindings.clear();
                }
            }
            rgb_theme(&mut info);
            pair.event(Event::ModeUpdate(info));
            for rows in [1, 2] {
                for cols in [5, 30, 80, 120, 200, 400] {
                    pair.render(rows, cols);
                }
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_custom_bindings_unicode_and_reconfiguration() {
    use strum::IntoEnumIterator;
    let config = zellij_config::config::Config::from_kdl(
        r#"
        keybinds clear-defaults=true {
            shared {
                bind "Ctrl Alt 界" { SwitchToMode "Pane"; }
                bind "Alt 🦀" { SwitchToMode "Tab"; }
                bind "Super F12" { SwitchToMode "Session"; }
                bind "Ctrl Shift a" { SwitchToMode "Agent"; }
                bind "Ctrl g" { SwitchToMode "Locked"; }
                bind "Enter" "Esc" { SwitchToMode "Normal"; }
            }
            pane {
                bind "Ctrl n" "N" "Alt n" "F2" { NewPane; SwitchToMode "Normal"; }
                bind "Ctrl Shift h" { MoveFocus "Left"; }
                bind "Ctrl Shift l" { MoveFocus "Right"; }
                bind "x" { CloseFocus; }
                bind "Alt x" { CloseFocus; SwitchToMode "Normal"; }
                bind "Alt z" { NewPane "Left"; }
            }
            normal {
                bind "Alt 界" { NewPane; }
                bind "Alt 🦀" { ToggleFloatingPanes; }
                bind "Shift +" { Resize "Increase"; }
                bind "Ctrl -" { Resize "Decrease"; }
            }
            resize {
                bind "Ctrl 界" { Resize "Increase Left"; }
                bind "Ctrl 🦀" { Resize "Increase Right"; }
            }
            session {
                bind "Ctrl q" { LaunchOrFocusPlugin "unrelated"; }
                bind "Ctrl a" { LaunchPlugin "session-manager"; SwitchToMode "Normal"; }
                bind "Alt b" { LaunchOrFocusPlugin "zellij:about"; }
                bind "c" { LaunchOrFocusPlugin "configuration"; }
                bind "d" { LaunchOrFocusPlugin "zellij:layout-manager"; }
                bind "e" { LaunchOrFocusPlugin "plugin-manager"; }
            }
            agent {
                bind "n" { LaunchOrFocusPlugin "hocus-new-agent"; }
                bind "d" { LaunchOrFocusPlugin "hocus-duplicate-agent"; }
                bind "f" { LaunchOrFocusPlugin "hocus-fork-agent"; }
                bind "a" { LaunchOrFocusPlugin "hocus-agents"; }
                bind "c" { LaunchOrFocusPlugin "hocus"; }
            }
        }
    "#,
        None,
    )
    .unwrap();
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        selectable_tiled_panes_count: 2,
        ..Default::default()
    }]));
    pair.event(Event::InitialKeybinds(config.keybinds.to_keybinds_vec()));
    for mode in InputMode::iter() {
        let mut info = ModeInfo {
            mode,
            ..Default::default()
        };
        rgb_theme(&mut info);
        pair.event(Event::ModeUpdate(info));
        for rows in [1, 2] {
            for cols in [1, 25, 60, 100, 160, 300] {
                pair.render(rows, cols);
            }
        }
    }
    // Rust key equality treats ASCII capitals as Shift plus the lowercase key.
    for (i, spelling) in ["N", "Shift n"].into_iter().enumerate() {
        pair.event(Event::ModeUpdate(ModeInfo {
            keybinds: vec![(
                InputMode::Normal,
                vec![(
                    spelling.parse().unwrap(),
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Pane,
                    }],
                )],
            )],
            ..Default::default()
        }));
        pair.render(1, 80 + i); // Resize invalidates Deno's unchanged-frame cache.
    }
    pair.event(Event::InitialKeybinds(shipped(InputMode::Normal).keybinds));
    pair.event(Event::ModeUpdate(ModeInfo::default()));
    pair.render(1, 500);
    pair.event(Event::ModifiersHeld {
        modifiers: [KeyModifier::Alt].into_iter().collect(),
        is_live: true,
    });
    pair.render(1, 500);
}

#[test]
fn status_bar_wasm_deno_nested_fullscreen_floating_and_swap_layouts() {
    let mut pair = Pair::new(false);
    for mode in [InputMode::Normal, InputMode::Locked, InputMode::Pane] {
        for (dimmed, ascended) in [(false, false), (true, false), (false, true)] {
            let mut info = shipped(mode);
            info.session_dimmed = Some(dimmed);
            info.session_ascended = Some(ascended);
            info.nested_ascend_keys = vec!["Ctrl Alt 界".parse().unwrap(), "Esc".parse().unwrap()];
            info.nested_descend_keys = vec!["Shift 🦀".parse().unwrap()];
            pair.event(Event::ModeUpdate(info));
            for (fullscreen, floating) in [(false, false), (true, false), (false, true)] {
                pair.event(Event::TabUpdate(vec![TabInfo {
                    active: true,
                    is_fullscreen_active: fullscreen,
                    are_floating_panes_visible: floating,
                    panes_to_hide: 3,
                    selectable_floating_panes_count: 2,
                    active_swap_layout_name: Some("vertical 界".into()),
                    is_swap_layout_dirty: floating,
                    ..Default::default()
                }]));
                for rows in [1, 2] {
                    for cols in [1, 15, 30, 80, 200, 400] {
                        pair.render(rows, cols);
                    }
                }
            }
        }
    }
    let mut info = shipped(InputMode::Normal);
    info.session_dimmed = Some(true);
    pair.event(Event::ModeUpdate(info)); // Unbound nested-session hint.
    pair.render(1, 80);
    pair.render(2, 80);
}

#[test]
fn status_bar_wasm_deno_all_cached_tips() {
    let names = [
        "quicknav",
        "floating_panes_mouse",
        "send_mouse_clicks_to_terminal",
        "move_focus_hjkl_tab_switch",
        "zellij_setup_check",
        "use_mouse",
        "sync_tab",
        "edit_scrollbuffer",
        "compact_layout",
        "move_tabs",
    ];
    let mut pair = Pair::new(false);
    pair.event(Event::ModeUpdate(shipped(InputMode::Normal)));
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    let cache = json!({"zellij_version":"test", "cached_data": names.iter().map(|name| ((*name).to_owned(), 21)).collect::<BTreeMap<_,_>>()});
    let path = pair.cache.path().join("status-bar-tips.cache");
    let configuration = BTreeMap::from([("classic".to_owned(), "false".to_owned())]);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..128 {
        std::fs::write(&path, cache.to_string()).unwrap();
        pair.wasm
            .load(&PluginUserConfiguration::new(configuration.clone()))
            .unwrap();
        let updated: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let chosen = updated["cached_data"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, count)| **count == json!(22))
            .expect("Wasm must read and update its controlled cache")
            .0;
        // Random sequences need not agree; direct Deno's choice to the tip the
        // real Wasm selected, then compare every rendered cell in each size form.
        let names: Vec<_> = cache["cached_data"].as_object().unwrap().keys().collect();
        let index = names.iter().position(|name| *name == chosen).unwrap();
        {
            let state = pair.deno.op_state();
            let mut state = state.borrow_mut();
            let host = state.borrow_mut::<TestHost>();
            host.replies.insert("FsMetadata".into(), json!({}));
            host.replies
                .insert("FsRead".into(), json!(cache.to_string().as_bytes()));
        }
        run(
            &mut pair.deno,
            &format!(
                "Math.random = () => {}; bridge('load', {}, 1); bridge('status');",
                (index as f64 + 0.5) / names.len() as f64,
                json!(configuration)
            ),
        );
        pair.compare_commands();
        if seen.insert(chosen.clone()) {
            for cols in [20, 60, 80, 100, 120, 160, 200, 400] {
                pair.render(2, cols);
            }
        }
        if seen.len() == names.len() {
            break;
        }
    }
    assert_eq!(seen, names.into_iter().map(str::to_owned).collect());
}

#[test]
fn status_bar_wasm_deno_narrow_width_transitions() {
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    for mode in [InputMode::Normal, InputMode::Pane] {
        pair.event(Event::ModeUpdate(shipped(mode)));
        for rows in [1, 2] {
            for cols in 0..=220 {
                pair.render(rows, cols);
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_mixed_agents_and_animation_frames() {
    let mut pair = Pair::new(false);
    let agent = |state, wait_needs_a_person| PaneInfo {
        agent: Some(AgentInfo {
            state,
            wait_needs_a_person,
            kind: "codex".into(),
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
        ..Default::default()
    };
    for simple in [false, true] {
        let mut info = shipped(InputMode::Normal);
        info.capabilities.arrow_fonts = simple;
        pair.event(Event::ModeUpdate(info));
        let manifest = PaneManifest {
            panes: [(
                zellij_types::internal_ids::TabPosition::new(0),
                vec![
                    agent(AgentState::Busy, false),
                    agent(AgentState::Waiting, false),
                    agent(AgentState::Waiting, true),
                    agent(AgentState::Idle, false),
                    agent(AgentState::Exited, false),
                ],
            )]
            .into_iter()
            .collect(),
        };
        pair.event(Event::PaneUpdate(manifest.clone()));
        pair.event(Event::PaneUpdate(manifest));
        for _ in 0..9 {
            pair.at(pair.now.load(Ordering::SeqCst) + 125);
            for cols in [4, 5, 10, 25, 60, 120, 150, 200, 300, 500] {
                pair.render(1, cols);
            }
        }
    }
}
