//! Opt-in tests use a checksum-verified published executable, never a version mock.
use super::*;
use std::{
    io::{Read, Write},
    process::Child,
};

fn previous() -> PathBuf {
    std::env::var_os("YINZ_PREVIOUS_RELEASE")
        .map(PathBuf::from)
        .expect("set YINZ_PREVIOUS_RELEASE to the checksum-verified published executable")
}

fn command(session: &Session, binary: &Path) -> Command {
    let original = session.command();
    let mut command = Command::new(binary);
    session.0.inherit(&mut command);
    command
        .env_clear()
        .envs(original.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))))
        .args(original.get_args())
        .current_dir(original.get_current_dir().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

struct Client {
    child: Child,
    master: fs::File,
    reader: Option<std::thread::JoinHandle<()>>,
    output: yinz_test_support::Watched<Vec<u8>>,
}
impl Client {
    fn attach(session: &Session, binary: &Path) -> Self {
        let pty = nix::pty::openpty(
            Some(&nix::pty::Winsize {
                ws_row: 24,
                ws_col: 100,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let slave = fs::File::from(pty.slave);
        let master = fs::File::from(pty.master);
        let child = command(session, binary)
            .args(["attach", NAME])
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave)
            .spawn()
            .unwrap();
        let output = yinz_test_support::Watched::new(Vec::new());
        let captured = output.clone();
        let mut read = master.try_clone().unwrap();
        let reader = std::thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n) = read.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut out = captured.lock();
                out.extend_from_slice(&buf[..n]);
                let excess = out.len().saturating_sub(1024 * 1024);
                if excess > 0 {
                    out.drain(..excess);
                }
            }
        });
        Self {
            child,
            master,
            reader: Some(reader),
            output,
        }
    }
    fn wait(&self, marker: &str) {
        self.output
            .wait_until(marker, Duration::from_secs(45), |bytes| {
                String::from_utf8_lossy(bytes)
                    .contains(marker)
                    .then_some(())
            });
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

#[test]
#[ignore = "requires checksum-verified published YINZ_PREVIOUS_RELEASE binary"]
fn previous_release_interactive_clients_and_servers_interoperate_both_directions() {
    let older = previous();
    let current = Path::new(env!("CARGO_BIN_EXE_yinz"));
    for (server, client) in [(older.as_path(), current), (current, older.as_path())] {
        let session = Session::new();
        let layout = r#"layout { pane command="/bin/sh" { args "-c" "printf 'PREVIOUS-RELEASE-READY\\n'; while IFS= read -r line; do printf 'PANE-RESPONSE:%s\\n' \"$line\"; done"; }; }"#;
        let output = command(&session, server)
            .args([
                "--layout-string",
                layout,
                "attach",
                "--create-background",
                NAME,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut attached = Client::attach(&session, client);
        attached.wait("PREVIOUS-RELEASE-READY");
        attached.master.write_all(b"INTEROPERABLE\r").unwrap();
        attached.wait("PANE-RESPONSE:INTEROPERABLE");
        let output = command(&session, client)
            .args(["--session", NAME, "action", "list-panes", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let panes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(panes.as_array().is_some_and(|panes| !panes.is_empty()));
    }
}

#[test]
#[ignore = "requires checksum-verified published YINZ_PREVIOUS_RELEASE binary"]
fn previous_release_bootstraps_enforce_contracts_and_new_bootstrap_accepts_legacy_clients() {
    let older = previous();
    let current = Path::new(env!("CARGO_BIN_EXE_yinz"));
    let session = Session::new();
    for binary in [older.as_path(), current] {
        let output = command(&session, binary)
            .args([
                "remote-sessions",
                "--client-version",
                "0.1.1",
                "--contract-version",
                &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
                "--bootstrap-version",
                &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        yinz_client::remote_sessions::parse_sessions("fixture", &output.stdout).unwrap();
        let output = command(&session, binary)
            .args([
                "remote-session",
                "--create",
                "--client-version",
                "0.1.1",
                "--contract-version",
                &(yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION + 1).to_string(),
                "--bootstrap-version",
                &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
                NAME,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            !session.socket().exists(),
            "incompatible bootstrap created a session"
        );
    }
}

#[test]
#[ignore = "requires checksum-verified published YINZ_PREVIOUS_RELEASE binary"]
fn previous_release_live_server_refuses_builtin_publication_without_disrupting_session() {
    let session = Session::new();
    let older = previous();
    let current = Path::new(env!("CARGO_BIN_EXE_yinz"));
    let output = command(&session, &older)
        .args(["attach", "--create-background", NAME])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let operation = uuid::Uuid::new_v4().to_string();
    let original = session.command();
    let input = serde_json::to_vec(&yinz_types::update_client::ParticipantInvocation {
        executable: current.to_owned(),
        data_dir: Some(session.0.path().join("data/yinz")),
        request: yinz_types::update_client::ParticipantRequest::Prepare {
            operation: operation.clone(),
            release: serde_json::json!({"tag_name":yinz_runtime::consts::VERSION,"assets":[]}),
        },
    })
    .unwrap();
    let mut helper = Command::new(current)
        .env_clear()
        .envs(
            original
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .arg(yinz_types::update_client::PARTICIPANT_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    helper.stdin.take().unwrap().write_all(&input).unwrap();
    let output = helper.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let refusal: Result<serde_json::Value, String> =
        serde_json::from_slice(&output.stdout).unwrap();
    assert!(refusal
        .unwrap_err()
        .contains("cannot preserve its builtin generation"));
    assert!(!session
        .0
        .path()
        .join("data/yinz/updates")
        .join(operation)
        .exists());
    let output = command(&session, current)
        .args(["--session", NAME, "action", "list-panes", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "old session must remain usable: {output:?}"
    );
}

#[test]
#[ignore = "requires checksum-verified published YINZ_PREVIOUS_RELEASE binary"]
fn previous_release_supervisor_adopts_without_restarting_command_and_refuses_wrong_contract() {
    use yinz_process::pty_host::{client, protocol};
    use yinz_types::{input::command::RunCommand, internal_ids::TerminalId};
    let older = previous();
    let command = RunCommand {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'OLD-SUPERVISOR-READY\\n'; while IFS= read -r line; do printf 'OLD-CHILD:%s\\n' \"$line\"; done".into()],
        ..Default::default()
    };
    let spawned = client::spawn(client::SpawnParams {
        host_bin: &older,
        terminal_id: TerminalId::new(1),
        session_name: Some("previous-release-source".into()),
        cmd: command,
        termios: None,
        initial_size: Some(protocol::WireWinsize {
            cols: 100,
            rows: 24,
            width_in_pixels: 0,
            height_in_pixels: 0,
        }),
        arg0: None,
        env: Vec::new(),
        login_session: false,
        job_control: true,
    })
    .unwrap()
    .finish(Box::new(|_, _, _| {}), None);
    let pid = spawned.child_pid;
    let released = spawned.handle.release();
    drop(spawned.master);
    let socket = released.control_socket.unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = client::terminate_via_socket(&self.0);
        }
    }
    let _cleanup = Cleanup(socket.clone());
    let mut wrong = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    wrong
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    protocol::write_msg(
        &mut wrong,
        &protocol::ServerToHostMsg::Attach {
            protocol_version: protocol::PROTOCOL_VERSION + 1,
            yinz_version: "0.1.1".into(),
            terminal_id: 2,
            session_name: Some("previous-release-destination".into()),
        },
    )
    .unwrap();
    assert!(matches!(
        protocol::read_msg::<protocol::HostToServerMsg, _>(&mut wrong).unwrap(),
        Some(protocol::HostToServerMsg::AttachRefused {
            reason: protocol::AttachRefusal::ProtocolMismatch,
            ..
        })
    ));
    let adopted = client::attach(client::AttachParams {
        control_socket: &socket,
        terminal_id: TerminalId::new(3),
        session_name: Some("previous-release-destination".into()),
    })
    .unwrap()
    .finish(Box::new(|_, _, _| {}), None);
    assert_eq!(adopted.child_pid, pid);
    assert_eq!(
        adopted.status.session_name.as_ref().unwrap().as_str(),
        "previous-release-destination"
    );
    let mut handle = adopted.handle;
    let mut writer = fs::File::from(adopted.master);
    // Detached supervisors make the shared master nonblocking to drain their
    // replay ring. This dedicated test reader uses blocking I/O after adoption.
    let flags = nix::fcntl::OFlag::from_bits_truncate(
        nix::fcntl::fcntl(&writer, nix::fcntl::FcntlArg::F_GETFL).unwrap(),
    );
    nix::fcntl::fcntl(
        &writer,
        nix::fcntl::FcntlArg::F_SETFL(flags & !nix::fcntl::OFlag::O_NONBLOCK),
    )
    .unwrap();
    let mut reader = writer.try_clone().unwrap();
    let output = yinz_test_support::Watched::new(adopted.buffered);
    let captured = output.clone();
    let read = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            captured.lock().extend_from_slice(&buffer[..n]);
        }
    });
    writer.write_all(b"AFTER-ADOPTION\n").unwrap();
    output.wait_until_described(
        "old command to respond after adoption",
        Duration::from_secs(30),
        |bytes| {
            String::from_utf8_lossy(bytes)
                .contains("OLD-CHILD:AFTER-ADOPTION")
                .then_some(())
        },
        |bytes| String::from_utf8_lossy(bytes).into_owned(),
    );
    handle.terminate();
    drop(writer);
    read.join().unwrap();
}
