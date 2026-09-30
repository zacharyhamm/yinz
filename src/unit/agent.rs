use super::*;
use yinz_types::pane_id::PaneId;

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
fn prompt_request_keeps_conversation_and_wait_deadline() {
    let (request, json, timeout) = request(
        Agent::Prompt {
            target: "terminal_7".parse::<PaneId>().unwrap(),
            text: None,
            file: Some("-".into()),
            wait: true,
            timeout: None,
            conversation: Some("thread-7".into()),
            json: true,
        },
        &mut &b"review the diff"[..],
    )
    .unwrap();
    assert!(json);
    assert_eq!(timeout, Duration::from_secs(600));
    assert!(
        matches!(request, AgentRequest::Prompt { prompt, wait: true, timeout_ms: 600_000, expected_session_id: Some(id), .. }
        if prompt == "review the diff" && id == "thread-7")
    );
    assert!(timeout_millis(Duration::from_micros(1)).is_err());
}

#[test]
fn json_returns_copyable_spawn_target_and_preserves_operation_outcome() {
    let mut response = AgentResponse::new(AgentOutcome::NeedsInput);
    response.acknowledged = true;
    response.turn_id = Some("turn-1".into());
    response.spawned = Some(yinz_types::agent::SpawnedAgent {
        pane_id: "terminal_7".parse().unwrap(),
        branch: Some("review-2".into()),
        cwd: Some("/repo/review-2".into()),
    });
    let mut output = Vec::new();
    print_response(&response, true, &mut output).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["outcome"], "needs_input");
    assert_eq!(value["acknowledged"], true);
    assert_eq!(value["turn_id"], "turn-1");
    assert_eq!(value["spawned"]["pane_id"], "terminal_7");
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
        status_available: true,
        prompt_supported: true,
    });
    for (text, expected) in [
        ("result\n> ", "result\n> \n"),
        ("result\n", "result\n"),
        ("", ""),
    ] {
        response.output = Some(text.into());
        let mut output = Vec::new();
        print_response(&response, false, &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), expected);
    }
    let mut output = Vec::new();
    print_response(&response, true, &mut output).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["outcome"], "ok");
    assert_eq!(value["output"], "");
    assert_eq!(value["agents"][0]["pane_id"], "terminal_7");
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
fn spawn_defaults_cwd_only_when_a_host_checkout_is_needed() {
    let caller = serde_json::json!(std::env::current_dir().unwrap());
    for input in [
        serde_json::json!({}),
        serde_json::json!({"cwd":null,"project":null,"remote_host":null}),
        serde_json::json!({"firecracker":"vm","branch":"topic"}),
        serde_json::json!({"firecracker":"vm","branch":"topic","worktree":false,"cwd":null}),
    ] {
        assert_eq!(spawn_args(&[], input)["cwd"], caller);
    }
    for input in [
        serde_json::json!({"firecracker":"vm"}),
        serde_json::json!({"firecracker":"vm","cwd":null}),
        serde_json::json!({"firecracker":"vm","branch":"  "}),
        serde_json::json!({"project":"work","cwd":null}),
        serde_json::json!({"remote_host":"ssh","cwd":null}),
    ] {
        assert!(spawn_args(&[], input)["cwd"].is_null());
    }
    assert_eq!(
        spawn_args(
            &["--firecracker", "vm", "--branch", "topic"],
            serde_json::json!({"cwd":null})
        )["cwd"],
        caller
    );
    assert!(spawn_args(&["--firecracker", "vm"], serde_json::json!({}))["cwd"].is_null());
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
fn cli_named_branch_vm_disables_automatic_naming() {
    let request = spawn_args(
        &["--firecracker", "review", "--branch", "topic"],
        serde_json::json!({"kind":"codex"}),
    );
    assert_eq!(request["automatic_firecracker_vm"], false);
    assert_eq!(request["spec"]["firecracker"], "review");
    assert_eq!(request["spec"]["branch"], "topic");
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
            "--allow-yinz applies only to --kind codex"
        );
        assert!(spec.args.is_empty());
    }
}
