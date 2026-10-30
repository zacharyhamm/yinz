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
    for (version, command) in [("old", None), (VERSION, Some(Command::Repl { eval: None }))] {
        let request = Request {
            version: version.into(),
            contract: CLIENT_SERVER_CONTRACT_VERSION,
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
fn remote_helper_reports_a_peers_undecodable_command_as_a_mismatch() {
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
        format!("{error:#}").contains("Yinz compatibility mismatch"),
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
fn a_hosts_layout_directory_does_not_locate_local_layouts() {
    let kdl = serde_json::to_vec(r#"layout_dir "/only/on/the/host""#).unwrap();
    assert_eq!(parse_remote_config(&kdl).unwrap().options.layout_dir, None);
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
