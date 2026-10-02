use super::*;

#[test]
fn provider_hook_files_are_quoted_owned_and_reversible() {
    let temp = tempfile::tempdir().unwrap();
    let hook = AgentHookArtifact {
        path: "fixture/hooks.ts".into(), marker: "owned-fixture".into(),
        content: "// owned-fixture\nconst binary = __YINZ_BIN__; const token = __TOKEN_ENV__;\n__YINZ_SHELL__".into(),
        ..Default::default()
    };
    let path = path(&hook, temp.path(), temp.path());
    let rendered = render(&hook, Path::new("/a b/it's-yinz"), "PANE_TOKEN").unwrap();
    assert!(rendered.contains("\"/a b/it's-yinz\""));
    assert!(rendered.contains("'/a b/it'\\''s-yinz'"));
    install(&path, &hook, &rendered).unwrap();
    install(&path, &hook, &rendered).unwrap();
    assert_eq!(read_owned(&path, &hook.marker).unwrap(), Some(rendered));
    uninstall(&path, &hook).unwrap();
    std::fs::write(&path, "somebody else's hook").unwrap();
    assert!(install(&path, &hook, "replacement").is_err());
    assert!(uninstall(&path, &hook).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "somebody else's hook"
    );
}
