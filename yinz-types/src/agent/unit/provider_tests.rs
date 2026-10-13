use super::*;

fn fixture(kind: &str) -> AgentProvider {
    AgentProvider {
        kind: kind.into(),
        executable: format!("/opt/bin/{kind}"),
        launch: AgentLaunch {
            prompt_args: vec!["--prompt".into(), "{value}".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn plugin_ownership_reload_detection_and_unload_preserve_snapshots() {
    let owner = 802_401;
    let mut descriptor = fixture("provider-test-owned");
    descriptor.aliases = vec!["provider-test-alias".into()];
    register(owner, descriptor.clone()).unwrap();
    let old = provider("provider-test-alias").unwrap();
    assert_eq!(
        super::super::kind_of("/opt/bin/provider-test-alias.js").as_deref(),
        Some("provider-test-owned")
    );
    assert_eq!(
        super::super::kind_from_program_and_args(
            Path::new("node"),
            &["/opt/bin/provider-test-alias.js".into()]
        )
        .as_deref(),
        Some("provider-test-owned")
    );
    assert!(register(owner + 1, descriptor.clone()).is_err());
    let mut collision = fixture("provider-test-conflict");
    collision.aliases = descriptor.aliases.clone();
    assert!(register(owner + 1, collision).is_err());
    descriptor.executable = "/new/provider-test-owned".into();
    register(owner, descriptor).unwrap();
    assert_eq!(old.executable, "/opt/bin/provider-test-owned");
    assert_eq!(
        provider("provider-test-owned").unwrap().executable,
        "/new/provider-test-owned"
    );
    assert!(!unregister_owner(owner + 1));
    assert!(unregister_owner(owner));
    assert!(provider("provider-test-owned").is_none());
    assert!(super::super::kind_of("provider-test-alias").is_none());
}

#[test]
fn descriptors_reject_shadowing_unknown_fields_unsafe_artifacts_and_unbounded_input() {
    for name in ["", "Bad", "a/b", "x".repeat(33).as_str(), "codex"] {
        assert!(fixture(name).validate().is_err(), "{name}");
    }
    let mut descriptor = fixture("provider-test-validation");
    descriptor.executable = "/usr/bin/CODEX".into();
    assert!(descriptor.validate().is_err());
    for executable in ["/bin/sh", "/usr/bin/node", "env"] {
        descriptor.executable = executable.into();
        assert!(descriptor.validate().is_err());
    }
    descriptor.executable = "custom".into();
    descriptor.input = Some(AgentInput {
        submit_delay_ms: 10_001,
        ..Default::default()
    });
    assert!(descriptor.validate().is_err());
    descriptor.input = Some(AgentInput {
        newline: "\x1b".into(),
        ..Default::default()
    });
    assert!(descriptor.validate().is_err());
    descriptor.input = None;
    descriptor.hooks.push(AgentHookArtifact {
        path: "../escape.js".into(),
        marker: "owned".into(),
        content: "// owned".into(),
        ..Default::default()
    });
    assert!(descriptor.validate().is_err());
    descriptor.hooks[0].path = "agent/plugin.js".into();
    descriptor.validate().unwrap();
    descriptor.hooks[0].content.clear();
    assert!(descriptor.validate().is_err());
    assert!(serde_json::from_str::<AgentProvider>(
        r#"{"kind":"test","executable":"test","unknown":true}"#
    )
    .is_err());
}

#[test]
fn snapshot_validation_is_atomic_and_templates_preserve_argument_boundaries() {
    let owner = 802_402;
    let descriptor = fixture("provider-test-snapshot");
    register(owner, descriptor.clone()).unwrap();
    let mut invalid = descriptor.clone();
    invalid.input = Some(AgentInput {
        submit_key: "not a key".into(),
        ..Default::default()
    });
    assert!(replace_snapshot(vec![descriptor.clone(), invalid]).is_err());
    assert_eq!(provider(&descriptor.kind).unwrap().as_ref(), &descriptor);
    assert!(replace_snapshot(vec![descriptor.clone(), descriptor.clone()]).is_err());
    assert_eq!(
        expand_args(&descriptor.launch.prompt_args, "hello; $(echo nope)\nworld"),
        ["--prompt", "hello; $(echo nope)\nworld"]
    );
    unregister_owner(owner);
}

#[test]
fn registry_size_limit_rejects_insertions_and_replacements_atomically() {
    let mut registry = Registry::new();
    let mut descriptor = fixture("provider-size-0");
    descriptor.hooks.push(AgentHookArtifact {
        path: "plugin/hook.js".into(),
        marker: "owned".into(),
        content: format!("// owned\n{}", "x".repeat(1024 * 1024 - 2048)),
        ..Default::default()
    });
    for index in 0..8 {
        let mut next = descriptor.clone();
        next.kind = format!("provider-size-{index}");
        next.executable = next.kind.clone();
        next.validate().unwrap();
        insert(&mut registry, 9, next).unwrap();
    }
    let original = Arc::clone(&registry["provider-size-0"].1);
    let mut replacement = original.as_ref().clone();
    let mut extra_hook = replacement.hooks[0].clone();
    extra_hook.path = "plugin/other.js".into();
    replacement.hooks.push(extra_hook);
    replacement.validate().unwrap();
    assert!(insert(&mut registry, 9, replacement)
        .unwrap_err()
        .contains("8 MiB"));
    assert!(Arc::ptr_eq(&registry["provider-size-0"].1, &original));
    descriptor.kind = "provider-size-9".into();
    descriptor.executable = descriptor.kind.clone();
    assert!(insert(&mut registry, 9, descriptor).is_err());
    assert_eq!(registry.len(), 8);
}

#[test]
fn clear_key_defaults_and_overrides_are_validated() {
    let input: AgentInput = serde_json::from_str("{}").unwrap();
    assert_eq!(input.clear_key, "Ctrl c");
    let mut descriptor = fixture("provider-clear-test");
    descriptor.input = Some(input);
    descriptor.validate().unwrap();
    descriptor.input.as_mut().unwrap().clear_key = "Ctrl u".into();
    descriptor.validate().unwrap();
    descriptor.input.as_mut().unwrap().clear_key = "not a key".into();
    assert!(descriptor.validate().is_err());
}
