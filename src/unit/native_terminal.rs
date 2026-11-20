//! Native frames from a real session, through the production protobuf writer.
#![cfg(unix)]
use clap::Parser;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
use yinz_native_terminal::emulation::terminal::TerminalState;
use yinz_types::structured_render::{decode, FrameHeader, GeometryRecord};
#[path = "real_fixture.rs"]
mod real_fixture;

const CONTENT: &str = "native-frame-marker";

struct NativeFixture {
    fixture: real_fixture::RealFixture,
    config: PathBuf,
    config_dir: PathBuf,
    layout: PathBuf,
    cache: PathBuf,
    log: PathBuf,
}

impl NativeFixture {
    fn new() -> Self {
        let fixture = real_fixture::RealFixture::new("native-");
        let path = fixture.path();
        let config = path.join("config.kdl");
        let config_dir = path.join("config-dir");
        let layout = path.join("layout.kdl");
        let cache = path.join("cache");
        let log = path.join("tmp").join(format!(
            "yinz-{}/yinz-log/yinz.log",
            nix::unistd::Uid::current()
        ));
        for dir in [
            path.join("home"),
            path.join("tmp"),
            path.join("data"),
            config_dir.clone(),
            cache.clone(),
        ] {
            fs::create_dir(dir).unwrap();
        }
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &path.join("data"))
            .unwrap();
        fs::write(
            &config,
            "detachable_panes false\nload_plugins clear-defaults=true\nterminal { initial_columns 100; initial_rows 30; padding 4; system_fonts false; }\n",
        ).unwrap();
        fs::write(&layout, r#"layout { pane command="/bin/sh" { args "-c" "stty -echo; printf native-frame-marker; exec /bin/sh"; }; }"#).unwrap();
        Self {
            fixture,
            config,
            config_dir,
            layout,
            cache,
            log,
        }
    }

    fn command(&self) -> Command {
        let path = self.fixture.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
        self.fixture.inherit(&mut command);
        for key in ["YINZ", "YINZ_SESSION_NAME", "YINZ_PANE_ID", "YINZ_PANE_REF"] {
            command.env_remove(key);
        }
        command
            .env("TMPDIR", path.join("tmp"))
            .env("YINZ_SOCKET_DIR", self.fixture.socket_dir())
            .env("HOME", path.join("home"))
            .env("YINZ_TEST_CACHE_DIR", &self.cache)
            .env("SHELL", "/bin/sh")
            .arg("--config")
            .arg(&self.config)
            .arg("--config-dir")
            .arg(&self.config_dir)
            .arg("--data-dir")
            .arg(path.join("data"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn terminal(&self, session: &str, create: bool, recording: &Path) -> Command {
        let mut command = self.command();
        if create {
            command.arg("--layout").arg(&self.layout);
        }
        command
            .args(["terminal", session, "--record"])
            .arg(recording);
        if create {
            command.arg("--create");
        }
        command
    }

    fn action(&self, session: &str, args: &[&str]) {
        assert_success(
            self.command()
                .args(["--session", session, "action"])
                .args(args)
                .output()
                .unwrap(),
        );
    }

    fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// This file belongs to a separate process. Poll the actual recorded condition
/// within its lifetime and tolerate the final record being concurrently appended.
fn while_running<T>(
    child: &mut Child,
    what: &str,
    timeout: Duration,
    mut observe: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "native client exited before {what}"
        );
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::park_timeout(Duration::from_millis(20));
    }
}

#[derive(Default)]
struct Recording {
    frames: Vec<FrameHeader>,
    geometry_records: Vec<(u64, GeometryRecord)>,
    state: Option<TerminalState>,
    applied_bytes: usize,
}

impl Recording {
    fn read(&mut self, path: &Path) {
        let Ok(bytes) = fs::read(path) else { return };
        let mut rest = &bytes[self.applied_bytes..];
        while let Some(length) = rest.get(..4) {
            let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
            let Some(frame) = rest.get(4..4 + length) else {
                break;
            };
            let view = decode(frame).unwrap();
            let header = *view.header();
            if let Some(geometry) = view.geometry() {
                self.geometry_records.push((header.seq, geometry));
            }
            let state = self
                .state
                .get_or_insert_with(|| TerminalState::new(header.rows.into(), header.cols.into()));
            state.apply_frame(frame).unwrap();
            assert!(self.frames.last().is_none_or(|last| last.seq < header.seq));
            self.frames.push(header);
            rest = &rest[4 + length..];
        }
        self.applied_bytes = bytes.len() - rest.len();
    }

    fn grid_and_marker(&self, marker: &str) -> Option<(u16, u16)> {
        let state = self.state.as_ref()?;
        let size = state.size();
        (0..size.rows)
            .any(|row| {
                let line: String = (0..size.cols)
                    .map(|col| state.screen().cell(row, col).character())
                    .collect();
                line.contains(marker)
            })
            .then_some((size.cols as u16, size.rows as u16))
    }
}

#[test]
fn native_headless_creates_and_reattaches_with_structured_content() {
    let fixture = NativeFixture::new();
    for (session, create, explicit) in [
        ("native-test", true, false),
        ("native-test", false, false),
        ("native-explicit", true, true),
    ] {
        let path = fixture
            .fixture
            .path()
            .join(format!("{session}-{create}.frames"));
        let mut command = fixture.terminal(session, create, &path);
        command.args(["--headless", "--duration-secs", "3"]);
        if explicit {
            command.args(["--cols", "96", "--rows", "26"]);
        }
        if create && !explicit {
            // Layout asset preparation needs the same stack as the native startup worker.
            let argv: Vec<_> = std::iter::once(std::ffi::OsString::from("yinz"))
                .chain(command.get_args().map(std::ffi::OsStr::to_owned))
                .collect();
            std::thread::Builder::new()
                .stack_size(32 * 1024 * 1024)
                .spawn(move || {
                    let cli = yinz_cli::args::CliArgs::try_parse_from(argv).unwrap();
                    let (config, layout, options, _, _) =
                        yinz_cli::setup::Setup::native_connection_config(&cli).unwrap();
                    let message = yinz_client::native_first_message(
                        &cli,
                        &options,
                        &config.env,
                        layout,
                        None,
                        yinz_types::pane_size::Size {
                            rows: 40,
                            cols: 120,
                        },
                        true,
                        false,
                        None,
                    );
                    let yinz_ipc::ipc::ClientToServerMsg::FirstClientConnected { cli_assets } =
                        message
                    else {
                        panic!("expected first client");
                    };
                    assert!(cli_assets.checked_native_layout);
                    assert!(!cli_assets.retains_source_on_switch);
                    let (_, layout) =
                        yinz_config::cli_assets::load_config_and_layout(&cli_assets).unwrap();
                    assert!(format!("{layout:?}").contains(CONTENT), "{layout:?}");
                })
                .unwrap()
                .join()
                .unwrap();
        }
        assert_success(command.output().unwrap());
        let mut recording = Recording::default();
        recording.read(&path);
        assert_eq!(
            recording.grid_and_marker(CONTENT),
            Some(if explicit { (96, 26) } else { (120, 40) })
        );
    }
}

#[test]
fn native_startup_layout_error_and_explicit_size_retry() {
    let fixture = NativeFixture::new();
    fs::write(
        &fixture.config,
        "detachable_panes false\nload_plugins clear-defaults=true\nterminal { initial_rows 60; }\n",
    )
    .unwrap();
    fs::write(
        &fixture.layout,
        r#"layout {
        pane size=45 command="/bin/sh" { args "-c" "printf native-frame-marker; exec /bin/sh"; }
        pane;
    }"#,
    )
    .unwrap();
    let path = fixture.fixture.path().join("opening.frames");
    let failed = fixture
        .terminal("bad-opening", true, &path)
        .args(["--headless", "--duration-secs", "3"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert_eq!(failed.status.code(), Some(1), "{stderr}");
    for expected in [
        "120 columns by 40 rows",
        "Not enough room for panes",
        "--rows or --cols",
    ] {
        assert!(stderr.contains(expected), "{stderr}");
    }
    assert!(fs::metadata(&path).unwrap().len() == 0);

    assert_success(
        fixture
            .terminal("good-opening", true, &path)
            .args(["--headless", "--rows", "60", "--duration-secs", "3"])
            .output()
            .unwrap(),
    );
    let mut recording = Recording::default();
    recording.read(&path);
    assert_eq!(recording.grid_and_marker(CONTENT), Some((120, 60)));

    // A failed tab alongside a usable one stays recoverable.
    fs::write(
        &fixture.layout,
        r#"layout {
        tab name="failed" { pane size=45; pane; }
        tab name="working" focus=true {
            pane command="/bin/sh" { args "-c" "printf native-frame-marker; exec /bin/sh"; }
        }
    }"#,
    )
    .unwrap();
    assert_success(
        fixture
            .terminal("partial-opening", true, &path)
            .args(["--headless", "--duration-secs", "3"])
            .output()
            .unwrap(),
    );
    let mut recording = Recording::default();
    recording.read(&path);
    assert_eq!(recording.grid_and_marker(CONTENT), Some((120, 40)));
}

#[test]
fn graphical_capture_create_rejects_invalid_destination_before_session_startup() {
    let fixture = NativeFixture::new();
    let root = fixture.fixture.path();
    let session = "invalid-recording";
    let socket = fixture
        .fixture
        .socket_dir()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
        .join(session);
    for path in [root.to_owned(), root.join("missing/capture.frames")] {
        let output = fixture
            .terminal(session, true, &path)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .arg("--duration-secs")
            .arg("1")
            .output()
            .unwrap();
        let message = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{message}");
        assert!(
            message.contains(&format!(
                "cannot record native frames to {}",
                path.display()
            )),
            "{message}"
        );
        assert!(!socket.try_exists().unwrap());
    }
    assert!(!root.join("missing").try_exists().unwrap());
}

#[test]
fn native_startup_reports_config_source_location() {
    let fixture = NativeFixture::new();
    let path = fixture.fixture.path().join("diagnostic.frames");
    for axis in ["initial_rows", "initial_columns"] {
        for (nodes, detail) in [
            (
                format!("// café\r\nterminal {{\r\n {axis} 1001\r\n}}"),
                "between 2 and 1000",
            ),
            (
                format!("terminal {{\n // quoted\n {axis} \"80\"\n}}"),
                "between 2 and 1000",
            ),
            (
                format!("terminal {{\n {axis} 80\n {axis} 90\n}}"),
                "duplicate terminal setting",
            ),
        ] {
            fs::write(&fixture.config, nodes).unwrap();
            // The graphical command reports invalid configuration before opening a display.
            for headless in [false, true] {
                let mut command = fixture.terminal("bad-config", true, &path);
                if headless {
                    command.arg("--headless");
                }
                let failed = command.output().unwrap();
                let stderr = String::from_utf8_lossy(&failed.stderr);
                assert_eq!(failed.status.code(), Some(1), "{stderr}");
                assert!(
                    stderr.contains(&format!("{}:3:", fixture.config.display())),
                    "{stderr}"
                );
                assert!(stderr.contains(axis) && stderr.contains(detail), "{stderr}");
            }
        }
    }
    fs::write(&fixture.config, "detachable_panes false\nload_plugins clear-defaults=true\nterminal { initial_columns 100; initial_rows 30; }\n").unwrap();
    let drop_ins = fixture.config_dir.join("config.d");
    fs::create_dir(&drop_ins).unwrap();
    let invalid = drop_ins.join("90-invalid.kdl");
    fs::write(
        &invalid,
        "// café\r\nterminal {\r\n initial_columns 1001\r\n initial_rows 60\r\n}\r\n",
    )
    .unwrap();
    assert_success(
        fixture
            .terminal("skip-drop-in", true, &path)
            .args(["--headless", "--duration-secs", "3"])
            .output()
            .unwrap(),
    );
    let log = fixture.log_text();
    assert!(
        log.contains(&format!(
            "Skipping config drop-in {}:3: terminal initial_columns",
            invalid.display()
        )),
        "{log}"
    );
    let loaded =
        yinz_config::config::Config::load(Some(&fixture.config), Some(&fixture.config_dir), false)
            .unwrap();
    assert_eq!(
        (
            loaded.terminal.initial_columns,
            loaded.terminal.initial_rows
        ),
        (Some(100), Some(30))
    );
    let mut recording = Recording::default();
    recording.read(&path);
    assert_eq!(recording.grid_and_marker(CONTENT), Some((120, 40)));
}

#[test]
fn native_startup_axes_save_preserves_live_frame() {
    let fixture = NativeFixture::new();
    let path = fixture.fixture.path().join("live.frames");
    let session = "live-frame";
    let mut child = fixture
        .terminal(session, true, &path)
        .args(["--headless", "--duration-secs", "12"])
        .spawn()
        .unwrap();
    let mut recording = Recording::default();
    while_running(
        &mut child,
        "initial pane marker",
        Duration::from_secs(5),
        || {
            recording.read(&path);
            recording.grid_and_marker(CONTENT)
        },
    );
    let baseline = recording.frames.len();
    let geometry = recording.state.as_ref().unwrap().geometry().clone();
    let baseline_seq = recording.frames[baseline - 1].seq;
    let log_marker = "native recording received ConfigFileUpdated";
    let notifications = || fixture.log_text().matches(log_marker).count();

    let overlay = fixture.config_dir.join("config.d");
    fs::create_dir(&overlay).unwrap();
    fs::write(
        overlay.join("10-axes.kdl"),
        "terminal { initial_columns 102; }",
    )
    .unwrap();
    while_running(
        &mut child,
        "watched axes edit notification",
        Duration::from_secs(4),
        || (notifications() > 0).then_some(()),
    );
    let original = fs::read_to_string(&fixture.config).unwrap();
    fs::write(
        &fixture.config,
        "// invalid edit\nterminal {\n initial_rows 1001\n}",
    )
    .unwrap();
    while_running(
        &mut child,
        "watcher to diagnose invalid startup axis",
        Duration::from_secs(4),
        || {
            fixture
                .log_text()
                .contains(&format!(
                    "{}:3: terminal initial_rows",
                    fixture.config.display()
                ))
                .then_some(())
        },
    );
    fs::write(&fixture.config, original).unwrap();
    let before_saved = notifications();
    fixture.action(
        session,
        &[
            "reconfigure",
            "--write",
            "--kdl",
            "terminal { initial_rows 32; }",
        ],
    );
    while_running(
        &mut child,
        "managed axes save notification",
        Duration::from_secs(4),
        || (notifications() > before_saved).then_some(()),
    );
    fixture.action(
        session,
        &["write-chars", "printf '\\npost-save-%s\\n' marker\n"],
    );
    assert_eq!(
        while_running(
            &mut child,
            "post-save pane marker",
            Duration::from_secs(4),
            || {
                recording.read(&path);
                recording.grid_and_marker("post-save-marker")
            }
        ),
        (120, 40)
    );
    assert_eq!(recording.grid_and_marker(CONTENT), Some((120, 40)));
    assert_eq!(recording.state.as_ref().unwrap().geometry(), &geometry);
    for (seq, record) in &recording.geometry_records {
        if *seq > baseline_seq {
            assert_eq!(
                record, &geometry,
                "startup axes save changed pixel geometry in frame {seq}"
            );
        }
    }
    for header in &recording.frames[baseline..] {
        assert!(
            !header.clear(),
            "startup axes save caused a clear frame: {header:?}"
        );
        assert_eq!((header.cols, header.rows), (120, 40));
    }
    assert_success(
        fixture
            .command()
            .args(["kill-session", session])
            .output()
            .unwrap(),
    );
    assert_success(child.wait_with_output().unwrap());
}

#[derive(Debug, serde::Deserialize)]
struct WindowObservation {
    backend: String,
    x11_id: Option<String>,
    output: bool,
    scale: f64,
    width: u32,
    height: u32,
    cell_width: u32,
    cell_height: u32,
    padding: u32,
    cols: u16,
    rows: u16,
    maximized: bool,
    fullscreen: bool,
}

impl WindowObservation {
    fn mode_applied(&self, mode: &str, x11_state: Option<&str>) -> Option<bool> {
        let (maximized, fullscreen) = match self.backend.as_str() {
            "wayland" => (self.maximized, self.fullscreen),
            "x11" => {
                let state = x11_state?;
                (
                    state.contains("_NET_WM_STATE_MAXIMIZED_VERT")
                        && state.contains("_NET_WM_STATE_MAXIMIZED_HORZ"),
                    state.contains("_NET_WM_STATE_FULLSCREEN"),
                )
            },
            _ => return None,
        };
        Some(match mode {
            "maximized" => maximized && !fullscreen,
            "fullscreen" => fullscreen,
            _ => !maximized && !fullscreen,
        })
    }

    fn has_settled_content(&self, recording: &Recording) -> bool {
        recording.grid_and_marker(CONTENT) == Some((self.cols, self.rows))
    }

    fn has_observed_scale(&self) -> bool {
        // Wayland can deliver the surface scale before assigning a wl_output.
        // Winit starts at 1×, so only a non-default scale is conclusive without an output.
        self.output || (self.backend == "wayland" && self.scale != 1.0)
    }

    fn expected_grid(&self, mode: &str) -> (u16, u16) {
        if mode == "remember" {
            let saved = 800.0;
            let physical = if self.backend == "wayland" {
                // Whole logical pixels, preferring the smaller of equal physical distances.
                let logical = saved / self.scale;
                let lower = (logical.floor() * self.scale).round();
                let upper = (logical.ceil() * self.scale).round();
                if saved - lower <= upper - saved {
                    lower
                } else {
                    upper
                }
            } else {
                saved
            };
            (
                ((physical as u32).saturating_sub(2 * self.padding) / self.cell_width) as u16,
                30,
            )
        } else {
            (100, 30)
        }
    }
}

#[test]
#[ignore = "requires a graphical desktop"]
fn native_graphical_startup_fits_configured_grid() {
    let mut outcomes = Vec::new();
    for mode in ["windowed", "remember", "maximized", "fullscreen"] {
        let fixture = NativeFixture::new();
        let path = fixture.fixture.path().join("desktop.frames");
        let mut command = fixture.terminal(mode, true, &path);
        command.args(["--startup-mode", mode, "--duration-secs", "5"]);
        if mode == "remember" {
            fs::write(
                fixture.cache.join("native-terminal-window.json"),
                "[800,600]",
            )
            .unwrap();
            command.args(["--rows", "30"]);
        }
        // Protocol logging can fill a pipe while we observe the running window.
        let stderr_path = fixture.fixture.path().join("desktop.stderr");
        command.stderr(fs::File::create(&stderr_path).unwrap());
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut recording = Recording::default();
        let mut x11_state = None;
        let read_observation = || {
            fixture.log_text().lines().rev().find_map(|line| {
                let (_, json) = line.split_once("native window observation ")?;
                serde_json::from_str::<WindowObservation>(json.trim()).ok()
            })
        };
        // An applied configure can follow an otherwise usable provisional grid.
        // X11 state must be read while the window still exists.
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                break;
            }
            recording.read(&path);
            if let Some(observation) = read_observation()
                && observation.has_settled_content(&recording)
                && let Some(id) = observation.x11_id.as_ref()
                && let Ok(output) = Command::new("xprop")
                    .args(["-id", id, "_NET_WM_STATE"])
                    .output()
                && output.status.success()
            {
                x11_state = Some(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            std::thread::park_timeout(Duration::from_millis(20));
        }
        let mut result = child.wait_with_output().unwrap();
        result.stderr = fs::read(stderr_path).unwrap();
        if std::env::var_os("WAYLAND_DEBUG").is_some() {
            for line in String::from_utf8_lossy(&result.stderr)
                .lines()
                .filter(|line| {
                    line.contains(".preferred_scale")
                        || line.contains(".set_maximized")
                        || line.contains(".set_fullscreen")
                        || line.contains(".wm_capabilities")
                })
            {
                eprintln!("{mode}: {line}");
            }
        }
        assert_success(result);
        recording.read(&path);
        let opening = recording
            .frames
            .first()
            .map(|header| (header.cols, header.rows));
        assert_eq!(
            opening,
            Some((120, if mode == "remember" { 30 } else { 40 })),
            "{mode} provisional grid"
        );
        let Some(observation) = read_observation() else {
            outcomes.push(format!(
                "{mode}: no window observation, pane grid={:?}",
                recording.grid_and_marker(CONTENT)
            ));
            continue;
        };
        let applied = observation.mode_applied(mode, x11_state.as_deref());
        let observed = (observation.cols, observation.rows);
        if !observation.has_settled_content(&recording) {
            outcomes.push(format!(
                "{mode}: no settled pane content matching {observation:?}, pane grid={:?}",
                recording.grid_and_marker(CONTENT)
            ));
            continue;
        }
        assert_eq!(
            observed,
            (
                (observation.width.saturating_sub(2 * observation.padding) / observation.cell_width)
                    .clamp(1, 4096) as u16,
                (observation.height.saturating_sub(2 * observation.padding)
                    / observation.cell_height)
                    .clamp(1, 4096) as u16,
            )
        );
        let exact_requested = std::env::var_os("YINZ_NATIVE_TEST_FLOATING").is_some();
        let exact_case = matches!(mode, "windowed" | "remember");
        let expected = observation.expected_grid(mode);
        eprintln!("{mode}: {observation:?}, applied mode={applied:?}, expected={expected:?}");
        if exact_requested
            && (!observation.has_observed_scale()
                || applied == Some(false)
                || (exact_case && applied == Some(true) && observed != expected))
        {
            outcomes.push(format!(
                "{mode}: applied={applied:?}, settled={observed:?}, expected={expected:?}"
            ));
        } else if !observation.has_observed_scale() {
            eprintln!("{mode}: output scale unverified");
        } else if applied == Some(false) {
            eprintln!("{mode}: desktop did not apply the requested mode");
        } else if applied.is_none() {
            eprintln!("{mode}: applied mode needs independent desktop confirmation");
        } else if exact_case && (!exact_requested || observed != expected) {
            eprintln!("{mode}: grant observed, exact floating fit unverified");
        }
    }
    assert!(outcomes.is_empty(), "{}", outcomes.join("\n"));
}

#[test]
fn native_graphical_observation_requires_settled_content_and_mode() {
    use yinz_types::structured_render::{FrameBuilder, WireCell};
    let mut recording = Recording::default();
    let mut state = TerminalState::new(40, 120);
    let mut first = FrameBuilder::new(120, 40, 1);
    let line: Vec<_> = CONTENT
        .chars()
        .map(|ch| WireCell {
            ch: ch as u32,
            ..WireCell::BLANK
        })
        .collect();
    first.push_row(0, 0, &line);
    state.apply_frame(&first.finish()).unwrap();
    recording.state = Some(state);
    let mut observed = WindowObservation {
        backend: "wayland".into(),
        x11_id: None,
        output: true,
        scale: 2.0,
        width: 1616,
        height: 1216,
        cell_width: 16,
        cell_height: 40,
        padding: 8,
        cols: 100,
        rows: 30,
        maximized: false,
        fullscreen: false,
    };
    assert!(!observed.has_settled_content(&recording));
    let mut settled = FrameBuilder::new(100, 30, 2);
    settled.set_full_repaint(true);
    settled.set_clear(true);
    recording
        .state
        .as_mut()
        .unwrap()
        .apply_frame(&settled.finish())
        .unwrap();
    assert!(!observed.has_settled_content(&recording));
    let mut settled = FrameBuilder::new(100, 30, 3);
    settled.push_row(0, 0, &line);
    recording
        .state
        .as_mut()
        .unwrap()
        .apply_frame(&settled.finish())
        .unwrap();
    assert!(observed.has_settled_content(&recording));
    observed.output = false;
    assert!(observed.has_observed_scale());
    observed.scale = 1.0;
    assert!(!observed.has_observed_scale());
    observed.output = true;
    assert!(observed.has_observed_scale());
    assert_eq!(observed.mode_applied("maximized", None), Some(false));
    observed.maximized = true;
    assert_eq!(observed.mode_applied("maximized", None), Some(true));
    assert_eq!(observed.mode_applied("fullscreen", None), Some(false));
    observed.backend = "x11".into();
    assert_eq!(observed.mode_applied("fullscreen", None), None);
    assert_eq!(
        observed.mode_applied("fullscreen", Some("_NET_WM_STATE_FULLSCREEN")),
        Some(true)
    );
}

#[cfg(feature = "native-terminal")]
fn native_startup_with_config(
    dir: &std::path::Path,
    config: &std::path::Path,
) -> std::process::Output {
    use std::time::{Duration, Instant};

    let mut child = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .env("HOME", dir)
        .env("TMPDIR", dir)
        .env("YINZ_TEST_CACHE_DIR", dir.join("cache"))
        .env("YINZ_SOCKET_DIR", dir)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("DISPLAY")
        .arg("--config")
        .arg(config)
        .arg("--config-dir")
        .arg(dir)
        .args(["terminal", "invalid-underline-config"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + yinz_test_support::default_timeout();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().unwrap(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            status => {
                let _ = child.kill();
                let output = child.wait_with_output();
                panic!(
                    "native startup did not reject {}: {status:?}, {output:?}",
                    config.display()
                );
            },
        }
    }
}

#[cfg(feature = "native-terminal")]
#[test]
fn underline_metrics_native_startup_reports_invalid_setting() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.kdl");
    fs::write(&config, "terminal { underline_thickness 65; }").unwrap();
    let output = native_startup_with_config(dir.path(), &config);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        config.to_str().unwrap(),
        "underline_thickness",
        "between -64 and 64",
    ] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[cfg(feature = "native-terminal")]
#[test]
fn underline_metrics_native_startup_reports_nonregular_main() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    fs::create_dir(&private).unwrap();
    let config = private.join("config.kdl");
    let name = std::ffi::CString::new(config.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { nix::libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let output = native_startup_with_config(dir.path(), &config);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        config.to_str().unwrap(),
        "not a regular file",
        "Restore a readable regular file",
    ] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn underline_metrics_saved_reconfigure_keeps_the_session_responsive_after_fifo_refusal() {
    use std::{
        cell::RefCell,
        os::unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, OpenOptionsExt},
        },
        process::Output,
        time::{Duration, Instant},
    };
    use yinz_config::config::{Config, MANAGED_CONFIG_FILE_NAME};

    // Drop the reader after the fixture so a regressed server can finish opening the lock.
    let lock_reader = RefCell::new(None);
    let fixture = real_fixture::RealFixture::new("underline-");
    let root = fixture.path();
    for directory in ["home", "tmp", "config.d"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    let main = root.join("config.kdl");
    fs::write(
        &main,
        "detachable_panes false\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    let layout = root.join("layout.kdl");
    fs::write(&layout, "layout { pane command=\"/bin/sh\"; }").unwrap();
    let managed = root.join("config.d").join(MANAGED_CONFIG_FILE_NAME);
    let lock = managed.with_file_name(".yinz-managed.kdl.lock");
    let run = |args: &[&str]| -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
        fixture.inherit(&mut command);
        for key in ["YINZ", "YINZ_SESSION_NAME", "YINZ_PANE_ID", "YINZ_PANE_REF"] {
            command.env_remove(key);
        }
        let mut child = command
            .env("HOME", root.join("home"))
            .env("TMPDIR", root.join("tmp"))
            .env("YINZ_SOCKET_DIR", fixture.socket_dir())
            .env("YINZ_TEST_CACHE_DIR", root.join("cache"))
            .env("SHELL", "/bin/sh")
            .current_dir(root)
            .arg("--config-dir")
            .arg(root)
            .arg("--config")
            .arg(&main)
            .arg("--data-dir")
            .arg(root.join("data"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + yinz_test_support::default_timeout();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return child.wait_with_output().unwrap(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                },
                status => {
                    eprintln!("CLI failed to complete {args:?}: {status:?}");
                    let _ = child.kill();
                    let _ = child.wait();
                    // Pair only after recording the failure, so the owned fixture
                    // guard can still shut the session down.
                    if managed.metadata().is_ok_and(|m| m.file_type().is_fifo()) {
                        let _ = fs::OpenOptions::new()
                            .write(true)
                            .custom_flags(nix::libc::O_NONBLOCK)
                            .open(&managed);
                    }
                    if lock.metadata().is_ok_and(|m| m.file_type().is_fifo()) {
                        *lock_reader.borrow_mut() = fs::OpenOptions::new()
                            .read(true)
                            .custom_flags(nix::libc::O_NONBLOCK)
                            .open(&lock)
                            .ok();
                    }
                    panic!("CLI failed to complete {args:?}: {status:?}");
                },
            }
        }
    };
    let started = run(&[
        "--layout",
        layout.to_str().unwrap(),
        "attach",
        "--create-background",
        "underline-test",
    ]);
    assert!(started.status.success(), "{started:?}");

    let reconfigure = |fragment: &str, write: bool| {
        let mut args = vec![
            "--session",
            "underline-test",
            "action",
            "reconfigure",
            "--kdl",
            fragment,
        ];
        if write {
            args.push("--write");
        }
        run(&args)
    };
    // A completed reconfigure establishes readiness on the same instruction loop.
    let ready = reconfigure("pane_frames true", false);
    assert!(ready.status.success(), "{ready:?}");
    let fragment = "terminal { underline_offset -2; }\nmouse_mode false";
    let original = "terminal { underline_thickness 3; }";
    for (fifo, detail, later_fragment) in [
        (&managed, "not a regular file", "pane_frames false"),
        (&lock, "regular lock file", "pane_frames true"),
    ] {
        if fifo == &lock {
            fs::remove_file(&lock).unwrap();
            fs::write(&managed, original).unwrap();
        }
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { nix::libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let refused = reconfigure(fragment, true);
        // The CLI maps a server action error with a message to exit status 2.
        assert_eq!(refused.status.code(), Some(2), "{refused:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        for expected in [fifo.to_str().unwrap(), detail, "then retry"] {
            assert!(stderr.contains(expected), "{stderr}");
        }
        assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo());
        if fifo == &lock {
            assert_eq!(fs::read_to_string(&managed).unwrap(), original);
        }
        let later = reconfigure(later_fragment, false);
        assert!(later.status.success(), "{later:?}");
        assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo());

        fs::remove_file(fifo).unwrap();
        if fifo == &managed {
            fs::write(&managed, original).unwrap();
        }
        let saved = reconfigure(fragment, true);
        assert!(saved.status.success(), "{saved:?}");
        let saved = Config::from_kdl(&fs::read_to_string(&managed).unwrap(), None).unwrap();
        assert_eq!(saved.terminal.underline_offset, Some(-2));
        assert_eq!(saved.terminal.underline_thickness, Some(3));
        assert_eq!(saved.options.mouse_mode, Some(false));
        assert!(lock.symlink_metadata().unwrap().is_file());
    }
}

#[test]
fn native_headless_uses_builtin_despite_extensionless_collision() {
    let fixture = NativeFixture::new();
    let layouts = fixture.config_dir.join("layouts");
    fs::create_dir(&layouts).unwrap();
    fs::write(
        layouts.join("default"),
        r#"layout { pane command="/bin/sh" { args "-c" "printf wrong-layout-marker; exec /bin/sh"; }; }"#,
    )
    .unwrap();
    let shell = fixture.fixture.path().join("native-shell");
    yinz_test_support::write_warm_script(&shell, "printf native-frame-marker\nexec /bin/sh");
    fs::write(
        &fixture.config,
        format!(
            "detachable_panes false\nload_plugins clear-defaults=true\ndefault_shell {:?}\n",
            shell.to_str().unwrap()
        ),
    )
    .unwrap();

    let path = fixture.fixture.path().join("builtin.frames");
    assert_success(
        fixture
            .command()
            .args([
                "terminal",
                "native-builtin",
                "--create",
                "--headless",
                "--duration-secs",
                "3",
                "--record",
            ])
            .arg(&path)
            .output()
            .unwrap(),
    );
    let mut recording = Recording::default();
    recording.read(&path);
    assert!(recording
        .frames
        .first()
        .is_some_and(FrameHeader::full_repaint));
    assert_eq!(recording.grid_and_marker(CONTENT), Some((120, 40)));
    assert!(recording.grid_and_marker("wrong-layout-marker").is_none());
}

#[path = "native_terminal/negotiation.rs"]
mod negotiation;

#[test]
fn native_refusal_prefix_headless() {
    negotiation::refusal();
}
