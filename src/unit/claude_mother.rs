use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use yinz_config::project_config::bubblewrap::{
    mother_directory, Environment, LaunchCheck, MotherSetupError,
};
use yinz_process::pty_host::client::{self, PtyHostHandle, SpawnParams};
use yinz_test_support::Watched;
use yinz_types::command::PaneTarget;
use yinz_types::input::command::{ClaudeSettings, RunCommand, CLAUDE_SETTINGS_SNAPSHOT};
use yinz_types::internal_ids::TerminalId;

struct Mother {
    handle: PtyHostHandle,
    input: File,
    output: Watched<String>,
    exit: Watched<Option<Option<i32>>>,
}

impl Mother {
    fn start(profile: &Environment, cwd: &Path, settings: ClaudeSettings, script: &str) -> Self {
        let exit = Watched::new(None);
        let exited = exit.clone();
        let spawned = client::spawn(SpawnParams {
            host_bin: Path::new(env!("CARGO_BIN_EXE_yinz")),
            terminal_id: TerminalId::new(1),
            session_name: None,
            cmd: RunCommand {
                command: "/bin/sh".into(),
                args: vec!["-eu".into(), "-c".into(), script.into()],
                cwd: Some(cwd.into()),
                target: PaneTarget::bubblewrap(profile.root()).unwrap(),
                ..Default::default()
            },
            failover_cmd: None,
            termios: None,
            initial_size: None,
            arg0: None,
            env: vec![(CLAUDE_SETTINGS_SNAPSHOT.into(), settings.0)],
            login_session: false,
            job_control: false,
        })
        .unwrap()
        .finish(
            Box::new(move |_, status, command| {
                assert!(command.claude_settings.is_none());
                *exited.lock() = Some(status);
            }),
            None,
        );
        let input = File::from(spawned.master);
        let mut reader = input.try_clone().unwrap();
        let output = Watched::new(String::new());
        let observed = output.clone();
        std::thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(len) = reader.read(&mut buf) {
                if len == 0 {
                    break;
                }
                observed
                    .lock()
                    .push_str(&String::from_utf8_lossy(&buf[..len]));
            }
        });
        Self {
            handle: spawned.handle,
            input,
            output,
            exit,
        }
    }

    fn wait_for(&self, text: &str) {
        self.output.wait_until_described(
            text,
            Duration::from_secs(30),
            |output| output.contains(text).then_some(()),
            |output| output.clone(),
        );
    }
}

impl Drop for Mother {
    fn drop(&mut self) {
        self.handle.terminate();
    }
}

#[test]
#[ignore = "requires real Bubblewrap and FUSE; run explicitly on Linux"]
fn real_claude_launch_reuses_snapshot_and_shares_live_credentials() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let fixture = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let home = fixture.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("YINZ_SOCKET_DIR", home.join("sockets"));
    std::env::set_var("YINZ_SESSION_NAME", "claude-test");
    std::env::remove_var("YINZ_PANE_REF");
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(home.join(".claude.json"), r#"{"snapshot":true}"#).unwrap();
    fs::write(home.join(".claude/.credentials.json"), "host").unwrap();
    fs::write(home.join(".claude/settings.json"), "host-settings").unwrap();
    let cwd = mother_directory(&home.join("cache"), "test").unwrap();
    let profile = Environment::at_root(&cwd).prepare_mother("claude").unwrap();
    let LaunchCheck::Ready(Some(snapshot)) = profile.check_launch(&cwd, Path::new("sh")).unwrap()
    else {
        panic!("real sandbox must be available")
    };
    // The supervisor must pass the checked snapshot through to the launcher unchanged.
    fs::write(home.join(".claude.json"), "partially rewritten").unwrap();
    let mut first = Mother::start(
        &profile,
        &cwd,
        snapshot.clone(),
        r#"
        test "$(cat "$CLAUDE_CONFIG_DIR/.claude.json")" = '{"snapshot":true}'
        test -z "${YINZ_CLAUDE_SETTINGS_FD-}${YINZ_CLAUDE_SETTINGS_SNAPSHOT-}"
        printf first > "$CLAUDE_CONFIG_DIR/.credentials.json"
        printf private > "$CLAUDE_CONFIG_DIR/settings.json"
        printf transcript > "$CLAUDE_CONFIG_DIR/projects/test.jsonl"
        echo first-ready
        read release
        test "$(cat "$CLAUDE_CONFIG_DIR/.credentials.json")" = host-replacement
        echo first-finished
        exit 23
    "#,
    );
    first.wait_for("first-ready");
    assert_eq!(
        fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
        "first"
    );
    let mut second = Mother::start(
        &profile,
        &cwd,
        snapshot,
        r#"
        test "$(cat "$CLAUDE_CONFIG_DIR/.credentials.json")" = first
        test "$(cat "$CLAUDE_CONFIG_DIR/settings.json")" = host-settings
        test "$(cat "$CLAUDE_CONFIG_DIR/projects/test.jsonl")" = transcript
        printf second > "$CLAUDE_CONFIG_DIR/credential-stage"
        mv "$CLAUDE_CONFIG_DIR/credential-stage" "$CLAUDE_CONFIG_DIR/.credentials.json"
        echo second-ready
        read release
        test "$(cat "$CLAUDE_CONFIG_DIR/.credentials.json")" = host-replacement
        echo second-finished
    "#,
    );
    second.wait_for("second-ready");
    assert_eq!(
        fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
        "second"
    );
    fs::write(home.join(".claude/new"), "host-replacement").unwrap();
    fs::rename(
        home.join(".claude/new"),
        home.join(".claude/.credentials.json"),
    )
    .unwrap();
    writeln!(first.input, "release").unwrap();
    writeln!(second.input, "release").unwrap();
    first.wait_for("first-finished");
    second.wait_for("second-finished");
    assert_eq!(
        first
            .exit
            .wait_until("first exit", Duration::from_secs(30), |exit| *exit),
        Some(23)
    );
    assert_eq!(
        second
            .exit
            .wait_until("second exit", Duration::from_secs(30), |exit| *exit),
        Some(0)
    );
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        "host-settings"
    );
    assert_eq!(
        fs::read_to_string(home.join(".claude.json")).unwrap(),
        "partially rewritten"
    );
    assert!(profile
        .check_launch(&cwd, Path::new("sh"))
        .unwrap_err()
        .is::<MotherSetupError>());
}
