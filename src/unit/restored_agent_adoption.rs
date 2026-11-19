//! Production observer enrollment and supervisor retention across real PTY handoff.
use super::*;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use yinz_types::internal_ids::TerminalId;

#[test]
fn production_observer_after_claude_adoption() {
    use yinz_types::agent::{
        report::{AgentReport, AgentReportState},
        AgentInfo, AgentSource, AgentState,
    };
    let session = Session::new();
    let root = session.0.path();
    let claude = root.join("claude");
    yinz_test_support::copy_executable(Path::new("/bin/sleep"), &claude);
    let launcher = root.join("launch");
    yinz_test_support::write_warm_script(
        &launcher,
        r#"if [ "$1" = run ]; then
        echo $$ > "$2/pid"
        export CLAUDE_CONFIG_DIR="$2"
        exec "$2/claude" 90
    fi"#,
    );
    fs::write(
        root.join("config.kdl"),
        "detachable_panes true\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    fs::create_dir(root.join("sessions")).unwrap();
    let layout = format!(
        "layout {{ pane command=\"{}\" {{ args \"run\" \"{}\"; }}; pane; }}",
        launcher.display(),
        root.display()
    );
    session.success(&[
        "--layout-string",
        &layout,
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(false);
    session.success(&["attach", "--create-background", "observer-dest"]);
    let pid_file = root.join("pid");
    let deadline = Instant::now() + Duration::from_secs(30);
    let pid: u32 = loop {
        if let Some(pid) = fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "Claude-shaped child failed to start"
        );
        std::thread::yield_now();
    };
    let panes = session.list_panes().unwrap();
    let pane = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|p| {
            p["terminal_command"]
                .as_str()
                .is_some_and(|c| c.contains("launch"))
        })
        .unwrap_or_else(|| panic!("missing fixture: {panes}"));
    let terminal = TerminalId::new(pane["id"].as_u64().unwrap().try_into().unwrap());
    let conversation = "restored-observer";
    let mut peer = super::session_update_real::Peer::connect(&session.socket());
    let report = AgentReport {
        report_id: uuid::Uuid::new_v4().to_string(),
        process_id: yinz_types::internal_ids::ProcessId::new(std::process::id()),
        kind: "claude".into(),
        pane_id: yinz_types::pane_id::PaneId::Terminal(terminal),
        event: None,
        trace: false,
        state: AgentReportState::Set(AgentInfo {
            kind: "claude".into(),
            source: AgentSource::Reported,
            state: AgentState::Busy,
            session_id: Some(conversation.into()),
            run_id: None,
            cwd: None,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
        }),
    };
    peer.sender
        .send_client_msg(ClientToServerMsg::AgentReport(report))
        .unwrap();
    assert!(matches!(
        peer.receiver.recv_server_msg_until(deadline),
        IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
    ));
    session.success(&[
        "--session",
        NAME,
        "action",
        "warp-pane",
        "--to-session",
        "observer-dest",
        "--focused-tab",
        "--pane-id",
        &format!("terminal_{terminal}"),
    ]);
    let updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    fs::write(root.join("sessions").join(format!("{pid}.json")), serde_json::json!({"pid":pid, "kind":"interactive","sessionId":conversation,"status":"idle","statusUpdatedAt":updated}).to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let out = session
            .command()
            .args([
                "--session",
                "observer-dest",
                "action",
                "list-panes",
                "--json",
            ])
            .output()
            .unwrap();
        let panes: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        if panes.as_array().unwrap().iter().any(|pane| {
            pane["agent"]["session_id"] == conversation && pane["agent"]["state"] == "Idle"
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "destination failed to publish the registry state: {panes}"
        );
    }
}

#[test]
fn supervisor_theme_retained_after_readoption() {
    use yinz_bus::os_input_output::{SupervisorThemeHooks, TerminalSupervision};
    use yinz_process::pty_host::client::{spawn, SpawnParams};
    use yinz_types::agent::theme::{
        AgentThemeEvent, AgentThemeLaunch, AgentThemeSnapshot, Availability, EVENT_VERSION,
        SNAPSHOT_VERSION,
    };
    let spawned = spawn(SpawnParams {
        host_bin: Path::new(env!("CARGO_BIN_EXE_yinz")),
        terminal_id: TerminalId::new(1),
        session_name: None,
        cmd: yinz_types::input::command::RunCommand {
            command: "/bin/sleep".into(),
            args: vec!["90".into()],
            ..Default::default()
        },
        termios: None,
        initial_size: None,
        arg0: None,
        env: vec![],
        login_session: false,
        job_control: true,
    })
    .unwrap()
    .finish(Box::new(|_, _, _| {}), None);
    let socket = spawned.handle.release().control_socket.unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = yinz_process::pty_host::client::terminate_via_socket(&self.0);
        }
    }
    let _cleanup = Cleanup(socket.clone());
    let os = yinz_server_os::get_server_os_input().unwrap();
    let adopted = os.adopt_terminal(&socket, Box::new(|_, _, _| {})).unwrap();
    assert!(adopted.cmd.agent_theme.is_none());
    let launch_id = format!(
        "{}fixture",
        yinz_types::agent::theme::DETECTED_LAUNCH_PREFIX
    );
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "claude".into(),
        provider_version: "test".into(),
        theme: "learned during adoption".into(),
        provenance: vec![],
        availability: Availability::Available,
        definitions: vec![],
        collisions: vec![],
    };
    os.retain_agent_theme(
        adopted.terminal_id,
        AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: launch_id.clone(),
            snapshot: Some(snapshot.clone()),
        },
    )
    .unwrap();
    os.release_terminal_for_warp(adopted.terminal_id, Duration::from_secs(30))
        .unwrap();
    let restored = os.adopt_terminal(&socket, Box::new(|_, _, _| {})).unwrap();
    assert_eq!(
        restored.cmd.agent_theme,
        Some(AgentThemeLaunch {
            launch_id,
            snapshot
        })
    );
    os.shutdown_pty_hosts();
}
