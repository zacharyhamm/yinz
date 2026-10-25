#[test]
fn warp_tab_uses_tab_id() {
    use yinz_types::input::actions::Action;

    for (direction, expected) in [
        (
            "--to-session",
            Action::WarpTab {
                tab_id: Some(TabId::new(7)),
                to_session: "work".into(),
            },
        ),
        (
            "--from-session",
            Action::WarpTabFrom {
                tab_id: TabId::new(7),
                from_session: "work".into(),
            },
        ),
    ] {
        let parsed = try_parse_cli([
            "yinz", "action", "warp-tab", direction, "work", "--tab-id", "7",
        ])
        .unwrap();
        let Some(Command::Action(action)) = parsed.command else {
            panic!("expected an action");
        };
        assert_eq!(
            crate::actions::actions_from_cli(*action, Box::new(PathBuf::new), None).unwrap(),
            vec![expected],
        );
        assert!(try_parse_cli([
            "yinz",
            "action",
            "warp-tab",
            direction,
            "work",
            "--tab-index",
            "7",
        ])
        .is_err());
    }
    assert_eq!(
        crate::actions::actions_from_cli(
            CliAction::WarpTab {
                tab_id: None,
                to_session: None,
                from_session: Some("work".into()),
            },
            Box::new(PathBuf::new),
            None
        )
        .unwrap_err(),
        "--from-session also needs --tab-id",
    );
}

#[test]
fn removed_automation_spellings_have_no_aliases() {
    for args in [
        vec!["yinz", "action", "agent-list"],
        vec!["yinz", "action", "agent-wait", "terminal_7", "idle"],
        vec!["yinz", "action", "agent-runs"],
        vec!["yinz", "agents", "skills", "install"],
    ] {
        assert!(try_parse_cli(args).is_err());
    }
}

#[test]
fn help_preserves_the_existing_cli() {
    for (args, expected) in [
        (
            vec!["yinz", "--help"],
            include_str!("../test-fixtures/help-root.txt"),
        ),
        (
            vec!["yinz", "options", "--help"],
            include_str!("../test-fixtures/help-options.txt"),
        ),
        (
            vec!["yinz", "action", "--help"],
            include_str!("../test-fixtures/help-action.txt"),
        ),
    ] {
        let help = try_parse_cli(args).unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        // Clap can pad blank lines with spaces. Compare the visible help text.
        assert_eq!(
            help.to_string()
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>(),
            expected.lines().map(str::trim_end).collect::<Vec<_>>()
        );
    }
}

use super::*;
use clap::Parser;
use hegel::generators;

const CLI_PARSE_STACK_SIZE: usize = 32 * 1024 * 1024;

#[test]
fn legacy_hocus_launchers_remain_compatible_with_running_servers() {
    for launcher in ["codex", "muse"] {
        let parsed = try_parse_cli([
            "yinz",
            "hocus",
            launcher,
            "--command",
            launcher,
            "--",
            "--help",
        ])
        .unwrap();
        match parsed.command {
            Some(Command::Agent(
                Agent::Codex { command, args } | Agent::Muse { command, args },
            )) => {
                assert_eq!(command, std::path::PathBuf::from(launcher));
                assert_eq!(args, ["--help"]);
            },
            other => panic!("expected agent launcher, got {other:?}"),
        }
    }
}

/// clap's derived builder for `CliArgs` needs ~2.5MB of stack in a debug build (the
/// derive expands the huge `CliAction` enum into one unoptimized function), more than
/// libtest's 2MiB per-test threads. Parse on a thread with room.
fn try_parse_cli<I, S>(args: I) -> Result<CliArgs, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    std::thread::Builder::new()
        .stack_size(CLI_PARSE_STACK_SIZE)
        .spawn(move || CliArgs::try_parse_from(args))
        .expect("failed to spawn cli parse thread")
        .join()
        .expect("cli parse thread panicked")
}

#[test]
fn max_panes_must_be_positive_when_provided() {
    let error = try_parse_cli(["yinz", "--max-panes", "0"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    for limit in [1, 12, usize::MAX] {
        let cli = try_parse_cli(["yinz", "--max-panes", &limit.to_string()]).unwrap();
        assert_eq!(cli.max_panes, Some(limit));
    }
    assert_eq!(try_parse_cli(["yinz"]).unwrap().max_panes, None);
}

#[test]
fn remote_bootstrap_accepts_startup_state_and_defaults_to_omitted_flags() {
    for explicit in [false, true] {
        let mut args = vec!["yinz"];
        if explicit {
            args.extend(["--max-panes", "3", "--debug"]);
        }
        args.extend([
            "remote-session",
            "--create",
            "--client-version",
            "test",
            "--contract-version",
            "1",
        ]);
        let size = r#"{"rows":60,"cols":120}"#;
        if explicit {
            args.extend(["--terminal-window-size", size, "--bootstrap-version", "1"]);
        }
        args.extend(["--", "saved session"]);
        let cli = try_parse_cli(args).unwrap();
        assert_eq!(cli.max_panes, explicit.then_some(3));
        assert_eq!(cli.debug, explicit);
        let Some(Command::RemoteSession {
            terminal_window_size,
            bootstrap_version,
            session_name,
            ..
        }) = cli.command
        else {
            panic!("expected remote session bootstrap");
        };
        assert_eq!(terminal_window_size.as_deref(), explicit.then_some(size));
        // Older clients omit it; 0 then fails the version check with a mismatch error.
        assert_eq!(bootstrap_version, usize::from(explicit));
        assert_eq!(session_name, "saved session");
    }
}

#[test]
fn removed_startup_tips_flag_is_unknown() {
    let error = try_parse_cli(["yinz", "options", "--show-startup-tips", "true"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn a_plugin_launch_takes_the_url_and_nothing_else_by_default() {
    let cli = try_parse_cli(["yinz", "action", "launch-or-focus-plugin", "agents"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Action(action))
            if matches!(*action, CliAction::LaunchOrFocusPlugin {
                floating: false,
                in_place: false,
                move_to_focused_tab: false,
                ..
            })
    ));
}

#[test]
fn list_sessions_exited_flag_is_opt_in() {
    let default = try_parse_cli(["yinz", "list-sessions"]).unwrap();
    assert!(matches!(
        default.command,
        Some(Command::Sessions(Sessions::ListSessions {
            exited: false,
            ..
        }))
    ));

    let exited = try_parse_cli(["yinz", "list-sessions", "--exited"]).unwrap();
    assert!(matches!(
        exited.command,
        Some(Command::Sessions(Sessions::ListSessions {
            exited: true,
            ..
        }))
    ));
}

/// Every field of a `yinz subscribe` invocation survives the argv round-trip, with
/// the documented convention that a bare `--scrollback` (all scrollback) is `Some(0)`.
#[hegel::test]
fn subscribe_argv_roundtrips(tc: hegel::TestCase) {
    let pane_ids: Vec<String> = tc
        .draw(
            generators::vecs(generators::sampled_from(vec![
                "terminal_1",
                "terminal_42",
                "plugin_2",
                "3",
            ]))
            .min_size(1)
            .max_size(4),
        )
        .iter()
        .map(|s| s.to_string())
        .collect();
    // None = flag absent, Some(None) = bare flag, Some(Some(n)) = flag with a value
    let scrollback_arg = tc.draw(generators::optional(generators::optional(
        generators::integers::<usize>().max_value(10_000),
    )));
    let json = tc.draw(generators::booleans());
    let ansi = tc.draw(generators::booleans());

    let mut args = vec!["yinz".to_string(), "subscribe".to_string()];
    for id in &pane_ids {
        args.push("--pane-id".into());
        args.push(id.clone());
    }
    match scrollback_arg {
        None => {},
        Some(None) => args.push("--scrollback".into()),
        Some(Some(n)) => {
            args.push("--scrollback".into());
            args.push(n.to_string());
        },
    }
    if json {
        args.push("--format".into());
        args.push("json".into());
    }
    if ansi {
        args.push("--ansi".into());
    }

    let parsed =
        try_parse_cli(args.clone()).unwrap_or_else(|e| panic!("failed to parse {:?}: {}", args, e));
    let s = match parsed.command {
        Some(Command::Subscribe(s)) => s,
        other => panic!("expected Subscribe from {:?}, got {:?}", args, other),
    };

    assert_eq!(s.pane_id, pane_ids, "args: {:?}", args);
    assert_eq!(
        s.scrollback,
        match scrollback_arg {
            None => None,
            Some(None) => Some(0),
            Some(Some(n)) => Some(n),
        },
        "args: {:?}",
        args
    );
    assert_eq!(
        matches!(s.format, SubscribeFormat::Json),
        json,
        "args: {:?}",
        args
    );
    assert_eq!(s.ansi, ansi, "args: {:?}", args);
}

#[test]
fn subscribe_requires_pane_id() {
    let result = try_parse_cli(["yinz", "subscribe"]);
    assert!(result.is_err());
}

#[test]
fn plugins_write_takes_a_name_a_file_and_the_two_flags() {
    let parsed = try_parse_cli([
        "yinz",
        "plugins",
        "write",
        "keys",
        "--file",
        "/tmp/keys.scm",
        "--overwrite",
    ])
    .unwrap();
    match parsed.command {
        Some(Command::Plugins(Plugins::Write {
            name,
            file,
            overwrite,
            no_check,
        })) => {
            assert_eq!(name, "keys");
            assert_eq!(file, Some(PathBuf::from("/tmp/keys.scm")));
            assert!(overwrite && !no_check);
        },
        other => panic!("expected plugins write, got {other:?}"),
    }
    assert!(try_parse_cli(["yinz", "plugins", "check"]).is_ok());
    assert!(try_parse_cli(["yinz", "plugins", "log", "keys", "-l", "5"]).is_ok());
    assert!(try_parse_cli(["yinz", "plugins", "dir"]).is_ok());
    assert!(try_parse_cli(["yinz", "plugins", "write"]).is_err());
}

#[test]
fn lifecycle_events_and_watch_share_filters() {
    let watch = try_parse_cli([
        "yinz",
        "agent",
        "watch",
        "--pane",
        "terminal_7",
        "--kind",
        "codex",
        "--kind",
        "claude",
        "--project",
        "yinz",
        "--lifecycle-type",
        "turn_completed",
        "--cursor",
        "saved",
        "--json",
    ])
    .unwrap();
    let Some(Command::Agent(Agent::Watch { filters, json })) = watch.command else {
        panic!("expected watch");
    };
    assert!(json);
    assert_eq!(filters.pane[0].to_string(), "terminal_7");
    assert_eq!(filters.kind, ["codex", "claude"]);
    assert_eq!(filters.project, ["yinz"]);
    assert_eq!(filters.cursor.as_deref(), Some("saved"));
    let events = try_parse_cli([
        "yinz",
        "events",
        "--event-type",
        "agent_lifecycle",
        "--pane",
        "7",
    ])
    .unwrap();
    let Some(Command::Events(events)) = events.command else {
        panic!("expected events");
    };
    assert_eq!(events.event_type, ["agent_lifecycle"]);
    assert_eq!(events.filters.pane, filters.pane);
    assert!(try_parse_cli(["yinz", "agent", "watch", "--pane", "plugin_7"]).is_err());
}

#[test]
fn events_takes_any_number_of_types() {
    let parsed = try_parse_cli(["yinz", "events"]).unwrap();
    match parsed.command {
        Some(Command::Events(events)) => assert!(events.event_type.is_empty()),
        other => panic!("expected Events, got {other:?}"),
    }
    let parsed = try_parse_cli([
        "yinz",
        "events",
        "--type",
        "PaneUpdate",
        "cwd_changed",
        "-t",
        "TabUpdate",
    ])
    .unwrap();
    match parsed.command {
        Some(Command::Events(events)) => assert_eq!(
            events.event_type,
            vec!["PaneUpdate", "cwd_changed", "TabUpdate"]
        ),
        other => panic!("expected Events, got {other:?}"),
    }
}

/// Both spellings name an event, and only a streamable one: a keystroke is one client's
/// input, not session state, however it is spelled.
#[test]
fn event_types_are_named_in_either_spelling_and_only_if_streamable() {
    use yinz_types::data::EventType;
    assert_eq!(
        EventType::from_cli_name("PaneUpdate"),
        Some(EventType::PaneUpdate)
    );
    assert_eq!(
        EventType::from_cli_name("pane_update"),
        Some(EventType::PaneUpdate)
    );
    assert_eq!(
        EventType::from_cli_name(" file_system_create "),
        Some(EventType::FileSystemCreate)
    );
    assert_eq!(EventType::from_cli_name("Key"), None);
    assert_eq!(EventType::from_cli_name("key"), None);
    assert_eq!(EventType::from_cli_name("NotAnEvent"), None);
    for name in EventType::cli_streamable_names() {
        assert!(EventType::from_cli_name(&name).is_some(), "{name}");
    }
}
#[test]
fn neutral_agent_cli_keeps_hooks_separate_from_launchers() {
    for args in [
        vec!["yinz", "hooks", "status"],
        vec!["yinz", "agent", "codex", "--", "--model", "local"],
        vec!["yinz", "agent", "muse-report"],
        vec!["yinz", "hocus", "codex"],
    ] {
        assert!(try_parse_cli(args).is_ok());
    }
    for args in [
        vec!["yinz", "hocus", "hooks", "status"],
        vec!["yinz", "agent", "hooks", "status"],
    ] {
        assert!(try_parse_cli(args).is_err());
    }
}
