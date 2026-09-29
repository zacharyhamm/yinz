use super::*;
use clap::Parser;
use serde_json::Value;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    operation: Operations,
}

fn spawn_args(flags: &[&str], input: Value) -> Value {
    let cli = Cli::try_parse_from(
        ["action", "agent-spawn", "--file", "-"]
            .into_iter()
            .chain(flags.iter().copied()),
    )
    .unwrap();
    let Action::CliPipe {
        payload: Some(payload),
        ..
    } = cli
        .operation
        .action(&mut input.to_string().as_bytes())
        .unwrap()
    else {
        panic!("expected an operation pipe");
    };
    serde_json::from_str::<Value>(&payload).unwrap()["args"].clone()
}

#[test]
fn spawn_resolves_local_cwd_after_merging_flags_and_json() {
    let caller = std::env::current_dir().unwrap();
    for (flags, input, expected) in [
        (
            vec!["--cwd", "from-flags"],
            json!({"cwd":"from-json"}),
            caller.join("from-flags"),
        ),
        (vec![], json!({"cwd":"from-json"}), caller.join("from-json")),
        (
            vec!["--cwd", "/absolute"],
            json!({}),
            PathBuf::from("/absolute"),
        ),
        (
            vec![],
            json!({"cwd":"/absolute"}),
            PathBuf::from("/absolute"),
        ),
        (
            vec!["--remote-host", "ssh", "--cwd", "relative"],
            json!({}),
            PathBuf::from("relative"),
        ),
        (
            vec![],
            json!({"remote_host":"ssh","cwd":"/remote"}),
            PathBuf::from("/remote"),
        ),
        (
            vec![],
            json!({"remote_host":null,"project":"work","cwd":"relative"}),
            caller.join("relative"),
        ),
    ] {
        assert_eq!(spawn_args(&flags, input)["cwd"], json!(expected));
    }
}

#[test]
fn spawn_defaults_cwd_only_when_a_host_checkout_is_needed() {
    let caller = json!(std::env::current_dir().unwrap());
    for input in [
        json!({}),
        json!({"cwd":null,"project":null,"remote_host":null}),
        json!({"firecracker":"vm","branch":"topic"}),
        json!({"firecracker":"vm","branch":"topic","worktree":false,"cwd":null}),
    ] {
        assert_eq!(spawn_args(&[], input)["cwd"], caller);
    }
    for input in [
        json!({"firecracker":"vm"}),
        json!({"firecracker":"vm","cwd":null}),
        json!({"firecracker":"vm","branch":"  "}),
        json!({"project":"work","cwd":null}),
        json!({"remote_host":"ssh","cwd":null}),
    ] {
        assert!(spawn_args(&[], input).get("cwd").is_none());
    }
    assert_eq!(
        spawn_args(
            &["--firecracker", "vm", "--branch", "topic"],
            json!({"cwd":null})
        )["cwd"],
        caller
    );
    assert!(spawn_args(&["--firecracker", "vm"], json!({}))
        .get("cwd")
        .is_none());
    assert_eq!(
        spawn_args(&[], json!({"args":null})).get("args"),
        Some(&Value::Null),
        "malformed spawn fields must still reach the planner's validation"
    );
}
