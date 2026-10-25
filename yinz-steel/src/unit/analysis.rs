use super::*;
use serde_json::json;

#[test]
fn new_forms_preserve_conservative_command_inference() {
    for source in [
        "(define-values (yinz-command) (values ignored)) (yinz-command \"GetPluginIds\")",
        "(let-values ([(yinz-command) (values ignored)]) (yinz-command \"GetPluginIds\"))",
        "(do ([yinz-command ignored]) [#t] (yinz-command \"GetPluginIds\"))",
        "(struct yinz-command ()) (yinz-command \"GetPluginIds\")",
        "`(yinz-command \"GetPluginIds\")",
        "``(,(yinz-command \"GetPluginIds\"))",
        "`('(yinz-command \"GetPluginIds\"))",
        "``(',(yinz-command \"GetPluginIds\"))",
    ] {
        assert!(analyze(source).unwrap().commands.is_empty(), "{source}");
    }
    let report =
        analyze("`(data ,(yinz-command \"GetPluginIds\") ,@(list (yinz-command \"GetPaneId\")))")
            .unwrap();
    assert_eq!(
        report
            .commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        ["GetPluginIds", "GetPaneId"]
    );
    let report = analyze("`#(,(yinz-command \"GetPluginIds\"))").unwrap();
    assert_eq!(report.commands.len(), 1);
    let report = analyze("`(',(yinz-command \"GetPluginIds\"))").unwrap();
    assert_eq!(report.commands.len(), 1);
    assert_eq!(report.commands[0].name, "GetPluginIds");
    let report =
        analyze("`(',(set! yinz-command ignored)) (yinz-command \"GetPluginIds\")").unwrap();
    assert!(report.commands.is_empty());
    let report =
        analyze("`#(,(set! yinz-command ignored)) (yinz-command \"GetPluginIds\")").unwrap();
    assert!(report.commands.is_empty());
    let report = analyze(
        "(case value [((yinz-command \"NotExecutable\")) (yinz-command \"GetPluginIds\")])",
    )
    .unwrap();
    assert_eq!(report.commands.len(), 1);
    assert_eq!(report.commands[0].name, "GetPluginIds");
}

#[test]
fn analysis_only_collects_executable_unshadowed_literal_commands() {
    let report = analyze(r#"
; (yinz-command (hash "RequestPluginPermissions" '("RunCommands")))
'(yinz-command (hash "RequestPluginPermissions" '("RunCommands")))
(define text "(yinz-command ignored)")
(define (shadow yinz-command) (yinz-command (hash "RequestPluginPermissions" '("RunCommands"))))
(define (load configuration)
  (yinz-command (hash "RequestPluginPermissions" '("ReadFiles" "PluginData")))
  (let ([hash (lambda arguments arguments)]) (yinz-command (hash "RequestPluginPermissions" '("RunCommands")))))
(yinz-command (hash "RequestPluginPermissions" (list "WriteFiles")))
(yinz-command (hash "RequestPluginPermissions" computed))
"#).unwrap();
    assert_eq!(report.commands.len(), 3, "{report:?}");
    assert_eq!(
        report.commands[0].argument,
        Some(json!(["ReadFiles", "PluginData"]))
    );
    assert!(report.commands[0].during_load);
    assert_eq!(report.commands[0].line, 7);
    assert_eq!(report.commands[1].argument, Some(json!(["WriteFiles"])));
    assert!(!report.commands[1].during_load);
    assert!(report.commands[2].argument.is_none());
    assert!(report
        .definitions
        .iter()
        .any(|(_, name)| name == "(load configuration)"));
}

#[test]
fn redefined_or_imported_command_bindings_never_infer_grants() {
    for source in [
        "(define yinz-command (lambda (x) x))",
        "(set! yinz-command (lambda (x) x))",
        "(require \"./other.scm\")",
        "(define hash (lambda xs xs))",
    ] {
        let report = analyze(&format!(
            "{source}\n(yinz-command (hash \"RequestPluginPermissions\" '(\"RunCommands\")))"
        ))
        .unwrap();
        assert!(report.commands.is_empty(), "{report:?}");
    }
}

#[test]
fn permission_waits_report_literal_grants_but_respect_shadowing() {
    let report = analyze(
        r#"
        '(yinz-wait-for-permissions '("RunCommands"))
        (define (shadow yinz-wait-for-permissions)
          (yinz-wait-for-permissions '("RunCommands")))
        (define (load configuration)
          (yinz-wait-for-permissions '("ReadFiles"))
          (yinz-wait-for-permissions computed))
        "#,
    )
    .unwrap();
    assert_eq!(report.commands.len(), 2, "{report:?}");
    assert_eq!(report.commands[0].name, "RequestPluginPermissions");
    assert_eq!(report.commands[0].argument, Some(json!(["ReadFiles"])));
    assert!(report.commands[0].during_load);
    assert!(report.commands[1].argument.is_none());
    for source in [
        "(require \"./other.scm\")",
        "(define yinz-wait-for-permissions (lambda (x) x))",
        "(set! yinz-wait-for-permissions (lambda (x) x))",
        "(define yinz-command (lambda (x) x))",
    ] {
        let report = analyze(&format!(
            "{source}\n(yinz-wait-for-permissions '(\"RunCommands\"))"
        ))
        .unwrap();
        assert!(report.commands.is_empty(), "{report:?}");
    }
}

#[test]
fn cond_collects_predicates_bodies_and_permissions_once_at_their_source_lines() {
    let report = analyze(
        r#"(define (load config)
  (cond
    [(and (yinz-command "GetPluginIds")
          (yinz-wait-for-permissions '("ReadFiles")))
     (yinz-command (hash "RequestPluginPermissions" '("PluginData")))
     (cond
       [(yinz-command "DataFetch")]
       [else (yinz-command "DataCount")])]
    [(yinz-command "GetPaneId")
     => (lambda (pane)
          (yinz-command (hash "RequestPluginPermissions" '("WriteFiles")))
          (yinz-command "DataWatch"))]
    [else
     (yinz-command (hash "RequestPluginPermissions" '("SharedPluginData")))
     '(yinz-command "Quoted")
     (quote (yinz-wait-for-permissions '("RunCommands")))]))"#,
    )
    .unwrap();
    assert_eq!(
        report
            .commands
            .iter()
            .map(|command| (
                command.line,
                command.name.as_str(),
                command.argument.clone()
            ))
            .collect::<Vec<_>>(),
        [
            (3, "GetPluginIds", Some(Value::Null)),
            (4, "RequestPluginPermissions", Some(json!(["ReadFiles"]))),
            (5, "RequestPluginPermissions", Some(json!(["PluginData"]))),
            (7, "DataFetch", Some(Value::Null)),
            (8, "DataCount", Some(Value::Null)),
            (9, "GetPaneId", Some(Value::Null)),
            (11, "RequestPluginPermissions", Some(json!(["WriteFiles"]))),
            (12, "DataWatch", Some(Value::Null)),
            (
                14,
                "RequestPluginPermissions",
                Some(json!(["SharedPluginData"]))
            ),
        ]
    );
    assert!(report.commands.iter().all(|command| command.during_load));
}

#[test]
fn cond_preserves_clause_scope_and_excludes_shadowed_or_mutated_commands() {
    let report = analyze(
        r#"(cond
  [(yinz-command "GetPluginIds")
   (define yinz-command (lambda (value) value))
   (yinz-command "Shadowed")
   (yinz-wait-for-permissions '("RunCommands"))]
  [else (yinz-command "GetPaneId")])"#,
    )
    .unwrap();
    assert_eq!(
        report
            .commands
            .iter()
            .map(|command| (command.line, command.name.as_str()))
            .collect::<Vec<_>>(),
        [(2, "GetPluginIds"), (6, "GetPaneId")]
    );
    for source in [
        r#"(let ([yinz-command ignored]) (cond [(yinz-command "DataFetch")]))"#,
        r#"(cond [(begin (set! yinz-command ignored) #t) (yinz-command "DataFetch")])"#,
        r#"(let ([yinz-wait-for-permissions ignored]) (cond [(yinz-wait-for-permissions '("ReadFiles"))]))"#,
        r#"(cond [(begin (set! yinz-wait-for-permissions ignored) #t) (yinz-wait-for-permissions '("ReadFiles"))])"#,
        r#"'(cond [(yinz-command "GetPluginIds") (yinz-wait-for-permissions '("ReadFiles"))])"#,
    ] {
        assert!(analyze(source).unwrap().commands.is_empty(), "{source}");
    }
    let call = r#"(cond (yinz-command (hash "RequestPluginPermissions" '("PluginData"))))"#;
    for source in [
        format!("(define (cond value) value) {call}"),
        format!("(define cond (lambda (value) value)) {call}"),
        format!("(set! cond (lambda (value) value)) {call}"),
        format!("(define (load cond) {call})"),
        format!("(let ([cond (lambda (value) value)]) {call})"),
        format!("(cond [#t (define (cond value) value) {call}])"),
    ] {
        let report = analyze(&source).unwrap();
        assert_eq!(report.commands.len(), 1, "{source}: {report:?}");
        assert_eq!(report.commands[0].name, "RequestPluginPermissions");
        assert_eq!(report.commands[0].argument, Some(json!(["PluginData"])));
    }
}

#[test]
fn both_load_definition_forms_preserve_outline_and_parameter_shadowing() {
    for (source, outline) in [
        (
            r#"(define (load config) (yinz-command "DataFetch"))"#,
            "(load config)",
        ),
        (
            r#"(define load (lambda (config) (yinz-command "DataFetch")))"#,
            "load",
        ),
    ] {
        let report = analyze(source).unwrap();
        assert_eq!(report.commands.len(), 1, "{report:?}");
        assert!(report.commands[0].during_load, "{report:?}");
        assert_eq!(report.definitions, [(1, outline.to_owned())]);
        let shadowed = source.replace("config", "yinz-command");
        assert!(
            analyze(&shadowed).unwrap().commands.is_empty(),
            "{shadowed}"
        );
    }
}

#[test]
fn deferred_inline_callbacks_retain_analysis_without_load_context() {
    for helper in ["yinz-then", "yinz-catch"] {
        let report = analyze(&format!(
            r#"(define (load config)
  ({helper}
    (begin
      (yinz-command "DataFetch")
      (yinz-wait-for-permissions '("PluginData")))
    (lambda (reply)
      (yinz-command "DataCount")
      (yinz-wait-for-permissions '("ReadFiles"))
      '(yinz-command "Quoted")))
  ({helper}
    ({helper} (yinz-wait-for-permissions '("PluginData")) (lambda (_) #t))
    (lambda (_) (yinz-command "DataCatalog")))
  ({helper} (yinz-wait-for-permissions '("PluginData")) (lambda (yinz-command) (yinz-command "Shadowed"))))"#
        ))
        .unwrap();
        assert_eq!(
            report
                .commands
                .iter()
                .map(|command| (command.line, command.name.as_str(), command.during_load))
                .collect::<Vec<_>>(),
            [
                (4, "DataFetch", true),
                (5, "RequestPluginPermissions", true),
                (7, "DataCount", false),
                (8, "RequestPluginPermissions", false),
                (11, "RequestPluginPermissions", true),
                (12, "DataCatalog", false),
                (13, "RequestPluginPermissions", true),
            ],
            "{helper}"
        );
        assert_eq!(report.commands[1].argument, Some(json!(["PluginData"])));
        assert_eq!(report.commands[3].argument, Some(json!(["ReadFiles"])));
    }
}

#[test]
fn deferred_callbacks_require_literal_plugin_data_permission() {
    for helper in ["yinz-then", "yinz-catch"] {
        for (permissions, during_load) in [
            (r#"'("ReadFiles")"#, true),
            ("'()", true),
            ("config", true),
            (r#""PluginData""#, true),
            (r#"'("PluginData")"#, false),
            (r#"'("ReadFiles" "PluginData")"#, false),
            (r#"(list "ReadFiles" "PluginData")"#, false),
        ] {
            let wait = format!("(yinz-wait-for-permissions {permissions})");
            for value in [
                wait.clone(),
                format!("(begin #t {wait})"),
                format!("({helper} {wait} (lambda (_) #t))"),
            ] {
                let source = format!(
                    r#"(define (load config)
  ({helper} {value}
    (lambda (_)
      (yinz-command (hash "RequestPluginPermissions" '("PluginData")))
      (yinz-command "DataCatalog"))))"#
                );
                let report = analyze(&source).unwrap();
                let data_command = report
                    .commands
                    .iter()
                    .find(|command| command.name == "DataCatalog")
                    .unwrap();
                assert_eq!(
                    data_command.during_load, during_load,
                    "{source}: {report:?}"
                );
            }
        }
    }
}

#[test]
fn shadowed_or_mutated_deferred_helpers_keep_conservative_load_context() {
    for helper in ["yinz-then", "yinz-catch"] {
        let call = format!(
            r#"({helper} (yinz-wait-for-permissions '("PluginData")) (lambda (_) (yinz-command "DataFetch")))"#
        );
        for source in [
            format!(
                "(define ({helper} value callback) (callback value)) (define (load config) {call})"
            ),
            format!("(set! {helper} ignored) (define (load config) {call})"),
            format!("(define (load {helper}) {call})"),
            format!("(define (load config) (let ([{helper} ignored]) {call}))"),
            format!(
                "(define (yinz-wait-for-permissions permissions) permissions) (define (load config) {call})"
            ),
            format!("(set! yinz-wait-for-permissions ignored) (define (load config) {call})"),
        ] {
            let report = analyze(&source).unwrap();
            assert!(
                report
                    .commands
                    .iter()
                    .any(|command| command.name == "DataFetch"),
                "{source}: {report:?}"
            );
            assert!(
                report.commands.iter().all(|command| command.during_load),
                "{source}: {report:?}"
            );
        }
    }
}

#[test]
fn eager_arguments_and_callback_producers_keep_conservative_load_context() {
    for helper in ["yinz-then", "yinz-catch", "consume"] {
        for call in [
            format!(r#"({helper} #t (lambda (_) (yinz-command "DataCatalog")))"#),
            format!(r#"({helper} config (lambda (_) (yinz-command "DataCatalog")))"#),
            format!(
                r#"({helper} (yinz-command (hash "RequestPluginPermissions" '("PluginData"))) (lambda (_) (yinz-command "DataCatalog")))"#
            ),
            format!(
                r#"({helper} (begin (yinz-wait-for-permissions '("PluginData")) #t) (lambda (_) (yinz-command "DataCatalog")))"#
            ),
            format!(r#"({helper} (lambda (_) (yinz-command "DataFetch")) (lambda (_) (void)))"#),
            format!(
                r#"({helper} (yinz-wait-for-permissions '("PluginData")) (begin (yinz-command "DataFetch") (lambda (_) (yinz-command "DataCount"))))"#
            ),
            format!(
                r#"({helper} (yinz-wait-for-permissions '("PluginData")) (make-callback (yinz-command "DataFetch")))"#
            ),
        ] {
            let report = analyze(&format!("(define (load config) {call})")).unwrap();
            assert!(!report.commands.is_empty(), "{call}");
            assert!(
                report.commands.iter().all(|command| command.during_load),
                "{call}: {report:?}"
            );
        }
    }
}

#[test]
fn pane_declaration_calls_record_their_placement_and_respect_shadowing() {
    let report = analyze(
        r#"
(yinz-declare-pane)
'(yinz-declare-pane)
(when #t (yinz-declare-pane-unless "mode" "quiet"))
(define (load configuration)
  (yinz-declare-pane))
(define (shadow yinz-declare-pane) (yinz-declare-pane))
(define (render rows columns) "")
"#,
    )
    .unwrap();
    assert_eq!(
        report
            .pane_declarations
            .iter()
            .map(|call| (call.line, call.top_level))
            .collect::<Vec<_>>(),
        [(2, true), (4, false), (6, false)],
        "{report:?}"
    );
    assert!(report.defines_render);
    for source in [
        "(define yinz-declare-pane (lambda () 1)) (yinz-declare-pane)",
        "(set! yinz-declare-pane-unless (lambda (k v) 1)) (yinz-declare-pane-unless \"a\" \"b\")",
    ] {
        assert!(
            analyze(source).unwrap().pane_declarations.is_empty(),
            "{source}"
        );
    }
}

#[test]
fn only_a_top_level_render_binding_counts() {
    for (source, expected) in [
        ("(define (render rows columns) \"\")", true),
        ("(define render (lambda (rows columns) \"\"))", true),
        ("(begin (define (render rows columns) \"\"))", true),
        (
            "(define (load configuration) (define (render r c) \"\") 1)",
            false,
        ),
        ("'(define (render rows columns) \"\")", false),
        ("(define (load configuration) 1)", false),
    ] {
        assert_eq!(
            analyze(source).unwrap().defines_render,
            expected,
            "{source}"
        );
    }
}
