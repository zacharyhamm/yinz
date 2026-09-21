#![cfg(unix)]

//! A mixed-runtime session: the real permission prompt, event delivery, reload and shutdown.

use std::fs;
use std::io::{Read, Write};
use zellij_cli::args::CliAction;
use zellij_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};

#[test]
fn deno_background_workers_await_real_commands_and_preserve_state() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("workers.ts");
    fs::write(&plugin, r#"
import {definePlugin, zellij} from 'zellij:plugin';
let count = 0;
let state = 'starting';
const results: string[] = [];
export default definePlugin({
  defaults: {},
  workers: {
    async command(name, payload) {
      const result = await zellij.runCommand({command:['/bin/sh', '-c', 'printf "%s" "$VALUE"'], envVariables:{VALUE:payload}});
      if (result.exitCode !== 0) throw Error('command failed');
      zellij.postMessageToPlugin(name, `${++count}:${zellij.decodeUtf8(result.stdout)}`);
    }
  },
  async load() {
    await zellij.waitForPermissions(['RunCommands']);
    zellij.on('CustomMessage', ([name, payload]) => {
      if (name !== 'result') return;
      results.push(payload);
      if (results.length === 2) {
        if (count !== 0 || results.join() !== '1:hé🙂,2:second') throw Error('worker state: ' + results);
        state = 'background commands verified'; return true;
      }
    });
    zellij.subscribe(['CustomMessage']);
    zellij.postMessageTo('command', 'result', 'hé🙂');
    zellij.postMessageTo('command', 'result', 'second');
  },
  render() {return state;}
});
"#).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{ plugin location=\"file:{}\"; }}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("worker command permission prompt", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("workers complete their real commands", |grid| {
        grid.contains("background commands verified")
    });
    zellij.quit();
}

#[test]
fn deno_worker_pane_sequences_and_reruns_complete_before_the_next_job() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("worker-panes.ts");
    fs::write(&plugin, r#"
let state = 'starting', replies = 0;
const events = ['CommandPaneOpened', 'CommandPaneReRun', 'CommandPaneExited'];
export default {
  workers: {
    async runner(name) {
      if (name === 'next') { zellij.postMessageTo('sibling', 'check'); return; }
      const sequence = zellij.paneSequence({steps:[{path:'first',stdin:'first-ready'}, {path:'second',stdin:'second-ready'}]});
      const result = await sequence.done;
      if (result.status !== 'success' || result.exitCodes.join() !== '0,0') throw Error('sequence result');
      const rerun = await zellij.runInPane(result.paneId, {path:'third',stdin:'third-ready'});
      if (rerun.exitCode !== 0) throw Error('rerun result');
      zellij.postMessageToPlugin('done');
    },
    sibling(name) {
      if (name === 'check') { zellij.postMessageToPlugin('sibling', String(replies)); return; }
      for (const event of events) zellij.on(event, () => { replies++; });
      zellij.subscribe(events);
      zellij.postMessageToPlugin('ready');
    }
  },
  async load() {
    await zellij.waitForPermissions(['RunCommands','ReadApplicationState','WriteToStdin']);
    for (const event of events) zellij.on(event, () => { replies++; });
    zellij.subscribe([...events, 'CustomMessage']);
    zellij.on('CustomMessage', ([name, payload]) => {
      if (name === 'ready') {
        zellij.postMessageTo('runner', 'run');
        zellij.postMessageTo('runner', 'next');
      }
      if (name === 'done') state = 'worker done';
      if (name === 'sibling') {
        if (state !== 'worker done' || replies !== 0 || payload !== '0') throw Error('misrouted pane completion');
        state = 'worker panes verified';
      }
      return true;
    });
    zellij.postMessageTo('sibling', 'observe');
  },
  render() {return state;}
};
"#).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("worker pane permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    let pane = zellij.expect_pty_spawn();
    for ready in ["first-ready", "second-ready", "third-ready"] {
        pane.wait_for_stdin(ready, |bytes| bytes.ends_with(ready.as_bytes()));
        pane.exit(Some(0));
    }
    zellij.wait_until(
        "pane completions reach only their worker and release its next job",
        |grid| grid.contains("worker panes verified"),
    );
    zellij.quit();
}

fn receive_http(listener: &std::net::TcpListener) -> std::net::TcpStream {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "HTTP guest did not connect"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            },
            Err(error) => panic!("HTTP accept: {error}"),
        }
    };
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    socket
        .set_write_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
        assert!(request.len() < 8192);
    }
    socket
}

#[test]
fn deno_observes_real_buffered_http_and_ordered_stream_bytes() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut socket = receive_http(&listener);
            socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\na\x00b\r\n2\r\n\x80\xff\r\n0\r\n\r\n").unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("http.ts");
    fs::write(&plugin, format!(r#"
let state = 'starting';
export default {{
  async load() {{
    await zellij.waitForPermissions(['WebAccess']);
    const url = 'http://{address}/bytes';
    const buffered = await zellij.webRequest({{url}});
    if (buffered.status !== 200 || [...buffered.body].join() !== '97,0,98,128,255') throw Error('buffered bytes');
    const chunks = [];
    let leadingSeen = false, terminators = 0;
    zellij.on('WebRequestResult', () => {{ leadingSeen = true; }});
    zellij.on('WebRequestStreamChunk', (e) => {{
      if (e.stream_id !== 'bytes') return;
      if (!leadingSeen) throw Error('stream before response');
      chunks.push(...e.chunk);
      if (e.done) terminators++;
    }});
    const done = zellij.waitForEvent('WebRequestStreamChunk', e => e.stream_id === 'bytes' && e.done);
    const leading = await zellij.webRequest({{url, streamId: 'bytes'}});
    if (leading.status !== 200 || leading.body.length !== 0 || (await done).error !== null) throw Error('stream response');
    if (chunks.join() !== '97,0,98,128,255' || terminators !== 1) throw Error('stream bytes');
    state = 'buffered and streamed HTTP verified';
  }},
  render() {{ return state; }}
}};
"#)).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n  tab {{\n    pane {{ plugin location=\"file:{}\"; }}\n  }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("HTTP permission prompt", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until(
        "the guest observes real buffered and streamed bytes",
        |grid| grid.contains("buffered and streamed HTTP verified"),
    );
    server.join().unwrap();
    zellij.quit();
}

const TYPESCRIPT: &str = r#"
import { zellij, definePlugin } from "zellij:plugin";
import { label } from "./dependency.ts";
let state: string = "starting";
export default definePlugin({
  async load() {
    zellij.subscribe(["PermissionRequestResult", "Key"]);
    await zellij.waitForPermissions(["ChangeApplicationState", "RunCommands"]);
    const ids = zellij.command("GetPluginIds");
    if (typeof ids !== "object" || ids === null || !("plugin_id" in ids)) {
      throw new Error("missing decoded plugin identity");
    }
    const key = zellij.waitForEvent("Key");
    state = `${label} granted; awaiting key`;
    zellij.requestRender();
    await key;
    const result = await zellij.runCommand({
      command: ['/bin/sh', '-c', 'printf "%s" "$MESSAGE"; printf "err" >&2; exit 7'],
      envVariables: {MESSAGE: `${label} command hé🙂`}, context: {label},
    });
    if (result.exitCode !== 7 || zellij.decodeUtf8(result.stdout) !== `${label} command hé🙂`
        || zellij.decodeUtf8(result.stderr) !== 'err' || result.context.label !== label
        || Object.hasOwn(result.context, '__zellij_async')) throw new Error('incorrect command result');
    zellij.command("RenameTabWithId", [0, `${label} host command`]);
    state = `${label} awaited key rendered`;
    zellij.requestRender();
    // Keep a helper pending through reload and session shutdown.
    await zellij.waitForPipe("acceptance-finish");
  },
  async render() {
    await zellij.sleep(0);
    return state;
  }
});
"#;

#[test]
fn deno_and_wasm_share_a_session_through_permissions_events_reload_and_close() {
    let directory = tempfile::tempdir().unwrap();
    let plugins = directory.path().join("plugins");
    fs::create_dir(&plugins).unwrap();
    fs::write(plugins.join("acceptance.ts"), TYPESCRIPT).unwrap();
    let dependency = plugins.join("dependency.ts");
    fs::write(&dependency, "export const label: string = 'first';").unwrap();
    fs::write(
        plugins.join("companion.js"),
        r#"
export default {render() {return "JavaScript companion is alive";}};
"#,
    )
    .unwrap();

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!("user_plugin_dir \"{}\"", plugins.display()))
        .with_layout(LayoutInfo::Stringified(
            r#"
layout {
  tab name="wasm companion" {
    pane size=1 borderless=true { plugin location="zellij:tab-bar"; }
    pane size=4 { plugin location="file:companion.js"; }
    pane focus=true { plugin location="file:acceptance.ts"; }
  }
}
"#
            .to_owned(),
        ))
        .start();

    zellij.wait_until(
        "all runtimes loaded and TypeScript requested permission",
        |grid| {
            grid.contains("wasm companion")
                && grid.contains("JavaScript companion is alive")
                && grid.contains("Allow? (y/n)")
        },
    );
    zellij.send_stdin(b"y");
    zellij.wait_until(
        "the async load resumed after the permission grant",
        |grid| grid.contains("first granted; awaiting key") && !grid.contains("Allow? (y/n)"),
    );
    zellij.send_stdin(b"a");
    zellij.wait_until(
        "the awaited key produced a TS frame and renamed the Wasm tab bar",
        |grid| {
            grid.contains("first awaited key rendered")
                && grid.contains("first host command")
                && grid.contains("JavaScript companion is alive")
        },
    );

    // Only the dependency changes. The entrypoint and its pane stay the same.
    fs::write(&dependency, "export const label: string = 'second';").unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugins.join("acceptance.ts").display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    zellij.wait_until(
        "reload imports the changed dependency and retains the permission grant",
        |grid| grid.contains("second granted; awaiting key") && !grid.contains("Allow? (y/n)"),
    );
    zellij.send_stdin(b"b");
    let grid = zellij.wait_until(
        "the replacement instance handles input alongside JavaScript and Wasm",
        |grid| {
            grid.contains("second awaited key rendered")
                && grid.contains("second host command")
                && grid.contains("JavaScript companion is alive")
        },
    );
    assert!(!grid.contains("first awaited key rendered"), "{grid}");
    // quit joins the real client and server with the harness's bounded shutdown checks.
    zellij.quit();
}

#[test]
fn deno_host_queries_configuration_layouts_recordings_and_cross_session_warp() {
    // As in the existing warp suite, start the receiver first: both servers share
    // this test process's session-name environment, which must name the sender.
    let receiver = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    let receiver_shell = receiver.expect_pty_spawn();
    receiver_shell.output(zellij_integration_tests::PROMPT);
    receiver.wait_for_app_load();
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("surfaces.ts");
    fs::write(&plugin, format!(r#"
let state = 'starting';
export default {{
  async load() {{ try {{
    const assert = (ok, what) => {{if (!ok) throw Error(what);}};
    await zellij.waitForPermissions(['ReadApplicationState','ChangeApplicationState','ReadPaneContents','Reconfigure']);
    const ids = zellij.getPluginIds();
    assert(zellij.command('GetPaneInfo', {{Plugin:ids.plugin_id}}).is_plugin, 'plugin identity');
    assert(zellij.command('GetPaneInfo', {{Terminal:4294967295}}) === null, 'missing pane');
    const pane = zellij.command('GetPaneInfo', {{Terminal:0}});
    assert(pane && !pane.is_plugin && pane.target === 'Local', 'terminal pane');
    assert(zellij.command('GetTabInfo', 0).name === 'surface test', 'tab reply');
    const parsed = zellij.command('ParseLayout', 'layout {{\n tab name="parsed" {{\n pane;\n }}\n}}');
    assert(parsed.tabs[0].name === 'parsed' && parsed.tabs[0].panes.length === 1, 'layout metadata');
    let failed = false;
    try {{zellij.command('ParseLayout', 'layout {{ !!!');}} catch (_) {{failed = true;}}
    assert(failed, 'invalid layout');
    const session = zellij.command('GetSessionList');
    assert(session.live_sessions.some(s => s.is_current_session), 'session snapshot');
    assert(Array.isArray(session.resurrectable_sessions), 'absent sessions');
    // Subscription bookkeeping must not block the thread answering this query.
    const initialKeys = zellij.waitForEvent('InitialKeybinds');
    assert(zellij.command('DumpSessionLayout', {{tab_index:0}}).layout.includes('pane'), 'serialized layout');
    assert(Array.isArray(await initialKeys), 'initial keybindings');
    const updated = zellij.waitForEvent('ModeUpdate', info => info.style.rounded_corners);
    zellij.command('Reconfigure', ['ui {{\n pane_frames {{\n rounded_corners true;\n }}\n}}', false]);
    await updated;
    assert(Array.isArray(zellij.command('ListRecordings')), 'recording inventory');
    assert(Array.isArray(zellij.command('SearchRecordings', {{query:'unrecorded unique text', limit:10}})), 'recording search');
    zellij.command('RenameTerminalPane', [0, 'Deno warp payload']);
    const moved = zellij.command('WarpPaneToSession', {{pane_id:{{Terminal:0}}, to_session:{receiver:?}, placement:'FocusedTab'}});
    assert(typeof moved === 'string', 'warp reply');
    assert(zellij.command('GetPaneInfo', {{Terminal:0}}) === null, 'pane left sender');
    state = 'host surfaces verified';
  }} catch (error) {{ state = 'host surfaces failed: ' + error.stack; }} }},
  render() {{return state;}}
}};
"#, receiver=receiver.session_name())).unwrap();
    let mut sender = TestRunner::new(TERMINAL_SIZE).skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}\nsession_recording true")
        .with_layout(LayoutInfo::Stringified(format!("layout {{\n tab name=\"surface test\" {{\n pane;\n pane focus=true {{plugin location=\"file:{}\";}}\n }}\n}}", plugin.display())))
        .start();
    let terminal = sender.expect_pty_spawn();
    terminal.output(b"Deno cross-session output\r\n");
    sender.wait_until("Deno requests host surface permissions", |screen| {
        screen.contains("Allow? (y/n)")
    });
    sender.send_stdin(b"y");
    let result = sender.wait_until("Deno completes the host surface round trips", |screen| {
        screen.contains("host surfaces")
    });
    assert!(result.contains("host surfaces verified"), "{result}");
    receiver.wait_until(
        "Deno moved the real terminal to the other session",
        |screen| screen.contains("Deno cross-session output"),
    );
    sender.quit();
}

#[test]
fn deno_reload_reuses_stream_id_without_delivering_or_cancelling_the_old_generation() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut old = receive_http(&listener);
        old.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nold-start\r\n")
            .unwrap();
        let mut replacement = receive_http(&listener);
        // Reload may close this socket. Either outcome is fine; no old bytes may
        // be observed by the new guest, and old cleanup must not cancel its request.
        let _ = old.write_all(b"8\r\nold-late\r\n0\r\n\r\n");
        replacement.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\na\r\nnew-result\r\n0\r\n\r\n").unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("replace-stream.ts");
    let source = format!(
        r#"
const label = 'old';
let state = 'starting', contents = '';
export default {{
  async load() {{
    await zellij.waitForPermissions(['WebAccess']);
    zellij.subscribe(['WebRequestStreamChunk']);
    zellij.on('WebRequestStreamChunk', event => {{
      contents += zellij.decodeUtf8(new Uint8Array(event.chunk));
      state = event.done ? label + ' stream complete ' + contents : label + ' stream pending';
      if (event.error) state = 'stream failed ' + event.error;
      return true;
    }});
    zellij.command('WebRequest', ['http://{address}/stream', 'Get', {{}}, [], {{}}, 'same-id', null]);
  }},
  render() {{return state;}}
}};
"#
    );
    fs::write(&plugin, &source).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("stream permission prompt", |screen| {
        screen.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("old stream remains in flight", |screen| {
        screen.contains("old stream pending")
    });
    fs::write(
        &plugin,
        source.replace("label = 'old'", "label = 'replacement'"),
    )
    .unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    let result = zellij.wait_until("replacement stream reaches completion", |screen| {
        screen.contains("stream complete") || screen.contains("stream failed")
    });
    assert!(
        result.contains("replacement stream complete new-result"),
        "{result}"
    );
    server.join().unwrap();
    zellij.quit();
}

#[test]
fn deno_forms_and_markdown_render_beside_wasm() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("forms.ts"),
        include_str!("../../example-plugins/forms.ts"),
    )
    .unwrap();
    fs::write(
        directory.path().join("companion.js"),
        "export default {render() {return 'JavaScript companion';}};",
    )
    .unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "user_plugin_dir \"{}\"",
            directory.path().display()
        ))
        .with_layout(LayoutInfo::Stringified(
            r#"layout {
          tab name="Wasm companion" {
            pane size=1 borderless=true {plugin location="zellij:tab-bar";}
            pane size=3 {plugin location="file:companion.js";}
            pane focus=true {plugin location="file:forms.ts";}
          }
        }"#
            .into(),
        ))
        .start();
    zellij.wait_until(
        "shared controls and Markdown render in the mixed session",
        |screen| {
            screen.contains("Wasm companion")
                && screen.contains("JavaScript companion")
                && screen.contains("Pane frames")
                && screen.contains("Shared widgets")
        },
    );
    zellij.send_stdin(b"x");
    zellij.wait_until("Deno form edits Unicode text", |screen| {
        screen.contains("日本語x")
    });
    zellij.quit();
}

#[test]
fn deno_native_compilation_failure_keeps_the_session_alive() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("recursive.ts");
    fs::write(
        &plugin,
        format!(
            "export default {}0{};",
            "[".repeat(10_000),
            "]".repeat(10_000)
        ),
    )
    .unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            "user_plugin_dir \"{}\"",
            directory.path().display()
        ))
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{ plugin location=\"file:{}\"; }}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("compilation failure appears in the live session", |grid| {
        grid.contains("compilation worker")
    });
    fs::write(
        &plugin,
        r#"
let state = 'checking';
export default {
  async load() {
    await zellij.waitForPermissions(['AuthorPlugins', 'ReadFiles']);
    const source = 'export default ' + '['.repeat(10000) + '0' + ']'.repeat(10000) + ';';
    for (const mode of ['Syntax', 'Semantic']) {
      const failed = zellij.command('CheckPlugin', {name: 'check.ts', source, mode});
      if (!failed.diagnostics.some(d => d.includes('compilation worker'))) throw Error('missing compilation diagnostic');
      const healthy = zellij.command('CheckPlugin', {name: 'check.ts', source: 'export default {};', mode});
      if (healthy.diagnostics.length) throw Error(healthy.diagnostics.join('\n'));
    }
    state = 'healthy after compilation failure';
  },
  render() {return state;}
};
"#,
    )
    .unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    zellij.wait_until("authoring permission prompt", |grid| {
        grid.contains("Allow? (y/n)")
    });
    // Failed initial loads are absent from the runtime map, so reload opens a new pane.
    let (code, output) = zellij.run_cli_action_reporting(CliAction::FocusNextPane);
    assert_eq!(code, 0, "{output}");
    zellij.send_stdin(b"y");
    zellij.wait_until(
        "a healthy replacement renders in the same session",
        |grid| grid.contains("healthy after compilation failure"),
    );
    zellij.quit();
}

#[test]
fn embedded_deno_context_menu_hook_filters_and_dispatches_selection() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
mouse_mode true
context_menu_on_right_click true
load_plugins clear-defaults=true {}
hooks { context_menu "zellij:context-menu"; }
context_menu {
    "Copy" { Copy; }
    separator
    "Deno menu action" { NewTab; }
}
"#,
        )
        .start();
    zellij_integration_tests::claim_first_terminal_and_wait_for_prompt(&zellij);
    zellij.send_stdin(b"\x1b[<2;5;5M\x1b[<2;5;5m");
    zellij.wait_until("embedded hook filters Copy from the menu", |grid| {
        grid.contains("Deno menu action") && !grid.contains("Copy")
    });
    zellij.send_stdin(b"\x1b[B\r");
    zellij.wait_until("configured menu action opens a second tab", |grid| {
        grid.contains("Tab #2")
    });
    zellij.quit();
}

#[test]
fn deno_pane_sequence_reuses_one_host_pane_and_cancels_its_child() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("sequence.ts");
    fs::write(&plugin, r#"
let state='starting';
export default {
  async load() {
    await zellij.waitForPermissions(['RunCommands','ReadApplicationState','ChangeApplicationState','WriteToStdin']);
    const sequence=zellij.paneSequence({steps:[{path:'first',stdin:'first-ready'},{path:'second',stdin:'second-ready'}]});
    const result=await sequence.done;
    if(result.status!=='success' || result.exitCodes.join()!=='0,0')throw Error('sequence result');
    state='sequence success';zellij.requestRender();
    const cancelled=zellij.paneSequence({steps:[{path:'cancel',stdin:'cancel-ready'}],killGraceMs:0});
    const started=zellij.waitForEvent('CommandPaneOpened',([id])=>id===cancelled.paneId.Terminal);
    await started;cancelled.cancel();cancelled.cancel();
    const stopped=await cancelled.done;
    if(stopped.status!=='cancelled')throw Error('cancel result');
    state='sequence and cancellation verified';zellij.requestRender();
  }, render(){return state;}
};
"#).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("sequence permissions", |screen| {
        screen.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    let pane = zellij.expect_pty_spawn();
    pane.wait_for_stdin("first command starts before its stdin", |bytes| {
        bytes.ends_with(b"first-ready")
    });
    pane.exit(Some(0));
    pane.wait_for_stdin("second command reuses the first pane", |bytes| {
        bytes.ends_with(b"second-ready")
    });
    pane.exit(Some(0));
    let cancelled = zellij.expect_pty_spawn();
    cancelled.wait_for_stdin("cancellable command starts", |bytes| {
        bytes.ends_with(b"cancel-ready")
    });
    zellij.wait_until("sequence and cancellation finish", |screen| {
        screen.contains("sequence and cancellation verified")
    });
    assert!(cancelled.is_exited());
    zellij.quit();
}

#[test]
fn deno_pane_sequence_reload_does_not_advance_the_old_steps() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("reload-sequence.ts");
    fs::write(&plugin, r#"
export default {async load(){
  await zellij.waitForPermissions(['RunCommands','ReadApplicationState','WriteToStdin']);
  const sequence=zellij.paneSequence({steps:[{path:'old',stdin:'old-ready'},{path:'must-not-start',stdin:'old-late'}]});
  await sequence.done;
},render(){return 'old sequence';}};
"#).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    zellij.wait_until("old sequence permissions", |screen| {
        screen.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    let pane = zellij.expect_pty_spawn();
    pane.wait_for_stdin("old step started", |bytes| bytes.ends_with(b"old-ready"));
    fs::write(
        &plugin,
        format!(
            r#"
let state='starting replacement';
export default {{async load(){{
  await zellij.waitForPermissions(['RunCommands','ReadApplicationState','WriteToStdin']);
  const waiting=zellij.runInPane({},{{path:'replacement',stdin:'replacement-ready'}});
  state='replacement loaded';zellij.requestRender();
  const result=await waiting;
  if(result.exitCode!==0)throw Error('replacement exit');
  state='replacement exit verified';zellij.requestRender();
}},render(){{return state;}}}};
"#,
            pane.terminal_id()
        ),
    )
    .unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    zellij.wait_until("replacement loaded", |screen| {
        screen.contains("replacement loaded")
    });
    pane.exit(Some(0));
    pane.wait_for_stdin("replacement starts on old pane", |bytes| {
        bytes.ends_with(b"replacement-ready")
    });
    pane.exit(Some(0));
    zellij.wait_until("replacement observes its own exit", |screen| {
        screen.contains("replacement exit verified")
    });
    assert!(!String::from_utf8_lossy(&pane.stdin_bytes()).contains("old-late"));
    zellij.quit();
}

#[test]
fn deno_pane_exit_observes_a_held_cli_pane() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("observer.ts");
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    zellij_integration_tests::claim_first_terminal_and_wait_for_prompt(&zellij);
    let (code, output) = zellij.run_cli_action_reporting(CliAction::NewPane {
        direction: None,
        command: vec!["controlled-command".to_owned()],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    });
    assert_eq!(code, 0, "{output}");
    let pane = zellij.expect_pty_spawn();
    fs::write(&plugin, format!(r#"
let state='starting observer';
export default {{async load(){{
  await zellij.waitForPermissions(['ReadApplicationState']);
  const waiting=zellij.waitForPaneExit({id});
  state='observer ready';zellij.requestRender();
  const result=await waiting;
  const info=zellij.command('GetPaneInfo',{{Terminal:{id}}});
  if(result.paneId.Terminal!=={id} || result.exitCode!==23 || result.closed || !info?.is_held || !info.exited || info.exit_status!==23)throw Error('held exit mismatch');
  state='held CLI exit verified';zellij.requestRender();
}},render(){{return state;}}}};
"#, id=pane.terminal_id())).unwrap();
    let (code, output) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
    });
    assert_eq!(code, 0, "{output}");
    zellij.wait_until("observer permission prompt", |screen| {
        screen.contains("Allow? (y/n)")
    });
    assert_eq!(zellij.run_cli_action(CliAction::FocusNextPane), 0);
    zellij.send_stdin(b"y");
    zellij.wait_until("observer registered before the exit", |screen| {
        screen.contains("observer ready")
    });
    pane.exit(Some(23));
    zellij.wait_until("separate observer sees held CLI exit status", |screen| {
        screen.contains("held CLI exit verified")
    });
    zellij.quit();
}

#[test]
fn deno_multipane_reserves_launch_slots_and_keeps_exit_and_failed_plugin_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("multipane.js");
    let permission_child = directory.path().join("permission-child.js");
    fs::write(
        &permission_child,
        r#"
import { zellij } from 'zellij:plugin';
let state = 'CHILD LOADING';
export default {
  async load() { await zellij.waitForPermissions(['WebAccess']); state = 'CHILD READY'; },
  render() { return state; }
};
"#,
    )
    .unwrap();
    fs::write(&plugin, r#"
import { zellij } from 'zellij:plugin';
let state = 'requesting composition', container;
export default {
  async load() {
    await zellij.waitForPermissions(['ChangeApplicationState', 'RunCommands', 'OpenTerminalsOrPlugins', 'ReadApplicationState']);
    const own = { Plugin: zellij.host.getPluginIds().plugin_id };
    const placement = {kind: 'Dismissible', coordinates: {x: {Fixed: 3}, y: {Fixed: 3}, width: {Fixed: 90}, height: {Fixed: 18}}};
    const split = {direction: 'LeftRight', ratio: 5000};
    try {
      zellij.host.createMultipane({children: [{Existing: own}, {Existing: own}], placement, split, focus: null});
      throw Error('duplicate adoption succeeded');
    } catch (error) { if (!String(error).includes('distinct')) throw error; }
    container = zellij.host.createMultipane({children: [{Existing: own}, {Terminal: {path: 'multipane-command'}}], placement, split, focus: 0});
    if (container.children[0].Plugin !== own.Plugin || container.children[1].Terminal === undefined) throw Error('child order');
    zellij.host.updateMultipane({id: container.id, coordinates: null, split: {direction: 'TopBottom', ratio: 4000}, override_user_placement: false});
    zellij.subscribe(['Key']);
    state = 'COMPOSITION OWNER';
  },
  update(event) {
    const key = event.Key?.bare_key?.Char;
    if (key === 't') {
      zellij.host.replaceMultipaneChild({id: container.id, slot: 1, child: {Terminal: {path: 'multipane-replacement'}}});
      return true;
    }
    if (key === 'p') {
      zellij.host.replaceMultipaneChild({id: container.id, slot: 1, child: {Plugin: {url: 'PERMISSION_CHILD', configuration: {}}}});
      return true;
    }
    if (key === 'q') zellij.host.closeSelf();
    if (key === 'r') {
      zellij.host.replaceMultipaneChild({id: container.id, slot: 1, child: {Plugin: {url: 'file:/missing-multipane-child.wasm', configuration: {}}}});
      state = 'SIBLING SURVIVED';
      return true;
    }
  },
  render() { return state; }
};
"#.replace("PERMISSION_CHILD", &format!("file:{}", permission_child.display()))).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n pane\n pane focus=true {{plugin location=\"file:{}\";}}\n}}",
            plugin.display()
        )))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"BASE SHELL");
    zellij.wait_until("composition permission prompt", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    let command = zellij.expect_pty_spawn();
    command.wait_for_command("multipane-command");
    command.output(b"LAUNCHED IN SLOT");
    zellij.wait_until("both owned slots render", |grid| {
        grid.contains("COMPOSITION OWNER") && grid.contains("LAUNCHED IN SLOT")
    });
    let list = || {
        let (code, output) = zellij.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: true,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        serde_json::from_str::<Vec<serde_json::Value>>(&output).unwrap()
    };
    let panes = list();
    let members: Vec<_> = panes
        .iter()
        .filter(|pane| pane.get("multipane").is_some())
        .collect();
    assert_eq!(members.len(), 2);
    let info = members[0]["multipane"].clone();
    assert_eq!(members[1]["multipane"], info);
    assert_eq!(info["split"]["direction"], "TopBottom");
    command.exit(Some(17));
    zellij.wait_until("exited command stays in its slot", |_| {
        list().iter().any(|pane| {
            pane["multipane"]["id"] == info["id"]
                && pane["is_held"] == true
                && pane["exit_status"] == 17
        })
    });
    zellij.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("terminal_{}", command.terminal_id()),
    });
    zellij.wait_until("focused exit status uses the outer frame", |grid| {
        grid.contains("COMPOSITION OWNER") && grid.contains("17")
    });
    let owner = members
        .iter()
        .find(|pane| pane["is_plugin"] == true)
        .unwrap();
    zellij.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", owner["id"]),
    });
    assert_eq!(
        list()
            .iter()
            .filter(|pane| pane["multipane"]["id"] == info["id"])
            .count(),
        2
    );
    // Replacement must start immediately even when the current child is still running.
    for message in ["FIRST REPLACEMENT", "RUNNING CHILD REPLACED"] {
        zellij.send_stdin(b"t");
        let replacement = zellij.expect_pty_spawn();
        replacement.wait_for_command("multipane-replacement");
        replacement.output(message.as_bytes());
        zellij.wait_until("terminal replacement stays beside its owner", |grid| {
            grid.contains(message) && grid.contains("COMPOSITION OWNER")
        });
    }
    zellij.send_stdin(b"r");
    zellij.wait_until("failed child launch stays beside its sibling", |grid| {
        grid.contains("SIBLING SURVIVED")
            && (grid.contains("Failed") || grid.contains("failed") || grid.contains("ERROR"))
    });
    let members: Vec<_> = list()
        .into_iter()
        .filter(|pane| pane["multipane"]["id"] == info["id"])
        .collect();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|pane| pane["is_plugin"] == true));
    assert_eq!(members[0]["multipane"], members[1]["multipane"]);
    zellij.send_stdin(b"p");
    zellij.wait_until("child permission prompt stays in its slot", |grid| {
        grid.contains("SIBLING SURVIVED") && grid.contains("Allow? (y/n)")
    });
    assert_eq!(
        list()
            .iter()
            .filter(|pane| pane["multipane"]["id"] == info["id"])
            .count(),
        2
    );
    let child = list()
        .into_iter()
        .find(|pane| pane["multipane"]["id"] == info["id"] && pane["id"] != owner["id"])
        .unwrap();
    zellij.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", child["id"]),
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("delayed child finishes in its slot", |grid| {
        grid.contains("SIBLING SURVIVED") && grid.contains("CHILD READY")
    });
    zellij.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", owner["id"]),
    });
    zellij.send_stdin(b"q");
    zellij.wait_until("close_self closes the container", |grid| {
        grid.contains("BASE SHELL")
            && !grid.contains("SIBLING SURVIVED")
            && !grid.contains("CHILD READY")
    });
    assert!(list().iter().all(|pane| pane.get("multipane").is_none()));
    zellij.quit();
}
