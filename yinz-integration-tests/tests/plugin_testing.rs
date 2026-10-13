use std::{path::Path, sync::atomic::AtomicBool};
use yinz_types::authoring::{PluginTestKind, PluginTestOptions};

#[test]
fn installed_builtin_typescript_integration_suites() {
    for name in yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES {
        let report = yinz_server::plugin_testing::run_named(
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
    for (body, failure) in [
        ("throw Error('intentional failure')", "intentional failure"),
        ("while(true){}", "deadline"),
        (
            "console.log('x'.repeat(65536)); throw Error('large output failure')",
            "large output failure",
        ),
    ] {
        std::fs::write(&path,format!("import {{test,session}} from 'yinz:testing'; test('cleanup',()=>{{session.start(); {body};}});")).unwrap();
        let report = yinz_server::plugin_testing::run(
            &path,
            &PluginTestOptions {
                // Discovery and server startup must finish under parallel suite load.
                timeout_ms: 30_000,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!report.passed(), "{report:#?}");
        assert_eq!(report.tests.len(), 1, "{report:#?}");
        assert_eq!(
            report.tests[0].name, "cleanup",
            "case execution must follow discovery: {report:#?}"
        );
        assert!(
            report.tests[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains(failure)),
            "expected case failure {failure}: {report:#?}"
        );
        assert!(
            report.tests[0].output.contains("owned server pid: "),
            "{report:#?}"
        );
        assert!(
            report.tests[0].output.len() <= 32 * 1024,
            "output stays bounded: {report:#?}"
        );
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
fn installed_integration_discovery_failure_has_no_owned_server() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("discovery.integration.test.ts");
    std::fs::write(&path, "throw Error('controlled discovery failure');").unwrap();
    let report = yinz_server::plugin_testing::run(
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.tests.len(), 1, "{report:#?}");
    let test = &report.tests[0];
    assert_eq!(
        Path::new(&test.name),
        path.canonicalize().unwrap(),
        "{report:#?}"
    );
    assert!(
        test.error
            .as_deref()
            .is_some_and(|error| error.contains("controlled discovery failure")),
        "{report:#?}"
    );
    assert!(
        !test.output.contains("owned server pid:"),
        "discovery must not claim to own a server: {report:#?}"
    );
}

#[test]
fn installed_controller_routes_input_enforces_permissions_and_isolates_storage() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("echo 日本 #?%23%2523.ts"), r#"
import z, {definePlugin} from 'yinz:plugin';
let text='ready';
export default definePlugin({
  async load() {
    await z.waitForPermissions(['ReadCliPipes','PluginData']);
    z.subscribe(['Key']);
    z.data.define('fixture', {value:{kind:'Text',required:true}}, {version:1});
    z.data.apply([z.data.put('fixture',{value:import.meta.url},{uid:'url'})]);
    try { z.command('RunCommand', [['/usr/bin/true'],{},'',{},64,null]); throw Error('permission bypass'); }
    catch (error) { if (!String(error).includes('permission')) throw error; z.log('permission denied as expected'); }
    z.requestRender();
  },
  update(event) {if (typeof event === 'object' && 'Key' in event && typeof event.Key.bare_key === 'object') {text += event.Key.bare_key.Char; return true;}},
  pipe(message) { if (typeof message.source === 'object' && 'Cli' in message.source) {
    z.command('CliPipeOutput',[message.source.Cli, JSON.stringify({echo:message.payload,url:import.meta.url})]);
    z.command('UnblockCliPipeInput',message.source.Cli);
  }},
  render(rows,cols) {return text + ' size=' + rows + 'x' + cols;}
});
"#).unwrap();
    let path = root.path().join("echo.integration.test.ts");
    std::fs::write(&path, r#"
import {test,session,assert,assertEquals,assertThrows,assertRejects} from 'yinz:testing';
const target=import.meta.resolve('./echo%20%E6%97%A5%E6%9C%AC%20%23%3F%2523%252523.ts');
for (const name of ['first isolated case','second isolated case']) test(name, {grants:['ReadCliPipes','PluginData']}, async () => {
  assertThrows(()=>session.fixture({}, {forbidden:'exit 0'}),'RunCommands');
  session.start();
  await assertRejects(()=>session.waitFor('bounded frozen clock',()=>false,20),'timed out');
  assertEquals(Date.now(),0);
  await session.waitForText('yinz-test>');
  assertEquals(session.catalog(), []);
  session.text("printf 'terminal-%s' 'input' "); session.key('Enter');
  await session.waitForText('terminal-input');
  session.loadPlugin(target); await session.waitForText('ready size=');
  session.text('xy'); await session.waitForText('readyxy');
  const before=session.screen.match(/size=\d+x\d+/)?.[0];
  session.resize(35,100); await session.waitFor('resize reaches plugin',()=>{
    const after=session.screen.match(/size=\d+x\d+/)?.[0]; return !!after && after !== before;
  });
  assertEquals(JSON.parse(session.pipe('echo','payload',{plugin:target})),{echo:'payload',url:target});
  const data=session.data(target,'fixture');
  assertEquals(data.rows.length,1);
  assertEquals(data.rows[0][data.columns.indexOf('value')],{Text:target});
  await session.waitFor('permission denial logged',()=>session.logs.includes('permission denied as expected'));
  assert(session.catalog().some(entity=>entity.def.name==='fixture'));
  session.reload(target); await session.waitForText('ready size=');
  assert(!session.screen.includes('readyxy'));
});
"#).unwrap();
    let report =
        yinz_server::plugin_testing::run(&path, &Default::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_async_plugin_api_returns_results_and_cancels_while_input_continues() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("hanging.test.ts"),
        "import {test} from 'yinz:testing'; test('loop',()=>{while(true){}});",
    )
    .unwrap();
    std::fs::write(root.path().join("caller.ts"), r#"
import z, {definePlugin, type CancellablePromise, type PluginTestReport} from 'yinz:plugin';
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
import {test,session,assert,assertEquals} from 'yinz:testing';
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
    let report =
        yinz_server::plugin_testing::run(&path, &Default::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(report.passed(), "{report:#?}");
}

fn assert_server_gone(output: &str) {
    let pid = output
        .split("owned server pid: ")
        .nth(1)
        .unwrap_or_else(|| panic!("owned server diagnostic missing from full output:\n{output}"))
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
import z, {definePlugin} from 'yinz:plugin';
export default definePlugin({async load(){await z.waitForPermissions(['RunCommands']); await z.runCommand({command:['crash-server']});}});
"#).unwrap();
    let path = root.path().join("crash.integration.test.ts");
    std::fs::write(
        &path,
        r#"
import {test,session} from 'yinz:testing';
test('server crash', {grants:['RunCommands']}, async () => {
  session.fixture({}, {'crash-server':'kill -KILL "$PPID"'});
  session.start(); session.loadPlugin('./crash.ts');
  await session.waitForText('never rendered');
});
"#,
    )
    .unwrap();
    let report = yinz_server::plugin_testing::run(
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
    assert_eq!(report.tests.len(), 1, "{report:#?}");
    assert_eq!(report.tests[0].name, "server crash", "{report:#?}");
    assert!(
        report.tests[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cleanup:")),
        "server death must be reported during cleanup: {report:#?}"
    );
    assert!(
        report.tests[0].error.as_ref().unwrap().contains("signal"),
        "{report:#?}"
    );
    assert_server_gone(&report.tests[0].output);
}

#[test]
fn installed_cli_runs_builtin_tests_without_a_configured_plugin_directory() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("yinz");
    std::fs::copy(yinz_server::plugin_testing::binary().unwrap(), &binary).unwrap();
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
    let report: yinz_types::authoring::PluginTestReport =
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
import z, {definePlugin} from 'yinz:plugin';
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
import {test,session,assert,assertEquals,assertRejects} from 'yinz:testing';
test('headless RPC uses authenticated callers and persists its result',{grants:['ReadCliPipes','MessageAndLaunchOtherPlugins','PluginData']},async()=>{
  session.start({backgroundPlugins:['./service.ts','yinz:swamp-bridge']});
  await session.waitFor('headless RPC ready',()=>session.logs.includes('RPC_SERVICE_READY'));
  const reply=await session.rpc('./service.ts','echo','hello') as {echo:string;source:number};
  assertEquals(reply.echo,'hello'); assert(typeof reply.source==='number');
  assertEquals(session.data('./service.ts','rpc_rows').rows.length,1);
  await assertRejects(()=>session.rpc('./service.ts','echo','fail'),'fixture RPC failure');
  await assertRejects(()=>session.rpc('yinz:swamp-bridge','swamp',{method:'lookup',work:'test',key:'test'}),'Only builtin workflow views');
});
"#).unwrap();
    let report =
        yinz_server::plugin_testing::run(&path, &Default::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_palette_uses_real_keyboard_discovery_and_execution() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("palette.ts"), r#"
import z, {definePlugin} from 'yinz:plugin';
export default definePlugin({async load(){
  await z.waitForPermissions(['ReadApplicationState']);
  z.commandPalette.registerProvider({id:'static-fixture',title:'Static fixture',actions:[{
    id:'static',label:'Unique static fixture action',aliases:['static-keyword'],run(){z.log('STATIC_EXECUTED');},
  }]});
  z.commandPalette.registerProvider({id:'function-fixture',title:'Function fixture',provideActions(request){
    return request.query.includes('dynamic-keyword') ? [{
      id:'dynamic',label:'Unique dynamic fixture action',run(){z.log('DYNAMIC_EXECUTED');},
    }] : [];
  }});
  z.log('PALETTE_REGISTERED');
}});
"#).unwrap();
    let path = root.path().join("palette.integration.test.ts");
    std::fs::write(
        &path,
        r#"
import {test,session,assert} from 'yinz:testing';
test('real palette input',{grants:['ReadApplicationState']},async()=>{
  session.start({backgroundPlugins:['./palette.ts']});
  await session.waitFor('palette registration',()=>session.logs.includes('PALETTE_REGISTERED'));
  for(const [query,label,marker] of [
    ['static-keyword','Unique static fixture action','STATIC_EXECUTED'],
    ['dynamic-keyword','Unique dynamic fixture action','DYNAMIC_EXECUTED'],
  ]) {
    session.key('Ctrl Alt p'); session.text(query);
    await session.waitForText(label);
    assert(!session.logs.includes(marker));
    session.key('Enter');
    await session.waitFor('palette action executed',()=>session.logs.includes(marker));
  }
});
"#,
    )
    .unwrap();
    let report =
        yinz_server::plugin_testing::run(&path, &Default::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_headless_lifecycle_exercises_the_permission_modal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("background.ts"),
        r#"
import z from 'yinz:plugin';
export default {async load() {
  await z.waitForPermissions(['ReadApplicationState']);
  z.log('HEADLESS_READY');
}};
"#,
    )
    .unwrap();
    let path = root.path().join("background.integration.test.ts");
    std::fs::write(&path, r#"
import {test,session,assert,assertEquals,assertThrows} from 'yinz:testing';
test('headless start and stop',{grants:['ReadApplicationState']},async()=>{
  session.start({autoGrantPermissions:false});
  await session.waitForText('yinz-test>');
  await session.waitFor('focused terminal metadata',()=>session.panes.some(p=>!p.is_plugin && p.is_focused));
  const terminal=session.panes.find(p=>!p.is_plugin && p.is_focused);
  assert(terminal);
  const first=session.loadPlugin('./background.ts',{headless:true,configuration:{shared:'true'}});
  assert(first);
  const id=first.plugins[0].id;
  assertEquals(session.loadPlugin('./background.ts',{headless:true,configuration:{shared:'true'}})?.plugins[0].id,id);
  await session.waitForText('Plugin permissions');
  assertEquals(session.status('./background.ts').plugins[0].state,'awaiting_permission');
  session.key('Tab'); session.key('Enter');
  await session.waitFor('permission promise settles',()=>session.logs.includes('HEADLESS_READY'));
  assertEquals(session.status('./background.ts').plugins[0].state,'running');
  assertEquals(session.reload(`plugin_${id}`).plugins[0].id,id);
  await session.waitFor('replacement available',()=>session.status(`plugin_${id}`).plugins[0].state==='running');
  assertEquals(session.panes.find(p=>!p.is_plugin && p.is_focused)?.id,terminal.id);
  assertEquals(session.panes.filter(p=>!p.is_plugin).length,1);
  assertEquals(session.stop(`plugin_${id}`).plugins[0].state,'stopped');
  assertEquals(session.status('./background.ts').plugins,[]);
  assertThrows(()=>session.reload('./background.ts'),'not loaded; use plugins start');
});
"#).unwrap();
    let report =
        yinz_server::plugin_testing::run(&path, &Default::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(report.passed(), "{report:#?}");
}
