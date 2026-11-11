use super::*;
use yinz_types::data::SessionConnection;
use yinz_types::internal_ids::SessionName;

fn sessions(names: &[&str]) -> Vec<(SessionName, Duration)> {
    names
        .iter()
        .map(|name| ((*name).into(), Duration::ZERO))
        .collect()
}

#[test]
fn unnamed_reconnect_creates_a_fresh_name_regardless_of_sessions_or_startup_args() {
    for names in [vec![], vec!["existing-a"], vec!["existing-a", "existing-b"]] {
        for terminal_name in [None, Some("stale-terminal")] {
            for cli_name in [None, Some("stale-cli")] {
                let args = TerminalArgs {
                    session_name: terminal_name.map(str::to_owned),
                    ..Default::default()
                };
                let opts = CliArgs {
                    session: cli_name.map(str::to_owned),
                    ..Default::default()
                };
                let name = select_session_name(
                    &args,
                    &opts,
                    &sessions(&names),
                    Some(&SessionConnection::default()),
                )
                .unwrap();
                assert!(!name.is_empty());
                assert!(!names.contains(&name.as_str()));
                assert_ne!(Some(name.as_str()), terminal_name);
                assert_ne!(Some(name.as_str()), cli_name);
                yinz_session::sessions::validate_session_name(&name).unwrap();
            }
        }
    }
}

#[test]
fn named_reconnect_uses_its_target_instead_of_startup_args() {
    let args = TerminalArgs {
        session_name: Some("stale-terminal".into()),
        ..Default::default()
    };
    let opts = CliArgs {
        session: Some("stale-cli".into()),
        ..Default::default()
    };
    for names in [
        vec![],
        vec!["destination"],
        vec!["existing-a", "existing-b"],
    ] {
        let target = SessionConnection {
            name: Some("destination".into()),
            ..Default::default()
        };
        assert_eq!(
            select_session_name(&args, &opts, &sessions(&names), Some(&target)).unwrap(),
            "destination"
        );
    }
}

#[test]
fn initial_launch_prefers_terminal_name_then_cli_name() {
    let mut args = TerminalArgs {
        session_name: Some("terminal-name".into()),
        ..Default::default()
    };
    let opts = CliArgs {
        session: Some("cli-name".into()),
        ..Default::default()
    };
    for names in [vec![], vec!["existing-a"], vec!["existing-a", "existing-b"]] {
        args.session_name = Some("terminal-name".into());
        assert_eq!(
            select_session_name(&args, &opts, &sessions(&names), None).unwrap(),
            "terminal-name"
        );
        args.session_name = None;
        assert_eq!(
            select_session_name(&args, &opts, &sessions(&names), None).unwrap(),
            "cli-name"
        );
    }
}

#[test]
fn initial_launch_uses_the_only_session_generates_when_empty_and_rejects_ambiguity() {
    let args = TerminalArgs::default();
    let opts = CliArgs::default();
    let name = select_session_name(&args, &opts, &[], None).unwrap();
    yinz_session::sessions::validate_session_name(&name).unwrap();
    assert_eq!(
        select_session_name(&args, &opts, &sessions(&["existing-a"]), None).unwrap(),
        "existing-a"
    );
    let error = select_session_name(&args, &opts, &sessions(&["existing-a", "existing-b"]), None)
        .unwrap_err();
    assert!(error.to_string().contains("several local sessions exist"));
}

#[test]
fn explicit_names_are_validated_for_initial_launch_and_reconnect() {
    let args = TerminalArgs {
        session_name: Some("invalid/name".into()),
        ..Default::default()
    };
    let opts = CliArgs::default();
    assert!(select_session_name(&args, &opts, &[], None).is_err());
    let target = SessionConnection {
        name: Some("invalid/name".into()),
        ..Default::default()
    };
    assert!(select_session_name(&args, &opts, &[], Some(&target)).is_err());
}
