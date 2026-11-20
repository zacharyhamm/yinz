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
fn graphical_capture_preflight_follows_the_recording_link_without_creating_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("capture.frames");
    std::os::unix::fs::symlink("output/new.frames", &link).unwrap();
    assert!(Recorder::check_destination(&link).is_err());
    let output = dir.path().join("output");
    std::fs::create_dir(&output).unwrap();
    Recorder::check_destination(&link).unwrap();
    assert!(!output.join("new.frames").try_exists().unwrap());
}

#[test]
fn render_negotiation_fallback_passes_literal_shell_arguments() {
    let name = "-team's demo $HOME";
    for (endpoint, expected) in [
        (Endpoint::Local, vec!["attach", "--", name]),
        (
            Endpoint::Forwarded(Some("Team's Mac".into())),
            vec!["attach", "--remote=Team's Mac", "--", name],
        ),
        (
            Endpoint::Forwarded(None),
            vec!["attach", "--remote", "HOST", "--", name],
        ),
    ] {
        let guidance = endpoint.ansi_attach(name);
        let command = guidance.split('`').nth(1).unwrap();
        // Capture argv from the literal command displayed to the user.
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("yinz() {{ printf '%s\\n' \"$@\"; }}\n{command}"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            expected
        );
    }
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
            assert!(!cli_assets.checked_native_layout);
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
        assert!(cli_assets.checked_native_layout);
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

#[cfg(unix)]
#[test]
fn underline_metrics_native_connection_prepares_with_unused_switch_swap() {
    use std::{fs, process::Command};
    use yinz_types::internal_ids::{TabNumber, TerminalId};
    use yinz_types::pane_id::PaneId;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let config = root.join("config.kdl");
    let startup_layout = root.join("startup.kdl");
    let target_layout = root.join("target.kdl");
    let binding_layout = root.join("binding.kdl");
    let unused_swap = binding_layout.with_extension("swap.kdl");
    fs::write(
        &config,
        format!(
            "layout_dir {:?}\nmouse_mode false\nterminal {{ underline_offset -2; }}\n\
             keybinds {{ normal {{ bind \"Ctrl y\" {{ SwitchSession {{ name \"other\"; layout \"binding\"; }}; }}; }}; }}",
            root.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(&startup_layout, "layout { pane; }\npane_frames false").unwrap();
    fs::write(&target_layout, "layout { pane; }\npane_frames true").unwrap();
    fs::write(
        &binding_layout,
        r#"layout { tab name="Binding" { pane; }; }"#,
    )
    .unwrap();
    fs::write(&unused_swap, "swap_tiled_layout { tab { pane; }; }").unwrap();
    // Request a qualifying missing path to also exercise the .kdl fallback.
    let startup_path = startup_layout.with_extension("layout");
    let opts = CliArgs {
        config: Some(config),
        config_dir: Some(root.clone()),
        layout: Some(startup_path.clone()),
        ..Default::default()
    };
    let args = TerminalArgs {
        session_name: Some("startup".into()),
        ..Default::default()
    };
    let target = SessionConnection {
        name: Some("target".into()),
        tab_position: Some(TabNumber::new(2)),
        pane_id: Some(PaneId::Terminal(TerminalId::new(3))),
        layout: Some(LayoutInfo::File(
            target_layout.display().to_string(),
            Default::default(),
        )),
        cwd: Some(root.clone()),
    };
    let sessions = sessions(&["startup", "target"]);

    for input in ["regular", "absent", "fifo"] {
        match input {
            "absent" => fs::remove_file(&unused_swap).unwrap(),
            "fifo" => assert!(Command::new("mkfifo")
                .arg(&unused_swap)
                .status()
                .unwrap()
                .success()),
            _ => {},
        }
        for (switch, name, expected_layout) in [
            (None, "startup", &startup_layout),
            (Some(&target), "target", &target_layout),
        ] {
            let prepared = prepare_connection(&args, &opts, SIZE, switch, &sessions).unwrap();
            assert_eq!(prepared.name, name);
            assert!(!prepared.new);
            let ClientToServerMsg::AttachClient {
                cli_assets,
                tab_number_to_focus,
                pane_to_focus,
            } = prepared.opening
            else {
                panic!("expected attach handshake for {name}");
            };
            let Some(LayoutInfo::File(path, metadata)) = cli_assets.layout else {
                panic!("expected a file layout in the {name} handshake");
            };
            assert!(cli_assets.retains_source_on_switch);
            assert_eq!(Path::new(&path), expected_layout);
            assert_eq!(metadata.tabs.len(), 1);
            assert_eq!(
                tab_number_to_focus,
                switch.and_then(|target| target.tab_position)
            );
            assert_eq!(pane_to_focus, switch.and_then(|target| target.pane_id));
            assert_eq!(
                cli_assets.cwd.as_ref(),
                switch.and_then(|target| target.cwd.as_ref())
            );
            assert_eq!(cli_assets.terminal_window_size, SIZE);
            let options = cli_assets.configuration_options.unwrap();
            assert_eq!(options.pane_frames, Some(switch.is_some()));
            assert_eq!(options.mouse_mode, Some(false));
        }
    }
}

#[cfg(unix)]
#[test]
fn underline_metrics_native_connection_refuses_unusable_selected_layout_inputs() {
    use std::{fs, os::unix::fs::symlink, process::Command};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let config = root.join("config.kdl");
    let selected = root.join("selected.kdl");
    let swap = selected.with_extension("swap.kdl");
    let contents = root.join("preview.txt");
    let pipe = root.join("pipe");
    assert!(Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap()
        .success());
    fs::write(
        &config,
        format!(
            "layout_dir {:?}\nterminal {{ underline_thickness 3; }}",
            root.to_str().unwrap()
        ),
    )
    .unwrap();
    let opts = CliArgs {
        config: Some(config),
        config_dir: Some(root),
        layout: Some(selected.clone()),
        ..Default::default()
    };
    let args = TerminalArgs {
        session_name: Some("selected".into()),
        ..Default::default()
    };
    let sessions = sessions(&["selected"]);
    let prepare = |opts: &CliArgs| prepare_connection(&args, opts, SIZE, None, &sessions);
    let assert_refused = |opts: &CliArgs, path: &Path| {
        let Err(error) = prepare(opts) else {
            panic!("expected preparation to refuse {}", path.display());
        };
        let error = format!("{error:#}");
        for expected in [
            path.to_str().unwrap(),
            "not a regular file",
            "Restore a readable regular file",
        ] {
            assert!(error.contains(expected), "{error}");
        }
    };

    symlink(&pipe, &selected).unwrap();
    assert_refused(&opts, &selected);
    fs::remove_file(&selected).unwrap();
    fs::write(&selected, "layout { pane; }").unwrap();
    assert!(prepare(&opts).is_ok());
    assert!(Command::new("mkfifo")
        .arg(&swap)
        .status()
        .unwrap()
        .success());
    assert_refused(&opts, &swap);
    fs::remove_file(&swap).unwrap();
    assert!(prepare(&opts).is_ok());

    fs::write(&selected, "layout { pane contents_file=\"preview.txt\"; }").unwrap();
    symlink(&pipe, &contents).unwrap();
    assert_refused(&opts, &contents);
    fs::remove_file(&contents).unwrap();
    fs::write(&contents, "preview").unwrap();
    assert!(prepare(&opts).is_ok());
}

#[cfg(unix)]
#[test]
fn checked_native_preparation_and_server_loader_agree_on_bare_default() {
    use std::{fs, process::Command};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let layouts = root.join("layouts");
    fs::create_dir(&layouts).unwrap();
    let main = root.join("config.kdl");
    fs::write(
        &main,
        "terminal { underline_thickness 2; }\npane_frames true",
    )
    .unwrap();
    let collision = layouts.join("default");
    let kdl = collision.with_extension("kdl");
    let args = TerminalArgs {
        create: true,
        session_name: Some("native".into()),
        ..Default::default()
    };
    let files = [(&collision, "Extensionless", 4), (&kdl, "WithExtension", 6)];
    let write = |path: &Path, tab: &str, thickness: i32| {
        fs::write(path, format!("layout {{ tab name=\"{tab}\" {{ pane; }}; }}\nterminal {{ underline_thickness {thickness}; }}\npane_frames false")).unwrap();
    };
    let opts = |layout| CliArgs {
        config_dir: Some(root.clone()),
        config: Some(main.clone()),
        layout,
        ..Default::default()
    };
    let prepare = |opts: &CliArgs| prepare_connection(&args, opts, SIZE, None, &[]);
    let check = |layout, expected_file: Option<(&Path, &str, i32)>| {
        let ClientToServerMsg::FirstClientConnected { cli_assets } =
            prepare(&opts(layout)).unwrap().opening
        else {
            panic!("expected first-client handshake");
        };
        assert!(cli_assets.checked_native_layout);
        assert!(cli_assets.retains_source_on_switch);
        let (config, layout) =
            yinz_config::cli_assets::load_config_and_layout(&cli_assets).unwrap();
        let options = cli_assets.configuration_options.unwrap();
        if let Some((expected_path, tab, thickness)) = expected_file {
            let Some(LayoutInfo::File(path, metadata)) = cli_assets.layout else {
                panic!("expected {expected_path:?}");
            };
            assert_eq!(Path::new(&path), expected_path);
            assert_eq!(metadata.tabs[0].name.as_deref(), Some(tab));
            assert_eq!(layout.tabs()[0].0.as_deref(), Some(tab));
            assert_eq!(config.terminal.underline_thickness, Some(thickness));
            assert_eq!(options.pane_frames, Some(false));
        } else {
            assert_eq!(
                cli_assets.layout,
                Some(LayoutInfo::BuiltIn("default".into()))
            );
            assert_eq!(layout, yinz_config::layout::default_layout_asset());
            assert_eq!(config.terminal.underline_thickness, Some(2));
            assert_eq!(options.pane_frames, Some(true));
        }
    };

    write(files[0].0, files[0].1, files[0].2);
    for bare in [None, Some("default".into())] {
        check(bare, None);
    }
    check(
        Some(collision.clone()),
        Some((&collision, "Extensionless", 4)),
    );
    write(files[1].0, files[1].1, files[1].2);
    check(None, Some((&collision, "Extensionless", 4)));
    fs::remove_file(&collision).unwrap();
    check(Some("default".into()), Some((&kdl, "WithExtension", 6)));
    check(
        Some(collision.with_extension("missing")),
        Some((&kdl, "WithExtension", 6)),
    );
    fs::remove_file(&kdl).unwrap();

    assert!(Command::new("mkfifo")
        .arg(&collision)
        .status()
        .unwrap()
        .success());
    for bare in [None, Some("default".into())] {
        check(bare, None);
    }
    let error = prepare(&opts(Some(collision.clone()))).err().unwrap();
    assert!(format!("{error:#}").contains(collision.to_str().unwrap()));
    assert!(format!("{error:#}").contains("not a regular file"));
}
