use super::*;
use clap::Parser;
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    operation: Operations,
}
#[test]
fn removed_agent_operations_are_rejected() {
    for name in [
        "agent-list",
        "agent-register",
        "agent-spawn",
        "agent-send",
        "agent-control",
    ] {
        assert!(Cli::try_parse_from(["action", name]).is_err());
    }
}

#[test]
fn pane_operations_accept_terminal_prefix_or_bare_number() {
    for pane_id in ["terminal_5", "5"] {
        assert!(matches!(
            Cli::try_parse_from(["action", "context", "--pane-id", pane_id])
                .unwrap()
                .operation,
            Operations::Context { pane_id: Some(5) }
        ));
        assert!(matches!(
            Cli::try_parse_from(["action", "pane-rerun", pane_id])
                .unwrap()
                .operation,
            Operations::PaneRerun { pane_id: 5 }
        ));
    }
    assert!(Cli::try_parse_from(["action", "context", "--pane-id", "plugin_5"]).is_err());
}
