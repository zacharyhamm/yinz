use super::*;

#[derive(clap::Parser)]
struct AgentCli {
    #[command(subcommand)]
    command: Agent,
}

#[test]
fn agent_commands_require_explicit_targets_and_one_prompt_source() {
    use clap::{CommandFactory, Parser};
    AgentCli::command().debug_assert();
    let parsed = AgentCli::try_parse_from([
        "agent",
        "prompt",
        "terminal_7",
        "--file",
        "-",
        "--wait",
        "--timeout",
        "10m",
        "--conversation",
        "conversation-1",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Agent::Prompt { target, file: Some(file), wait: true, timeout: Some(timeout), conversation: Some(conversation), json: true, .. }
            if target.to_string() == "terminal_7" && file == std::path::Path::new("-")
                && timeout == std::time::Duration::from_secs(600) && conversation == "conversation-1"
    ));
    for arguments in [
        vec!["agent", "status", "plugin_7"],
        vec!["agent", "wait", "terminal_7", "--timeout", "0s"],
        vec!["agent", "wait", "terminal_7", "--until", "quiet"],
        vec!["agent", "read", "terminal_7", "--lines", "0"],
        vec!["agent", "send-keys", "terminal_7"],
    ] {
        assert!(
            AgentCli::try_parse_from(arguments.clone()).is_err(),
            "{arguments:?}"
        );
    }
    assert!(matches!(
        AgentCli::try_parse_from(["agent", "wait", "terminal_7", "--until", "idle"])
            .unwrap()
            .command,
        Agent::Wait {
            until: yinz_types::agent::AgentState::Idle,
            ..
        }
    ));
    assert!(matches!(
        AgentCli::try_parse_from(["agent", "spawn", "--kind", "codex", "--cwd", ".", "--branch", "review-auth", "--json"]).unwrap().command,
        Agent::Spawn { kind, branch: Some(branch), json: true, .. } if kind.as_deref() == Some("codex") && branch == "review-auth"
    ));
    assert!(matches!(
        AgentCli::try_parse_from(["agent", "spawn", "--kind", "codex", "--allow-yinz"])
            .unwrap()
            .command,
        Agent::Spawn {
            allow_yinz: true,
            ..
        }
    ));
}

#[test]
fn archive_commands_accept_and_ignore_json() {
    use clap::Parser;
    for arguments in [
        vec!["agent", "runs", "--json"],
        vec!["agent", "run", "run-1", "--json"],
        vec!["agent", "history-search", "query", "--json"],
        vec!["agent", "predecessors", "run-1", "--json"],
    ] {
        let parsed = AgentCli::try_parse_from(arguments.clone()).unwrap();
        let Agent::Archive(archive) = &parsed.command else {
            panic!("{arguments:?}");
        };
        assert!(parsed.command.json());
        let value = serde_json::to_value(archive).unwrap();
        assert!(value["args"].get("json").is_none(), "{value}");
    }
}

#[test]
fn submission_waits_are_distinct_from_state_waits_and_registration_accepts_json() {
    use clap::Parser;
    for args in [
        vec![
            "agent",
            "prompt",
            "7",
            "review",
            "--submission",
            "review_1.v2",
        ],
        vec!["agent", "wait", "7", "--submission", "review_1.v2"],
        vec!["agent", "wait", "7", "--until", "idle"],
    ] {
        assert!(AgentCli::try_parse_from(args.clone()).is_ok(), "{args:?}");
    }
    for args in [
        vec![
            "agent",
            "wait",
            "7",
            "--submission",
            "review",
            "--until",
            "idle",
        ],
        vec!["agent", "wait", "7", "--submission", ""],
        vec![
            "agent",
            "prompt",
            "7",
            "review",
            "--submission",
            "spaces forbidden",
        ],
        vec![
            "agent",
            "prompt",
            "7",
            "review",
            "--submission",
            "shell;input",
        ],
    ] {
        assert!(AgentCli::try_parse_from(args.clone()).is_err(), "{args:?}");
    }
    let registered =
        AgentCli::try_parse_from(["agent", "register", "7", "claude", "--json"]).unwrap();
    assert!(registered.command.json());
    assert!(matches!(
        registered.command,
        Agent::Register {
            pane_id: 7,
            json: true,
            ..
        }
    ));
}

#[test]
fn pane_ids_accept_terminal_prefix_or_bare_number_everywhere() {
    use clap::Parser;
    for target in ["terminal_7", "7"] {
        assert!(matches!(
            AgentCli::try_parse_from(["agent", "status", target]).unwrap().command,
            Agent::Status { target, .. } if target.to_string() == "terminal_7"
        ));
        assert!(matches!(
            AgentCli::try_parse_from(["agent", "register", target, "claude"])
                .unwrap()
                .command,
            Agent::Register { pane_id: 7, .. }
        ));
    }
    let error = AgentCli::try_parse_from(["agent", "status", "plugin_7"])
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("plugin panes do not run agents"), "{error}");
    let error = AgentCli::try_parse_from(["agent", "register", "pane7", "claude"])
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("expected terminal_7 or 7"), "{error}");
}

#[test]
fn recovery_requires_conversation_and_explicit_empty_draft_confirmation() {
    use clap::Parser;
    for args in [
        vec!["agent", "recover", "terminal_7"],
        vec!["agent", "recover", "terminal_7", "--conversation", "id"],
        vec!["agent", "recover", "terminal_7", "--confirm-empty"],
    ] {
        assert!(AgentCli::try_parse_from(args).is_err());
    }
    assert!(matches!(
        AgentCli::try_parse_from([
            "agent",
            "recover",
            "terminal_7",
            "--conversation",
            "id",
            "--confirm-empty",
            "--json"
        ])
        .unwrap()
        .command,
        Agent::Recover {
            confirm_empty: true,
            interrupted: false,
            json: true,
            ..
        }
    ));
    assert!(matches!(
        AgentCli::try_parse_from([
            "agent",
            "recover",
            "terminal_7",
            "--conversation",
            "id",
            "--confirm-empty",
            "--interrupted"
        ])
        .unwrap()
        .command,
        Agent::Recover {
            interrupted: true,
            ..
        }
    ));
}

#[test]
fn archive_options_keep_the_service_contract() {
    use clap::Parser;
    let cli = AgentCli::try_parse_from([
        "agent",
        "runs",
        "--host-id",
        "host",
        "--run-session",
        "session",
        "--limit",
        "7",
        "--offset",
        "14",
    ])
    .unwrap();
    let Agent::Archive(command) = cli.command else {
        panic!("archive command");
    };
    let yinz_types::input::actions::Action::CliPipe {
        payload: Some(payload),
        ..
    } = command.action().unwrap()
    else {
        panic!("archive pipe");
    };
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"op":"agent_runs","args":{"host_id":"host","run_session":"session","limit":7,"offset":14},"structured_reply":true})
    );
}

#[test]
fn history_regex_preserves_pattern_and_serializes_filters() {
    use clap::Parser;
    let parsed = AgentCli::try_parse_from([
        "agent",
        "history-search",
        " (?i)panic|ERROR ",
        "--regex",
        "--run-id",
        "run-1",
        "--project",
        "work",
        "--host-id",
        "host-a",
        "--run-session",
        "main",
    ])
    .unwrap();
    let Agent::Archive(archive) = parsed.command else {
        panic!("archive command")
    };
    let value = serde_json::to_value(&archive).unwrap();
    assert_eq!(value["args"]["regex"], true);
    assert_eq!(value["args"]["query"], " (?i)panic|ERROR ");
    assert_eq!(value["args"]["run_id"], "run-1");
    assert_eq!(value["args"]["project"], "work");
    assert_eq!(value["args"]["host_id"], "host-a");
    assert_eq!(value["args"]["run_session"], "main");
    let mut legacy = value;
    legacy["args"].as_object_mut().unwrap().remove("regex");
    let legacy: crate::agent_archive::AgentArchive = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["args"]["regex"],
        false
    );
    let yinz_types::input::actions::Action::CliPipe { name, payload, .. } =
        archive.action().unwrap()
    else {
        panic!("store pipe")
    };
    assert_eq!(name.as_deref(), Some("store_query"));
    let request: serde_json::Value = serde_json::from_str(payload.as_deref().unwrap()).unwrap();
    assert_eq!(request["args"]["regex"], true);
    assert_eq!(request["args"]["query"], " (?i)panic|ERROR ");
    assert_eq!(request["structured_reply"], true);
}

#[test]
fn clear_draft_requires_explicit_guards_and_valid_numbers() {
    use clap::Parser;
    let args = [
        "agent",
        "clear-draft",
        "terminal_7",
        "--conversation",
        "id",
        "--input-revision",
        "42",
        "--confirm-discard",
        "--interrupted",
        "--json",
    ];
    assert!(matches!(AgentCli::try_parse_from(args).unwrap().command,
        Agent::ClearDraft { input_revision: 42, confirm_discard: true, interrupted: true, json: true, timeout, .. }
        if timeout == std::time::Duration::from_secs(30)));
    for missing in ["--conversation", "--input-revision", "--confirm-discard"] {
        let mut reduced = args.to_vec();
        let index = reduced.iter().position(|arg| *arg == missing).unwrap();
        reduced.remove(index);
        if missing != "--confirm-discard" {
            reduced.remove(index);
        }
        assert!(AgentCli::try_parse_from(reduced).is_err(), "{missing}");
    }
    for revision in ["-1", "no", "18446744073709551616"] {
        let mut invalid = args;
        invalid[6] = revision;
        assert!(AgentCli::try_parse_from(invalid).is_err());
    }
    for timeout in ["0s", "25h", "bad"] {
        assert!(AgentCli::try_parse_from(args.into_iter().chain(["--timeout", timeout])).is_err());
    }
}

#[test]
fn control_agent_registration_is_removed() {
    use clap::Parser;
    for operation in ["register", "unregister"] {
        assert!(AgentCli::try_parse_from(["agent", "control", operation]).is_err());
    }
}

#[test]
fn spawn_tab_flags_accept_stable_zero_and_optional_new_tab_name() {
    use clap::Parser;
    assert!(matches!(
        AgentCli::try_parse_from(["agent", "spawn", "--tab", "0", "--project", "work"])
            .unwrap()
            .command,
        Agent::Spawn {
            tab: Some(0),
            project: Some(_),
            ..
        }
    ));
    for args in [
        vec!["--tab", "review"],
        vec!["--tab", "-1"],
        vec!["--tab", "18446744073709551616"],
        vec!["--tab", "0", "--new-tab"],
        vec!["--suppressed", "--new-tab"],
        vec!["--suppressed", "--new-tab", "review"],
    ] {
        assert!(AgentCli::try_parse_from(["agent", "spawn"].into_iter().chain(args)).is_err());
    }
    assert!(matches!(
        AgentCli::try_parse_from(["agent", "spawn", "--new-tab", "--json"])
            .unwrap()
            .command,
        Agent::Spawn {
            new_tab: Some(None),
            json: true,
            ..
        }
    ));
    assert!(
        matches!(AgentCli::try_parse_from(["agent", "spawn", "--new-tab", "review"]).unwrap().command,
        Agent::Spawn { new_tab: Some(Some(name)), .. } if name == "review")
    );
}

#[test]
fn bubblewrap_is_an_explicit_spawn_option() {
    use clap::Parser;
    for (args, expected) in [
        (vec!["agent", "spawn", "--kind", "codex"], false),
        (
            vec!["agent", "spawn", "--kind", "codex", "--bubblewrap"],
            true,
        ),
    ] {
        let parsed = AgentCli::try_parse_from(args).unwrap();
        assert!(
            matches!(parsed.command, Agent::Spawn { bubblewrap, .. } if bubblewrap == expected)
        );
    }
}

#[test]
fn preserve_draft_is_accepted_only_as_a_hidden_compatibility_flag() {
    use clap::{CommandFactory, Parser};
    let parsed =
        AgentCli::try_parse_from(["agent", "prompt", "7", "review", "--preserve-draft"]).unwrap();
    assert!(matches!(
        parsed.command,
        Agent::Prompt {
            preserve_draft: true,
            ..
        }
    ));
    let mut command = AgentCli::command();
    let help = command
        .find_subcommand_mut("prompt")
        .unwrap()
        .render_long_help()
        .to_string();
    assert!(!help.contains("--preserve-draft"));
}

#[test]
fn suppressed_launch_and_visibility_commands_parse() {
    use clap::Parser;
    for args in [vec!["--suppressed"], vec!["--suppressed", "--tab", "0"]] {
        let command = AgentCli::try_parse_from(["agent", "spawn"].into_iter().chain(args))
            .unwrap()
            .command;
        assert!(matches!(
            command,
            Agent::Spawn {
                suppressed: true,
                ..
            }
        ));
    }
    for verb in ["show", "hide"] {
        assert!(AgentCli::try_parse_from(["agent", verb]).is_err());
        assert!(AgentCli::try_parse_from(["agent", verb, "plugin_7"]).is_err());
        let command =
            AgentCli::try_parse_from(["agent", verb, "7", "--conversation", "thread", "--json"])
                .unwrap()
                .command;
        assert!(command.json());
        assert!(matches!(command,
            Agent::Show { target, conversation: Some(ref conversation), json: true }
            | Agent::Hide { target, conversation: Some(ref conversation), json: true }
            if target.to_string() == "terminal_7" && conversation == "thread"));
    }
}
