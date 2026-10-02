use super::{display_command, plan, Spec};

#[test]
fn custom_executables_keep_their_path_and_receive_their_agent_launch_support() {
    for kind in [None, Some(" CoDeX ")] {
        let spec = Spec {
            kind: kind.map(str::to_owned),
            command: Some("/opt/tools/codex-cli".into()),
            args: vec!["--search".into()],
            effort: Some("high".into()),
            resume: Some("conversation".into()),
            ..Default::default()
        };
        assert_eq!(super::kind_for_spec(&spec).as_deref(), Some("codex"));
        let (command, _) = plan(&spec).unwrap();
        assert_eq!(command.path.to_str(), Some("yinz"));
        assert_eq!(
            &command.args[..7],
            &[
                "hocus",
                "codex",
                "--command",
                "/opt/tools/codex-cli",
                "--",
                "resume",
                "conversation"
            ]
        );
        assert!(command.args.contains(&"--search".into()));
        assert!(command
            .args
            .contains(&"model_reasoning_effort=\"high\"".into()));
    }
    let (command, _) = plan(&Spec {
        command: Some("/opt/tools/claude.js".into()),
        permission_mode: Some("plan".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(command.path.to_str(), Some("env"));
    assert!(command.args.contains(&"/opt/tools/claude.js".into()));
    assert!(command.args.contains(&"--settings".into()));
    assert!(command.args.contains(&"--permission-mode".into()));
    assert!(command.args.contains(&"YINZ_CLAUDE_INLINE_HOOKS=1".into()));
}

#[test]
fn executable_aliases_share_argument_translation_and_validate_conflicting_kinds() {
    for alias in ["cursor-agent", "CuRsOr-AgEnT"] {
        let (command, _) = plan(&Spec {
            kind: Some(alias.into()),
            permission_mode: Some("force".into()),
            resume: Some("session".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(command.path.to_str(), Some("cursor-agent"));
        assert_eq!(command.args, ["--force", "--resume", "session"]);
    }

    let error = plan(&Spec {
        kind: Some("claude".into()),
        command: Some("/opt/codex".into()),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("conflicts"), "{error}");
    let error = plan(&Spec {
        command: Some("node".into()),
        args: vec!["/opt/codex.js".into()],
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("executable shim"), "{error}");
}

#[test]
fn indirect_agent_resumes_reject_subcommands_that_would_precede_the_executable() {
    for (program, arguments, resume_arguments) in [
        ("node", vec!["/opt/cursor-agent.js"], vec!["resume"]),
        ("node", vec!["/opt/codex.js"], vec!["resume", "--last"]),
        (
            "env",
            vec!["SETTING=value", "/opt/codex"],
            vec!["resume", "--last"],
        ),
    ] {
        let spec = Spec {
            command: Some(program.into()),
            args: arguments.iter().map(|arg| (*arg).to_owned()).collect(),
            hooks: Some(false),
            resume: Some("last".into()),
            ..Default::default()
        };
        let error = plan(&spec).unwrap_err();
        assert!(error.contains("directly executable agent shim"), "{error}");
        assert!(
            error.contains("complete resume invocation in args"),
            "{error}"
        );

        // Callers can supply an exact invocation without Yinz moving the wrapper prefix.
        let mut exact = Spec {
            resume: None,
            ..spec
        };
        exact
            .args
            .extend(resume_arguments.into_iter().map(str::to_owned));
        let (command, _) = plan(&exact).unwrap();
        assert_eq!(command.path.to_str(), Some(program));
        assert_eq!(command.args, exact.args);
    }
}

#[test]
fn managed_codex_resumes_reject_permission_overrides() {
    for mode in super::CODEX_PERMISSION_MODES {
        for resume in ["conversation-id", "last"] {
            for hooks in [None, Some(true), Some(false)] {
                let spec = Spec {
                    kind: Some(" CoDeX ".into()),
                    permission_mode: Some(format!(" {mode} ")),
                    resume: Some(format!(" {resume} ")),
                    hooks,
                    ..Default::default()
                };
                if hooks == Some(false) {
                    let (command, _) = plan(&spec).unwrap();
                    assert_eq!(command.path.to_str(), Some("codex"));
                    assert_eq!(command.args[0], "resume");
                } else {
                    let error = plan(&spec).unwrap_err();
                    assert!(error.contains("omit --permission-mode"), "{error}");
                    assert!(error.contains("start a new conversation"), "{error}");
                }
                for blank in [None, Some(" \t".into())] {
                    assert!(plan(&Spec {
                        resume: blank.clone(),
                        ..spec.clone()
                    })
                    .is_ok());
                    assert!(plan(&Spec {
                        permission_mode: blank,
                        ..spec.clone()
                    })
                    .is_ok());
                }
            }
        }
    }
}

#[test]
fn claude_launches_override_the_inline_marker_to_match_their_hooks() {
    for hooks in [None, Some(true), Some(false)] {
        for firecracker in [None, Some(" ".into()), Some("vm".into())] {
            let inline = hooks != Some(false) && firecracker.as_deref() != Some("vm");
            let (command, _) = plan(&Spec {
                kind: Some(" ClAuDe ".into()),
                hooks,
                firecracker,
                env: [("YINZ_CLAUDE_INLINE_HOOKS".into(), "caller-value".into())].into(),
                ..Default::default()
            })
            .unwrap();
            assert_eq!(command.path.to_str(), Some("env"));
            assert_eq!(
                command.args[0],
                format!("YINZ_CLAUDE_INLINE_HOOKS={}", u8::from(inline))
            );
            assert_eq!(command.args.iter().any(|arg| arg == "--settings"), inline);
        }
    }
}

#[test]
fn codex_rejects_claude_aliases_without_restricting_provider_model_ids() {
    for model in [
        "haiku",
        "sonnet",
        "opus",
        "opusplan",
        "sonnet[1m]",
        " OPUS ",
    ] {
        let spec = Spec {
            kind: Some(" CoDeX ".into()),
            model: Some(model.into()),
            ..Default::default()
        };
        let error = plan(&spec).unwrap_err();
        assert!(error.contains(model.trim()), "{error}");
        assert!(error.contains("--kind claude"), "{error}");
        assert!(error.contains("Codex model ID"), "{error}");
        assert!(plan(&Spec {
            kind: Some("claude".into()),
            ..spec
        })
        .is_ok());
    }
    for model in [
        "gpt-6-luna",
        "future-model",
        "claude-sonnet-4",
        "provider/haiku",
        "my-local-model",
    ] {
        let (command, _) = plan(&Spec {
            kind: Some("codex".into()),
            model: Some(model.into()),
            hooks: Some(false),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(command.args, ["--model", model]);
    }
}

#[test]
fn display_command_drops_only_the_planners_hook_wiring() {
    let (command, _) = plan(&Spec {
        kind: Some("claude".into()),
        model: Some("haiku".into()),
        env: [("FOO".into(), "bar".into())].into(),
        ..Default::default()
    })
    .unwrap();
    let raw = shown(command);
    assert!(raw.len() > 1000, "{raw}");
    assert_eq!(display_command(&raw), "env FOO=bar claude --model haiku");

    let (command, _) = plan(&Spec {
        kind: Some("claude".into()),
        model: Some("haiku".into()),
        ..Default::default()
    })
    .unwrap();
    let raw = shown(command);
    assert_eq!(display_command(&raw), "claude --model haiku");

    // The same settings with their keys in another order, as a plugin may print them.
    let mut reordered = String::from("{\"hooks\":{");
    let settings = crate::agent::claude_hook_settings(crate::agent::CLAUDE_HOOK_REPORT_COMMAND);
    let events = settings["hooks"].as_object().unwrap();
    let mut names: Vec<&String> = events.keys().collect();
    names.reverse();
    for (index, name) in names.into_iter().enumerate() {
        if index > 0 {
            reordered.push(',');
        }
        reordered.push_str(&format!("{}:{}", serde_json::json!(name), events[name]));
    }
    reordered.push_str("}}");
    assert_eq!(
        display_command(&format!(
            "env {}=1 claude --model haiku --settings {reordered} --resume x",
            yinz_types::agent::AGENT_PANE_ENV
        )),
        "claude --model haiku --resume x"
    );

    for kept in [
        "claude --settings {\"hooks\":{}}",
        "env YINZ_CLAUDE_INLINE_HOOKS=1",
        "vim YINZ_CLAUDE_INLINE_HOOKS=1",
        "zsh",
    ] {
        assert_eq!(display_command(kept), kept);
    }
}

fn shown(command: yinz_types::command::CommandToRun) -> String {
    yinz_types::input::command::RunCommand::from(yinz_types::input::command::RunCommandAction {
        command: command.path,
        args: command.args,
        ..Default::default()
    })
    .to_string()
}

#[test]
fn pi_launches_use_host_resolved_reporting_and_keep_user_extensions() {
    use crate::agent::pi::{self, EXTENSION_ARGUMENT};
    for hooks in [None, Some(false), Some(true)] {
        for remote in [false, true] {
            let (mut command, _) = plan(&Spec {
                kind: Some("pi".into()),
                hooks,
                model: Some("provider/model".into()),
                effort: Some("none".into()),
                resume: Some("archived-session".into()),
                prompt: Some("review".into()),
                args: vec!["--extension".into(), "/user/report.ts".into()],
                ..Default::default()
            })
            .unwrap();
            assert!(command
                .args
                .windows(2)
                .any(|pair| pair == ["--thinking", "off"]));
            assert!(command
                .args
                .windows(2)
                .any(|pair| pair == ["--session", "archived-session"]));
            assert!(command
                .args
                .windows(2)
                .any(|pair| pair == ["--model", "provider/model"]));
            assert_eq!(command.args.last().map(String::as_str), Some("review"));
            assert_eq!(
                command.args.iter().any(|arg| arg == EXTENSION_ARGUMENT),
                hooks != Some(false)
            );
            if remote {
                command.target = yinz_types::command::PaneTarget::ssh("test-host").unwrap();
                pi::prepare_command(&mut command).unwrap();
                assert!(!command.args.iter().any(|arg| arg == EXTENSION_ARGUMENT));
            }
            assert!(command
                .args
                .windows(2)
                .any(|pair| pair == ["--extension", "/user/report.ts"]));
        }
    }
    for resume in ["last", "session"] {
        let (command, _) = plan(&Spec {
            kind: Some("pi".into()),
            resume: Some(resume.into()),
            firecracker: Some("vm".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(!command.args.iter().any(|arg| arg == EXTENSION_ARGUMENT));
        assert!(command.args.iter().any(|arg| arg
            == if resume == "last" {
                "--continue"
            } else {
                "--session"
            }));
    }
    assert!(plan(&Spec {
        kind: Some("pi".into()),
        permission_mode: Some("skip".into()),
        ..Default::default()
    })
    .unwrap_err()
    .contains("pi has no permission mode"));
}

#[test]
fn pi_display_and_remote_filtering_match_only_the_generated_argument() {
    use crate::agent::pi::{self, EXTENSION_ARGUMENT};
    let generated = format!(
        "/data with spaces/yinz/pi/{EXTENSION_ARGUMENT}/{}",
        pi::PI_EXTENSION_FILE
    );
    for extension in [EXTENSION_ARGUMENT, generated.as_str()] {
        let args: Vec<String> = [
            "--extension",
            "/user/first.ts",
            "--extension",
            extension,
            "--extension",
            "/user/last.ts",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            super::display_args(&args),
            [
                "--extension",
                "/user/first.ts",
                "--extension",
                "/user/last.ts"
            ]
        );
        let command = yinz_types::input::command::RunCommand {
            command: "pi".into(),
            args,
            ..Default::default()
        };
        assert_eq!(
            display_command(&command.to_string()),
            "pi --extension /user/first.ts --extension /user/last.ts"
        );
        let similar = format!("pi --extension {extension}.backup");
        assert_eq!(display_command(&similar), similar);
        let user_args = yinz_types::input::command::RunCommand {
            command: "pi".into(),
            args: vec![
                "--extension".into(),
                "/user/first.ts".into(),
                format!("explain --extension {extension}"),
            ],
            ..Default::default()
        };
        assert_eq!(
            display_command(&user_args.to_string()),
            format!("pi --extension /user/first.ts explain --extension {extension}")
        );
    }
    let mut args = vec![
        "--extension".into(),
        "/user/first.ts".into(),
        "--extension".into(),
        EXTENSION_ARGUMENT.into(),
    ];
    pi::strip_extension(&mut args);
    assert_eq!(args, ["--extension", "/user/first.ts"]);
}

#[test]
fn pi_archive_resumes_use_the_host_extension_path_and_remote_installations() {
    use crate::agent::pi;
    for host in [None, Some("remote-host")] {
        let mut command = super::resume_command("pi", "archived", Some("/work"), host).unwrap();
        assert!(command
            .args
            .windows(2)
            .any(|args| args == ["--session", "archived"]));
        assert!(command.args.iter().any(|arg| arg == pi::EXTENSION_ARGUMENT));
        if host.is_some() {
            pi::prepare_command(&mut command).unwrap();
            assert!(!command.args.iter().any(|arg| arg == pi::EXTENSION_ARGUMENT));
        }
    }
}
