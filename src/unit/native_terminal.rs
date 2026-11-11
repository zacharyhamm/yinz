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
