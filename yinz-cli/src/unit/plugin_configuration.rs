use crate::args::{CliAction, CliArgs, Command, Sessions};
use clap::Parser;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yinz_types::input::{actions::Action, layout::RunPluginOrAlias};

const COMMANDS: &[&[&str]] = &[
    &["plugin"],
    &["action", "new-pane", "--plugin", "file:/tmp/reader.scm"],
    &["action", "start-or-reload-plugin", "file:/tmp/reader.scm"],
    &["action", "launch-or-focus-plugin", "file:/tmp/reader.scm"],
    &["action", "launch-plugin", "file:/tmp/reader.scm"],
    &["pipe"],
    &["action", "pipe"],
];

fn parse(command: &[&str], flags: &[&str]) -> Result<CliArgs, clap::Error> {
    let mut args = vec!["yinz".to_owned()];
    args.extend(command.iter().chain(flags).map(|s| s.to_string()));
    if command == ["plugin"] || command == ["p"] {
        args.extend(["--".into(), "file:/tmp/reader.scm".into()]);
    }
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || CliArgs::try_parse_from(args))
        .unwrap()
        .join()
        .unwrap()
}

fn action(parsed: CliArgs) -> CliAction {
    match parsed.command.unwrap() {
        Command::Action(action) => *action,
        other => panic!("expected action, got {other:?}"),
    }
}

fn configuration(parsed: CliArgs) -> Value {
    match parsed.command.unwrap() {
        Command::Sessions(Sessions::Plugin {
            configuration_json, ..
        })
        | Command::Sessions(Sessions::Pipe {
            configuration_json, ..
        }) => serde_json::to_value(configuration_json.unwrap()).unwrap(),
        Command::Action(action) => {
            let serialized = serde_json::to_value(action).unwrap();
            serialized.as_object().unwrap().values().next().unwrap()["configuration_json"].clone()
        },
        other => panic!("unexpected command: {other:?}"),
    }
}

fn convert(action: CliAction) -> Result<Vec<Action>, String> {
    crate::actions::actions_from_cli(action, Box::new(|| "/tmp".into()), None)
}

fn runtime_configuration(action: Action) -> BTreeMap<String, String> {
    let plugin = match action {
        Action::NewTiledPluginPane { plugin, .. }
        | Action::NewFloatingPluginPane { plugin, .. }
        | Action::NewInPlacePluginPane { plugin, .. }
        | Action::StartOrReloadPlugin { plugin }
        | Action::LaunchOrFocusPlugin { plugin, .. }
        | Action::LaunchPlugin { plugin, .. } => plugin,
        Action::CliPipe { configuration, .. } => return configuration.unwrap(),
        other => panic!("unexpected action: {other:?}"),
    };
    match plugin {
        RunPluginOrAlias::RunPlugin(plugin) => plugin.configuration.into_inner(),
        RunPluginOrAlias::Alias(alias) => alias.configuration.unwrap().into_inner(),
    }
}

#[test]
fn json_configuration_is_lossless_at_every_entry_point() {
    let expected = json!({"file": " a,b=c=\"quoted\"-日本語-😀-%2C-%25-%ZZ\\ ", "empty": ""});
    for command in COMMANDS.iter().copied().chain([&["p"][..]]) {
        for value in [expected.clone(), json!({}), json!({"unicode": "日"})] {
            let input = if value == json!({"unicode": "日"}) {
                r#"{"unicode":"\u65e5"}"#.to_string()
            } else {
                value.to_string()
            };
            assert_eq!(
                configuration(parse(command, &["--configuration-json", &input]).unwrap()),
                value
            );
            if command[0] == "action" {
                let converted = convert(action(
                    parse(command, &["--configuration-json", &input]).unwrap(),
                ))
                .unwrap();
                let map = runtime_configuration(converted.into_iter().next().unwrap());
                assert_eq!(serde_json::to_value(map).unwrap(), value);
            }
        }
        assert_eq!(
            configuration(
                parse(
                    command,
                    &["--configuration-json", r#"{"a":"first","a":"last"}"#]
                )
                .unwrap()
            ),
            json!({"a":"last"})
        );
    }
}

#[test]
fn invalid_json_and_conflicting_options_are_rejected() {
    for command in COMMANDS {
        for invalid in [
            r#"{"a":"b""#,
            "{} trailing",
            r#"{"a":"\q"}"#,
            "null",
            "[]",
            "1",
            "true",
            r#""text""#,
            r#"{"a":1}"#,
            r#"{"a":true}"#,
            r#"{"a":null}"#,
            r#"{"a":[]}"#,
            r#"{"a":{}}"#,
        ] {
            assert!(
                parse(command, &["--configuration-json", invalid]).is_err(),
                "{command:?}: {invalid}"
            );
        }
        let long = if command.last() == Some(&"pipe") {
            "--plugin-configuration"
        } else {
            "--configuration"
        };
        for legacy in [long, "-c"] {
            if legacy == "-c" && command.get(1) == Some(&"new-pane") {
                continue;
            }
            for value in ["", "a=b"] {
                for flags in [
                    vec![legacy, value, "--configuration-json", "{}"],
                    vec!["--configuration-json", "{}", legacy, value],
                ] {
                    assert_eq!(
                        parse(command, &flags).unwrap_err().kind(),
                        clap::error::ErrorKind::ArgumentConflict
                    );
                }
            }
        }
        assert_eq!(
            configuration(
                parse(
                    command,
                    &[
                        "--configuration-json",
                        r#"{"a":"first"}"#,
                        "--configuration-json",
                        r#"{"a":"last"}"#
                    ]
                )
                .unwrap()
            ),
            json!({"a":"last"})
        );
    }
}

#[test]
fn conversion_preserves_existing_reserved_key_and_alias_behavior() {
    let json = r#"{"file":"plain","cwd":"/reserved","name":"reserved","title":"reserved"}"#;
    let legacy = "file=plain,cwd=/reserved,name=reserved,title=reserved";
    for command in COMMANDS.iter().filter(|command| command[0] == "action") {
        for alias in [false, true] {
            if alias && command.get(1) == Some(&"launch-plugin") {
                continue;
            }
            let command: Vec<_> = command
                .iter()
                .map(|s| {
                    if alias && s.starts_with("file:") {
                        "reader-alias"
                    } else {
                        *s
                    }
                })
                .collect();
            let placements: &[&[&str]] = if command[1] == "new-pane" {
                &[&[], &["--floating"], &["--in-place"]]
            } else {
                &[&[]]
            };
            for placement in placements {
                let flag = if command[1] == "pipe" {
                    "--plugin-configuration"
                } else {
                    "--configuration"
                };
                let mut results = vec![];
                for flags in [["--configuration-json", json], [flag, legacy]] {
                    let flags: Vec<_> = flags.iter().chain(placement.iter()).copied().collect();
                    let converted = convert(action(parse(&command, &flags).unwrap())).unwrap();
                    results.push(runtime_configuration(converted.into_iter().next().unwrap()));
                }
                assert_eq!(results[0], results[1], "{command:?} {placement:?}");
                assert_eq!(results[0]["file"], "plain");
            }
        }
    }
}

#[test]
fn serialization_and_programmatic_conflicts() {
    for command in COMMANDS {
        let old = serde_json::to_value(parse(command, &[]).unwrap()).unwrap();
        assert!(!old.to_string().contains("configuration_json"));
        let decoded: CliArgs = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), old);
        let parsed = parse(command, &["--configuration-json", r#"{"file":"a,b%2C"}"#]).unwrap();
        let serialized = serde_json::to_value(parsed).unwrap();
        let decoded: CliArgs = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), serialized);
        assert_eq!(configuration(decoded), json!({"file":"a,b%2C"}));
    }
    for command in COMMANDS.iter().filter(|command| command[0] == "action") {
        let cli = action(parse(command, &[]).unwrap());
        let mut old = serde_json::to_value(cli).unwrap();
        let fields = old.as_object().unwrap().values().next().unwrap();
        assert!(fields.get("configuration_json").is_none());
        let decoded: CliAction = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), old);
        let fields = old
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .as_object_mut()
            .unwrap();
        fields.insert("configuration_json".into(), json!({}));
        let decoded: CliAction = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), old);
        let field = if command[1] == "pipe" {
            "plugin_configuration"
        } else {
            "configuration"
        };
        old.as_object_mut().unwrap().values_mut().next().unwrap()[field] = json!({});
        assert!(convert(serde_json::from_value(old).unwrap())
            .unwrap_err()
            .contains("mutually exclusive"));
    }
}

#[test]
fn legacy_parser_and_pipe_arguments_remain_unchanged() {
    use yinz_types::input::layout::PluginUserConfiguration;
    for (input, expected) in [
        ("a=one,b=two", json!({"a":"one","b":"two"})),
        ("a=x=y,bare,a=last", json!({"a":"last","bare":""})),
        ("a=x=y", json!({"a":"x=y"})),
        ("a=\"日本語%2C\"", json!({"a":"\"日本語%2C\""})),
        ("file=a,b.pdf", json!({"file":"a","b.pdf":""})),
    ] {
        assert_eq!(
            serde_json::to_value(input.parse::<PluginUserConfiguration>().unwrap()).unwrap(),
            expected
        );
    }
    let converted = convert(action(
        parse(
            &["action", "pipe"],
            &[
                "--configuration-json",
                r#"{"theme":"dark,contrast"}"#,
                "--args",
                "a=x=y,bare",
                "--",
                "payload,%2C",
            ],
        )
        .unwrap(),
    ))
    .unwrap();
    let Action::CliPipe {
        args,
        payload,
        configuration,
        ..
    } = &converted[0]
    else {
        panic!("expected pipe")
    };
    assert_eq!(payload.as_deref(), Some("payload,%2C"));
    assert_eq!(
        serde_json::to_value(args).unwrap(),
        json!({"a":"x=y","bare":""})
    );
    assert_eq!(
        serde_json::to_value(configuration).unwrap(),
        json!({"theme":"dark,contrast"})
    );
}

#[test]
fn session_plugin_lifecycle_commands_parse_without_an_agent() {
    use crate::args::Plugins;
    for operation in ["start", "reload", "stop"] {
        let parsed = parse(
            &["plugins", operation, "plugin_10"],
            &["--configuration-json", r#"{"shared":"true"}"#, "--json"],
        )
        .unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Plugins(
                Plugins::Start { .. } | Plugins::Reload { .. } | Plugins::Stop { .. }
            ))
        ));
    }
    assert!(matches!(
        parse(&["plugins", "list"], &[]).unwrap().command,
        Some(Command::Plugins(Plugins::List { running: false, .. }))
    ));
    assert!(matches!(
        parse(&["plugins", "list"], &["--running", "--json"])
            .unwrap()
            .command,
        Some(Command::Plugins(Plugins::List {
            running: true,
            json: true
        }))
    ));
    assert!(matches!(
        parse(
            &["plugins", "reload", "file:///tmp/palette.scm"],
            &["--headless"]
        )
        .unwrap()
        .command,
        Some(Command::Plugins(Plugins::Reload { headless: true, .. }))
    ));
}
