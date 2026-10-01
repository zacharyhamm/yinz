use super::{display_command, plan, Spec};

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
