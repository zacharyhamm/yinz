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
fn spawn_help_describes_every_option_and_all_recognized_kinds() {
    use clap::{CommandFactory, Parser};
    let help = AgentCli::try_parse_from(["agent", "spawn", "--help"])
        .err()
        .unwrap()
        .to_string();
    for kind in yinz_types::agent::KINDS {
        assert!(help.contains(kind), "{kind}: {help}");
        assert!(
            matches!(AgentCli::try_parse_from(["agent", "spawn", "--kind", kind]).unwrap().command,
            Agent::Spawn { kind: Some(value), .. } if value == *kind)
        );
    }
    let command = AgentCli::command();
    let spawn = command.find_subcommand("spawn").unwrap();
    for argument in spawn.get_arguments() {
        assert!(argument.get_help().is_some(), "{}", argument.get_id());
    }
    for text in [
        "stdin",
        "Flags override JSON",
        "command, args, env and hooks",
        "default: true",
        "SSH",
        "Firecracker",
    ] {
        // Long descriptions wrap at terminal width.
        assert!(
            help.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(text),
            "{help}"
        );
    }
    for alias in ["CODEX", "cursor-agent", "claude.js", "gemini"] {
        assert!(AgentCli::try_parse_from(["agent", "spawn", "--kind", alias]).is_ok());
    }
}
