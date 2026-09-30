use super::{plan, Spec};

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
