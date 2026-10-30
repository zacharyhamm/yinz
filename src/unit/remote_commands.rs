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
fn ssh_request_quotes_json_and_leaves_stdin_available() {
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
