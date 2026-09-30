use super::{plan, Spec};

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
