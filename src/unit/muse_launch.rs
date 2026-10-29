use super::*;

#[test]
fn muse_announces_before_starting_a_program_that_reports_immediately() {
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    let _supervisor = pane_routing::Supervisor::new(dir.path(), "report-test", 15);
    let config = dir.path().join("muse");
    let hook = yinz_muse::hook_script_path(&config);
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    yinz_test_support::write_warm_script(&hook, "exit 0");
    std::fs::write(
        yinz_muse::settings_path(&config),
        yinz_types::agent::muse_hook_settings(&format!(
            "'{}'",
            hook.display().to_string().replace('\'', r"'\''")
        ))
        .to_string(),
    )
    .unwrap();
    let program = dir.path().join("muse-program");
    yinz_test_support::write_warm_script(
        &program,
        r#"if ! test -f "$YINZ_TEST_MUSE_BOOTSTRAP"; then
    echo 'Muse started before its startup report was accepted'
fi
printf '%s\n' "$@"
printf '{"hook_event_name":"SessionStart","session_id":"%s","source":"startup"}' "$YINZ_TEST_MUSE_SESSION" | "$YINZ_TEST_LAUNCHER" agent muse-report
printf '{"hook_event_name":"UserPromptSubmit","session_id":"%s","turn_id":"turn-1","prompt":"initial prompt"}' "$YINZ_TEST_MUSE_SESSION" | "$YINZ_TEST_LAUNCHER" agent muse-report
exit 23"#,
    );
    let bootstrap = dir.path().join("bootstrap-accepted");
    let session = uuid::Uuid::new_v4().to_string();
    let mut command = agent_command(dir.path(), &["muse", "--command"]);
    command
        .arg(&program)
        .args(["--", "--prompt", "initial prompt"])
        .env("YINZ_PANE_REF", "abcdef012345")
        .env("YINZ_TEST_MUSE_BOOTSTRAP", &bootstrap)
        .env("YINZ_TEST_MUSE_SESSION", &session)
        .env("YINZ_TEST_LAUNCHER", env!("CARGO_BIN_EXE_yinz"));
    let child = command.spawn().unwrap();
    let mut hooks = Vec::new();
    for _ in 0..3 {
        let mut receiver =
            IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
        let report = match receiver.recv_client_msg() {
            IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => report,
            _ => panic!("expected a Muse report"),
        };
        assert_eq!(
            report.pane_id,
            yinz_types::pane_id::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
        );
        let event = report.event.unwrap();
        assert_eq!(event.kind, "muse");
        let hook: yinz_types::agent::HookEvent = serde_json::from_str(&event.raw).unwrap();
        if hook.muse_hooks_ready == Some(true) {
            std::fs::write(&bootstrap, []).unwrap();
        }
        hooks.push(hook);
        receiver
            .get_sender::<ServerToClientMsg>()
            .unwrap()
            .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                report_id: report.report_id,
            })
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "--prompt\ninitial prompt\n"
    );
    assert_eq!(hooks[0].hook_event_name, "SessionStart");
    assert!(hooks[0].session_id.is_empty());
    assert_eq!(hooks[0].muse_hooks_ready, Some(true));
    assert_eq!(hooks[1].hook_event_name, "SessionStart");
    assert_eq!(hooks[1].session_id, session);
    assert_eq!(hooks[2].hook_event_name, "UserPromptSubmit");
    assert_eq!(hooks[2].session_id, session);
    assert_eq!(hooks[2].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(hooks[2].prompt.as_deref(), Some("initial prompt"));
}
