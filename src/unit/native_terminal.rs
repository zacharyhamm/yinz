//! Native frames from a real session, through the production protobuf writer.
#![cfg(unix)]
use clap::Parser;
use std::{
    fs,
    process::{Command, Stdio},
};
use yinz_types::structured_render::decode;
#[path = "real_fixture.rs"]
mod real_fixture;

#[cfg(feature = "native-terminal")]
#[test]
fn underline_metrics_native_startup_reports_invalid_setting() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.kdl");
    fs::write(&config, "terminal { underline_thickness 65; }").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("YINZ_TEST_CACHE_DIR", dir.path().join("cache"))
        .env("YINZ_SOCKET_DIR", dir.path())
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("DISPLAY")
        .arg("--config")
        .arg(&config)
        .arg("--config-dir")
        .arg(dir.path())
        .args(["terminal", "invalid-underline-config"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
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

#[test]
fn underline_metrics_saved_reconfigure_keeps_the_session_responsive_after_fifo_refusal() {
    use std::{
        os::unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, OpenOptionsExt},
        },
        process::Output,
        time::{Duration, Instant},
    };
    use yinz_config::config::{Config, MANAGED_CONFIG_FILE_NAME};

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
                    // Release a regressed server read only after recording the failure,
                    // so the owned fixture guard can still shut the session down.
                    if managed.metadata().is_ok_and(|m| m.file_type().is_fifo()) {
                        let _ = fs::OpenOptions::new()
                            .write(true)
                            .custom_flags(nix::libc::O_NONBLOCK)
                            .open(&managed);
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
    let name = std::ffi::CString::new(managed.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { nix::libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let fragment = "terminal { underline_offset -2; }\nmouse_mode false";
    let refused = reconfigure(fragment, true);
    // The CLI maps a server action error with a message to exit status 2.
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    for expected in [
        managed.to_str().unwrap(),
        "not a regular file",
        "then retry",
    ] {
        assert!(stderr.contains(expected), "{stderr}");
    }
    let later = reconfigure("pane_frames false", false);
    assert!(later.status.success(), "{later:?}");
    assert!(managed.symlink_metadata().unwrap().file_type().is_fifo());

    fs::remove_file(&managed).unwrap();
    fs::write(&managed, "terminal { underline_thickness 3; }").unwrap();
    let saved = reconfigure(fragment, true);
    assert!(saved.status.success(), "{saved:?}");
    let saved = Config::from_kdl(&fs::read_to_string(&managed).unwrap(), None).unwrap();
    assert_eq!(saved.terminal.underline_offset, Some(-2));
    assert_eq!(saved.terminal.underline_thickness, Some(3));
    assert_eq!(saved.options.mouse_mode, Some(false));
}

#[test]
fn native_headless_creates_and_reattaches_with_structured_content() {
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(native_headless_fixture)
        .unwrap()
        .join()
        .unwrap();
}

fn native_headless_fixture() {
    let fixture = real_fixture::RealFixture::new("native-");
    fs::create_dir(fixture.path().join("home")).unwrap();
    fs::create_dir(fixture.path().join("tmp")).unwrap();
    fs::create_dir(fixture.path().join("data")).unwrap();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &fixture.path().join("data"),
    )
    .unwrap();
    fs::write(
        fixture.path().join("config.kdl"),
        "detachable_panes false\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    fs::write(fixture.path().join("layout.kdl"), "layout { pane command=\"/bin/sh\" { args \"-c\" \"printf native-frame-marker; exec /bin/sh\"; }; }\n").unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
        fixture.inherit(&mut command);
        for key in ["YINZ", "YINZ_SESSION_NAME", "YINZ_PANE_ID", "YINZ_PANE_REF"] {
            command.env_remove(key);
        }
        command
            .env("TMPDIR", fixture.path().join("tmp"))
            .env("YINZ_SOCKET_DIR", fixture.socket_dir())
            .env("HOME", fixture.path().join("home"))
            .env("YINZ_TEST_CACHE_DIR", fixture.path().join("cache"))
            .env("SHELL", "/bin/sh")
            .arg("--config")
            .arg(fixture.path().join("config.kdl"))
            .arg("--data-dir")
            .arg(fixture.path().join("data"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    };
    for create in [true, false] {
        let recording = fixture.path().join(if create {
            "first.frames"
        } else {
            "reattach.frames"
        });
        let mut child = command();
        if create {
            child.arg("--layout").arg(fixture.path().join("layout.kdl"));
        }
        child
            .args([
                "terminal",
                "native-test",
                "--headless",
                "--duration-secs",
                "3",
                "--record",
            ])
            .arg(&recording);
        if create {
            child.arg("--create");
        }
        if create {
            let cli = yinz_cli::args::CliArgs::try_parse_from(
                std::iter::once(std::ffi::OsStr::new("yinz")).chain(child.get_args()),
            )
            .unwrap();
            assert_eq!(cli.layout, Some(fixture.path().join("layout.kdl")));
            let (config, layout, options, _, _) =
                yinz_cli::setup::Setup::from_cli_args(&cli).unwrap();
            let message = yinz_client::native_first_message(
                &cli,
                &options,
                &config.env,
                layout,
                yinz_types::pane_size::Size {
                    rows: 40,
                    cols: 120,
                },
                true,
                None,
            );
            let yinz_ipc::ipc::ClientToServerMsg::FirstClientConnected { cli_assets } = message
            else {
                panic!("expected first client");
            };
            let (layout, _) = yinz_config::layout::from_layout_info_with_config(
                &None,
                cli_assets.layout.as_ref().unwrap(),
                Some(config),
            )
            .unwrap();
            assert!(
                format!("{layout:?}").contains("native-frame-marker"),
                "{layout:?}"
            );
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(recording).unwrap();
        let mut rest = bytes.as_slice();
        let mut content = String::new();
        let mut last_seq = 0;
        while !rest.is_empty() {
            let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            let view = decode(&rest[4..4 + len]).unwrap();
            assert!(view.header().seq > last_seq);
            last_seq = view.header().seq;
            for (_, cells) in view.rows() {
                content.extend(cells.iter().map(|cell| cell.character()));
                content.push('\n');
            }
            rest = &rest[4 + len..];
        }
        assert!(
            content.contains("native-frame-marker"),
            "create={create}, frames={last_seq}, content={:?}",
            content.chars().take(600).collect::<String>()
        );
    }
}
