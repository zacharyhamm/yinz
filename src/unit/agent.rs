use super::*;
use serde_json::json;
use yinz_types::pane_id::PaneId;

#[test]
fn agent_output_accepts_closed_stdout_but_reports_other_write_errors() {
    struct FailingOutput {
        remaining: usize,
        error: std::io::ErrorKind,
    }
    impl Write for FailingOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(self.error.into());
            }
            let written = bytes.len().min(self.remaining);
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(self.error.into())
        }
    }
    let mut response = AgentResponse::new(AgentOutcome::Ok);
    response.message = Some("the response".into());
    for json in [false, true] {
        // Fail before writing, during writing, or only when flushing.
        for remaining in [0, 5, usize::MAX] {
            for error in [
                std::io::ErrorKind::BrokenPipe,
                std::io::ErrorKind::PermissionDenied,
            ] {
                let result =
                    print_response(&response, json, &mut FailingOutput { remaining, error });
                if error == std::io::ErrorKind::BrokenPipe {
                    result.unwrap();
                } else {
                    assert_eq!(
                        result
                            .unwrap_err()
                            .downcast::<std::io::Error>()
                            .unwrap()
                            .kind(),
                        error
                    );
                }
            }
        }
    }
}

#[test]
fn prompt_inputs_normalize_newlines_and_require_safe_nonempty_content() {
    let prompt = "Review this diff:\n\t+added line";
    assert_eq!(
        read_prompt(Some(prompt.into()), None, &mut &b""[..]).unwrap(),
        prompt
    );
    assert_eq!(
        read_prompt(
            Some("Review this diff:\r\n\t+added line\r\n".into()),
            None,
            &mut &b""[..]
        )
        .unwrap(),
        prompt
    );
    assert_eq!(
        read_prompt(None, Some(Path::new("-")), &mut prompt.as_bytes()).unwrap(),
        prompt
    );
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), prompt).unwrap();
    assert_eq!(
        read_prompt(None, Some(file.path()), &mut &b""[..]).unwrap(),
        prompt
    );
    for invalid in ["", " \n\t", "x\u{1b}[201~\r", "x\0y", "x\u{7}y", "x\ry"] {
        assert!(read_prompt(Some(invalid.into()), None, &mut &b""[..]).is_err());
    }
    assert!(read_prompt(None, Some(Path::new("-")), &mut &b"\xff"[..]).is_err());
}

#[test]
fn prompt_sources_name_the_mistake() {
    let both = read_prompt(Some("text".into()), Some(Path::new("-")), &mut &b"x"[..])
        .unwrap_err()
        .to_string();
    assert_eq!(both, "pass prompt TEXT or --file PATH, not both");
    let neither = read_prompt(None, None, &mut &b""[..])
        .unwrap_err()
        .to_string();
    assert!(neither.contains("--file -"), "{neither}");
}

#[test]
fn prompt_request_ignores_preserve_draft_and_keeps_conversation_and_wait_deadline() {
    let (request, json, timeout) = request(
        Agent::Prompt {
            preserve_draft: true,
            target: "terminal_7".parse::<PaneId>().unwrap(),
            text: None,
            file: Some("-".into()),
            wait: true,
            timeout: None,
            conversation: Some("thread-7".into()),
            submission: Some("review-7".into()),
            json: true,
        },
        &mut &b"review the diff"[..],
    )
    .unwrap();
    assert!(matches!(
        &request,
        AgentRequest::Prompt {
            preserve_draft: false,
            ..
        }
    ));
    assert!(json);
    assert_eq!(timeout, Duration::from_secs(600));
    assert!(
        matches!(request, AgentRequest::Prompt { prompt, wait: true, timeout_ms: 600_000, expected_session_id: Some(id), submission_id: Some(submission), .. }
        if prompt == "review the diff" && id == "thread-7" && submission == "review-7")
    );
    assert!(timeout_millis(Duration::from_micros(1)).is_err());
}

#[test]
fn submission_identity_survives_request_preparation_and_waits_select_the_exact_turn() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Agent,
    }
    let mut command = Cli::try_parse_from(["agent", "prompt", "7", "review the diff", "--json"])
        .unwrap()
        .command;
    let id = submission_id(&mut command).unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(submission_id(&mut command).as_deref(), Some(id.as_str()));
    let (request, _, _) = request(command, &mut &b""[..]).unwrap();
    assert!(
        matches!(request, AgentRequest::Prompt { submission_id: Some(submitted), .. } if submitted == id)
    );

    let args = Cli::try_parse_from([
        "agent",
        "wait",
        "7",
        "--submission",
        &id,
        "--conversation",
        "conversation-7",
        "--timeout",
        "2m",
        "--json",
    ])
    .unwrap();
    let (request, json, timeout) = super::request(args.command, &mut &b""[..]).unwrap();
    assert!(json);
    assert_eq!(timeout, Duration::from_secs(120));
    assert!(matches!(request, AgentRequest::WaitTurn {
        pane_id, submission_id, expected_session_id: Some(conversation), timeout_ms: 120_000,
    } if pane_id.to_string() == "terminal_7" && submission_id == id && conversation == "conversation-7"));
}

#[test]
fn json_returns_copyable_spawn_target_and_preserves_operation_outcome() {
    let mut response = AgentResponse::new(AgentOutcome::NeedsInput);
    response.acknowledged = true;
    response.turn_id = Some("turn-1".into());
    response.submission_id = Some("submission-1".into());
    response.spawned = Some(yinz_types::agent::SpawnedAgent {
        pane_id: "terminal_7".parse().unwrap(),
        branch: Some("review-2".into()),
        cwd: Some("/repo/review-2".into()),
        occupied_by: vec!["terminal_3".parse().unwrap()],
        ..Default::default()
    });
    let mut output = Vec::new();
    print_response(&response, true, &mut output).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["outcome"], "needs_input");
    assert_eq!(value["acknowledged"], true);
    assert_eq!(value["turn_id"], "turn-1");
    assert_eq!(value["submission_id"], "submission-1");
    assert_eq!(value["spawned"]["pane_id"], "terminal_7");
    assert_eq!(value["spawned"]["occupied_by"], json!(["terminal_3"]));
    assert_eq!(value["spawned"]["branch"], "review-2");
}

#[test]
fn successful_read_prints_only_terminal_text_and_json_keeps_metadata() {
    use yinz_types::agent::control::AgentStatus;
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    let mut response = AgentResponse::new(AgentOutcome::Ok);
    response.message = Some("metadata must not contaminate terminal output".into());
    response.agents.push(AgentStatus {
        pane_id: "terminal_7".parse().unwrap(),
        session: "development".into(),
        host_id: "host".into(),
        tab_id: None,
        target: Default::default(),
        agent: AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: Some("conversation-7".into()),
            run_id: None,
        },
        cwd: None,
        branch: None,
        ready: true,
        not_ready_reason: None,
        status_available: true,
        prompt_supported: true,
        readiness: None,
        input_revision: None,
        clear_draft_supported: false,
        background_tasks: Vec::new(),
    });
    for (text, expected) in [
        ("result\n> ", "result\n> \n"),
        ("result\n", "result\n"),
        ("  first\n\n \n  second", "  first\n\n \n  second\n"),
        ("", ""),
    ] {
        response.output = Some(text.into());
        let mut output = Vec::new();
        print_response(&response, false, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), expected);

        let mut output = Vec::new();
        print_response(&response, true, &mut output).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["outcome"], "ok");
        assert_eq!(value["output"], text);
        assert_eq!(value["agents"][0]["pane_id"], "terminal_7");
    }
    response.output = None;
    for reason in [
        None,
        Some(yinz_types::agent::control::NotReadyReason::PromptUnsupported),
        Some(yinz_types::agent::control::NotReadyReason::NotIdle),
        Some(yinz_types::agent::control::NotReadyReason::PendingSubmission),
        Some(yinz_types::agent::control::NotReadyReason::UnconsumedInput),
    ] {
        response.agents[0].ready = reason.is_none();
        response.agents[0].not_ready_reason = reason;
        let mut text = Vec::new();
        print_response(&response, false, &mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        if let Some(reason) = reason {
            assert!(text.contains(&format!("not_ready_reason: {reason}")));
        } else {
            assert!(!text.contains("not_ready_reason"));
        }
        let mut json = Vec::new();
        print_response(&response, true, &mut json).unwrap();
        let decoded: AgentResponse = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded, response);
    }
}

#[test]
fn background_work_is_shown_with_how_to_get_its_answer() {
    let mut response = AgentResponse::new(AgentOutcome::Completed);
    response.message = Some("Started.".into());
    let mut output = Vec::new();
    print_response(&response, false, &mut output).unwrap();
    assert!(!String::from_utf8(output).unwrap().contains("background"));
    let mut json = Vec::new();
    print_response(&response, true, &mut json).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert!(value.get("background_tasks").is_none());

    let status: yinz_types::agent::control::AgentStatus = serde_json::from_value(json!({
        "pane_id": "terminal_7", "session": "s", "host_id": "h", "tab_id": null,
        "target": "Local", "cwd": null, "branch": null, "ready": true,
        "status_available": true, "prompt_supported": true,
        "agent": {"kind": "claude", "state": "Idle", "source": "Reported"},
    }))
    .unwrap();
    assert!(
        status.background_tasks.is_empty(),
        "older servers omit the field"
    );
    response
        .agents
        .push(yinz_types::agent::control::AgentStatus {
            background_tasks: vec!["bj30txqod".into()],
            ..status
        });
    response.background_tasks = vec!["bj30txqod".into()];
    let mut output = Vec::new();
    print_response(&response, false, &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("  background_tasks: bj30txqod\n"));
    assert!(text.ends_with(
        "Started.\nBackground work is still running (bj30txqod). `yinz agent wait terminal_7 --until idle` returns its final answer.\n"
    ));
    let mut json = Vec::new();
    print_response(&response, true, &mut json).unwrap();
    let decoded: AgentResponse = serde_json::from_slice(&json).unwrap();
    assert_eq!(decoded, response);
}

#[test]
fn recovery_request_preserves_the_explicit_assertion_and_pinned_conversation() {
    let (request, json, _) = request(
        Agent::Recover {
            target: "terminal_7".parse().unwrap(),
            conversation: "conversation".into(),
            confirm_empty: true,
            interrupted: true,
            json: true,
        },
        &mut &b""[..],
    )
    .unwrap();
    assert!(json);
    assert!(
        matches!(request, AgentRequest::Recover { expected_session_id, confirm_empty: true, interrupted: true, .. } if expected_session_id == "conversation")
    );
}

fn spawn_args(flags: &[&str], input: serde_json::Value) -> serde_json::Value {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Agent,
    }
    let cli = Cli::try_parse_from(
        ["agent", "spawn", "--file", "-"]
            .into_iter()
            .chain(flags.iter().copied()),
    )
    .unwrap();
    let (request, _, _) = request(cli.command, &mut input.to_string().as_bytes()).unwrap();
    serde_json::to_value(request).unwrap()["Spawn"].clone()
}
#[test]
fn spawn_resolves_local_cwd_after_merging_flags_and_json() {
    let caller = std::env::current_dir().unwrap();
    for (flags, input, expected) in [
        (
            vec!["--cwd", "from-flags"],
            serde_json::json!({"cwd":"from-json"}),
            caller.join("from-flags"),
        ),
        (
            vec![],
            serde_json::json!({"cwd":"from-json"}),
            caller.join("from-json"),
        ),
        (
            vec!["--cwd", "/absolute"],
            serde_json::json!({}),
            std::path::PathBuf::from("/absolute"),
        ),
        (
            vec![],
            serde_json::json!({"cwd":"/absolute"}),
            std::path::PathBuf::from("/absolute"),
        ),
        (
            vec!["--remote-host", "ssh", "--cwd", "relative"],
            serde_json::json!({}),
            std::path::PathBuf::from("relative"),
        ),
        (
            vec![],
            serde_json::json!({"remote_host":"ssh","cwd":"/remote"}),
            std::path::PathBuf::from("/remote"),
        ),
        (
            vec![],
            serde_json::json!({"remote_host":null,"project":"work","cwd":"relative"}),
            caller.join("relative"),
        ),
    ] {
        assert_eq!(
            spawn_args(&flags, input)["cwd"],
            serde_json::json!(expected)
        );
    }
}

#[test]
fn spawn_rejects_opening_prompt_fields_in_json_including_null() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Agent,
    }
    for field in ["prompt", "reuse_prompt"] {
        for value in [serde_json::Value::Null, "act now".into()] {
            let cli = Cli::try_parse_from(["agent", "spawn", "--file", "-"]).unwrap();
            let source = serde_json::json!({"kind":"codex",field:value}).to_string();
            assert!(request(cli.command, &mut source.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("spawn does not accept"));
        }
    }
}

#[test]
fn allowing_yinz_opens_only_a_codex_sandbox() {
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        args: vec!["--search".into()],
        ..Default::default()
    };
    allow_sandboxed_yinz(&mut spec).unwrap();
    assert_eq!(
        spec.args,
        [
            "--search",
            "-c",
            "sandbox_workspace_write.network_access=true"
        ]
    );
    for kind in [Some("claude"), None] {
        let mut spec = AgentSpawnSpec {
            kind: kind.map(Into::into),
            ..Default::default()
        };
        assert_eq!(
            allow_sandboxed_yinz(&mut spec).unwrap_err().to_string(),
            "--allow-yinz applies only to Codex launches (--kind codex or a Codex command)"
        );
        assert!(spec.args.is_empty());
    }
    for (kind, command) in [(Some(" CODEX "), None), (None, Some("/opt/bin/codex"))] {
        let mut spec = AgentSpawnSpec {
            kind: kind.map(Into::into),
            command: command.map(Into::into),
            ..Default::default()
        };
        allow_sandboxed_yinz(&mut spec).unwrap();
        assert_eq!(
            spec.args,
            ["-c", "sandbox_workspace_write.network_access=true"]
        );
    }
}

#[test]
fn spawn_json_keeps_command_arguments_environment_and_flag_overrides() {
    let launch = spawn_args(
        &["--model", "from-flag"],
        serde_json::json!({
            "command":"/opt/bin/claude.js", "args":["--verbose"], "env":{"TEST":"value"},
            "hooks":false, "model":"from-json"
        }),
    );
    assert_eq!(launch["spec"]["command"], "/opt/bin/claude.js");
    assert_eq!(launch["spec"]["args"], serde_json::json!(["--verbose"]));
    assert_eq!(launch["spec"]["env"]["TEST"], "value");
    assert_eq!(launch["spec"]["hooks"], false);
    assert_eq!(launch["spec"]["model"], "from-flag");
}

#[test]
fn clear_draft_request_preserves_authorized_revision_and_deadline() {
    let (request, json, timeout) = request(
        Agent::ClearDraft {
            target: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            conversation: "conversation".into(),
            input_revision: 42,
            confirm_discard: true,
            interrupted: true,
            timeout: Duration::from_secs(7),
            json: true,
        },
        &mut &b""[..],
    )
    .unwrap();
    assert!(json);
    assert_eq!(timeout, Duration::from_secs(7));
    assert!(matches!(request, AgentRequest::ClearDraft {
        expected_session_id, input_revision: 42, confirm_discard: true,
        interrupted: true, timeout_ms: 7000, ..
    } if expected_session_id == "conversation"));
}

#[test]
fn spawn_tab_file_forms_and_cli_placement_precedence() {
    for (flags, input, expected) in [
        (vec![], json!({}), json!("automatic")),
        (vec![], json!({"tab": 0}), json!({"tab": 0})),
        (vec![], json!({"new_tab": true}), json!({"new_tab": null})),
        (
            vec![],
            json!({"new_tab": "review"}),
            json!({"new_tab": "review"}),
        ),
        (
            vec![],
            json!({"tab": null, "new_tab": false}),
            json!("automatic"),
        ),
        (
            vec!["--tab", "7"],
            json!({"new_tab": "review"}),
            json!({"tab": 7}),
        ),
        (
            vec!["--new-tab"],
            json!({"tab": 7}),
            json!({"new_tab": null}),
        ),
        (
            vec!["--new-tab", "review"],
            json!({"tab": "invalid", "new_tab": 42}),
            json!({"new_tab": "review"}),
        ),
    ] {
        assert_eq!(spawn_args(&flags, input)["placement"], expected);
    }
}

#[test]
fn spawn_placement_waits_for_support_reply_before_sending_spawn() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_types::{agent::control::AgentPlacement, internal_ids::TabId};

    for placement in [
        AgentPlacement::Tab(TabId::new(7)),
        AgentPlacement::NewTab(Some("images".into())),
    ] {
        for ack_first in [true, false] {
            for supported in [true, false] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("session.sock");
                let listener = ListenerOptions::new()
                    .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                    .create_sync()
                    .unwrap();
                let request = AgentRequest::Spawn {
                    placement: placement.clone(),
                    spec: AgentSpawnSpec::default(),
                    cwd: None,
                    project: None,
                    remote_host: None,
                };
                let expected = request.clone();
                let server = std::thread::spawn(move || {
                    let mut sender =
                        IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
                    let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
                    receiver
                        .set_recv_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    assert!(matches!(
                        receiver.recv_client_msg(),
                        IpcRecv::Msg(
                            ClientToServerMsg::AgentControl(AgentRequest::SpawnPlacementSupport),
                            _
                        )
                    ));
                    let mut response = AgentResponse::new(AgentOutcome::Ok);
                    response.output = supported.then(|| "agent-tab-placement-v1".into());
                    // Deterministically exercise both router/screen scheduling orders.
                    if ack_first {
                        sender
                            .send_server_msg(ServerToClientMsg::UnblockInputThread)
                            .unwrap();
                    }
                    sender
                        .send_server_msg(ServerToClientMsg::AgentControl(response))
                        .unwrap();
                    if !ack_first && supported {
                        sender
                            .send_server_msg(ServerToClientMsg::UnblockInputThread)
                            .unwrap();
                    }
                    if supported {
                        assert!(
                            matches!(receiver.recv_client_msg(), IpcRecv::Msg(ClientToServerMsg::AgentControl(request), _) if request == expected)
                        );
                        sender
                            .send_server_msg(ServerToClientMsg::UnblockInputThread)
                            .unwrap();
                        sender
                            .send_server_msg(ServerToClientMsg::AgentControl(AgentResponse::new(
                                AgentOutcome::Submitted,
                            )))
                            .unwrap();
                        assert!(matches!(
                            receiver.recv_client_msg(),
                            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
                        ));
                    } else {
                        assert!(
                            matches!(receiver.recv_client_msg(), IpcRecv::Eof),
                            "unsupported placement must not send a spawn"
                        );
                    }
                });
                let socket = yinz_ipc::consts::ipc_connect(&path).unwrap();
                let result = exchange_on_socket(socket, request, Duration::from_secs(5));
                server.join().unwrap();
                if supported {
                    assert_eq!(result.unwrap().outcome, AgentOutcome::Submitted);
                } else {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("spawn was not sent"));
                }
            }
        }
    }
}

#[test]
fn spawn_rejects_unknown_fields_before_launch_regardless_of_permissions() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Agent,
    }
    for flags in [vec![], vec!["--permission-mode", "skip"]] {
        for field in ["firecracker", "unknown_option"] {
            for value in [serde_json::Value::Null, json!({})] {
                let cli = Cli::try_parse_from(
                    ["agent", "spawn", "--file", "-"]
                        .into_iter()
                        .chain(flags.iter().copied()),
                )
                .unwrap();
                let source = json!({"kind":"codex", "remote_host":null, field:value}).to_string();
                let error = request(cli.command, &mut source.as_bytes()).unwrap_err();
                assert!(format!("{error:#}").contains(&format!("unknown field `{field}`")));
            }
        }
    }
}

#[test]
fn spawn_preserves_supported_json_envelope_fields() {
    for placement in [json!({"tab":3}), json!({"new_tab":"review"})] {
        let mut input =
            json!({"kind":"codex", "project":"work", "remote_host":"build", "cwd":"relative"});
        input
            .as_object_mut()
            .unwrap()
            .extend(placement.as_object().unwrap().clone());
        let spawn = spawn_args(&[], input);
        assert_eq!(spawn["project"], "work");
        assert_eq!(spawn["remote_host"], "build");
        assert_eq!(spawn["cwd"], "relative");
        assert_eq!(spawn["spec"]["kind"], "codex");
    }
}

#[test]
fn bubblewrap_negotiation_requires_confirmation_before_spawn() {
    use interprocess::{
        local_socket::{prelude::*, GenericFilePath, ListenerOptions, Stream},
        TryClone,
    };
    use yinz_types::agent::control::AgentPlacement;

    for placement in [AgentPlacement::Automatic, AgentPlacement::NewTab(None)] {
        for reply in [
            "success",
            "wrong token",
            "missing token",
            "rejected",
            "disconnect",
            "malformed",
            "timeout",
            "placement rejected",
        ] {
            if reply == "placement rejected" && placement == AgentPlacement::Automatic {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.sock");
            let listener = ListenerOptions::new()
                .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .create_sync()
                .unwrap();
            // Exercise the finalized JSON request, as well as combined placement negotiation.
            let mut request: AgentRequest = serde_json::from_value(json!({
                "Spawn": spawn_args(&[], json!({"bubblewrap": true})),
            }))
            .unwrap();
            let AgentRequest::Spawn {
                placement: target, ..
            } = &mut request
            else {
                panic!("expected spawn");
            };
            *target = placement.clone();
            let expected = request.clone();
            let explicit_placement = placement != AgentPlacement::Automatic;
            let server = std::thread::spawn(move || {
                let mut raw = listener.accept().unwrap();
                let mut sender =
                    IpcSenderWithContext::<ServerToClientMsg>::new(raw.try_clone().unwrap());
                let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
                receiver
                    .set_recv_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                if explicit_placement {
                    assert!(matches!(
                        receiver.recv_client_msg(),
                        IpcRecv::Msg(
                            ClientToServerMsg::AgentControl(AgentRequest::SpawnPlacementSupport),
                            _
                        )
                    ));
                    let mut response = AgentResponse::new(AgentOutcome::Ok);
                    response.output =
                        (reply != "placement rejected").then(|| "agent-tab-placement-v1".into());
                    sender
                        .send_server_msg(ServerToClientMsg::AgentControl(response))
                        .unwrap();
                    if reply == "placement rejected" {
                        assert!(matches!(receiver.recv_client_msg(), IpcRecv::Eof));
                        return;
                    }
                    sender
                        .send_server_msg(ServerToClientMsg::UnblockInputThread)
                        .unwrap();
                }
                assert!(matches!(
                    receiver.recv_client_msg(),
                    IpcRecv::Msg(
                        ClientToServerMsg::AgentControl(AgentRequest::SpawnBubblewrapSupport),
                        _
                    )
                ));
                sender
                    .send_server_msg(ServerToClientMsg::UnblockInputThread)
                    .unwrap();
                match reply {
                    "disconnect" => {
                        let Stream::UdSocket(socket) = &raw;
                        // Close only replies so we can still prove that no spawn arrives.
                        socket.inner().shutdown(std::net::Shutdown::Write).unwrap();
                    },
                    "malformed" => {
                        raw.write_all(&1u32.to_le_bytes()).unwrap();
                        raw.write_all(&[0xff]).unwrap();
                    },
                    "timeout" => {}, // Keep the connection open until the client's deadline.
                    _ => {
                        let mut response = AgentResponse::new(if reply == "rejected" {
                            AgentOutcome::Rejected
                        } else {
                            AgentOutcome::Ok
                        });
                        response.output = match reply {
                            "missing token" => None,
                            "wrong token" => Some("agent-bubblewrap-v2".into()),
                            _ => Some("agent-bubblewrap-v1".into()),
                        };
                        sender
                            .send_server_msg(ServerToClientMsg::AgentControl(response))
                            .unwrap();
                    },
                }
                if reply == "success" {
                    assert!(matches!(receiver.recv_client_msg(), IpcRecv::Msg(
                        ClientToServerMsg::AgentControl(request), _
                    ) if request == expected));
                    sender
                        .send_server_msg(ServerToClientMsg::UnblockInputThread)
                        .unwrap();
                    sender
                        .send_server_msg(ServerToClientMsg::AgentControl(AgentResponse::new(
                            AgentOutcome::Submitted,
                        )))
                        .unwrap();
                    assert!(matches!(
                        receiver.recv_client_msg(),
                        IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
                    ));
                } else {
                    assert!(
                        matches!(receiver.recv_client_msg(), IpcRecv::Eof),
                        "{reply}: failed negotiation must not send a spawn"
                    );
                }
            });
            let socket = yinz_ipc::consts::ipc_connect(&path).unwrap();
            let started = Instant::now();
            let result = exchange_on_socket(socket, request, Duration::from_secs(5));
            server.join().unwrap();
            if reply == "success" {
                assert_eq!(result.unwrap().outcome, AgentOutcome::Submitted);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("spawn was not sent"), "{reply}: {error}");
                assert!(error.contains("restart the session"), "{reply}: {error}");
                if reply == "timeout" {
                    assert!(started.elapsed() >= Duration::from_secs(5));
                }
            }
        }
    }
}

#[test]
fn bubblewrap_spawn_cli_and_json_keep_the_opt_in() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Agent,
    }
    for (extra, input, expected) in [
        (vec![], "{}", false),
        (vec![], r#"{"bubblewrap":true}"#, true),
        (vec!["--bubblewrap"], r#"{"bubblewrap":false}"#, true),
    ] {
        let mut args = vec!["agent", "spawn", "--kind", "codex", "--file", "-"];
        args.extend(extra);
        let parsed = Cli::try_parse_from(args).unwrap();
        let (request, _, _) = request(parsed.command, &mut input.as_bytes()).unwrap();
        let AgentRequest::Spawn { spec, .. } = request else {
            panic!("expected spawn");
        };
        assert_eq!(spec.bubblewrap, expected);
    }
}

#[test]
fn spawn_defaults_cwd_only_when_a_host_checkout_is_needed() {
    let caller = serde_json::json!(std::env::current_dir().unwrap());
    for input in [
        serde_json::json!({}),
        serde_json::json!({"cwd":null,"project":null,"remote_host":null}),
    ] {
        assert_eq!(spawn_args(&[], input)["cwd"], caller);
    }
    for input in [
        serde_json::json!({"project":"work","cwd":null}),
        serde_json::json!({"remote_host":"ssh","cwd":null}),
    ] {
        assert!(spawn_args(&[], input)["cwd"].is_null());
    }
    assert_eq!(
        spawn_args(&["--branch", "topic"], serde_json::json!({}))["cwd"],
        caller
    );
    assert_eq!(
        spawn_args(&[], serde_json::json!({"cwd":"/explicit"}))["cwd"],
        "/explicit"
    );
}
