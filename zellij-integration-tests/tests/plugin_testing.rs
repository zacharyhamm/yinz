use std::{path::Path, sync::atomic::AtomicBool};
use zellij_types::authoring::{PluginTestKind, PluginTestOptions};

#[test]
fn installed_builtin_typescript_integration_suites() {
    for name in zellij_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES {
        let report = zellij_server::plugin_testing::run_named(
            Path::new("/unused"),
            name,
            &PluginTestOptions {
                kind: PluginTestKind::Integration,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(report.passed(), "{name}: {report:#?}");
    }
}

#[test]
fn installed_integration_failure_and_timeout_reap_the_owned_session() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cleanup.integration.test.ts");
    for body in ["throw Error('intentional failure')", "while(true){}"] {
        std::fs::write(&path,format!("import {{test,session}} from 'zellij:testing'; test('cleanup',()=>{{session.start(); {body};}});")).unwrap();
        let report = zellij_server::plugin_testing::run(
            &path,
            &PluginTestOptions {
                timeout_ms: 3000,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!report.passed(), "{report:#?}");
        assert_server_gone(&report.tests[0].output);
        // The runner's result follows shutdown and waitpid. A teardown failure is reported
        // separately and cannot turn the original failure into a successful cleanup.
        assert!(
            !report.tests[0].error.as_ref().unwrap().contains("cleanup:"),
            "{report:#?}"
        );
    }
}

#[test]
fn installed_controller_routes_input_enforces_permissions_and_isolates_storage() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("echo.ts"), r#"
import z, {definePlugin} from 'zellij:plugin';
let text='ready';
export default definePlugin({
  async load() {
    await z.waitForPermissions(['ReadCliPipes','PluginData']);
    z.subscribe(['Key']);
    z.data.define('fixture', {value:{kind:'Text',required:true}}, {version:1});
    try { z.command('RunCommand', [['/usr/bin/true'],{},'',{},64,null]); throw Error('permission bypass'); }
    catch (error) { if (!String(error).includes('permission')) throw error; z.log('permission denied as expected'); }
    z.requestRender();
  },
  update(event) {if (typeof event === 'object' && 'Key' in event && typeof event.Key.bare_key === 'object') {text += event.Key.bare_key.Char; return true;}},
  pipe(message) { if (typeof message.source === 'object' && 'Cli' in message.source) {
    z.command('CliPipeOutput',[message.source.Cli, JSON.stringify({echo:message.payload})]);
    z.command('UnblockCliPipeInput',message.source.Cli);
  }},
  render(rows,cols) {return text + ' size=' + rows + 'x' + cols;}
});
"#).unwrap();
    let path = root.path().join("echo.integration.test.ts");
    std::fs::write(&path, r#"
import {test,session,assert,assertEquals} from 'zellij:testing';
for (const name of ['first isolated case','second isolated case']) test(name, {grants:['ReadCliPipes','PluginData']}, async () => {
  session.start();
  await session.waitForText('zellij-test>');
  assertEquals(session.catalog(), []);
  session.text("printf 'terminal-%s' 'input' "); session.key('Enter');
  await session.waitForText('terminal-input');
  session.loadPlugin('./echo.ts'); await session.waitForText('ready size=');
  session.text('xy'); await session.waitForText('readyxy');
  const before=session.screen.match(/size=\d+x\d+/)?.[0];
  session.resize(35,100); await session.waitFor('resize reaches plugin',()=>{
    const after=session.screen.match(/size=\d+x\d+/)?.[0]; return !!after && after !== before;
  });
  assertEquals(JSON.parse(session.pipe('echo','payload',{plugin:'./echo.ts'})),{echo:'payload'});
  await session.waitFor('permission denial logged',()=>session.logs.includes('permission denied as expected'));
  assert(session.catalog().some(entity=>entity.def.name==='fixture'));
  session.reload('./echo.ts'); await session.waitForText('ready size=');
  assert(!session.screen.includes('readyxy'));
});
"#).unwrap();
    let report = zellij_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_async_plugin_api_returns_results_and_cancels_while_input_continues() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("hanging.test.ts"),
        "import {test} from 'zellij:testing'; test('loop',()=>{while(true){}});",
    )
    .unwrap();
    std::fs::write(root.path().join("caller.ts"), r#"
import z, {definePlugin, type CancellablePromise, type PluginTestReport} from 'zellij:plugin';
let status='starting', pending: CancellablePromise<PluginTestReport> | undefined;
export default definePlugin({
  async load() {
    await z.waitForPermissions(['AuthorPlugins']); z.subscribe(['Key']);
    const report=await z.testPlugin('agent-policy',{kind:'unit',filter:'session'});
    z.log('API_REPORT '+JSON.stringify(report));
    pending=z.testPlugin('hanging.test.ts',{kind:'unit'});
    status='test running; press c to cancel'; z.requestRender();
    try {await pending; status='unexpected completion';} catch(error){status='cancelled: '+String(error);}
    z.requestRender();
  },
  update(event) {if(typeof event==='object' && 'Key' in event) {pending?.cancel(); return true;}},
  render(){return status;}
});
"#).unwrap();
    let path = root.path().join("api.integration.test.ts");
    std::fs::write(
        &path,
        r#"
import {test,session,assert,assertEquals} from 'zellij:testing';
test('async API agrees and stays responsive', {grants:['AuthorPlugins']}, async () => {
  session.start(); session.loadPlugin('./caller.ts');
  await session.waitForText('test running; press c');
  const line=session.logs.split('API_REPORT ').at(-1)!.split('\n')[0].trim();
  const report=JSON.parse(line);
  assertEquals(report.tests.length,1);
  assertEquals(report.tests[0].kind,'unit'); assertEquals(report.tests[0].status,'passed');
  assert(report.tests[0].name.includes('session'));
  session.key('c'); await session.waitForText('cancelled:');
});
"#,
    )
    .unwrap();
    let report = zellij_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

fn assert_server_gone(output: &str) {
    let pid = output
        .split("owned server pid: ")
        .nth(1)
        .expect("owned server diagnostic")
        .lines()
        .next()
        .unwrap();
    assert!(
        !std::process::Command::new("/bin/sh")
            .args(["-c", "kill -0 \"$1\" 2>/dev/null", "test-server", pid])
            .status()
            .unwrap()
            .success(),
        "owned server {pid} still exists"
    );
}

#[test]
fn installed_integration_server_crash_is_a_failure_and_is_reaped() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("crash.ts"), r#"
import z, {definePlugin} from 'zellij:plugin';
export default definePlugin({async load(){await z.waitForPermissions(['RunCommands']); await z.runCommand({command:['crash-server']});}});
"#).unwrap();
    let path = root.path().join("crash.integration.test.ts");
    std::fs::write(
        &path,
        r#"
import {test,session} from 'zellij:testing';
test('server crash', {grants:['RunCommands']}, async () => {
  session.fixture({}, {'crash-server':'kill -KILL "$PPID"'});
  session.start(); session.loadPlugin('./crash.ts');
  await session.waitForText('never rendered');
});
"#,
    )
    .unwrap();
    let report = zellij_server::plugin_testing::run(
        &path,
        &PluginTestOptions {
            timeout_ms: 5000,
            ..Default::default()
        },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed(), "{report:#?}");
    assert!(
        report.tests[0].error.as_ref().unwrap().contains("signal"),
        "{report:#?}"
    );
    assert_server_gone(&report.tests[0].output);
}

#[test]
fn installed_cli_runs_builtin_tests_without_a_configured_plugin_directory() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("zellij");
    std::fs::copy(zellij_server::plugin_testing::binary().unwrap(), &binary).unwrap();
    let result = std::process::Command::new(binary)
        .args([
            "plugins",
            "test",
            "agent-policy",
            "--kind",
            "unit",
            "--json",
        ])
        .current_dir(root.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("TMPDIR", root.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: zellij_types::authoring::PluginTestReport =
        serde_json::from_slice(&result.stdout).unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 2);
}

#[test]
fn installed_session_rpc_uses_real_headless_routing_storage_and_caller_identity() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("service.ts"),
        r#"
import z, {definePlugin} from 'zellij:plugin';
export default definePlugin({async load(){
  await z.waitForPermissions(['PluginData','MessageAndLaunchOtherPlugins']);
  z.data.define('rpc_rows',{value:{kind:'Text',required:true}},{version:1});
  z.rpc.handle('echo',(payload,call)=>{
    if(payload==='fail')throw Error('fixture RPC failure');
    z.data.apply([z.data.put('rpc_rows',{value:String(payload)},{uid:'last'})]);
    return {echo:payload,source:call.source?.Plugin ?? null};
  });
  z.log('RPC_SERVICE_READY');
}});
"#,
    )
    .unwrap();
    let path = root.path().join("rpc.integration.test.ts");
    std::fs::write(&path,r#"
import {test,session,assert,assertEquals,assertRejects} from 'zellij:testing';
test('headless RPC uses authenticated callers and persists its result',{grants:['ReadCliPipes','MessageAndLaunchOtherPlugins','PluginData']},async()=>{
  session.start({backgroundPlugins:['./service.ts','zellij:swamp-bridge']});
  await session.waitFor('headless RPC ready',()=>session.logs.includes('RPC_SERVICE_READY'));
  const reply=await session.rpc('./service.ts','echo','hello') as {echo:string;source:number};
  assertEquals(reply.echo,'hello'); assert(typeof reply.source==='number');
  assertEquals(session.data('./service.ts','rpc_rows').rows.length,1);
  await assertRejects(()=>session.rpc('./service.ts','echo','fail'),'fixture RPC failure');
  await assertRejects(()=>session.rpc('zellij:swamp-bridge','swamp',{method:'lookup',work:'test',key:'test'}),'Only builtin workflow views');
});
"#).unwrap();
    let report = zellij_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}
