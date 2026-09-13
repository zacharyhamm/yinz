#[test]
fn pane_reply_exposes_restart_error() {
    use prost::Message;
    use zellij_plugin_api::plugin_command::ProtobufGetPaneInfoResponse;
    use zellij_types::data::{CommandType, PaneInfo};
    let mut runtime = sdk();
    for error in [None, Some("Cannot restart agent".to_owned())] {
        let reply = ProtobufGetPaneInfoResponse {
            pane_info: Some(PaneInfo { restart_error: error.clone(), ..Default::default() }.try_into().unwrap()),
        };
        let crate::response::DecodedResponse::Json(value) = crate::response::decode_response(CommandType::GetPaneInfo, &reply.encode_to_vec()).unwrap().unwrap() else {
            panic!("pane reply must be JSON");
        };
        run(&mut runtime, &format!("check(({}).restart_error === {});", value, json!(error)));
    }
}

fn camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[test]
fn host_sugar_covers_the_rust_command_inventory_and_is_frozen() {
    use strum::IntoEnumIterator;
    use zellij_types::data::CommandType;
    let mut expected = CommandType::iter()
        .map(|name| camel(&format!("{name:?}")))
        .collect::<Vec<_>>();
    expected.sort();
    let mut runtime = sdk();
    run(&mut runtime, &format!(r#"
      const expected = {};
      check(JSON.stringify(Object.keys(zellij.host).sort()) === JSON.stringify(expected));
      check(Object.isFrozen(zellij.host) && Object.isFrozen(zellij));
      check(Object.values(zellij.host).every((member) => typeof member === 'function'));
      let refused = false;
      try {{ zellij.host.getPluginIds(); }} catch (error) {{ refused = String(error).includes('module initialization'); }}
      check(refused);
    "#, json!(expected)));
}

#[test]
fn host_sugar_builds_unit_positional_array_and_named_payloads() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.refused = (fn, text) => { let message = null; try { fn(); } catch (error) { message = String(error); } check(message !== null && message.includes(text)); };
      bind({load() {
        check(zellij.host.getPluginIds().client_id === 5);
        zellij.host.switchTabTo(2);
        zellij.host.renameTab(0, 'logs');
        zellij.host.renameTab([0, 'logs']);
        zellij.host.renameTab({tabIndex: 0, name: 'logs'});
        zellij.host.renameTabWithId({tabId: 4, name: 'four'});
        zellij.host.renameTerminalPane({paneId: 1, name: 'one'});
        zellij.host.renamePluginPane({paneId: 2, name: 'two'});
        zellij.host.focusTerminalPane({paneId: 7});
        zellij.host.focusPluginPane({paneId: 8, shouldFloatIfHidden: true, shouldBeInPlaceIfHidden: false});
        zellij.host.showPaneWithId({paneId: {Terminal: 7}, shouldFloatIfHidden: false, shouldFocusPane: true});
        zellij.host.setFloatingPanePinned({paneId: {Plugin: 3}, shouldBePinned: true});
        zellij.host.setPaneBorderless({paneId: {Terminal: 3}, borderless: false});
        zellij.host.revealPaneWithId({paneId: {Terminal: 3}});
        zellij.host.notify({message: 'hi'});
        zellij.host.notify({message: 'hi', paneId: {Terminal: 1}, click: {message: 'm', payload: 'p'}});
        zellij.host.cliPipeOutput({pipeName: 'p', output: 'o'});
        zellij.host.reconfigure({configuration: 'keybinds {}', writeToDisk: false});
        zellij.host.runCommand({command: ['true'], envVariables: {}, cwd: '/host', context: {}});
        zellij.host.runCommand([['true'], {}, '/host', {}, null]);
        zellij.host.openCommandPaneInGitCheckoutNearPlugin({command: {path: 'sh'}, sourceDirectory: '/src', checkout: 'Current'});
        zellij.host.write(new Uint8Array([1, 2]));
        refused(() => zellij.host.renameTab({tab_index: 0, name: 'x'}), 'unknown RenameTab argument: tab_index');
        refused(() => zellij.host.renameTab(new Proxy({tabIndex: 0, name: 'x'}, {})), 'proxies');
        refused(() => zellij.host.renameTab({get tabIndex() { throw Error('accessed SECRET'); }, name: 'x'}), 'accessor');
        zellij.waitForEvent('Key');
        zellij.host.unsubscribe(['Key', 'Timer']);
        check(zellij.host.requestPluginPermissions(['ReadFiles']) === null);
      }});
      bridge('load', {}, 5);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const expected = [
        'GetPluginIds', {SwitchTabTo: 2}, {RenameTab: [0, 'logs']}, {RenameTab: [0, 'logs']}, {RenameTab: [0, 'logs']},
        {RenameTabWithId: [4, 'four']}, {RenameTerminalPane: [1, 'one']}, {RenamePluginPane: [2, 'two']},
        {FocusTerminalPane: [7, null, null]}, {FocusPluginPane: [8, true, false]},
        {ShowPaneWithId: [{Terminal: 7}, false, true]}, {SetFloatingPanePinned: [{Plugin: 3}, true]},
        {SetPaneBorderless: [{Terminal: 3}, false]}, {RevealPaneWithId: [{Terminal: 3}, null]},
        {Notify: ['hi', null, null]}, {Notify: ['hi', {Terminal: 1}, {message: 'm', payload: 'p'}]},
        {CliPipeOutput: ['p', 'o']}, {Reconfigure: ['keybinds {}', false]},
        {RunCommand: [['true'], {}, '/host', {}, null, null]}, {RunCommand: [['true'], {}, '/host', {}, null, null]},
        {OpenCommandPaneInGitCheckoutNearPlugin: [{path: 'sh'}, '/src', 'Current']},
        {Write: [1, 2]}, {Subscribe: ['Key']}, {Unsubscribe: ['Timer']},
        {Subscribe: ['PermissionRequestResult']}, {RequestPluginPermissions: ['ReadFiles']},
      ];
      check(sent.length === expected.length, 'count');
      expected.forEach((command, i) => check(JSON.stringify(sent[i][0]) === JSON.stringify(command) && sent[i][1] === 5, i));
    "#);
}

#[test]
fn post_message_to_is_refused_before_dispatch_in_every_form() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.errors = [];
      bind({defaults: {trace: 'true'}, load() {
        for (const fn of [
          () => zellij.command('PostMessageTo', {name: 'x', payload: 'y'}),
          () => zellij.command({PostMessageTo: {name: 'x', payload: 'y', worker_name: 'w'}}),
          () => zellij.host.postMessageTo({name: 'x', payload: 'y'}),
        ]) { try { fn(); errors.push('accepted'); } catch (error) { errors.push(String(error)); } }
        zellij.command({PostMessageToPlugin: {name: 'a', payload: 'b'}});
      }});
      bridge('load', {}, 1);
    "#);
    capture_commands(&mut runtime);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(errors.length === 3 && errors.every((e) => e.includes('no workers')));
      check(sent.length === 1 && sent[0][0].PostMessageToPlugin.name === 'a');
      check(markers.filter((m) => m.includes('PostMessageTo" refused')).length === 3);
      check(!markers.some((m) => m === '1 command "PostMessageTo"'));
    "#);
}

#[test]
fn helpers_encode_writes_spell_pane_keys_and_normalize_keys_mouse_and_bytes() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.refused = (fn) => { let rejected = false; try { fn(); } catch (error) { rejected = error instanceof TypeError; } check(rejected); };
      bind({load() {
        zellij.write('hi'); zellij.write(new Uint8Array([1, 2]));
        zellij.writeToPaneId('a', {Terminal: 7});
        zellij.readPluginLog('x.ts'); zellij.readPluginLog('x.ts', 20);
        zellij.openTerminal(); zellij.openTerminal('.'); zellij.openTerminal({cwd: '/tmp'}); zellij.openTerminal({path: '/var'});
        zellij.closeSelf(); zellij.setSelectable(false);
        refused(() => zellij.setSelectable('no'));
        refused(() => zellij.readPluginLog('x', -1));
      }});
      bridge('load', {}, 3);
      check(zellij.keyName('Esc') === 'Esc' && zellij.keyName({Char: 'x'}) === 'x' && zellij.keyName({F: 5}) === 'F5');
      refused(() => zellij.keyName(1)); refused(() => zellij.keyName({Char: 'x', F: 1}));
      check(zellij.paneKey({Terminal: 3}) === 'terminal_3' && zellij.paneKey({Plugin: 4}) === 'plugin_4' && zellij.paneKey(9) === 'terminal_9');
      refused(() => zellij.paneKey('x')); refused(() => zellij.paneKey({Other: 1})); refused(() => zellij.paneKey({Terminal: -1}));
      check(JSON.stringify(zellij.terminalPane(3)) === '{"Terminal":3}' && JSON.stringify(zellij.pluginPane(4)) === '{"Plugin":4}');
      refused(() => zellij.terminalPane(-1)); refused(() => zellij.pluginPane(4294967296));
      const raw = {LeftClick: [3, 4]};
      check(JSON.stringify(zellij.mouse(raw)) === '{"kind":"LeftClick","line":3,"col":4}');
      check(JSON.stringify(zellij.mouse({Mouse: {ScrollUp: 2}})) === '{"kind":"ScrollUp","lines":2}');
      check(JSON.stringify(raw) === '{"LeftClick":[3,4]}');
      refused(() => zellij.mouse({LeftClick: [1]})); refused(() => zellij.mouse('LeftClick')); refused(() => zellij.mouse({}));
      const existing = new Uint8Array([7]);
      check(Object.is(zellij.bytes(existing), existing));
      check(zellij.bytes([104, 105]) instanceof Uint8Array && zellij.decodeUtf8(zellij.bytes([104, 105])) === 'hi');
      check(zellij.decodeUtf8(zellij.bytes('日')) === '日');
      refused(() => zellij.bytes([256])); refused(() => zellij.bytes([1.5])); refused(() => zellij.bytes(null));
      globalThis.sanitized = zellij.text.sanitizeBytes([0x1b, 0x5b, 0x41, 0xff, 0x68, 0x69]);
      check(zellij.text.sanitizeBytes('a\x1bb') === zellij.text.sanitize('a\x1bb'));
      refused(() => zellij.text.sanitizeBytes(null));
    "#);
    let expected = zellij_tile::text::sanitize(&String::from_utf8_lossy(&[
        0x1b, 0x5b, 0x41, 0xff, 0x68, 0x69,
    ]));
    run(&mut runtime, &format!("check(sanitized === {});", json!(expected)));
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const expected = [
        {Write: [104, 105]}, {Write: [1, 2]}, {WriteToPaneId: [[97], {Terminal: 7}]},
        {ReadPluginLog: {name: 'x.ts', lines: 50}}, {ReadPluginLog: {name: 'x.ts', lines: 20}},
        {OpenTerminal: {}}, {OpenTerminal: {path: '.'}}, {OpenTerminal: {path: '/tmp'}}, {OpenTerminal: {path: '/var'}},
        'CloseSelf', {SetSelectable: false},
      ];
      check(sent.length === expected.length, 'count');
      expected.forEach((command, i) => check(JSON.stringify(sent[i][0]) === JSON.stringify(command) && sent[i][1] === 3, i));
    "#);
}

#[test]
fn default_keybindings_emit_stable_kdl_and_dispatch_reserved_pipes() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.check = (ok, label = '') => { if (!ok) throw new Error('SDK assertion failed ' + label); };
      globalThis.runs = []; globalThis.pipes = 0; globalThis.waited = false;
      globalThis.keybind = (name, source = 'Keybind') => ({source, name, payload: null, args: {}, is_private: true, pane_id: null});
      globalThis.refused = (fn, text) => { let message = null; try { fn(); } catch (error) { message = String(error); } check(message !== null && message.includes(text), text); };
      bind({
        load() {
          zellij.waitForPipe('__zellij_default_keybinding:1').then(() => waited = true);
          zellij.registerDefaultKeybindings([
            {mode: 'normal', key: 'Ctrl g', handler(message) { runs.push([message.name, zellij.getPluginIds().client_id]); return true; }},
            {mode: 'normal', key: 'Ctrl n', actions: [zellij.action('NewPane')]},
            {mode: 'locked', key: 'Alt q', actions: ['ToggleTab', zellij.action('SwitchToMode', 'normal'),
              zellij.action('Run', {args: ['sh'], props: {cwd: '/tmp'}}), zellij.action('MoveFocus', 'Left')]},
            {mode: 'pane', key: 'x', async handler() { await Promise.resolve(); runs.push(['async', zellij.getPluginIds().client_id]); return true; }},
          ]);
        },
        pipe() { pipes++; },
      });
      bridge('load', {}, 1);
      check(bridge('status') === true);
      bridge('pipe', keybind('__zellij_default_keybinding:1'), 2);
      check(bridge('status') === true);
      bridge('pipe', keybind('__zellij_default_keybinding:1', {Plugin: 7}), 2);
      bridge('pipe', keybind('__zellij_default_keybinding:9'), 2);
      bridge('pipe', keybind('other'), 2);
      check(bridge('status') === false);
      check(JSON.stringify(runs) === JSON.stringify([['__zellij_default_keybinding:1', 2]]));
      check(pipes === 1 && !waited);
      bridge('pipe', keybind('__zellij_default_keybinding:4'), 3);
      check(bridge('status') === false);
    "#);
    run(&mut runtime, r#"
      check(bridge('status') === true, 'async status');
      check(JSON.stringify(runs) === JSON.stringify([['__zellij_default_keybinding:1', 2], ['async', 3]]), JSON.stringify(runs));
      for (const [binding, text] of [
        [{mode: 'normal', key: 'a'}, 'default keybinding 2 needs exactly one of actions or handler'],
        [{mode: 'normal', key: 'a', actions: [], handler() {}}, 'exactly one'],
        [{key: 'a', actions: []}, 'needs a mode'],
        [{mode: 'normal', actions: []}, 'needs a key'],
        [{mode: 'normal', key: 'a', actions: [], extra: 1}, 'unknown default keybinding field: extra'],
        [{mode: 'normal', key: 'a', actions: [{args: []}]}, 'an action needs a name'],
        [{mode: 'normal', key: 'a', actions: [zellij.action('X', {props: {a: null}})]}, 'KDL values'],
      ]) bind({load() { refused(() => zellij.registerDefaultKeybindings([{mode: 'normal', key: 'b', actions: []}, binding]), text); }}), bridge('load', {}, 1);
      bind({load() { refused(() => zellij.registerDefaultKeybindings({}), 'must be an array'); }}); bridge('load', {}, 1);
      refused(() => zellij.action('', 1), 'an action needs a name');
      refused(() => zellij.action('X', {args: [], typo: 1}), 'unknown action field: typo');
      // A refused registration keeps the previous handlers.
      bridge('pipe', keybind('__zellij_default_keybinding:1'), 5);
      check(runs.length === 3 && runs[2][1] === 5);
      // Escaping follows the Deno SDK: backslash, quote, newline, return and tab only.
      bind({load() { zellij.registerDefaultKeybindings([{mode: 'normal', key: 'a "b"', actions: [
        zellij.action('Run', {args: ['sh', 'new\nline\t\\'], props: {'odd key': true, n: 2}, children: {floating: true, name: 'a "b"'}}),
        zellij.action('X', {direction: 'Left'}),
      ]}]); }});
      bridge('load', {}, 1);
      bind({load() { zellij.registerDefaultKeybindings([]); }});
      bridge('load', {}, 1);
      bridge('pipe', keybind('__zellij_default_keybinding:1'), 6);
      check(runs.length === 3);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.registrations = sent.filter(([c]) => c.RegisterDefaultKeybindings).map(([c]) => c.RegisterDefaultKeybindings);
      check(registrations.length === 3 && registrations[2] === 'keybinds clear-defaults=true { }', JSON.stringify(registrations));
      check(registrations[1] === 'keybinds clear-defaults=true { normal { bind "a \\"b\\"" { Run "sh" "new\\nline\\t\\\\" n=2 "odd key"=true { floating true; name "a \\"b\\""; }; X direction="Left"; }; }; }');
      globalThis.kdl = registrations[0];
    "#);
    let kdl: String = {
        let value = runtime
            .execute_script("kdl.js", "globalThis.kdl".to_owned())
            .unwrap();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, value);
        local.to_rust_string_lossy(scope)
    };
    assert_eq!(
        kdl,
        r#"keybinds clear-defaults=true { normal { bind "Ctrl g" { MessagePlugin { name "__zellij_default_keybinding:1"; }; }; }; normal { bind "Ctrl n" { NewPane; }; }; locked { bind "Alt q" { ToggleTab; SwitchToMode "normal"; Run "sh" cwd="/tmp"; MoveFocus "Left"; }; }; pane { bind "x" { MessagePlugin { name "__zellij_default_keybinding:4"; }; }; }; }"#
    );
    // What the host at register_default_keybindings parses: the string must be valid KDL and
    // name the reserved pipe the SDK dispatches from.
    use zellij_types::input::actions::Action;
    use zellij_types::input::keybinds::Keybinds;
    use zellij_types::input::options::Options;
    let parsed = zellij_config::kdl::keybinds::from_string(
        kdl,
        Keybinds::default(),
        &Options::default(),
    )
    .unwrap();
    let actions = parsed
        .0
        .values()
        .flat_map(|bindings| bindings.values())
        .flatten()
        .collect::<Vec<_>>();
    let mut pipes = actions
        .iter()
        .filter_map(|action| match action {
            Action::KeybindPipe { name: Some(name), .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    pipes.sort();
    assert_eq!(
        pipes,
        ["__zellij_default_keybinding:1", "__zellij_default_keybinding:4"]
    );
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::NewPane { .. })));
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::MoveFocus { .. })));
}

#[test]
fn a_failed_registration_keeps_the_previous_keybinding_handlers() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.runs = 0;
      bind({load() { zellij.registerDefaultKeybindings([{mode: 'normal', key: 'a', handler() { runs++; }}]); }});
      bridge('load', {}, 1);
    "#);
    runtime
        .op_state()
        .borrow_mut()
        .borrow_mut::<TestHost>()
        .fail_command = Some("RegisterDefaultKeybindings");
    run(&mut runtime, r#"
      let failed = false;
      bind({load() { try { zellij.registerDefaultKeybindings([{mode: 'normal', key: 'b', handler() { runs += 100; }}]); } catch (_) { failed = true; } }});
      bridge('load', {}, 1);
      check(failed);
      bridge('pipe', {source: 'Keybind', name: '__zellij_default_keybinding:1', payload: null, args: {}, is_private: true, pane_id: null}, 1);
      check(runs === 1);
    "#);
}

/// The events whose raw shape is positional get named fields; everything keeps `value` as the
/// payload the host delivered, so nothing a plugin already does with the raw shape changes.
#[test]
fn event_decoder_names_tuple_fields_decodes_bytes_and_leaves_raw_untouched() {
    use zellij_types::data::{FileMetadata, Mouse, PermissionStatus};
    use zellij_types::keys::{BareKey, KeyModifier, KeyWithModifier};
    let context = BTreeMap::from([("work".to_owned(), "one".to_owned())]);
    let events = json!({
        "result": Event::RunCommandResult(Some(0), b"out".to_vec(), vec![255], context.clone()),
        "exited": Event::CommandPaneExited(zellij_types::internal_ids::TerminalId::new(7), None, context.clone()),
        "web": Event::WebRequestResult(404, BTreeMap::from([("a".to_owned(), "b".to_owned())]), vec![1, 2], BTreeMap::new()),
        "message": Event::CustomMessage("name".into(), "payload".into()),
        "key": Event::Key(KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()),
        "esc": Event::Key(KeyWithModifier::new(BareKey::Esc)),
        "intercepted": Event::InterceptedKeyPress(KeyWithModifier::new(BareKey::F(2))),
        "mouse": Event::Mouse(Mouse::ScrollUp(2)),
        "click": Event::Mouse(Mouse::LeftClick(3, 4)),
        "created": Event::FileSystemCreate(vec![
            (std::path::PathBuf::from("/host/a.txt"), Some(FileMetadata { is_dir: false, is_file: true, is_symlink: false, len: 3 })),
            (std::path::PathBuf::from("/host/b"), None),
        ]),
        "changed": Event::PluginDataChanged {
            watch_id: zellij_types::internal_ids::DataWatchId::new(4),
            rows: zellij_types::data::DataRows { columns: vec!["id".into(), "text".into()], rows: vec![vec![zellij_types::data::DataValue::Int(1), zellij_types::data::DataValue::Text("t".into())]], truncated: false, next_cursor: None },
            truncated: false,
        },
        "timer": Event::Timer(zellij_types::units::FractionalSeconds::new(1.5)),
        "granted": Event::PermissionRequestResult(PermissionStatus::Granted),
        "input": Event::InputReceived,
    });
    let _ = KeyModifier::Ctrl;
    let mut runtime = sdk();
    run(&mut runtime, &format!(r#"
      const events = {events};
      const decode = (name) => zellij.event(events[name]);
      const before = JSON.stringify(events);
      const result = decode('result');
      check(result.type === 'RunCommandResult' && result.exitCode === 0 && result.context.work === 'one');
      check(result.stdout instanceof Uint8Array && zellij.decodeUtf8(result.stdout) === 'out' && result.stderr[0] === 255);
      check(Object.is(result.value, events.result.RunCommandResult) && Object.is(result.raw, events.result));
      const exited = decode('exited');
      check(exited.paneId === 7 && exited.exitCode === null && exited.context.work === 'one');
      const web = decode('web');
      check(web.status === 404 && web.headers.a === 'b' && web.body instanceof Uint8Array && web.body.length === 2 && Object.keys(web.context).length === 0);
      const message = decode('message');
      check(message.message === 'name' && message.payload === 'payload');
      check(decode('key').key === 't' && decode('key').value.key_modifiers.includes('Ctrl'));
      check(decode('esc').key === 'Esc' && decode('intercepted').key === 'F2' && decode('intercepted').type === 'InterceptedKeyPress');
      const mouse = decode('mouse');
      check(mouse.kind === 'ScrollUp' && mouse.lines === 2 && mouse.value.ScrollUp === 2);
      const click = decode('click');
      check(click.kind === 'LeftClick' && click.line === 3 && click.col === 4);
      const created = decode('created');
      check(created.files.length === 2 && created.files[0].path === '/host/a.txt' && created.files[0].metadata.len === 3 && created.files[1].metadata === null);
      check(Array.isArray(created.value) && Array.isArray(created.value[0]));
      const changed = decode('changed');
      check(changed.records.length === 1 && changed.records[0].id === 1 && changed.records[0].text === 't' && changed.value.watch_id === 4);
      const timer = decode('timer');
      check(timer.type === 'Timer' && timer.value === 1.5 && Object.keys(timer).sort().join() === 'raw,type,value');
      check(decode('granted').value === 'Granted');
      const input = decode('input');
      check(input.type === 'InputReceived' && input.value === undefined && input.raw === 'InputReceived');
      check(JSON.stringify(events) === before, 'decoding never rewrites the raw event');
      for (const bad of [{{}}, {{A: 1, B: 2}}, 42, null, [], {{RunCommandResult: {{}}}}]) {{
        let refused = false;
        try {{ zellij.event(bad); }} catch (error) {{ refused = error instanceof TypeError; }}
        check(refused, 'not an event: ' + JSON.stringify(bad));
      }}
    "#));
}

/// `onEvent` is `on` with the decoded event: it registers through `on`, so it runs where `on`
/// handlers run - after `update` and the pipe waiters - and its `true` requests a render.
#[test]
fn on_event_handlers_receive_decoded_events_after_update_and_pipe_waiters() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.order = [];
      bind({ update(event) { order.push('update:' + Object.keys(event)[0]); return false; } });
      bridge('load', {}, 1);
      const off = zellij.onEvent('CommandPaneExited', (event) => { order.push(`onEvent:${event.paneId}:${event.exitCode}:${event.context.work}`); return true; });
      zellij.on('CommandPaneExited', (payload) => { order.push('on:' + payload[0]); });
      check(typeof off === 'function');
      bridge('update', {CommandPaneExited: [7, 3, {work: 'one'}]}, 1);
      check(bridge('status') === true, 'a decoded handler returning true requests a render');
      check(order.join(' ') === 'update:CommandPaneExited onEvent:7:3:one on:7', order.join(' '));
      off();
      bridge('update', {CommandPaneExited: [8, null, {}]}, 1);
      check(order.join(' ') === 'update:CommandPaneExited onEvent:7:3:one on:7 update:CommandPaneExited on:8', order.join(' '));
      let refused = false;
      try { zellij.onEvent('Key', 'not a function'); } catch (error) { refused = error instanceof TypeError; }
      check(refused);
    "#);
}

/// The small conveniences the Deno SDK and the Wasm shim have: the revision guard that
/// means "must not exist", the focused tab and pane of an update payload, a fire-and-forget
/// command in a pane, and a named form of the raw web request that fills the required slots.
#[test]
fn small_helpers_match_the_prelude_and_the_shim() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.refused = (fn, text) => { let message = null; try { fn(); } catch (error) { message = String(error); } check(message !== null && message.includes(text), message); };
      const d = zellij.data;
      check(d.REV_ABSENT === 0 && Object.isFrozen(d));
      check(JSON.stringify(d.put('note', {text: 'new'}, {uid: 'a', expectRev: d.REV_ABSENT})) === JSON.stringify({Put: {entity: 'note', uid: 'a', values: [['text', {Text: 'new'}]], expect_rev: 0}}));
      const tabs = [{position: 0, name: 'one', active: false}, {position: 1, name: 'two', active: true}];
      check(zellij.getFocusedTab(tabs).name === 'two' && zellij.getFocusedTab([]) === null && zellij.getFocusedTab([tabs[0]]) === null);
      refused(() => zellij.getFocusedTab({}), 'TabUpdate');
      const manifest = {panes: {'0': [{id: 3, is_plugin: true, is_focused: true, title: 'plugin'}, {id: 4, is_plugin: false, is_focused: true, title: 'shell'}], '1': [{id: 5, is_plugin: false, is_focused: false, title: 'idle'}]}};
      check(zellij.getFocusedPane(0, manifest).id === 4, 'a focused plugin pane is skipped');
      check(zellij.getFocusedPane(1, manifest) === null && zellij.getFocusedPane(2, manifest) === null);
      refused(() => zellij.getFocusedPane(-1, manifest), 'tab position');
      refused(() => zellij.getFocusedPane(0, null), 'must be an object');
      bind({load() {
        zellij.runCommandInPane(3, {path: 'sh', args: ['-c', 'true']});
        zellij.runCommandInPane({Terminal: 4}, {path: 'ls'}, {context: {work: 'one'}, keepScrollback: true});
        refused(() => zellij.runCommandInPane({Plugin: 3}, {path: 'ls'}), 'terminal ID');
        refused(() => zellij.runCommandInPane(3, {path: 'ls'}, {keepScrollback: 'yes'}), 'keepScrollback');
        zellij.host.webRequest({url: 'http://localhost/x'});
        zellij.host.webRequest({url: 'http://localhost/y', verb: 'Post', headers: {a: 'b'}, body: 'hé', context: {work: 'one'}, streamId: 's1'});
        zellij.host.webRequest({url: 'http://localhost/z', body: new Uint8Array([1, 2])});
        refused(() => zellij.host.webRequest({url: 'http://localhost/w', method: 'Get'}), 'unknown WebRequest argument: method');
      }});
      bridge('load', {}, 1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const expected = [
        {RunCommandInPane: {pane_id: {Terminal: 3}, command: {path: 'sh', args: ['-c', 'true']}, context: {}, keep_scrollback: false}},
        {RunCommandInPane: {pane_id: {Terminal: 4}, command: {path: 'ls'}, context: {work: 'one'}, keep_scrollback: true}},
        {WebRequest: ['http://localhost/x', 'Get', {}, [], {}, null, null]},
        {WebRequest: ['http://localhost/y', 'Post', {a: 'b'}, [104, 195, 169], {work: 'one'}, 's1', null]},
        {WebRequest: ['http://localhost/z', 'Get', {}, [1, 2], {}, null, null]},
      ];
      const commands = sent.map(([command]) => command).filter((command) => Object.hasOwn(command, 'RunCommandInPane') || Object.hasOwn(command, 'WebRequest'));
      check(JSON.stringify(commands) === JSON.stringify(expected), JSON.stringify(commands));
    "#);
}
