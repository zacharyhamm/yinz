use super::*;

#[test]
fn checks_compile_without_running_the_plugin() {
    let path = Path::new("example.scm");
    let report = check(
        path,
        r#"(display "checking must not execute this")
           (yinz-command "GetPluginIds")
           (error "checking must not execute this either")
           (define (load configuration) (void))"#,
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert!(report.formatted_source.is_none());
    for source in ["(define (", "(not-a-yinz-function)"] {
        let report = check(path, source, PluginCheckMode::Syntax, None);
        assert!(!report.diagnostics.is_empty(), "{report:?}");
        assert!(report.diagnostics[0].contains("example.scm"));
    }
}

#[test]
fn prepared_source_preserves_comments_and_string_contents() {
    let source = "; keep this comment\n(define text \"a  b\\n\")\n\n";
    for mode in [PluginCheckMode::Format, PluginCheckMode::Prepare] {
        let report = check(Path::new("example.scm"), source, mode, None);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(
            report.formatted_source.unwrap(),
            source.trim_end().to_owned() + "\n"
        );
    }
}

#[test]
fn formatting_a_module_only_parses_the_named_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("module.scm");
    for import in ["./missing.scm", "../outside.scm"] {
        let source = format!("(require \"{import}\")\n(error \"must not execute\")\n");
        let report = check(&path, &source, PluginCheckMode::Format, None);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert!(report.formatted_source.is_some());
        for mode in [PluginCheckMode::Semantic, PluginCheckMode::Prepare] {
            assert!(!check(&path, &source, mode, None).diagnostics.is_empty());
        }
    }
    let report = check(&path, "(define (", PluginCheckMode::Format, None);
    assert!(!report.diagnostics.is_empty());
    assert!(report.diagnostics[0].contains("module.scm"));
}

#[test]
fn source_analysis_infers_literal_grants_and_reports_data_mistakes() {
    let report = check(
        Path::new("example.scm"),
        r#"
        (define wanted '("RunCommands"))
        (define (load config)
          (yinz-command (hash "RequestPluginPermissions" '("ReadFiles" "ReadFile")))
          (yinz-command (hash "RequestPluginPermissions" wanted))
          (yinz-command (hash "DataDefineEntity"
            (hash "namespace" "shared:notes" "allow_destructive" #f
                  "def" (hash "name" "note" "attrs" (list (hash "name" "id" "kind" "text")))))))
    "#,
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.permissions, [PermissionType::ReadFiles]);
    assert!(report
        .outline
        .iter()
        .any(|entry| entry.text == "(load config)"));
    for finding in [
        "did you mean ReadFiles",
        "computed permissions",
        "requires a request for PluginData",
        "wait for PermissionRequestResult",
        "SharedPluginData",
        "reserved attribute",
    ] {
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains(finding)),
            "missing {finding}: {report:?}"
        );
    }
}

#[test]
fn checks_follow_confined_imports_without_evaluating_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.scm");
    let dependency = dir.path().join("value.scm");
    std::fs::write(
        &dependency,
        "(provide value) (define value (error \"must not run\"))",
    )
    .unwrap();
    let source = "(require \"./value.scm\") (define (render rows columns) value)";
    let report = check(&path, source, PluginCheckMode::Semantic, None);
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert!(report
        .outline
        .iter()
        .any(|entry| entry.kind == OutlineKind::Require));

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("value.scm"),
        "(provide value) (define value 1)",
    )
    .unwrap();
    std::fs::remove_file(&dependency).unwrap();
    std::os::unix::fs::symlink(outside.path().join("value.scm"), &dependency).unwrap();
    let report = check(&path, source, PluginCheckMode::Semantic, None);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.contains("escapes")),
        "{report:?}"
    );
}

#[test]
fn repository_checks_allow_sibling_modules_only_inside_the_repository() {
    let repository = tempfile::tempdir().unwrap();
    std::fs::create_dir(repository.path().join("plugin")).unwrap();
    std::fs::write(
        repository.path().join("value.scm"),
        "(provide value) (define value \"hello\")",
    )
    .unwrap();
    let path = repository.path().join("plugin/main.scm");
    let source = "(require \"../value.scm\") (define (render rows columns) value)";
    assert!(!check(&path, source, PluginCheckMode::Semantic, None)
        .diagnostics
        .is_empty());
    let report = check(
        &path,
        source,
        PluginCheckMode::Semantic,
        Some(repository.path()),
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
}
