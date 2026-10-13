#[test]
fn pane_reply_exposes_restart_error() {
    use prost::Message;
    use yinz_plugin_api::plugin_command::ProtobufGetPaneInfoResponse;
    use yinz_types::data::{CommandType, PaneInfo};
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
    use yinz_types::data::CommandType;
    let mut expected = CommandType::iter()
        // Private bridge transport has no public command helper.
        .filter(|kind| *kind != CommandType::CommandPalette)
        .map(|name| camel(&format!("{name:?}")))
        .collect::<Vec<_>>();
    expected.sort();
    let mut runtime = sdk();
    run(&mut runtime, &format!(r#"
      const expected = {};
      check(JSON.stringify(Object.keys(yinz.host).sort()) === JSON.stringify(expected));
      check(Object.isFrozen(yinz.host) && Object.isFrozen(yinz));
      check(Object.values(yinz.host).every((member) => typeof member === 'function'));
      let refused = false;
      try {{ yinz.host.getPluginIds(); }} catch (error) {{ refused = String(error).includes('module initialization'); }}
      check(refused);
    "#, json!(expected)));
}

#[test]
fn host_sugar_builds_unit_positional_array_and_named_payloads() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.refused = (fn, text) => { let message = null; try { fn(); } catch (error) { message = String(error); } check(message !== null && message.includes(text)); };
      bind({load() {
        check(yinz.host.getPluginIds().client_id === 5);
        yinz.host.switchTabTo(2);
        yinz.host.setProjectContext(["alpha"], [[{Terminal: 7}, "alpha"]]);
        yinz.host.renameTab(0, 'logs');
        yinz.host.renameTab([0, 'logs']);
        yinz.host.renameTab({tabIndex: 0, name: 'logs'});
        yinz.host.renameTabWithId({tabId: 4, name: 'four'});
        yinz.host.renameTerminalPane({paneId: 1, name: 'one'});
        yinz.host.renamePluginPane({paneId: 2, name: 'two'});
        yinz.host.focusTerminalPane({paneId: 7});
        yinz.host.focusPluginPane({paneId: 8, shouldFloatIfHidden: true, shouldBeInPlaceIfHidden: false});
        yinz.host.showPaneWithId({paneId: {Terminal: 7}, shouldFloatIfHidden: false, shouldFocusPane: true});
        yinz.host.setFloatingPanePinned({paneId: {Plugin: 3}, shouldBePinned: true});
        yinz.host.setPaneBorderless({paneId: {Terminal: 3}, borderless: false});
        yinz.host.revealPaneWithId({paneId: {Terminal: 3}});
        yinz.host.notify({message: 'hi'});
        yinz.host.notify({message: 'hi', paneId: {Terminal: 1}, click: {message: 'm', payload: 'p'}});
        yinz.host.cliPipeOutput({pipeName: 'p', output: 'o'});
        yinz.host.reconfigure({configuration: 'keybinds {}', writeToDisk: false});
        yinz.host.runCommand({command: ['true'], envVariables: {}, cwd: '/host', context: {}});
        yinz.host.runCommand([['true'], {}, '/host', {}, null]);
        yinz.host.openCommandPaneInGitCheckoutNearPlugin({command: {path: 'sh'}, sourceDirectory: '/src', checkout: 'Current'});
        yinz.host.write(new Uint8Array([1, 2]));
        refused(() => yinz.host.renameTab({tab_index: 0, name: 'x'}), 'unknown RenameTab argument: tab_index');
        refused(() => yinz.host.renameTab(new Proxy({tabIndex: 0, name: 'x'}, {})), 'proxies');
        refused(() => yinz.host.renameTab({get tabIndex() { throw Error('accessed SECRET'); }, name: 'x'}), 'accessor');
        yinz.waitForEvent('Key');
        yinz.host.unsubscribe(['Key', 'Timer']);
        check(yinz.host.requestPluginPermissions(['ReadFiles']) === null);
      }});
      bridge('load', {}, 5);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const expected = [
        'GetPluginIds', {SwitchTabTo: 2}, {SetProjectContext: [['alpha'], [[{Terminal: 7}, 'alpha']]]}, {RenameTab: [0, 'logs']}, {RenameTab: [0, 'logs']}, {RenameTab: [0, 'logs']},
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
fn post_message_to_dispatches_in_every_form_and_workers_serialize_async_jobs() {
    use background_worker::{post_job, Message, PostReservation, POST_BUDGET, POST_OVERHEAD};
    use std::sync::{atomic::AtomicUsize, mpsc};

    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load() {
        yinz.command('PostMessageTo', {name:'x', payload:'y', worker_name:'w'});
        yinz.command({PostMessageTo:{name:'x', payload:'y', worker_name:'w'}});
        yinz.host.postMessageTo({name:'x', payload:'y', worker_name:'w'});
        yinz.postMessageTo('w', 'x', 'y');
        yinz.postMessageToPlugin('a', 'b');
      }});
      bridge('load', {}, 1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.length === 5 && sent.slice(0,4).every(([c]) => c.PostMessageTo.worker_name === 'w'));
      check(sent[4][0].PostMessageToPlugin.payload === 'b');
      check(typeof Deno === 'undefined' && !('workerStarted' in yinz) && !('op_yinz_worker_started' in globalThis));
      globalThis.started = [];
      globalThis.completed = [];
      bind({workers:{async w(name, payload) {
        started.push([name, payload, yinz.getPluginIds().client_id]);
        if (name === 'error') throw Error('worker callback failure');
        await yinz.waitForEvent('Timer');
        completed.push([name, payload, yinz.getPluginIds().client_id]);
      }}, load() {throw Error('worker ran load');}});
      check(bridge('workers').join() === 'w');
      bridge('workerBind', 'w');
    "#);
    let (sender, receiver) = mpsc::channel();
    let used = Arc::new(AtomicUsize::new(0));
    let post = |name: &str, payload: &str, client| {
        // Reserve half the budget without copying a large string into V8.
        let mut retained = String::with_capacity(POST_BUDGET / 2 - POST_OVERHEAD - name.len());
        retained.push_str(payload);
        post_job(&sender, &used, ClientId::new(client), name.into(), retained)
    };
    let receive = |runtime: &mut JsRuntime| {
        let Message::Job(client, name, payload, reservation) = receiver.recv().unwrap() else {
            panic!("expected worker job");
        };
        runtime.op_state().borrow_mut()
            .borrow_mut::<VecDeque<PostReservation>>().push_back(reservation);
        run(runtime, &format!("bridge('workerMessage', {}, {client});", json!([name, payload])));
    };
    post("one", "first", 2).unwrap();
    receive(&mut runtime);
    assert_eq!(used.load(Ordering::Relaxed), 0); // Executing callbacks do not count.
    post("error", "", 3).unwrap();
    receive(&mut runtime);
    assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET / 2);
    post("two", "second", 4).unwrap();
    receive(&mut runtime);
    assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
    assert!(post("refused", "", 5).unwrap_err().to_string().contains("64 MiB queue budget"));
    run(&mut runtime, "check(completed.length === 0); bridge('update', {Timer:0}, 9);");
    assert_eq!(used.load(Ordering::Relaxed), 0);
    run(&mut runtime, r#"
      check(JSON.stringify(started) === '[["one","first",2],["error","",3],["two","second",4]]');
      check(JSON.stringify(completed) === '[["one","first",2]]');
      bridge('update', {Timer:0}, 9);
    "#);
    run(&mut runtime, r#"check(JSON.stringify(completed) === '[["one","first",2],["two","second",4]]');"#);
    post("blocker", "", 5).unwrap();
    receive(&mut runtime);
    post("pending", "", 6).unwrap();
    receive(&mut runtime);
    post("also-pending", "", 7).unwrap();
    receive(&mut runtime);
    assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
    drop(runtime);
    assert_eq!(used.load(Ordering::Relaxed), 0);
}

#[test]
fn helpers_encode_writes_spell_pane_keys_and_normalize_keys_mouse_and_bytes() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.refused = (fn) => { let rejected = false; try { fn(); } catch (error) { rejected = error instanceof TypeError; } check(rejected); };
      bind({load() {
        yinz.write('hi'); yinz.write(new Uint8Array([1, 2]));
        yinz.writeToPaneId('a', {Terminal: 7});
        yinz.readPluginLog('x.ts'); yinz.readPluginLog('x.ts', 20);
        yinz.openTerminal(); yinz.openTerminal('.'); yinz.openTerminal({cwd: '/tmp'}); yinz.openTerminal({path: '/var'});
        yinz.closeSelf(); yinz.setSelectable(false);
        refused(() => yinz.setSelectable('no'));
        refused(() => yinz.readPluginLog('x', -1));
      }});
      bridge('load', {}, 3);
      check(yinz.keyName('Esc') === 'Esc' && yinz.keyName({Char: 'x'}) === 'x' && yinz.keyName({F: 5}) === 'F5');
      refused(() => yinz.keyName(1)); refused(() => yinz.keyName({Char: 'x', F: 1}));
      check(yinz.paneKey({Terminal: 3}) === 'terminal_3' && yinz.paneKey({Plugin: 4}) === 'plugin_4' && yinz.paneKey(9) === 'terminal_9');
      refused(() => yinz.paneKey('x')); refused(() => yinz.paneKey({Other: 1})); refused(() => yinz.paneKey({Terminal: -1}));
      check(JSON.stringify(yinz.terminalPane(3)) === '{"Terminal":3}' && JSON.stringify(yinz.pluginPane(4)) === '{"Plugin":4}');
      refused(() => yinz.terminalPane(-1)); refused(() => yinz.pluginPane(4294967296));
      const raw = {LeftClick: [3, 4]};
      check(JSON.stringify(yinz.mouse(raw)) === '{"kind":"LeftClick","line":3,"col":4}');
      check(JSON.stringify(yinz.mouse({Mouse: {ScrollUp: 2}})) === '{"kind":"ScrollUp","lines":2}');
      check(JSON.stringify(raw) === '{"LeftClick":[3,4]}');
      refused(() => yinz.mouse({LeftClick: [1]})); refused(() => yinz.mouse('LeftClick')); refused(() => yinz.mouse({}));
      const existing = new Uint8Array([7]);
      check(Object.is(yinz.bytes(existing), existing));
      check(yinz.bytes([104, 105]) instanceof Uint8Array && yinz.decodeUtf8(yinz.bytes([104, 105])) === 'hi');
      check(yinz.decodeUtf8(yinz.bytes('日')) === '日');
      refused(() => yinz.bytes([256])); refused(() => yinz.bytes([1.5])); refused(() => yinz.bytes(null));
      globalThis.sanitized = yinz.text.sanitizeBytes([0x1b, 0x5b, 0x41, 0xff, 0x68, 0x69]);
      check(yinz.text.sanitizeBytes('a\x1bb') === yinz.text.sanitize('a\x1bb'));
      refused(() => yinz.text.sanitizeBytes(null));
    "#);
    let expected = yinz_tile::text::sanitize(&String::from_utf8_lossy(&[
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
          yinz.waitForPipe('__yinz_default_keybinding:1').then(() => waited = true);
          yinz.registerDefaultKeybindings([
            {mode: 'normal', key: 'Ctrl g', handler(message) { runs.push([message.name, yinz.getPluginIds().client_id]); return true; }},
            {mode: 'normal', key: 'Ctrl n', actions: [yinz.action('NewPane')]},
            {mode: 'locked', key: 'Alt q', actions: ['ToggleTab', yinz.action('SwitchToMode', 'normal'),
              yinz.action('Run', {args: ['sh'], props: {cwd: '/tmp'}}), yinz.action('MoveFocus', 'Left')]},
            {mode: 'pane', key: 'x', async handler() { await Promise.resolve(); runs.push(['async', yinz.getPluginIds().client_id]); return true; }},
          ]);
        },
        pipe() { pipes++; },
      });
      bridge('load', {}, 1);
      check(bridge('status') === true);
      bridge('pipe', keybind('__yinz_default_keybinding:1'), 2);
      check(bridge('status') === true);
      bridge('pipe', keybind('__yinz_default_keybinding:1', {Plugin: 7}), 2);
      bridge('pipe', keybind('__yinz_default_keybinding:9'), 2);
      bridge('pipe', keybind('other'), 2);
      check(bridge('status') === false);
      check(JSON.stringify(runs) === JSON.stringify([['__yinz_default_keybinding:1', 2]]));
      check(pipes === 1 && !waited);
      bridge('pipe', keybind('__yinz_default_keybinding:4'), 3);
      check(bridge('status') === false);
    "#);
    run(&mut runtime, r#"
      check(bridge('status') === true, 'async status');
      check(JSON.stringify(runs) === JSON.stringify([['__yinz_default_keybinding:1', 2], ['async', 3]]), JSON.stringify(runs));
      for (const [binding, text] of [
        [{mode: 'normal', key: 'a'}, 'default keybinding 2 needs exactly one of actions or handler'],
        [{mode: 'normal', key: 'a', actions: [], handler() {}}, 'exactly one'],
        [{key: 'a', actions: []}, 'needs a mode'],
        [{mode: 'normal', actions: []}, 'needs a key'],
        [{mode: 'normal', key: 'a', actions: [], extra: 1}, 'unknown default keybinding field: extra'],
        [{mode: 'normal', key: 'a', actions: [{args: []}]}, 'an action needs a name'],
        [{mode: 'normal', key: 'a', actions: [yinz.action('X', {props: {a: null}})]}, 'KDL values'],
      ]) bind({load() { refused(() => yinz.registerDefaultKeybindings([{mode: 'normal', key: 'b', actions: []}, binding]), text); }}), bridge('load', {}, 1);
      bind({load() { refused(() => yinz.registerDefaultKeybindings({}), 'must be an array'); }}); bridge('load', {}, 1);
      refused(() => yinz.action('', 1), 'an action needs a name');
      refused(() => yinz.action('X', {args: [], typo: 1}), 'unknown action field: typo');
      // A refused registration keeps the previous handlers.
      bridge('pipe', keybind('__yinz_default_keybinding:1'), 5);
      check(runs.length === 3 && runs[2][1] === 5);
      // Escaping follows the Deno SDK: backslash, quote, newline, return and tab only.
      bind({load() { yinz.registerDefaultKeybindings([{mode: 'normal', key: 'a "b"', actions: [
        yinz.action('Run', {args: ['sh', 'new\nline\t\\'], props: {'odd key': true, n: 2}, children: {floating: true, name: 'a "b"'}}),
        yinz.action('X', {direction: 'Left'}),
      ]}]); }});
      bridge('load', {}, 1);
      bind({load() { yinz.registerDefaultKeybindings([]); }});
      bridge('load', {}, 1);
      bridge('pipe', keybind('__yinz_default_keybinding:1'), 6);
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
        r#"keybinds clear-defaults=true { normal { bind "Ctrl g" { MessagePlugin { name "__yinz_default_keybinding:1"; }; }; }; normal { bind "Ctrl n" { NewPane; }; }; locked { bind "Alt q" { ToggleTab; SwitchToMode "normal"; Run "sh" cwd="/tmp"; MoveFocus "Left"; }; }; pane { bind "x" { MessagePlugin { name "__yinz_default_keybinding:4"; }; }; }; }"#
    );
    // What the host at register_default_keybindings parses: the string must be valid KDL and
    // name the reserved pipe the SDK dispatches from.
    use yinz_types::input::actions::Action;
    use yinz_types::input::keybinds::Keybinds;
    use yinz_types::input::options::Options;
    let parsed = yinz_config::kdl::keybinds::from_string(
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
        ["__yinz_default_keybinding:1", "__yinz_default_keybinding:4"]
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
      bind({load() { yinz.registerDefaultKeybindings([{mode: 'normal', key: 'a', handler() { runs++; }}]); }});
      bridge('load', {}, 1);
    "#);
    runtime
        .op_state()
        .borrow_mut()
        .borrow_mut::<TestHost>()
        .fail_command = Some("RegisterDefaultKeybindings");
    run(&mut runtime, r#"
      let failed = false;
      bind({load() { try { yinz.registerDefaultKeybindings([{mode: 'normal', key: 'b', handler() { runs += 100; }}]); } catch (_) { failed = true; } }});
      bridge('load', {}, 1);
      check(failed);
      bridge('pipe', {source: 'Keybind', name: '__yinz_default_keybinding:1', payload: null, args: {}, is_private: true, pane_id: null}, 1);
      check(runs === 1);
    "#);
}

/// The events whose raw shape is positional get named fields; everything keeps `value` as the
/// payload the host delivered, so nothing a plugin already does with the raw shape changes.
#[test]
fn event_decoder_names_tuple_fields_decodes_bytes_and_leaves_raw_untouched() {
    use yinz_types::data::{FileMetadata, Mouse, PermissionStatus};
    use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
    let context = BTreeMap::from([("work".to_owned(), "one".to_owned())]);
    let events = json!({
        "result": Event::RunCommandResult(Some(0), b"out".to_vec(), vec![255], context.clone()),
        "exited": Event::CommandPaneExited(yinz_types::internal_ids::TerminalId::new(7), None, context.clone()),
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
            watch_id: yinz_types::internal_ids::DataWatchId::new(4),
            rows: yinz_types::data::DataRows { columns: vec!["id".into(), "text".into()], rows: vec![vec![yinz_types::data::DataValue::Int(1), yinz_types::data::DataValue::Text("t".into())]], truncated: false, next_cursor: None },
            truncated: false,
        },
        "timer": Event::Timer(yinz_types::units::FractionalSeconds::new(1.5)),
        "granted": Event::PermissionRequestResult(PermissionStatus::Granted),
        "input": Event::InputReceived,
    });
    let _ = KeyModifier::Ctrl;
    let mut runtime = sdk();
    run(&mut runtime, &format!(r#"
      const events = {events};
      const decode = (name) => yinz.event(events[name]);
      const before = JSON.stringify(events);
      const result = decode('result');
      check(result.type === 'RunCommandResult' && result.exitCode === 0 && result.context.work === 'one');
      check(result.stdout instanceof Uint8Array && yinz.decodeUtf8(result.stdout) === 'out' && result.stderr[0] === 255);
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
        try {{ yinz.event(bad); }} catch (error) {{ refused = error instanceof TypeError; }}
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
      const off = yinz.onEvent('CommandPaneExited', (event) => { order.push(`onEvent:${event.paneId}:${event.exitCode}:${event.context.work}`); return true; });
      yinz.on('CommandPaneExited', (payload) => { order.push('on:' + payload[0]); });
      check(typeof off === 'function');
      bridge('update', {CommandPaneExited: [7, 3, {work: 'one'}]}, 1);
      check(bridge('status') === true, 'a decoded handler returning true requests a render');
      check(order.join(' ') === 'update:CommandPaneExited onEvent:7:3:one on:7', order.join(' '));
      off();
      bridge('update', {CommandPaneExited: [8, null, {}]}, 1);
      check(order.join(' ') === 'update:CommandPaneExited onEvent:7:3:one on:7 update:CommandPaneExited on:8', order.join(' '));
      let refused = false;
      try { yinz.onEvent('Key', 'not a function'); } catch (error) { refused = error instanceof TypeError; }
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
      const d = yinz.data;
      check(d.REV_ABSENT === 0 && Object.isFrozen(d));
      check(JSON.stringify(d.put('note', {text: 'new'}, {uid: 'a', expectRev: d.REV_ABSENT})) === JSON.stringify({Put: {entity: 'note', uid: 'a', values: [['text', {Text: 'new'}]], expect_rev: 0}}));
      const tabs = [{position: 0, name: 'one', active: false}, {position: 1, name: 'two', active: true}];
      check(yinz.getFocusedTab(tabs).name === 'two' && yinz.getFocusedTab([]) === null && yinz.getFocusedTab([tabs[0]]) === null);
      refused(() => yinz.getFocusedTab({}), 'TabUpdate');
      const manifest = {panes: {'0': [{id: 3, is_plugin: true, is_focused: true, title: 'plugin'}, {id: 4, is_plugin: false, is_focused: true, title: 'shell'}], '1': [{id: 5, is_plugin: false, is_focused: false, title: 'idle'}]}};
      check(yinz.getFocusedPane(0, manifest).id === 4, 'a focused plugin pane is skipped');
      check(yinz.getFocusedPane(1, manifest) === null && yinz.getFocusedPane(2, manifest) === null);
      refused(() => yinz.getFocusedPane(-1, manifest), 'tab position');
      refused(() => yinz.getFocusedPane(0, null), 'must be an object');
      bind({load() {
        yinz.runCommandInPane(3, {path: 'sh', args: ['-c', 'true']});
        yinz.runCommandInPane({Terminal: 4}, {path: 'ls'}, {context: {work: 'one'}, keepScrollback: true});
        refused(() => yinz.runCommandInPane({Plugin: 3}, {path: 'ls'}), 'terminal ID');
        refused(() => yinz.runCommandInPane(3, {path: 'ls'}, {keepScrollback: 'yes'}), 'keepScrollback');
        yinz.host.webRequest({url: 'http://localhost/x'});
        yinz.host.webRequest({url: 'http://localhost/y', verb: 'Post', headers: {a: 'b'}, body: 'hé', context: {work: 'one'}, streamId: 's1'});
        yinz.host.webRequest({url: 'http://localhost/z', body: new Uint8Array([1, 2])});
        refused(() => yinz.host.webRequest({url: 'http://localhost/w', method: 'Get'}), 'unknown WebRequest argument: method');
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

#[test]
fn pane_tabs_sdk_helpers_use_the_shared_command_and_decode_the_shared_reply() {
    use prost::Message;
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load() {
        yinz.mountPaneTabs({Plugin:7}, {tabs:[{label:'Files',location:'yinz:strider',configuration:{cwd:'/work'}}]});
        yinz.selectPaneTab(1, 0);
        yinz.focusPaneTabs(1);
        yinz.retryPaneTab(1, 0);
      }}); bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const tabs=sent.map(([c])=>c.PaneTabs).filter(Boolean);
      check(tabs.length===4 && tabs[0].Mount.existing===0);
      check(tabs[0].Mount.layout.tabs[0].configuration.cwd==='/work');
      check(tabs[1].Select.index===0 && tabs[2].Focus.bar && tabs[3].Retry.id===1);
    "#);
    let reply = yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse { json: r#"{"Ok":null}"#.into() };
    assert!(matches!(crate::response::decode_response(yinz_types::data::CommandType::PaneTabs, &reply.encode_to_vec()).unwrap(), Ok(crate::response::DecodedResponse::Json(serde_json::Value::Null))));
    let denied = yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse { json: r#"{"Err":"Permission denied"}"#.into() };
    assert!(crate::response::decode_response(yinz_types::data::CommandType::PaneTabs, &denied.encode_to_vec()).unwrap().is_err());
}

#[test]
fn unload_runs_once_preserves_async_operations_and_suppresses_lifecycle_calls() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.calls = [];
      bind({
        load() {
          yinz.sleep(100000).catch(error => calls.push(error.name));
        },
        update() { throw Error('update after unload'); },
        pipe() { throw Error('pipe after unload'); },
        render() { throw Error('render after unload'); },
        async unload() {
          calls.push('start');
          const result = await yinz.runCommand({command:['true']});
          check(result.exitCode === 0);
          calls.push('done');
        }
      });
      bridge('load', {}, 7);
      check(bridge('unload', null, 7) === false);
      check(bridge('unload', null, 7) === false);
      bridge('update', 'Visible', 7);
      bridge('pipe', {name:'ignored'}, 7);
      check(bridge('render', [10,20], 7) === '');
      check(calls.join() === 'start');
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const command = sent.find(([cmd]) => cmd.RunCommand)[0].RunCommand;
      bridge('update', {RunCommandResult:[0,[],[],command[3]]}, 7);
    "#);
    run(&mut runtime, r#"
      check(calls.join() === 'start,done');
      check(bridge('unloaded', null) === true);
      check(bridge('unload', null, 7) === true);
      bridge('cancelPending', null, 7);
    "#);
    run(&mut runtime, "check(calls.join() === 'start,done,AbortError');");
}

#[test]
fn unload_accepts_absent_sync_and_rejected_hooks() {
    for body in ["{}", "{unload(){}}", "{unload(){throw Error('cleanup')}}", "{async unload(){throw Error('cleanup')}}"] {
        let mut runtime = sdk();
        run(&mut runtime, &format!("bind({body}); bridge('unload', null, 1);"));
        run(&mut runtime, "check(bridge('unloaded', null) === true);");
        let logs = runtime.op_state().borrow().borrow::<TestHost>().logs.clone();
        assert_eq!(logs.len(), usize::from(body.contains("throw")));
        if body.contains("throw") {
            assert!(logs[0].contains("unload failed: Error: cleanup"));
        }
    }
    let mut runtime = sdk();
    run(&mut runtime, "let rejected=false; try {bind({unload:42});} catch(e) {rejected=e.message==='unload must be a function';} check(rejected);");
}

#[test]
fn unload_deadline_interrupts_sync_loops_and_unsettled_promises() {
    for hook in ["unload(){while(true){}}", "async unload(){await new Promise(()=>{});}"] {
        let mut plugin = DenoPlugin::new(
            &format!("export default {{{hook}}};"),
            Path::new("/tmp/unload-deadline.js"),
            crate::test_env::test_env(),
            Arc::new(AtomicBool::new(false)),
        ).unwrap();
        let started = std::time::Instant::now();
        plugin.dispose();
        assert!(plugin.is_disposed());
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(10));
        plugin.dispose();
    }
}

#[test]
fn unload_awaits_a_command_completed_before_cleanup() {
    use yinz_bus::{background_jobs::BackgroundJob, plugins::PluginInstruction, screen::ScreenInstruction};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let (jobs, requests) = unbounded();
    let (notifications, queued) = unbounded();
    let (screen, observed) = unbounded();
    let mut env = crate::test_env::test_env();
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([
        yinz_types::data::PermissionType::RunCommands,
        yinz_types::data::PermissionType::ChangeApplicationState,
    ]));
    env.senders.to_background_jobs = Some(SenderWithContext::new(jobs));
    env.senders.to_plugin = Some(SenderWithContext::new(notifications));
    env.senders.to_screen = Some(SenderWithContext::new(screen));
    let senders = env.senders.clone();
    let mut plugin = DenoPlugin::new(
        r#"let pending;
        export default {
          load() { pending = yinz.runCommand({command: ['/usr/bin/true']}); },
          async unload() {
            const result = await pending;
            if (result.exitCode !== 0) throw Error('command failed');
            yinz.host.setSelectable(false);
          }
        };"#,
        Path::new("/tmp/unload-completed-command.js"), env,
        Arc::new(AtomicBool::new(false)),
    ).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let (plugin_id, client, generation, event) = loop {
        if let BackgroundJob::RunCommand(id, client, command, args, _, _, context, _, _, generation) = requests.recv_timeout(yinz_test_support::default_timeout()).unwrap().0 {
            let output = std::process::Command::new(command).args(args).output().unwrap();
            break (id, client, generation.unwrap(), Event::RunCommandResult(output.status.code(), output.stdout, output.stderr, context));
        }
    };
    senders.send_to_plugin(PluginInstruction::DenoEvent(plugin_id, client, generation, event)).unwrap();
    let reply = loop {
        if let PluginInstruction::DenoReply(_, _, _, Event::RunCommandResult(..), reply) = queued.recv_timeout(yinz_test_support::default_timeout()).unwrap().0 { break reply; }
    };
    // The command has exited and its notification is held here, outside the executor.
    plugin.dispose();
    assert!(observed.try_iter().any(|(instruction, _)| matches!(instruction, ScreenInstruction::SetSelectable(_, false))));
    assert!(reply.claim().is_none());
}

#[test]
fn unload_drains_an_undelivered_timer_before_a_new_cleanup_sleep() {
    use yinz_bus::{plugins::PluginInstruction, screen::ScreenInstruction};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let (notifications, queued) = unbounded();
    let (screen, observed) = unbounded();
    let mut env = crate::test_env::test_env();
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([
        yinz_types::data::PermissionType::ChangeApplicationState,
    ]));
    env.senders.to_plugin = Some(SenderWithContext::new(notifications));
    env.senders.to_screen = Some(SenderWithContext::new(screen));
    let mut plugin = DenoPlugin::new(
        r#"let pending;
        export default {
          load() { pending = yinz.sleep(0); },
          async unload() {
            await pending;
            await yinz.sleep(1);
            yinz.host.setSelectable(false);
          }
        };"#,
        Path::new("/tmp/unload-pending-timer.js"), env,
        Arc::new(AtomicBool::new(false)),
    ).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let reply = loop {
        if let PluginInstruction::DenoReply(_, _, _, Event::Timer(_), reply) = queued.recv_timeout(yinz_test_support::default_timeout()).unwrap().0 { break reply; }
    };
    // Channel receipt is the barrier: the first timer is retained but never delivered.
    plugin.dispose();
    assert!(observed.try_iter().any(|(instruction, _)| matches!(instruction, ScreenInstruction::SetSelectable(_, false))));
    assert!(reply.claim().is_none());
    assert!(!queued.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::DenoReply(..))));
}

#[test]
fn disconnect_target_reply_decodes_ids_and_errors() {
    use prost::Message;
    use yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse;
    use yinz_types::data::CommandType;
    let response = ProtobufGetDisconnectTargetsResponse { client_ids: vec![2, 3], error: None };
    let crate::response::DecodedResponse::Json(value) = crate::response::decode_response(CommandType::GetDisconnectTargets, &response.encode_to_vec()).unwrap().unwrap() else { panic!() };
    assert_eq!(value, serde_json::json!([2,3]));
    let response = ProtobufGetDisconnectTargetsResponse { client_ids: vec![], error: Some("timeout".into()) };
    assert!(crate::response::decode_response(CommandType::GetDisconnectTargets, &response.encode_to_vec()).unwrap().is_err());
}
