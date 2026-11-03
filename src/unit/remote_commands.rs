use super::*;
use clap::Parser;

fn parse(args: &[&str]) -> CliArgs {
    CliArgs::try_parse_from(args).unwrap()
}

#[test]
fn remote_is_global_and_routes_each_live_command_family() {
    // Clap's complete command tree requires more stack than a default test thread.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for (tail, expected) in [
                (vec!["action", "query-tab-names"], Route::Session),
                (vec!["agent", "list"], Route::Session),
                (vec!["data", "catalog"], Route::Session),
                (vec!["events"], Route::Session),
                (vec!["subscribe", "--pane-id", "1"], Route::Session),
                (vec!["repl", "--eval", "(+ 1 2)"], Route::Session),
                (vec!["watch", "work"], Route::Session),
                (vec!["plugins", "list", "--running"], Route::Host),
                (vec!["plugins", "log", "plugin_1"], Route::Host),
                (vec!["list-sessions"], Route::Host),
                (vec!["kill-session", "work"], Route::Host),
                (vec!["delete-session", "work", "--force"], Route::Host),
                (vec!["kill-all-sessions", "--yes"], Route::Host),
                (vec!["delete-all-sessions", "--yes"], Route::Host),
                (vec!["attach", "work"], Route::Attach),
                (vec!["plugins", "dir"], Route::Local),
                (vec!["hooks", "status"], Route::Local),
                (vec!["skills", "list"], Route::Local),
                (vec!["update"], Route::Local),
            ] {
                let before: Vec<_> = ["yinz", "--remote", "host"]
                    .into_iter()
                    .chain(tail.iter().copied())
                    .collect();
                let after: Vec<_> = ["yinz"]
                    .into_iter()
                    .chain(tail.iter().copied())
                    .chain(["--remote", "host"])
                    .collect();
                for args in [before, after] {
                    let opts = parse(&args);
                    assert_eq!(opts.remote.as_deref(), Some("host"), "{args:?}");
                    assert_eq!(route(opts.command.as_ref()), expected, "{args:?}");
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn remote_selection_never_uses_local_session_context() {
    let names = || vec![SessionName::new("one"), SessionName::new("two")];
    assert_eq!(
        select_name(Some("explicit"), names(), "host")
            .unwrap()
            .as_str(),
        "explicit"
    );
    assert_eq!(
        select_name(None, vec!["only".into()], "host")
            .unwrap()
            .as_str(),
        "only"
    );
    assert!(select_name(None, vec![], "host")
        .unwrap_err()
        .to_string()
        .contains("No active"));
    assert!(select_name(None, names(), "host")
        .unwrap_err()
        .to_string()
        .contains("one, two"));
    assert!(select_name(Some("../bad"), names(), "host").is_err());
}

#[test]
fn remote_helper_rejects_incompatible_and_non_host_requests_before_dispatch() {
    for (contract, command) in [
        (CLIENT_SERVER_CONTRACT_VERSION + 1, None),
        (
            CLIENT_SERVER_CONTRACT_VERSION,
            Some(Command::Repl { eval: None }),
        ),
    ] {
        let request = Request {
            version: VERSION.into(),
            contract,
            bootstrap: REMOTE_BOOTSTRAP_VERSION,
            command,
            session: None,
        };
        assert!(handle(
            &serde_json::to_string(&request).unwrap(),
            &mut CliArgs::default()
        )
        .is_err());
    }
}

#[test]
fn remote_helper_rejects_unknown_commands_with_matching_contracts() {
    let request = serde_json::json!({
        "version": "other",
        "contract": CLIENT_SERVER_CONTRACT_VERSION,
        "bootstrap": REMOTE_BOOTSTRAP_VERSION,
        "command": {"CommandFromAnotherVersion": {}},
        "session": null,
    })
    .to_string();
    let error = handle(&request, &mut CliArgs::default()).unwrap_err();
    assert!(
        format!("{error:#}").contains("decode remote CLI request"),
        "{error:#}"
    );
}

#[test]
fn remote_helper_keeps_this_hosts_configuration_location() {
    let parsed = CliArgs {
        config: Some("/host/config.kdl".into()),
        config_dir: Some("/host/config".into()),
        session: Some("ignored".into()),
        ..Default::default()
    };
    let opts = host_options(
        Command::Sessions(Sessions::ListSessions {
            no_formatting: false,
            short: false,
            exited: false,
            reverse: false,
        }),
        Some("work".into()),
        &parsed,
    );
    assert_eq!(opts.config, parsed.config);
    assert_eq!(opts.config_dir, parsed.config_dir);
    assert_eq!(opts.session.as_deref(), Some("work"));
}

#[test]
fn only_running_sessions_can_be_selected_implicitly() {
    use yinz_cli::{
        session_picker::{Destination, PickerRow},
        sessions::SessionListing,
    };
    let row = |name: &str, listing| PickerRow {
        label: name.into(),
        destination: Some(Destination::Remote {
            host: "host".into(),
            name: name.into(),
        }),
        status: None,
        age: Duration::ZERO,
        listing,
        current: false,
    };
    let names = running_names(vec![
        row("wedged", SessionListing::Unresponsive),
        row("work", SessionListing::Running),
        row("sandboxed", SessionListing::Denied),
    ]);
    assert_eq!(names, vec![SessionName::new("work")]);
}

#[test]
fn only_config_consuming_actions_read_the_remote_configuration() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for (tail, expected) in [
                (vec!["action", "list-panes"], false),
                // Layouts are read here and name no host directory, so these need no read.
                (vec!["action", "new-tab"], false),
                (vec!["action", "switch-session", "other"], false),
                (
                    vec!["action", "reconfigure", "--kdl", "pane_frames false"],
                    true,
                ),
                (vec!["run", "--", "/usr/bin/true"], false),
            ] {
                let args: Vec<_> = ["yinz"].into_iter().chain(tail.iter().copied()).collect();
                let opts = parse(&args);
                assert_eq!(
                    reads_session_config(opts.command.as_ref()),
                    expected,
                    "{args:?}"
                );
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn large_remote_configuration_preserves_custom_plugin_aliases() {
    let config = Config::from_kdl(
        &format!(
            "plugins {{ custom-remote location=\"yinz:tab-bar\" {{ greeting \"{}\"; }}; }}",
            "x".repeat(128 * 1024)
        ),
        None,
    )
    .unwrap();
    let response = serde_json::to_vec(&config.to_string(false).unwrap()).unwrap();
    assert!(response.len() > 64 * 1024);
    let dir = tempfile::tempdir().unwrap();
    let response_path = dir.path().join("response.json");
    std::fs::write(&response_path, response).unwrap();
    yinz_test_support::write_warm_script(
        &dir.path().join("ssh"),
        "exec /bin/cat \"$YINZ_TEST_CONFIG_RESPONSE\"",
    );
    let env = yinz_types::envs::EnvironmentVariables::from_data(
        [
            ("PATH".into(), dir.path().to_str().unwrap().into()),
            (
                "YINZ_TEST_CONFIG_RESPONSE".into(),
                response_path.to_str().unwrap().into(),
            ),
        ]
        .into(),
    );
    let request = Request {
        version: VERSION.into(),
        contract: CLIENT_SERVER_CONTRACT_VERSION,
        bootstrap: REMOTE_BOOTSTRAP_VERSION,
        command: None,
        session: None,
    };

    let parsed = read_remote_config(
        "host",
        &request,
        &env,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    assert_eq!(
        parsed.plugins.aliases.get("custom-remote").unwrap(),
        config.plugins.aliases.get("custom-remote").unwrap()
    );
}

#[test]
fn only_a_rejected_helper_command_reports_a_version_mismatch() {
    let rejected = b"error: unrecognized subcommand 'remote-command'\n";
    assert!(version_rejection("host", 2, rejected)
        .unwrap()
        .to_string()
        .contains("Yinz compatibility mismatch"));
    assert!(version_rejection("host", 0, rejected).is_none());
    assert!(version_rejection("host", 1, b"plugin: unexpected argument 'x'\n").is_none());
    let compared = b"Error: check remote CLI request: Yinz compatibility mismatch: this machine has Yinz 9; peer has Yinz 1.\n";
    assert!(version_rejection("host", 1, compared)
        .unwrap()
        .to_string()
        .starts_with("host reported: "));
}

#[test]
fn host_commands_keep_their_exit_status_and_forward_stderr() {
    let cancellation = StartupCancellation::new().unwrap();
    let mut command = ProcessCommand::new("sh");
    command.args(["-c", "echo diagnostic >&2; exit 3"]);
    assert_eq!(run_host("host", command, &cancellation).unwrap(), 3);
    let mut command = ProcessCommand::new("sh");
    command.args([
        "-c",
        "echo \"error: unrecognized subcommand 'remote-command'\" >&2; exit 2",
    ]);
    assert!(format!(
        "{:#}",
        run_host("host", command, &cancellation).unwrap_err()
    )
    .contains("Yinz compatibility mismatch"));
}

#[test]
fn ssh_request_quotes_json_without_a_remote_terminal() {
    let request = Request {
        version: VERSION.into(),
        contract: CLIENT_SERVER_CONTRACT_VERSION,
        bootstrap: REMOTE_BOOTSTRAP_VERSION,
        command: Some(Command::Sessions(Sessions::KillSession {
            target_session: Some("a'$(touch bad)".into()),
        })),
        session: None,
    };
    let command = ssh_command("alias", &request, &Default::default()).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"-T".to_owned()));
    assert!(args.last().unwrap().contains("'\\''"));
    assert!(ssh_command("-oProxyCommand=bad", &request, &Default::default()).is_err());
}
