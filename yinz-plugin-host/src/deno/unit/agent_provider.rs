use super::*;

#[test]
fn builtin_provider_metadata_comes_from_typescript() {
    let definition = inspect_agent_provider("yinz:agent-opencode").unwrap();
    assert_eq!(definition.kind, "opencode");
    assert_eq!(definition.launch.prompt_args, ["--prompt", "{value}"]);
    assert!(definition.input.is_none());
    assert!(definition.hooks[0]
        .content
        .contains("JSON.stringify(toHookEvent(payload))"));
    assert_eq!(definition.hooks[0].path, "opencode/plugin/hocus.js");
}

#[test]
fn provider_inspection_executes_typescript_without_host_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("provider.ts");
    std::fs::write(&path, r#"
        import yinz, { definePlugin, type AgentProvider } from 'yinz:plugin';
        const names: string[] = ['fixture', 'agent'];
        export const agentProvider: AgentProvider = { kind: names.join('-'), executable: '/usr/bin/true' };
        export default definePlugin({ load() { throw new Error('load must never run'); } });
    "#).unwrap();
    assert_eq!(
        inspect_agent_provider(path.to_str().unwrap()).unwrap().kind,
        "fixture-agent"
    );
    std::fs::write(
        &path,
        r#"
        import yinz from 'yinz:plugin';
        yinz.host.runCommand({command:['/usr/bin/true'], envVariables:{}, cwd:'/', context:{}});
        export const agentProvider = {kind:'fixture', executable:'fixture'};
    "#,
    )
    .unwrap();
    let error = inspect_agent_provider(path.to_str().unwrap()).unwrap_err();
    assert!(
        format!("{error:#}").contains("module initialization"),
        "{error:#}"
    );
}
