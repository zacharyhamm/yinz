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
    for mode in [PluginCheckMode::Semantic, PluginCheckMode::Prepare] {
        let report = check(path, "(not-a-yinz-function)", mode, None);
        assert!(!report.diagnostics.is_empty(), "{report:?}");
        assert!(report.diagnostics[0].contains("example.scm"));
    }
}

#[test]
fn syntax_checks_only_parse_the_supplied_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing/example.scm");
    let source = "(require \"./missing.scm\")\n(not-a-yinz-function)";
    let report = check(&path, source, PluginCheckMode::Syntax, None);
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert!(report.formatted_source.is_none());
    assert!(report.outline.iter().any(|entry| {
        entry.kind == OutlineKind::Require
            && entry.line == 1
            && entry.text == "(require \"./missing.scm\")"
    }));

    let report = check(&path, "(define (", PluginCheckMode::Syntax, None);
    assert!(!report.diagnostics.is_empty(), "{report:?}");
    assert!(report.diagnostics[0].contains("example.scm"));
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
fn source_analysis_reports_cond_permissions_and_load_warnings() {
    let report = check(
        Path::new("example.scm"),
        r#"(define (load config)
  (cond
    [(yinz-wait-for-permissions '("PluginData"))
     (cond
       [(yinz-command "GetPaneId") =>
        (lambda (_) (yinz-command (hash "RequestPluginPermissions" '("ReadFiles"))))])]
    [else (yinz-command "DataFetch")]))"#,
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(
        report.permissions,
        [PermissionType::PluginData, PermissionType::ReadFiles]
    );
    assert_eq!(
        report
            .outline
            .iter()
            .filter(|entry| entry.kind == OutlineKind::Permission)
            .map(|entry| (entry.line, entry.text.as_str()))
            .collect::<Vec<_>>(),
        [(3, r#"["PluginData"]"#), (6, r#"["ReadFiles"]"#)]
    );
    assert_eq!(
        report.warnings,
        ["line 7: load must wait for PermissionRequestResult before accessing the data store"]
    );
    let report = check(
        Path::new("example.scm"),
        r#"(define (cond value) value)
(define (load config)
  (cond (yinz-command (hash "RequestPluginPermissions" '("PluginData"))))
  (cond (yinz-command "DataFetch")))"#,
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.permissions, [PermissionType::PluginData]);
    assert_eq!(
        report.warnings,
        ["line 4: load must wait for PermissionRequestResult before accessing the data store"]
    );
}

#[test]
fn source_analysis_warns_for_direct_data_access_in_both_load_forms() {
    for definition in ["(load config)", "load (lambda (config)"] {
        let close = if definition.starts_with('(') {
            ")"
        } else {
            "))"
        };
        let source = format!(
            r#"(define {definition}
  (yinz-wait-for-permissions '("PluginData"))
  (yinz-command "DataFetch"){close}"#
        );
        let report = check(
            Path::new("example.scm"),
            &source,
            PluginCheckMode::Semantic,
            None,
        );
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(report.permissions, [PermissionType::PluginData]);
        assert_eq!(report.warnings, ["line 3: load must wait for PermissionRequestResult before accessing the data store"]);
    }
}

#[test]
fn source_analysis_distinguishes_deferred_callbacks_from_eager_arguments() {
    for helper in ["yinz-then", "yinz-catch"] {
        for (value, warnings) in [
            (r#"(yinz-wait-for-permissions '("PluginData"))"#, vec![]),
            (
                r#"(begin (yinz-wait-for-permissions '("PluginData")) #t)"#,
                vec!["line 4: load must wait for PermissionRequestResult before accessing the data store"],
            ),
            (
                r#"(begin (yinz-command "DataFetch") (yinz-wait-for-permissions '("PluginData")))"#,
                vec!["line 2: load must wait for PermissionRequestResult before accessing the data store"],
            ),
        ] {
            let source = format!(
                r#"(define load (lambda (config)
  ({helper} {value}
    (lambda (_)
      (yinz-command (hash "DataCatalog" (hash "namespace" json-null)))))))"#
            );
            let report = check(
                Path::new("example.scm"),
                &source,
                PluginCheckMode::Semantic,
                None,
            );
            assert!(report.diagnostics.is_empty(), "{report:?}");
            assert_eq!(report.permissions, [PermissionType::PluginData]);
            assert_eq!(report.warnings, warnings, "{helper}: {value}");
        }
        let settled = format!(
            r#"(define (load config)
  (yinz-command (hash "RequestPluginPermissions" '("PluginData")))
  ({helper} #t
    (lambda (_)
      (yinz-command (hash "DataCatalog" (hash "namespace" json-null))))))"#
        );
        let report = check(
            Path::new("example.scm"),
            &settled,
            PluginCheckMode::Semantic,
            None,
        );
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(report.permissions, [PermissionType::PluginData]);
        assert_eq!(
            report.warnings,
            ["line 5: load must wait for PermissionRequestResult before accessing the data store"],
            "{helper}"
        );
    }
}

#[test]
fn source_analysis_warns_when_unrelated_permission_is_awaited() {
    for helper in ["yinz-then", "yinz-catch"] {
        let source = format!(
            r#"(define (load config)
  ({helper} (yinz-wait-for-permissions '("ReadFiles"))
    (lambda (_)
      (yinz-command (hash "RequestPluginPermissions" '("PluginData")))
      (yinz-command (hash "DataCatalog" (hash "namespace" json-null))))))"#
        );
        let report = check(
            Path::new("example.scm"),
            &source,
            PluginCheckMode::Semantic,
            None,
        );
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(
            report.permissions,
            [PermissionType::ReadFiles, PermissionType::PluginData]
        );
        assert_eq!(
            report.warnings,
            ["line 5: load must wait for PermissionRequestResult before accessing the data store"],
            "{helper}"
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

#[test]
fn checks_report_the_pane_declaration_and_headless_mistakes() {
    let path = Path::new("example.scm");
    let report = check(
        path,
        "(yinz-declare-pane)\n(define (render rows columns) \"\")\n(define (load configuration) (yinz-command \"HideSelf\"))",
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.pane.as_deref(), Some("always"));
    assert!(report.warnings.is_empty(), "{report:?}");

    let report = check(
        path,
        "(yinz-declare-pane-unless \"duplicate_focused\" \"true\")\n(define (load configuration) (yinz-command \"SetSelectable\"))",
        PluginCheckMode::Syntax,
        None,
    );
    assert_eq!(
        report.pane.as_deref(),
        Some("unless duplicate_focused=true")
    );
    assert!(report.warnings.is_empty(), "{report:?}");

    let report = check(
        path,
        "(define (render rows columns) \"\")\n(define (load configuration)\n  (yinz-command \"HideSelf\")\n  (yinz-command (hash \"SetSelectable\" #f))\n  (yinz-declare-pane))",
        PluginCheckMode::Semantic,
        None,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.pane.as_deref(), Some("none"));
    for finding in [
        "render is never called",
        "line 3: 'HideSelf' needs the plugin's own pane",
        "line 4: 'SetSelectable' needs the plugin's own pane",
        "line 5: yinz-declare-pane must be a top-level form",
    ] {
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains(finding)),
            "missing {finding}: {report:?}"
        );
    }

    let report = check(
        path,
        "(yinz-declare-pane)\n(yinz-declare-pane)",
        PluginCheckMode::Syntax,
        None,
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.contains("declared more than once")),
        "{report:?}"
    );
}

#[test]
fn pane_warnings_name_each_extra_declaration_and_spare_correct_plugins() {
    let path = Path::new("example.scm");
    let report = check(
        path,
        "(yinz-declare-pane)\n(define (load configuration)\n  (yinz-declare-pane))",
        PluginCheckMode::Syntax,
        None,
    );
    assert_eq!(report.pane.as_deref(), Some("always"));
    assert_eq!(
        report.warnings,
        ["line 3: yinz-declare-pane must be a top-level form of the entry file; this call fails the load"],
        "a declared plugin still fails on a second, nested call: {report:?}"
    );

    for (source, pane) in [
        (
            "(yinz-declare-pane-unless \"mode\" \"quiet\")\n(define (render rows columns) \"\")",
            "unless mode=quiet",
        ),
        (
            "(define (load configuration) (yinz-command \"GetPluginIds\"))",
            "none",
        ),
        (
            "(define (load configuration) (define (render r c) \"\") (yinz-command \"Notify\"))",
            "none",
        ),
    ] {
        let report = check(path, source, PluginCheckMode::Syntax, None);
        assert!(report.diagnostics.is_empty(), "{source}: {report:?}");
        assert_eq!(report.pane.as_deref(), Some(pane), "{source}");
        assert!(report.warnings.is_empty(), "{source}: {report:?}");
    }
}

/// Formatting needs only a parse. A malformed pane declaration does not stop it; the report
/// carries the mistake as a warning beside the formatted source. Every other mode still fails.
#[test]
fn formatting_reports_a_malformed_declaration_and_still_formats() {
    let path = Path::new("example.scm");
    for (source, mistake) in [
        (
            "(yinz-declare-pane)\n(yinz-declare-pane)\n",
            "line 2: the pane is declared more than once",
        ),
        ("(yinz-declare-pane #t)\n", "takes no arguments"),
        (
            "(yinz-declare-pane-unless \"k\" 'v)\n",
            "takes two string literals",
        ),
    ] {
        let report = check(path, &format!("  {source}"), PluginCheckMode::Format, None);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(
            report.formatted_source.as_deref(),
            Some(source),
            "{report:?}"
        );
        assert!(report.pane.is_none(), "{report:?}");
        assert!(
            report.warnings[0].contains("reading the pane declaration of example.scm")
                && report.warnings[0].contains(mistake),
            "{report:?}"
        );
        for mode in [
            PluginCheckMode::Syntax,
            PluginCheckMode::Semantic,
            PluginCheckMode::Prepare,
        ] {
            let report = check(path, source, mode, None);
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|error| error.contains(mistake)),
                "{mode:?}: {report:?}"
            );
        }
    }
}

/// `plugins check` warns about each command that needs the plugin's own pane whatever its
/// arguments, with the words the host refuses it with.
#[test]
fn headless_warnings_cover_every_unconditional_own_pane_command() {
    let calls: String = yinz_types::own_pane::UNCONDITIONAL_OWN_PANE_COMMANDS
        .iter()
        .map(|name| format!("  (yinz-command \"{name}\")\n"))
        .collect();
    let report = check(
        Path::new("example.scm"),
        &format!("(define (load configuration)\n{calls})"),
        PluginCheckMode::Syntax,
        None,
    );
    for (line, name) in yinz_types::own_pane::UNCONDITIONAL_OWN_PANE_COMMANDS
        .iter()
        .enumerate()
    {
        let expected = format!(
            "line {}: {}",
            line + 2,
            yinz_types::own_pane::own_pane_refusal(name, "the plugin")
        );
        assert!(
            report.warnings.contains(&expected),
            "{expected}: {report:?}"
        );
    }
}
