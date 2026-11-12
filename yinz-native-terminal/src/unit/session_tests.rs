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

fn checkpoint_fixture(layout: Option<&str>) -> (tempfile::TempDir, TerminalArgs, CliArgs) {
    let cache = &*yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR;
    std::fs::create_dir_all(cache).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("native-checkpoint-")
        .tempdir_in(cache)
        .unwrap();
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    let args = TerminalArgs {
        session_name: Some(name.to_owned()),
        ..Default::default()
    };
    let config = directory.path().join("config.kdl");
    std::fs::write(&config, "load_plugins clear-defaults=true\n").unwrap();
    let opts = CliArgs {
        config: Some(config),
        config_dir: Some(directory.path().to_owned()),
        ..Default::default()
    };
    if let Some(layout) = layout {
        yinz_runtime::session_checkpoint::write_checkpoint(
            directory.path(),
            "",
            &(layout.into(), Default::default()),
        )
        .unwrap();
    }
    (directory, args, opts)
}

const SIZE: Size = Size { rows: 24, cols: 80 };
const SAVED_LAYOUT: &str = "layout { pane command=\"/usr/bin/true\" start_suspended=true; }";
const SUPPLIED_LAYOUT: &str = "layout { pane name=\"supplied\"; }";

#[test]
fn dead_named_sessions_prepare_a_resurrection_handshake_before_spawning() {
    let (_directory, args, mut opts) = checkpoint_fixture(Some(SAVED_LAYOUT));
    let name = SessionName::new(args.session_name.as_ref().unwrap());
    for supplied in [None, Some(SUPPLIED_LAYOUT)] {
        opts.layout_string = supplied.map(str::to_owned);
        // Exercise terminal names, CLI names, and named switches, including their layout override.
        for source in 0..3 {
            let mut args = args.clone();
            let mut opts = opts.clone();
            let target = SessionConnection {
                name: Some(name.clone()),
                layout: supplied.map(|text| LayoutInfo::Stringified(text.into())),
                ..Default::default()
            };
            if source == 1 {
                args.session_name = None;
                opts.session = Some(name.to_string());
            }
            let prepared =
                prepare_connection(&args, &opts, SIZE, (source == 2).then_some(&target), &[])
                    .unwrap();
            assert_eq!(prepared.name, name.as_str());
            assert!(prepared.new);
            let ClientToServerMsg::FirstClientConnected { cli_assets } = prepared.opening else {
                panic!("expected resurrection handshake");
            };
            assert_eq!(
                cli_assets.layout,
                Some(LayoutInfo::File(
                    yinz_runtime::consts::session_layout_cache_file_name(&name)
                        .display()
                        .to_string(),
                    Default::default(),
                ))
            );
            assert!(!cli_assets.force_run_layout_commands);
            // The logical path must resolve the published checkpoint, not a flat layout file.
            let (_, layout) = yinz_config::cli_assets::load_config_and_layout(&cli_assets).unwrap();
            assert!(format!("{layout:?}").contains("hold_on_start: true"));
        }
    }
}

#[test]
fn missing_checkpoints_keep_normal_new_session_behavior() {
    let (_directory, mut args, mut opts) = checkpoint_fixture(None);
    let error = prepare_connection(&args, &opts, SIZE, None, &[])
        .err()
        .unwrap();
    assert!(error.to_string().contains("use --create"));
    args.create = true;
    for supplied in [None, Some(SUPPLIED_LAYOUT)] {
        opts.layout_string = supplied.map(str::to_owned);
        let prepared = prepare_connection(&args, &opts, SIZE, None, &[]).unwrap();
        assert!(prepared.new);
        let ClientToServerMsg::FirstClientConnected { cli_assets } = prepared.opening else {
            panic!("expected new-session handshake");
        };
        assert_eq!(
            cli_assets.layout,
            Some(match supplied {
                Some(text) => LayoutInfo::Stringified(text.into()),
                None => LayoutInfo::BuiltIn("default".into()),
            })
        );
    }
}

#[test]
fn invalid_checkpoints_fail_preparation_even_with_a_supplied_layout() {
    let (_directory, args, mut opts) = checkpoint_fixture(Some("layout { invalid"));
    opts.layout_string = Some(SUPPLIED_LAYOUT.into());
    let error = prepare_connection(&args, &opts, SIZE, None, &[])
        .err()
        .unwrap();
    let error = format!("{error:#}");
    assert!(error.contains("load native session checkpoint"), "{error}");
    assert!(
        error.contains("Failed to parse resurrection layout"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn broken_checkpoint_pointers_fail_preparation() {
    let (directory, args, opts) = checkpoint_fixture(None);
    std::os::unix::fs::symlink(".checkpoint-missing", directory.path().join("current")).unwrap();
    let error = prepare_connection(&args, &opts, SIZE, None, &[])
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("current pointer but no lock file"));
}

#[test]
fn running_sessions_attach_without_reading_the_checkpoint_and_keep_focus() {
    let (_directory, args, opts) = checkpoint_fixture(Some("invalid saved layout"));
    let name = args.session_name.as_ref().unwrap();
    let target = SessionConnection {
        name: Some(name.clone().into()),
        tab_position: Some(yinz_types::internal_ids::TabNumber::new(2)),
        pane_id: Some(yinz_types::pane_id::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(3),
        )),
        layout: Some(LayoutInfo::Stringified(SUPPLIED_LAYOUT.into())),
        ..Default::default()
    };
    let prepared =
        prepare_connection(&args, &opts, SIZE, Some(&target), &sessions(&[name])).unwrap();
    assert!(!prepared.new);
    let ClientToServerMsg::AttachClient {
        cli_assets,
        tab_number_to_focus,
        pane_to_focus,
    } = prepared.opening
    else {
        panic!("expected attachment handshake");
    };
    assert_eq!(cli_assets.layout, target.layout);
    assert_eq!(tab_number_to_focus, target.tab_position);
    assert_eq!(pane_to_focus, target.pane_id);
}

#[test]
fn unnamed_switches_start_fresh_and_ignore_startup_names_layouts_and_checkpoints() {
    let (_directory, args, mut opts) = checkpoint_fixture(Some("invalid saved layout"));
    opts.session = args.session_name.clone();
    opts.layout_string = Some("invalid startup layout".into());
    let target = SessionConnection {
        layout: Some(LayoutInfo::Stringified(SUPPLIED_LAYOUT.into())),
        ..Default::default()
    };
    let prepared = prepare_connection(&args, &opts, SIZE, Some(&target), &[]).unwrap();
    assert!(prepared.new);
    assert_ne!(Some(prepared.name), args.session_name);
    let ClientToServerMsg::FirstClientConnected { cli_assets } = prepared.opening else {
        panic!("expected fresh-session handshake");
    };
    assert_eq!(cli_assets.layout, target.layout);
}
