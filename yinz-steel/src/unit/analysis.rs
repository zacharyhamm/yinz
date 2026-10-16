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
