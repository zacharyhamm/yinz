use super::*;

#[test]
fn provider_metadata_does_not_run_load_or_grant_host_authority() {
    let provider = inspect(
        Path::new("provider.scm"),
        r#"
        (define agent-provider
          (hash "kind" (string-append "fixture" "-agent") "executable" "/usr/bin/true"))
        (define (load configuration) (error "load must never run"))
        "#,
    )
    .unwrap();
    assert_eq!(provider.kind, "fixture-agent");
    let error = inspect(
        Path::new("provider.scm"),
        r#"(yinz-command "GetPluginIds")"#,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("host commands are unavailable"));
    assert!(inspect(Path::new("provider.scm"), "(define agent-provider (hash))").is_err());
}

#[test]
fn builtin_provider_preserves_opencode_launch_and_hook_metadata() {
    let source = &yinz_steel_builtins::fixture_source("agent-opencode").unwrap();
    let provider = inspect(Path::new("agent-opencode.scm"), source).unwrap();
    assert_eq!(provider.kind, "opencode");
    assert_eq!(provider.launch.prompt_args, ["--prompt", "{value}"]);
    assert!(provider.input.is_none());
    assert_eq!(provider.hooks[0].path, "opencode/plugin/yinz.js");
    assert!(provider.hooks[0]
        .content
        .contains("JSON.stringify(toHookEvent(payload))"));
}
