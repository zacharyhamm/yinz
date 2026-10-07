use yinz_config::config::Config;
use yinz_types::{
    data::InputMode,
    keys::{BareKey, KeyWithModifier},
};

#[test]
fn cli_pipe_moves_arguments_and_configuration_without_cloning() {
    use yinz_types::input::{actions::Action, layout::PluginUserConfiguration};

    let args: PluginUserConfiguration = "cwd=/tmp,title=a=b,empty=".parse().unwrap();
    let configuration: PluginUserConfiguration = "theme=dark".parse().unwrap();
    let args_pointer = args.inner()["cwd"].as_ptr();
    let configuration_pointer = configuration.inner()["theme"].as_ptr();
    let actions = crate::actions::actions_from_cli(
        crate::args::CliAction::Pipe {
            focus_plugin: false,
            name: None,
            payload: None,
            args: Some(args),
            plugin: None,
            plugin_configuration: Some(configuration),
            configuration_json: None,
            force_launch_plugin: false,
            skip_plugin_cache: false,
            floating_plugin: None,
            in_place_plugin: None,
            plugin_cwd: None,
            plugin_title: None,
        },
        Box::new(std::path::PathBuf::new),
        None,
    )
    .unwrap();
    let Action::CliPipe {
        args: Some(args),
        configuration: Some(configuration),
        ..
    } = &actions[0]
    else {
        panic!("expected a pipe with arguments and configuration");
    };
    assert_eq!(args["cwd"], "/tmp");
    assert_eq!(args["title"], "a=b");
    assert_eq!(args["empty"], "");
    assert_eq!(configuration["theme"], "dark");
    assert_eq!(args["cwd"].as_ptr(), args_pointer);
    assert_eq!(configuration["theme"].as_ptr(), configuration_pointer);
}

fn new_tab_config(metadata: &str, layout_dir: &std::path::Path) -> Config {
    Config::from_kdl(
        &format!(
            r#"
            layout_dir {:?}
            default_layout "custom"
            keybinds clear-defaults=true {{
                normal {{
                    bind "Ctrl t" {{ NewTab {metadata}; SwitchToMode "Normal"; }}
                }}
            }}
            "#,
            layout_dir.display().to_string(),
        ),
        None,
    )
    .unwrap()
}
#[test]
fn new_tab_without_swap_layouts_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    let mut config = new_tab_config(r#"{ layout "custom"; }"#, dir.path());
    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
        .unwrap();
    // The plugin action wire, like IPC, represents absent swap lists as empty lists.
    let wire: yinz_plugin_api::action::ProtobufAction = binding[0].clone().try_into().unwrap();
    binding[0] = wire.try_into().unwrap();
    config
        .write_config_to_disk(
            config.to_string(false).unwrap(),
            &dir.path().join("config.kdl"),
        )
        .unwrap();
}
#[test]
fn new_tab_with_pane_colours_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("custom.kdl"),
        r##"
layout {
    pane default_fg="#ff0000" default_bg="#000000" { pane; pane default_fg="#00ff00"; }
    floating_panes {
        pane default_fg="#0000ff" default_bg="#ffffff"
    }
    swap_tiled_layout name="columns" {
        tab max_panes=3 { pane default_bg="#101010"; pane; pane; }
    }
    swap_floating_layout name="floats" {
        floating_panes max_panes=2 { pane default_fg="#202020"; pane; }
    }
}
"##,
    )
    .unwrap();
    for metadata in ["{}", r#"{ layout "custom"; }"#] {
        let mut config = new_tab_config(metadata, dir.path());
        let binding = config
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
            .unwrap();
        assert!(format!("{:?}", binding[0]).contains("default_fg: Some("));
        // The plugin action wire carries no pane colours.
        let wire: yinz_plugin_api::action::ProtobufAction = binding[0].clone().try_into().unwrap();
        binding[0] = wire.try_into().unwrap();
        assert!(!format!("{:?}", binding[0]).contains("default_fg: Some("));
        config
            .write_config_to_disk(
                config.to_string(false).unwrap(),
                &dir.path().join("config.kdl"),
            )
            .unwrap_or_else(|e| panic!("{metadata}: {e:?}"));
    }
}

#[test]
fn cli_pipe_focus_is_opt_in_and_requires_a_destination() {
    // The derived debug CLI parser exceeds libtest's stack (see args.rs tests).
    std::thread::Builder::new().stack_size(32 * 1024 * 1024).spawn(|| {
    use crate::args::{CliAction, CliArgs, Command, Sessions};
    use clap::Parser;
    use yinz_types::input::actions::Action;

    for prefix in [vec!["yinz", "pipe"], vec!["yinz", "action", "pipe"]] {
        for focus in [false, true] {
            let mut argv = prefix.clone();
            argv.extend(["--plugin", "recipient"]);
            if focus {
                argv.push("--focus-plugin");
            }
            let command = CliArgs::try_parse_from(argv).unwrap().command.unwrap();
            if let Command::Sessions(session) = &command {
                let mut old_session = serde_json::to_value(session).unwrap();
                old_session["Pipe"].as_object_mut().unwrap().remove("focus_plugin");
                assert!(matches!(serde_json::from_value::<Sessions>(old_session).unwrap(),
                    Sessions::Pipe { focus_plugin: false, .. }));
            }
            let action = match command {
                Command::Action(action) => *action,
                Command::Sessions(Sessions::Pipe {
                    focus_plugin,
                    name,
                    payload,
                    args,
                    plugin,
                    plugin_configuration,
                    configuration_json,
                }) => CliAction::Pipe {
                    focus_plugin,
                    name,
                    payload,
                    args,
                    plugin,
                    plugin_configuration,
                    configuration_json,
                    force_launch_plugin: false,
                    skip_plugin_cache: false,
                    floating_plugin: None,
                    in_place_plugin: None,
                    plugin_cwd: None,
                    plugin_title: None,
                },
                other => panic!("unexpected command: {other:?}"),
            };
            let mut old_cli = serde_json::to_value(&action).unwrap();
            old_cli["Pipe"]
                .as_object_mut()
                .unwrap()
                .remove("focus_plugin");
            assert!(matches!(
                serde_json::from_value::<CliAction>(old_cli).unwrap(),
                CliAction::Pipe {
                    focus_plugin: false,
                    ..
                }
            ));
            let actions =
                crate::actions::actions_from_cli(action, Box::new(std::path::PathBuf::new), None)
                    .unwrap();
            assert!(
                matches!(actions[0], Action::CliPipe { focus_plugin, .. } if focus_plugin == focus)
            );
            let mut old_action = serde_json::to_value(&actions[0]).unwrap();
            old_action["CliPipe"]
                .as_object_mut()
                .unwrap()
                .remove("focus_plugin");
            assert!(matches!(
                serde_json::from_value::<Action>(old_action).unwrap(),
                Action::CliPipe {
                    focus_plugin: false,
                    ..
                }
            ));
        }
        let mut invalid = prefix.clone();
        invalid.push("--focus-plugin");
        assert!(CliArgs::try_parse_from(invalid).is_err());
        let mut help = prefix;
        help.push("--help");
        let help = CliArgs::try_parse_from(help).unwrap_err().to_string();
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            help.contains("Does not focus an existing or already-loading destination"),
            "{help}"
        );
        assert!(help.contains("hidden floating layer"), "{help}");
    }
    }).unwrap().join().unwrap();
}
