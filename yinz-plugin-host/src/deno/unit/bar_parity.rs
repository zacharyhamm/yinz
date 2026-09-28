use super::*;
use crate::{
    plugin_map::{BoundedOutputStream, RunningPlugin, VecDequeInputStream},
    test_env::test_env,
    yinz_exports::{wasi_read_bytes, wasi_write_object},
};
use prost::Message;
use std::sync::atomic::AtomicU64;
use wasmi::{Caller, Engine, Linker, Module, Store};
use wasmi_wasi::{
    sync::WasiCtxBuilder,
    wasi_common::pipe::{ReadPipe, WritePipe},
    Dir,
};
use yinz_plugin_api::plugin_command::{
    ProtobufGetLocalUtcOffsetResponse, ProtobufNewTabResponse, ProtobufPluginCommand,
};
use yinz_plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufYinzVersion};
use yinz_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter};
use yinz_types::{
    data::{InputMode, ModeInfo, PluginCommand},
    input::layout::PluginUserConfiguration,
    units::FractionalSeconds,
};

pub(super) const NOW: u64 = 1_757_078_399_750; // Just before a UTC minute, with a deterministic offset.

pub(super) struct Pair {
    pub(super) wasm: RunningPlugin,
    pub(super) deno: JsRuntime,
    pub(super) now: Arc<AtomicU64>,
    pub(super) offset: Arc<std::sync::atomic::AtomicI32>,
    pub(super) commands: Arc<Mutex<Vec<Value>>>,
    pub(super) mode: ModeInfo,
    pub(super) cache: tempfile::TempDir,
    status_bar: bool,
}

fn eval(runtime: &mut JsRuntime, source: String) -> Value {
    let result = runtime.execute_script("bar-parity.js", source).unwrap();
    deno_core::scope!(scope, runtime);
    let value = v8::Local::new(scope, result);
    serde_v8::from_v8(scope, value).unwrap()
}

impl Pair {
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
        let engine = Engine::default();
        let artifact = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../target/wasm32-wasip1/debug/{name}.wasm"));
        let module = Module::new(
            &engine,
            std::fs::read(&artifact).expect("cargo x test builds the Wasm reference"),
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(
            cache.path().join("status-bar-tips.cache"),
            r#"{"yinz_version":"test","cached_data":{"quicknav":1}}"#,
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
        wasi.stdout(Box::new(WritePipe::new(BoundedOutputStream(
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
        let host = hostname.to_owned();
        linker
            .func_wrap(
                "yinz",
                "host_run_plugin_command",
                move |mut caller: Caller<'_, PluginEnv>| {
                    let env = caller.data_mut();
                    let bytes = wasi_read_bytes(env).unwrap();
                    let wire = ProtobufPluginCommand::decode(bytes.as_slice()).unwrap();
                    let command: PluginCommand = wire.try_into().unwrap();
                    recorded.lock().unwrap().push(json!(command));
                    match command {
                        PluginCommand::GetYinzVersion => wasi_write_object(
                            env,
                            &ProtobufYinzVersion {
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
                        PluginCommand::GetPluginIds => wasi_write_object(
                            env,
                            &ProtobufPluginIds {
                                hostname: host.clone(),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        )
                        .unwrap(),
                        PluginCommand::NewTab { .. } => wasi_write_object(
                            env,
                            &ProtobufNewTabResponse::default().encode_to_vec(),
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
        wasm.load(&PluginUserConfiguration::new(configuration.clone()))
            .unwrap();

        let loader = Rc::new(
            loader::PluginModuleLoader::embedded(
                name,
                yinz_deno_builtins::builtin_deno_plugin(name).unwrap(),
            )
            .unwrap(),
        );
        let mut deno = limits::Limits::new(format!("{name} parity"))
            .runtime(RuntimeOptions {
                module_loader: Some(loader.clone()),
                extensions: vec![sdk_test_host::init()],
                ..Default::default()
            })
            .unwrap();
        deno.op_state().borrow_mut().put(TestHost {
            replies: BTreeMap::from([
                ("GetPluginIds".into(), json!({"hostname": hostname})),
                ("NewTab".into(), Value::Null),
                ("GetLocalUtcOffset".into(), json!(19_800)),
                ("GetYinzVersion".into(), json!("test")),
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
            status_bar: name == "status-bar",
        };
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

    pub(super) fn event(&mut self, event: Event) {
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

    pub(super) fn at(&mut self, now: u64) {
        self.now.store(now, Ordering::SeqCst);
        eval(&mut self.deno, format!("globalThis.now = {now}"));
        self.event(Event::Timer(FractionalSeconds::new(0.0)));
    }

    pub(super) fn render(&mut self, rows: usize, cols: usize) -> String {
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

pub(super) fn shipped(mode: InputMode) -> ModeInfo {
    let mut info = ModeInfo {
        mode,
        ..Default::default()
    };
    info.keybinds = yinz_config::config::Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    info
}

pub(super) fn rgb_theme(info: &mut ModeInfo) {
    // Give every role a distinct RGB value so mismatched emphasis cannot hide.
    use yinz_types::{colors::PaletteColor, style::StyleDeclaration};
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
