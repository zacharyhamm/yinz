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
