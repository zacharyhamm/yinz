#![cfg(unix)]

//! A mixed-runtime session: the real permission prompt, event delivery, reload and shutdown.

use std::fs;
use std::io::{Read, Write};
use zellij_cli::args::CliAction;
use zellij_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};

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
      if (!leadingSeen) throw Error('stream before response');
      chunks.push(...e.chunk);
      if (e.done) terminators++;
    }});
    const leading = zellij.waitForEvent('WebRequestResult');
    const done = zellij.waitForEvent('WebRequestStreamChunk', e => e.done);
    zellij.command('WebRequest', [url, 'Get', {{}}, [], {{}}, 'bytes', null]);
    if ((await leading)[0] !== 200 || (await done).error !== null) throw Error('stream response');
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
fn deno_lua_and_wasm_share_a_session_through_permissions_events_reload_and_close() {
    let directory = tempfile::tempdir().unwrap();
    let plugins = directory.path().join("plugins");
    fs::create_dir(&plugins).unwrap();
    fs::write(plugins.join("acceptance.ts"), TYPESCRIPT).unwrap();
    let dependency = plugins.join("dependency.ts");
    fs::write(&dependency, "export const label: string = 'first';").unwrap();
    fs::write(
        plugins.join("companion.lua"),
        r#"
return { render = function() return "Lua companion is alive" end }
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
    pane size=4 { plugin location="file:companion.lua"; }
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
                && grid.contains("Lua companion is alive")
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
                && grid.contains("Lua companion is alive")
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
        "the replacement instance handles input alongside Lua and Wasm",
        |grid| {
            grid.contains("second awaited key rendered")
                && grid.contains("second host command")
                && grid.contains("Lua companion is alive")
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
