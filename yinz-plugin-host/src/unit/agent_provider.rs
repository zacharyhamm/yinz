use super::*;

#[test]
fn inspection_reads_selected_installed_source_and_imports_again() {
    let data = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("agent-opencode.scm"),
        "(require \"./provider.scm\")",
    )
    .unwrap();
    for kind in ["first", "second"] {
        std::fs::write(root.join("provider.scm"), format!("(provide agent-provider) (define agent-provider (hash \"kind\" \"{kind}\" \"executable\" \"/usr/bin/true\"))")).unwrap();
        let provider = inspect_agent_provider("yinz:agent-opencode", data.path()).unwrap();
        assert_eq!(provider.kind, kind);
    }
    assert!(inspect_agent_provider("yinz:agent-opencode", other.path()).is_err());
    assert!(inspect_agent_provider("yinz:../agent-opencode", data.path()).is_err());
}

#[test]
fn named_tests_use_the_selected_installed_suite_and_read_edits() {
    let data = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("agent-opencode.scm"), "; entry").unwrap();
    std::fs::write(
        user.path().join("agent-opencode.test.scm"),
        "invalid user suite",
    )
    .unwrap();
    for name in ["first", "second"] {
        std::fs::write(
            root.join("agent-opencode.test.scm"),
            format!("(test \"{name}\" (lambda () (assert #t)))"),
        )
        .unwrap();
        let report = crate::testing::run_named(
            data.path(),
            user.path(),
            "agent-opencode",
            &Default::default(),
            None,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(report.passed(), "{report:#?}");
        assert_eq!(report.tests.len(), 1);
        assert_eq!(report.tests[0].name, name);
    }
    assert!(crate::testing::run_named(
        user.path(),
        user.path(),
        "agent-opencode",
        &Default::default(),
        None,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
}
