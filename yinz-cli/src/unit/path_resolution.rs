use super::*;
use clap::Parser;
use yinz_types::input::layout::{RunPluginLocation, RunPluginOrAlias};

fn translate(args: &[&str], paths: PathResolution) -> Vec<Action> {
    let args: Vec<String> = ["yinz", "action"]
        .into_iter()
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    let parsed = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || crate::args::CliArgs::try_parse_from(args))
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
    let Some(crate::args::Command::Action(action)) = parsed.command else {
        panic!("expected action")
    };
    actions_from_cli(*action, paths, None).unwrap()
}

fn local(base: &str) -> PathResolution {
    let base = PathBuf::from(base);
    PathResolution::Local(Box::new(move || base.clone()))
}

#[test]
fn cwd_defaults_and_explicit_paths_follow_the_selected_mode() {
    for base in [Some("/caller"), Some(""), None] {
        for cwd in [None, Some("relative/work"), Some("/absolute/work")] {
            let expected = match base {
                Some(base) => Some(match cwd {
                    Some(cwd) => PathBuf::from(base).join(cwd),
                    None => PathBuf::from(base),
                }),
                None => cwd.map(PathBuf::from),
            };
            for command in ["new-pane", "new-tab", "pipe"] {
                let mut args = vec![command];
                if let Some(cwd) = cwd {
                    args.extend([
                        if command == "pipe" {
                            "--plugin-cwd"
                        } else {
                            "--cwd"
                        },
                        cwd,
                    ]);
                }
                if command == "new-pane" {
                    args.extend(["--", "/usr/bin/true"]);
                }
                let actions = translate(&args, base.map_or(PathResolution::Session, local));
                let actual = match &actions[0] {
                    Action::NewTiledPane {
                        command: Some(command),
                        ..
                    } => &command.cwd,
                    Action::NewTab { cwd, .. } | Action::CliPipe { cwd, .. } => cwd,
                    other => panic!("unexpected action: {other:?}"),
                };
                assert_eq!(actual, &expected, "{args:?}, base {base:?}");
            }
        }
    }
}

#[test]
fn edit_paths_follow_the_selected_cwd() {
    for base in [Some("/caller"), None] {
        for cwd in [None, Some("relative/work"), Some("/absolute/work")] {
            for file in ["relative/file.rs", "/absolute/file.rs"] {
                let mut args = vec!["edit", file];
                if let Some(cwd) = cwd {
                    args.extend(["--cwd", cwd]);
                }
                let actions = translate(&args, base.map_or(PathResolution::Session, local));
                let expected_cwd = match base {
                    Some(base) => Some(PathBuf::from(base).join(cwd.unwrap_or(""))),
                    None => cwd.map(PathBuf::from),
                };
                let expected_file = expected_cwd
                    .as_ref()
                    .map_or_else(|| PathBuf::from(file), |cwd| cwd.join(file));
                let Action::EditFile { payload, .. } = &actions[0] else {
                    panic!("expected edit action")
                };
                assert_eq!(payload.cwd, expected_cwd);
                assert_eq!(payload.path, expected_file);
            }
        }
    }
}

#[test]
fn plugin_files_and_aliases_use_explicit_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let relative = "relative/plugin.scm";
    let absolute = base.join("absolute.scm");
    for session in [false, true] {
        for url in [
            format!("file:{relative}"),
            format!("file:{}", absolute.display()),
            "my-plugin".into(),
        ] {
            for command in [
                "launch-plugin",
                "launch-or-focus-plugin",
                "start-or-reload-plugin",
                "new-pane",
                "new-tab",
            ] {
                let args = if command == "new-pane" {
                    vec![command, "--plugin", &url]
                } else if command == "new-tab" {
                    vec![command, "--initial-plugin", &url]
                } else {
                    vec![command, "--", &url]
                };
                let paths = if session {
                    PathResolution::Session
                } else {
                    local(base.to_str().unwrap())
                };
                let actions = translate(&args, paths);
                let plugin = match &actions[0] {
                    Action::LaunchPlugin { plugin, .. }
                    | Action::LaunchOrFocusPlugin { plugin, .. }
                    | Action::StartOrReloadPlugin { plugin }
                    | Action::NewTiledPluginPane { plugin, .. } => plugin,
                    Action::NewTab {
                        initial_panes: Some(panes),
                        ..
                    } => {
                        let yinz_types::data::CommandOrPlugin::Plugin(plugin) = &panes[0] else {
                            panic!("expected initial plugin")
                        };
                        plugin
                    },
                    other => panic!("unexpected action: {other:?}"),
                };
                if url == "my-plugin" {
                    assert!(matches!(plugin, RunPluginOrAlias::Alias(alias) if alias.name == url));
                } else {
                    let expected = if url == format!("file:{relative}") {
                        if session {
                            PathBuf::from(relative)
                        } else {
                            base.join(relative)
                        }
                    } else {
                        absolute.clone()
                    };
                    assert_eq!(
                        plugin.get_run_plugin().unwrap().location,
                        RunPluginLocation::File(expected),
                        "{args:?}, session {session}"
                    );
                }
            }
        }
    }
}

#[test]
fn empty_local_cwd_still_resolves_plugin_files_locally() {
    let actions = translate(
        &["launch-plugin", "--", "file:relative/plugin.scm"],
        local(""),
    );
    let Action::LaunchPlugin { plugin, cwd, .. } = &actions[0] else {
        panic!("expected plugin")
    };
    assert_eq!(*cwd, Some(PathBuf::new()));
    assert_eq!(
        plugin.get_run_plugin().unwrap().location,
        RunPluginLocation::File(std::env::current_dir().unwrap().join("relative/plugin.scm"))
    );
}

#[test]
fn empty_local_cwd_keeps_layout_references_local() {
    let dir = tempfile::tempdir().unwrap();
    let layout = dir.path().join("local.kdl");
    let source = "layout { pane; }\n";
    std::fs::write(&layout, source).unwrap();
    let args = [
        "switch-session",
        "other",
        "--layout",
        layout.to_str().unwrap(),
    ];
    let actions = translate(&args, local(""));
    assert!(
        matches!(&actions[0], Action::SwitchSession { layout: Some(yinz_types::data::LayoutInfo::File(path, _)), .. } if path == layout.to_str().unwrap())
    );
    let actions = translate(&args, PathResolution::Session);
    assert!(
        matches!(&actions[0], Action::SwitchSession { layout: Some(yinz_types::data::LayoutInfo::Stringified(text)), .. } if text == source)
    );
}
