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
        vec!["agent", "prompt", "terminal_7"],
        vec!["agent", "prompt", "terminal_7", "text", "--file", "-"],
        vec!["agent", "prompt", "7", "text"],
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
        Agent::Spawn { kind, branch: Some(branch), json: true, .. } if kind == "codex" && branch == "review-auth"
    ));
}
