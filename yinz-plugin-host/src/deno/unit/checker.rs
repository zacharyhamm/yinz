use super::*;

#[test]
fn sdk_inventories_match_rust_commands_events_permissions_and_decoded_replies() {
    use strum::IntoEnumIterator;
    use yinz_types::data::{CommandType, EventType, PermissionType};
    let commands = CommandType::iter()
        // Private bridge transport has no public command helper.
        .filter(|kind| *kind != CommandType::CommandPalette)
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let events = EventType::iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let permissions = PermissionType::iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let replies = CommandType::iter()
        // Private bridge transport has no public command helper.
        .filter(|kind| *kind != CommandType::CommandPalette)
        .filter(|command| super::super::super::response::decode_response(*command, &[]).is_some())
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    // Enum Debug names are identifiers, so quote each union member for TS.
    let quoted = |names: String| {
        names
            .split(" | ")
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    // The host namespace lowercases the first character; TS Uncapitalize must agree with the SDK.
    let host = CommandType::iter()
        // Private bridge transport has no public command helper.
        .filter(|kind| *kind != CommandType::CommandPalette)
        .map(|name| {
            let name = format!("{name:?}");
            let mut chars = name.chars();
            let first = chars.next().unwrap().to_lowercase().collect::<String>();
            format!("{first}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let source = format!(
        r#"
      import type {{CommandArgs, CommandReplies, CommandResults, EventType, Permission, HostCommands}} from 'yinz:plugin';
      type Equal<A, B> = [A, B] extends [B, A] ? true : false;
      type Assert<T extends true> = T;
      type Commands = Assert<Equal<keyof CommandArgs, {}>>;
      type Results = Assert<Equal<keyof CommandResults, keyof CommandArgs>>;
      type Events = Assert<Equal<EventType, {}>>;
      type Permissions = Assert<Equal<Permission, {}>>;
      type Replies = Assert<Equal<keyof CommandReplies, {}>>;
      type Host = Assert<Equal<keyof HostCommands, {}>>;
    "#,
        quoted(commands),
        quoted(events),
        quoted(permissions),
        quoted(replies),
        quoted(host)
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inventory.ts");
    check_semantic_source(&path, &source).unwrap();
    // Prove that this check fails when a Rust inventory grows independently.
    let changed = source.replace(
        "keyof CommandArgs,",
        "keyof CommandArgs | 'UndeclaredHostCommand',",
    );
    assert!(check_semantic_source(&path, &changed).is_err());
    let changed = source.replace(
        "keyof HostCommands,",
        "keyof HostCommands | 'undeclaredHelper',",
    );
    assert!(check_semantic_source(&path, &changed).is_err());
}

#[test]
fn sdk_host_sugar_types_derive_from_command_args() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("host.ts"), r#"
      import yinz, {type PaneId, type PluginIds} from 'yinz:plugin';
      const ids: PluginIds = yinz.host.getPluginIds();
      const tab: number | null = yinz.host.newTab({name: 'x', cwd: null});
      const nothing: null = yinz.host.renameTab(0, 'logs');
      yinz.host.renameTab([0, 'logs']);
      yinz.host.renameTab({tabIndex: 0, name: 'logs'});
      yinz.host.focusTerminalPane(7, true, false);
      yinz.host.focusTerminalPane({paneId: 7, shouldFloatIfHidden: true, shouldBeInPlaceIfHidden: false});
      yinz.host.notify({message: 'hi'});
      yinz.host.notify('hi', null, null);
      yinz.host.runCommand([['true'], {}, '/host', {}, null]);
      yinz.host.runCommand(['true'], {}, '/host', {}, null, true);
      yinz.host.runCommand({command: ['true'], envVariables: {}, cwd: '/host', context: {}});
      const pane: PaneId | null = yinz.host.openTerminal({path: '.'});
      yinz.host.switchTabTo(2);
      yinz.host.write([1, 2]);
      yinz.host.write(new Uint8Array([1]));
      yinz.host.openCommandPaneInGitCheckoutNearPlugin({command: {path: 'sh'}, sourceDirectory: '/src', checkout: 'Current'});
      const log: string = yinz.host.readPluginLog({name: 'x', lines: 1});
      yinz.host.webRequest({url: 'http://x'});
      yinz.host.webRequest({url: 'http://x', verb: 'Post', body: 'text', headers: {}, streamId: null});
      yinz.host.webRequest({url: 'http://x', body: new Uint8Array([1])});
      // @ts-expect-error a web request needs a url
      yinz.host.webRequest({verb: 'Get'});
      // @ts-expect-error wrong tuple order
      yinz.host.renameTab('logs', 0);
      // @ts-expect-error unit commands take no payload
      yinz.host.getPluginIds({});
      // @ts-expect-error unknown member
      yinz.host.madeUp();
      // @ts-expect-error named arguments are camelCase
      yinz.host.renameTab({tab_index: 0, name: 'logs'});
      // @ts-expect-error required named field
      yinz.host.renameTab({tabIndex: 0});
      // @ts-expect-error byte arrays are one payload, not spread
      yinz.host.write(1, 2);
      yinz.host.postMessageTo({worker_name:'w', name: 'x', payload: 'y'});
      yinz.command('PostMessageTo', {worker_name:'w', name: 'x', payload: 'y'});
      yinz.command({PostMessageTo: {worker_name:'w', name: 'x', payload: 'y'}});
      // @ts-expect-error a worker name is required
      yinz.host.postMessageTo({name:'x'});
      yinz.postMessageTo('w', 'job');
      yinz.postMessageToPlugin('done', 'result');
      // @ts-expect-error worker payloads are strings
      yinz.postMessageTo('w', 'job', {});
      // @ts-expect-error the namespace is frozen
      yinz.host.renameTab = () => null;
    "#).unwrap();
}

#[test]
fn sdk_keybinding_and_helper_types_check_modes_actions_and_pane_keys() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("helpers.ts"), r#"
      import yinz, {type KeybindAction, type DefaultKeybinding, type PaneId, type MouseSummary, type TabInfo, type PaneInfo} from 'yinz:plugin';
      const run: KeybindAction = yinz.action('Run', 'sh', 'ls');
      const props: KeybindAction = yinz.action('MoveFocus', {direction: 'Left'});
      const full: KeybindAction = yinz.action('Run', {args: ['sh'], props: {cwd: '/tmp'}, children: {floating: true}});
      const bindings: DefaultKeybinding[] = [
        {mode: 'normal', key: 'Ctrl g', handler(message) { const name: string = message.name; return true; }},
        {mode: 'locked', key: 'Ctrl n', actions: ['ToggleTab', run, props, full]},
        {mode: 'shared', key: 'x', async handler() {}},
      ];
      yinz.registerDefaultKeybindings(bindings);
      yinz.on('PaneRenderReport', (report) => {
        const contents = report[yinz.paneKey({Terminal: 3})];
        const rows: string[] = contents.viewport;
      });
      yinz.on('Mouse', (payload, event) => {
        const summary: MouseSummary = yinz.mouse(payload);
        if (summary.kind === 'LeftClick') { const line: bigint | number = summary.line; }
        else if (summary.kind === 'ScrollUp') { const lines: bigint | number = summary.lines; }
        yinz.mouse(event as {Mouse: typeof payload});
      });
      yinz.on('Key', (key) => { const name: string = yinz.keyName(key.bare_key); });
      const terminal: {Terminal: number} = yinz.terminalPane(3);
      const pane: PaneId = yinz.pluginPane(4);
      yinz.write('hi'); yinz.write(new Uint8Array([1]));
      yinz.writeToPaneId('hi', terminal);
      const log: string = yinz.readPluginLog('x.ts');
      const opened: PaneId | null = yinz.openTerminal('.');
      yinz.openTerminal(); yinz.openTerminal({cwd: '/tmp'});
      yinz.closeSelf(); yinz.setSelectable(false);
      const bytes: Uint8Array = yinz.bytes([1, 2]);
      const text: string = yinz.text.sanitizeBytes([27, 91]);
      yinz.on('TabUpdate', (tabs) => { const active: TabInfo | null = yinz.getFocusedTab(tabs); });
      yinz.on('PaneUpdate', (manifest) => { const focused: PaneInfo | null = yinz.getFocusedPane(0, manifest); });
      yinz.runCommandInPane(3, {path: 'ls'}); yinz.runCommandInPane({Terminal: 3}, {path: 'ls'}, {context: {a: 'b'}, keepScrollback: true});
      const absent: 0 = yinz.data.REV_ABSENT;
      yinz.data.put('note', {}, {expectRev: yinz.data.REV_ABSENT});
      const rendered: string = yinz.ui.text('x').toString(); const inline: string = `${yinz.ui.text('x')}`;
      const input = yinz.ui.textInput({value:'日🙂'});
      input.setHistory(['earlier']).replaceText('draft',2).setCursorPosition(1).insertText('x');
      const cursor: number = input.cursorPosition(); const history: string[] = input.history();
      const undone: boolean = input.undo(); const redone: boolean = input.redo(); const draft: string = input.drainText();
      const intent: import('yinz:plugin').InputAction = input.handleInputKey({Key:{bare_key:'Enter',key_modifiers:[]}});
      const lines: {text:string;start:number}[] = yinz.text.wrapInput(draft,20);
      const matches: {index:number;score:number;indices:number[]}[] = yinz.text.fuzzyFilter(['日本 layout'],'lay');
      for (const line of yinz.ui.markdown('**hello**').lines(40)) {
        const content: string = line.content();
        line.selected().emphasis(2).strike().render({width:40,height:1});
      }
      // @ts-expect-error editor offsets are numeric character positions
      input.setCursorPosition('1');
      // @ts-expect-error checkbox controls do not expose editor state
      yinz.ui.checkbox().cursorPosition();
      yinz.on('Key', (key, event) => { yinz.ui.form([]).handleKey(key); yinz.ui.form([]).handleKey(event as {Key: typeof key}); });
      yinz.on('Mouse', (mouse, event) => { yinz.ui.form([]).handleMouse(mouse); yinz.ui.form([]).handleMouse(event as {Mouse: typeof mouse}); });
      // @ts-expect-error a plugin pane is not a command pane
      yinz.runCommandInPane({Plugin: 3}, {path: 'ls'});
      // @ts-expect-error the guard is a constant
      yinz.data.REV_ABSENT = 1;
      // @ts-expect-error modes are lowercase
      yinz.registerDefaultKeybindings([{mode: 'Normal', key: 'x', actions: []}]);
      // @ts-expect-error exactly one of actions or handler
      yinz.registerDefaultKeybindings([{mode: 'normal', key: 'x', actions: [], handler() {}}]);
      // @ts-expect-error one of actions or handler is required
      yinz.registerDefaultKeybindings([{mode: 'normal', key: 'x'}]);
      // @ts-expect-error action names are strings
      yinz.action(1);
      // @ts-expect-error KDL scalars only
      yinz.action('X', {a: null});
      // @ts-expect-error keys are BareKeys
      yinz.keyName(1);
      // @ts-expect-error pane keys need a PaneId
      yinz.paneKey('terminal_3');
      // @ts-expect-error selectable is a boolean
      yinz.setSelectable('no');
      // @ts-expect-error mouse takes the event payload
      yinz.mouse('LeftClick');
    "#).unwrap();
}

#[test]
fn sdk_raw_commands_reject_unknown_names_and_wrong_payloads() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("commands.ts"), r#"
      import yinz, {type Integer, type PaneId, type DataRows} from 'yinz:plugin';
      yinz.command('RunCommand', [['true'], {}, '/host', {}, null]);
      yinz.command({RunCommand: [['true'], {}, '/host', {}, 0, true]});
      yinz.command('RunCommand', [['true'], {}, '/host', {}, null, false]);
      yinz.command({RunCommand: [['true'], {}, '/host', {}, null]});
      yinz.command('SubscribePaneOutput', {work:'one'});
      yinz.command({SubscribePaneOutput: null});
      yinz.on('PaneOutput', p=>{const id:number=p.pane_id; const data:Uint8Array=p.data; const context:string=p.context.work;});
      yinz.on('RunCommandOutput', p=>{const stderr:boolean=p.stderr; const data:Uint8Array=p.data;});
      // @ts-expect-error streaming flag is boolean or null
      yinz.command('RunCommand', [['true'], {}, '/host', {}, null, 1]);
      // @ts-expect-error streaming flag is boolean or null in object form too
      yinz.command({RunCommand: [['true'], {}, '/host', {}, null, 'yes']});
      // @ts-expect-error context values are strings
      yinz.command('SubscribePaneOutput', {work:1});
      // @ts-expect-error live output arrives as a Uint8Array, not a JSON array
      yinz.on('PaneOutput', p=>{const bytes:number[]=p.data;});
      // @ts-expect-error command output has no pane ID
      yinz.on('RunCommandOutput', p=>p.pane_id);
      const pid: number = yinz.command('GetPluginIds').plugin_id;
      const objectPid: number = yinz.command({GetPluginIds: null}).plugin_id;
      const pane: PaneId | null = yinz.command('OpenFile', [{path: '/host/file'}, {}]);
      const bytes: Uint8Array = yinz.command({FsRead: {path: '/host/file', max_bytes: 9223372036854775807n}});
      const nothing: null = yinz.command('RenameTab', [0, 'renamed']);
      const job: Integer = yinz.command('EnqueueQueueJob', {queue:'q',plugin_url:'x',config:{},context:{}});
      yinz.command('RunAction', [{MoveFocus: {direction: 'Left'}}, {}]);
      // @ts-expect-error unknown command
      yinz.command('MadeUp');
      // @ts-expect-error unknown object command
      yinz.command({MadeUp: {}});
      // @ts-expect-error unknown keys beside a known command
      yinz.command({SetTimeout: 1, MadeUp: {}});
      // @ts-expect-error one command per request
      yinz.command({SetTimeout: 1, SwitchTabTo: 0});
      // @ts-expect-error required tuple payload
      yinz.command('RenameTab');
      // @ts-expect-error wrong tuple position
      yinz.command('RenameTab', ['name', 0]);
      // @ts-expect-error unit command has no payload
      yinz.command('GetPluginIds', {});
      // @ts-expect-error wrong nested action
      yinz.command('RunAction', [{MoveFocus: {direction: 'Diagonal'}}, {}]);
      // @ts-expect-error raw byte read does not return a string
      const text: string = yinz.command('FsRead', {path:'x', max_bytes:null});
      // @ts-expect-error real values cannot be bigint
      yinz.command('SetTimeout', 1n);
      const file: Uint8Array = yinz.fs.read('/host/file', {maxBytes: 8n});
      const range: Uint8Array = yinz.fs.read('/host/file', {maxBytes: 8n, offset: 9007199254740993n});
      yinz.fs.write('/host/file', file, {append:true, createDirs:true});
      const absent: import('yinz:plugin').FileMetadata | null = yinz.fs.metadata('/host/missing');
      yinz.projectConfig.write('.', 'agent_exit {}', null);
      const branch: string | null = yinz.projectConfig.read('.').config.default_branch;
      const paneInfo = yinz.command('GetPaneInfo', {Terminal: 1});
      if (paneInfo) { const restartError: string | null = paneInfo.restart_error; }
      const d = yinz.data;
      d.define('note', {text:'Text', score:{kind:'Real', default:d.real(1)}});
      d.apply([d.put('note', {text:'hello', bytes:new Uint8Array([255])}, {uid:'a', expectRev:1n})]);
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[]}}]);
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2], [3n, 4n], [5, 6n], [7n, 8]]}}]);
      yinz.command('DataApply', {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[]}}]});
      yinz.command({DataApply: {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2], [3n, 4n], [5, 6n], [7n, 8]]}}]}});
      // @ts-expect-error snapshot pairs require an ID and revision
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1]]}}]);
      // @ts-expect-error snapshot pairs cannot have extra elements
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2, 3]]}}]);
      // @ts-expect-error snapshot IDs cannot be text
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[['1', 2]]}}]);
      // @ts-expect-error snapshot revisions cannot be text
      yinz.command('DataApply', {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[[1, '2']]}}]});
      // @ts-expect-error snapshot assertions require a filter
      d.apply([{AssertRows: {entity:'note', rows:[]}}]);
      // @ts-expect-error raw snapshot assertions also require a filter
      yinz.command({DataApply: {namespace:null, mutations:[{AssertRows: {entity:'note', rows:[]}}]}});
      const rows = d.fetch({entity:'note', filter:d.and(d.eq('uid','a'),d.gt('score',0))});
      const raw: DataRows = rows.raw;
      for (const select of [undefined, ['uid', 'rev'], ['uid']]) {
        const record = d.fetch({entity:'note', select}).records[0];
        const rev: Integer | undefined = record.rev;
        d.put('note', {text:'edited'}, {uid:'a', expectRev:record.rev});
        d.update('note', d.eq('uid', 'a'), {text:'edited'}, {expectRev:record.rev});
        // @ts-expect-error projections can omit the revision
        const requiredRev: Integer = record.rev;
      }
      const projected: import('yinz:plugin').DataRecord = {uid:'a'};
      const revision: import('yinz:plugin').DataRecord = {rev:1n};
      // @ts-expect-error a record revision cannot be text
      const textRevision: import('yinz:plugin').DataRecord = {rev:'1'};
      // @ts-expect-error a record revision cannot be null
      const nullRevision: import('yinz:plugin').DataRecord = {rev:null};
      // @ts-expect-error a record revision cannot be boolean
      const boolRevision: import('yinz:plugin').DataRecord = {rev:true};
      // @ts-expect-error update revisions cannot be text
      d.update('note', d.eq('uid', 'a'), {}, {expectRev:'1'});
      const watch = d.watch({entity:'note'}, change => {const raw: DataRows = change.raw;});
      const watchId: Integer = watch.watchId;
      watch.dispose();
      // @ts-expect-error optimistic source guard is required
      yinz.projectConfig.write('.', 'agent_exit {}');
      // @ts-expect-error helpers use camelCase options
      yinz.fs.write('/host/file', 'text', {create_dirs:true});
      // @ts-expect-error file payloads are text or Uint8Array
      yinz.fs.write('/host/file', [1,2]);
      // @ts-expect-error revisions are integers, not strings
      d.put('note', {}, {expectRev:'1'});
      // @ts-expect-error actual JSON content cannot contain BigInt
      d.json({n:1n});
      // @ts-expect-error supported watch modes retain their raw names
      d.watch({entity:'note'}, () => {}, {mode:'changes'});
    "#).unwrap();
}

#[test]
fn shim_reply_readers_independently_require_decoder_entries() {
    use std::collections::{BTreeMap, BTreeSet};
    use yinz_types::data::CommandType;
    // The trailing native test module calls reply readers itself; it is not part of
    // the last shim's body and must not classify a fire-and-forget command as a reader.
    let source = include_str!("../../../../yinz-tile/src/shim.rs")
        .split("\n#[cfg(test)]\nmod tests {")
        .next()
        .unwrap();
    let comments = regex::Regex::new(r"(?m)//[^\n]*").unwrap();
    let source = comments.replace_all(source, "");
    let functions = regex::Regex::new(r"(?m)^(?:pub )?fn (\w+)").unwrap();
    let positions: Vec<_> = functions
        .captures_iter(&source)
        .map(|capture| (capture[1].to_owned(), capture.get(0).unwrap().start()))
        .collect();
    let bodies: BTreeMap<_, _> = positions
        .iter()
        .enumerate()
        .map(|(index, (name, start))| {
            (
                name.as_str(),
                &source[*start
                    ..positions
                        .get(index + 1)
                        .map_or(source.len(), |(_, start)| *start)],
            )
        })
        .collect();
    let mut readers = BTreeSet::from(["bytes_from_stdin", "object_from_stdin"]);
    loop {
        let previous = readers.len();
        for (name, body) in &bodies {
            if readers.iter().any(|reader| {
                body.contains(&format!("{reader}(")) || body.contains(&format!("{reader}::<"))
            }) {
                readers.insert(name);
            }
        }
        if readers.len() == previous {
            break;
        }
    }
    let commands = regex::Regex::new(r"PluginCommand::(\w+)").unwrap();
    let mut found = BTreeSet::new();
    for reader in readers {
        if let Some(body) = bodies.get(reader) {
            for capture in commands.captures_iter(body) {
                let name = &capture[1];
                let command: CommandType = name.parse().unwrap();
                assert!(
                    super::super::super::response::decode_response(command, &[]).is_some(),
                    "shim {reader} reads {name}, but its decoder is missing"
                );
                found.insert(name.to_owned());
            }
        }
    }
    assert!(
        found.len() >= 90,
        "shim analysis missed reply readers: {found:?}"
    );
}

#[test]
fn shipped_builtins_typecheck_offline() {
    for name in yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES {
        let source = yinz_deno_builtins::builtin_deno_plugin(name).unwrap();
        let loader = PluginModuleLoader::embedded(name, source).unwrap();
        check_loader(
            Path::new(name),
            loader,
            Limits::new("builtin checker".into()),
            false,
            true,
            false,
        )
        .unwrap();
    }
}

#[test]
fn pane_frame_styles_match_the_serde_wire_values() {
    use yinz_types::data::{Event, ModeInfo, PaneFrameStyle, PluginCommand};
    let dir = tempfile::tempdir().unwrap();
    let mut source = "import type * as SDK from 'yinz:plugin';\n".to_owned();
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let mode = Event::ModeUpdate(ModeInfo {
            pane_frame_style: Some(style),
            ..Default::default()
        });
        let command = PluginCommand::SetPaneFrameStyle(style);
        source.push_str(&format!(
            "({}) satisfies SDK.Event;\n({}) satisfies SDK.CommandObject;\n",
            serde_json::to_string(&mode).unwrap(),
            serde_json::to_string(&command).unwrap()
        ));
    }
    check_semantic_source(&dir.path().join("frame-styles.ts"), &source).unwrap();
}

#[test]
fn embedded_module_graphs_keep_types_and_reject_unregistered_dependencies() {
    for (source, valid) in [
        (
            "import hook from './agent-exit.ts'; hook.render?.(8,24);",
            true,
        ),
        (
            "import hook from './agent-exit.ts'; hook.render?.('bad',24);",
            false,
        ),
        ("import './not-embedded.ts';", false),
        ("import 'file:///tmp/other.ts';", false),
    ] {
        let loader = PluginModuleLoader::embedded("graph-test", source).unwrap();
        let result = check_loader(
            Path::new("graph-test.ts"),
            loader,
            Limits::new("embedded checker".into()),
            false,
            true,
            false,
        );
        assert_eq!(result.is_ok(), valid, "{source}: {:?}", result.err());
    }
}

#[test]
fn shipped_examples_typecheck_offline() {
    let mut checked = 0;
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins");
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        assert_ne!(path.extension().and_then(|s| s.to_str()), Some("lua"));
        if path.extension().and_then(|s| s.to_str()) == Some("ts") {
            check_plugin(&path).unwrap();
            checked += 1;
        }
    }
    assert!(
        checked >= 15,
        "only {checked} shipped examples were checked"
    );
}

#[test]
fn syntax_accepts_unsaved_typescript_and_declarations_but_not_jsx() {
    assert!(check_syntax(Path::new("unsaved.ts"), "const n: number = 1;").is_ok());
    assert!(check_syntax(Path::new("unsaved.d.ts"), "export declare const n: number;").is_ok());
    assert!(check_syntax(Path::new("unsaved.ts"), "const = ;").is_err());
    assert!(check_syntax(Path::new("unsaved.js"), "const n: number = 1;").is_err());
    assert!(check_syntax(Path::new("unsaved.tsx"), "const x = <div />;").is_err());
}

#[test]
fn checker_watchdog_interrupts_v8() {
    let mut runtime = JsRuntime::try_new(RuntimeOptions::default()).unwrap();
    let error = with_deadline(&mut runtime, Duration::from_millis(20), |runtime| {
        runtime
            .execute_script("yinz:watchdog-test", "while (true) {}")
            .context("interrupt test")?;
        Ok(())
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("time limit"));
}

#[test]
fn semantic_checks_use_es2022_without_executing_plugin_code() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    std::fs::write(
        dir.path().join("dep.mts"),
        "export const answer: number = 42;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("types.d.ts"),
        "export interface Answer { value: number }",
    )
    .unwrap();
    let source = "import { answer } from './dep.mts';\nimport type { Answer } from './types.d.ts';\nconst result: Answer = { value: answer };\nconst n: number | undefined = [1, 2].at(-1);\nconst m = new Map<string, number>();\nawait Promise.resolve(m);\nthrow new Error('MUST NOT EXECUTE');";
    check_semantic_source(&entry, source).unwrap();
    assert!(!entry.exists());
    std::fs::write(&entry, source).unwrap();
    check_plugin(&entry).unwrap();
    let error = check_semantic_source(&entry, "const n: number = 'wrong';").unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains("TS2322"), "{error}");
    assert!(error.contains("main.ts(1,"), "{error}");
    let error = check_semantic_source(&entry, "document.title = 'no DOM';").unwrap_err();
    assert!(format!("{error:#}").contains("Cannot find name 'document'"));
    let error = check_semantic_source(&entry, "const n: number = [1].toSorted()[0];").unwrap_err();
    assert!(format!("{error:#}").contains("toSorted"));
}

#[test]
fn semantic_checks_reject_external_imports_and_dependency_errors() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    std::fs::write(&entry, "export {};").unwrap();
    for specifier in [
        "react",
        "https://example.com/module.ts",
        "node:fs",
        "./missing.ts",
        "./missing",
    ] {
        let error = check_semantic_source(&entry, &format!("import '{specifier}';")).unwrap_err();
        assert!(format!("{error:#}").contains(specifier));
    }
    std::fs::write(
        dir.path().join("dep.ts"),
        "export const n: number = 'wrong';",
    )
    .unwrap();
    let error = check_semantic_source(&entry, "import {n} from './dep.ts'; n;").unwrap_err();
    assert!(format!("{error:#}").contains("dep.ts(1,"));
    let error =
        check_semantic_source(&entry, "/// <reference types=\"node\" />\nexport {};").unwrap_err();
    assert!(format!("{error:#}").contains("package type references"));
}

#[cfg(unix)]
#[test]
fn checker_cannot_read_through_symlinks_or_reference_paths() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    let declaration = outside.path().join("outside.d.ts");
    std::fs::write(&declaration, "export interface Secret {}").unwrap();
    std::os::unix::fs::symlink(&declaration, dir.path().join("escape.d.ts")).unwrap();
    for source in [
        "import type {Secret} from './escape.d.ts';",
        "/// <reference path=\"./escape.d.ts\" />\nexport {};",
    ] {
        let error = check_semantic_source(&entry, source).unwrap_err();
        assert!(format!("{error:#}").contains("escapes plugin directory"));
    }
}

#[test]
fn sdk_checks_imported_helpers_event_types_and_render_results() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    check_semantic_source(&entry, "import yinz, {definePlugin, type EventType} from 'yinz:plugin';\nconst event: EventType = 'PaneUpdate';\nyinz.subscribe([event]);\nexport default definePlugin({render: () => 'ok'});").unwrap();
    for (source, expected) in [
        ("import yinz from 'yinz:plugin'; yinz.subscribee(['PaneUpdate']);", "subscribee"),
        ("import type {EventType} from 'yinz:plugin'; const event: EventType = 'PaneUpdat';", "PaneUpdat"),
        ("import {definePlugin} from 'yinz:plugin'; export default definePlugin({render: () => 42});", "No overload"),
    ] {
        let error = check_semantic_source(&entry, source).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("main.ts(1,"), "{error}");
    }
}

#[test]
fn sdk_async_helpers_have_precise_options_results_and_cancellation_types() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("async.ts"), r#"
        import yinz, {definePlugin, type CancellablePromise, type RunCommandResult,
            type WebRequestResult, type PipeMessage} from 'yinz:plugin';
        export default definePlugin({defaults: {greeting: 'hello'}, async load(config) {
            const command: CancellablePromise<RunCommandResult> = yinz.runCommand({
                command: ['echo', config.greeting], cwd: '/host', envVariables: {LANG: 'C'},
                maxBytesReturned: 1024, context: {tag: 'one'},
                onOutput(output) {const bytes:Uint8Array=output.data; const stderr:boolean=output.stderr; const tag:string=output.context.tag;},
            });
            const web: CancellablePromise<WebRequestResult> = yinz.webRequest({
                url: 'https://example.com', verb: 'Post', body: yinz.encodeUtf8('hello'),
                headers: {'content-type': 'text/plain'}, proxy: 'http://localhost:8080', context: {}, streamId: 'stream',
            });
            const [result, response] = await Promise.all([command, web]);
            const exit: number | null = result.exitCode;
            const bytes: Uint8Array = result.stderr;
            const status: number = response.status;
            const text: string = yinz.decodeUtf8(response.body);
            const context: Record<string, string> = result.context;
            const pipe: CancellablePromise<PipeMessage> = yinz.waitForPipe('reply', msg => msg.payload !== null);
            const event = yinz.waitForEvent('RunCommandResult', ([code]) => code === 0);
            const raw: number[] = (await event)[1];
            const timer = yinz.every(100, async () => { await yinz.sleep(1); return true; });
            const once = yinz.after(0, () => {});
            for (const handle of [command, web, pipe, event, timer, once, yinz.sleep(1)]) handle.cancel();
            // @ts-expect-error command argv must be strings
            yinz.runCommand({command: [1]});
            // @ts-expect-error command is required
            yinz.runCommand({cwd: '/host'});
            // @ts-expect-error maximum is numeric
            yinz.runCommand({command: ['true'], maxBytesReturned: '1'});
            // @ts-expect-error context values must be strings
            yinz.runCommand({command: ['true'], context: {tag: 1}});
            // @ts-expect-error only host verbs are supported
            yinz.webRequest({url: 'x', verb: 'Head'});
            // @ts-expect-error only strings or Uint8Array bodies
            yinz.webRequest({url: 'x', body: [1]});
            // @ts-expect-error stream IDs must be strings
            yinz.webRequest({url: 'x', streamId: 1});
            // @ts-expect-error wrapper uses camelCase
            yinz.webRequest({url: 'x', stream_id: 'stream'});
            // @ts-expect-error exitCode can be null
            const code: number = result.exitCode;
            // @ts-expect-error helper output is bytes
            const output: string = result.stdout;
            // @ts-expect-error pipe names are strings
            yinz.waitForPipe(1);
            // @ts-expect-error predicates are synchronous
            yinz.waitForPipe('reply', async () => true);
            // @ts-expect-error callbacks only return boolean or void
            yinz.after(0, () => 'render');
            // @ts-expect-error chained promises have no cancel method
            command.then(result => result).cancel();
            // @ts-expect-error permission promises are not cancellable
            yinz.waitForPermissions([]).cancel();
            // @ts-expect-error callback must be callable
            yinz.runCommand({command:['true'], onOutput:true});
            // @ts-expect-error callback receives binary bytes
            yinz.runCommand({command:['true'], onOutput(output) {const text:string=output.data;}});
            // @ts-expect-error decoding requires bytes
            yinz.decodeUtf8('hello');
        }});
        // @ts-expect-error defaults must be strings
        definePlugin({defaults: {count: 1}});
    "#).unwrap();
}

#[test]
fn sdk_decoded_event_types_narrow_by_type_and_keep_raw_values() {
    use yinz_types::data::*;
    use yinz_types::keys::{BareKey, KeyWithModifier};
    let context = std::collections::BTreeMap::from([("work".to_owned(), "one".to_owned())]);
    let fixtures = [
        (
            "result",
            serde_json::to_value(Event::RunCommandResult(
                Some(0),
                b"out".to_vec(),
                vec![],
                context.clone(),
            ))
            .unwrap(),
        ),
        (
            "exited",
            serde_json::to_value(Event::CommandPaneExited(
                yinz_types::internal_ids::TerminalId::new(7),
                None,
                context,
            ))
            .unwrap(),
        ),
        (
            "key",
            serde_json::to_value(Event::Key(KeyWithModifier::new(BareKey::Char('t')))).unwrap(),
        ),
        (
            "mouse",
            serde_json::to_value(Event::Mouse(Mouse::LeftClick(3, 4))).unwrap(),
        ),
        ("input", serde_json::to_value(Event::InputReceived).unwrap()),
        (
            "reset",
            serde_json::to_value(Event::ClientGridReset(ClientId::new(42))).unwrap(),
        ),
    ];
    let mut source = String::from("import yinz, {type DecodedEvent, type DecodedEventOf, type HookPoint, type BareKey, type Event} from 'yinz:plugin';\n");
    for (name, value) in &fixtures {
        source.push_str(&format!("const {name} = {value} satisfies Event;\n"));
    }
    source.push_str(r#"
      const events: Event[] = [result, exited, key, mouse, input, reset];
      for (const raw of events) {
        const e: DecodedEvent = yinz.event(raw);
        const t: string = e.type; const r: Event = e.raw;
        if (e.type === 'RunCommandResult') { const c: number | null = e.exitCode; const out: Uint8Array = e.stdout; const w: string = e.context.work; const v: [number | null, number[], number[], Record<string, string>] = e.value; }
        if (e.type === 'CommandPaneExited') { const id: number = e.paneId; const c: number | null = e.exitCode; }
        if (e.type === 'Key') { const k: string = e.key; const b: BareKey = e.value.bare_key; }
        if (e.type === 'Mouse') { if (e.kind === 'LeftClick') { const line: number | bigint = e.line; } }
        if (e.type === 'ClientGridReset') { const clientId: number = e.value; }
        if (e.type === 'InputReceived') { const v: undefined = e.value; }
        if (e.type === 'Hook') { const p: HookPoint = e.value.point; }
        if (e.type === 'Timer') { const seconds: number = e.value; }
      }
      const one: DecodedEventOf<'CommandPaneExited'> = yinz.event(exited) as DecodedEventOf<'CommandPaneExited'>;
      const off: () => void = yinz.onEvent('Hook', (e) => { const p: HookPoint = e.value.point; return true; });
      yinz.onEvent('Key', async (e) => { const k: string = e.key; });
      yinz.onEvent('PluginDataChanged', (e) => { const first: Record<string, unknown> | undefined = e.records[0]; });
      yinz.onEvent('FileSystemUpdate', (e) => { const path: string = e.files[0].path; });
      // @ts-expect-error struct payloads are not spread onto the event
      yinz.onEvent('Hook', (e) => e.point);
      // @ts-expect-error stdout is a Uint8Array, not a string
      yinz.onEvent('RunCommandResult', (e) => { const s: string = e.stdout; });
      // @ts-expect-error not an event type
      yinz.onEvent('Nope', () => {});
      // @ts-expect-error a decoded event is not a raw one
      const raw: Event = yinz.event(exited);
    "#);
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("decoded.ts"), &source).unwrap();
}

/// The bundled TypeScript compiler is what `provenance.json` says it is: every vendored file
/// hashes to the recorded digest, and every `<reference lib>` a bundled declaration makes
/// resolves to a bundled declaration. A re-vendor that forgets a file, or an edit to one, fails
/// the gate here rather than being found when a plugin's types check against the wrong library.
#[test]
fn bundled_compiler_matches_its_provenance_hashes() {
    use sha2::{Digest, Sha256};
    let compiler = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/deno/compiler");
    let provenance: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(compiler.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(provenance["package"], "typescript");
    assert_eq!(
        provenance["version"], "5.9.3",
        "bump this with the vendored compiler"
    );
    let libs: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(compiler.join("libs.json")).unwrap())
            .unwrap();
    let files = provenance["files"].as_object().unwrap();
    assert!(files.len() >= 40, "{} files recorded", files.len());
    // Names are the npm package's: `lib/lib.<name>.d.ts` is a declaration bundled into
    // libs.json, and any other `lib/<file>` is vendored beside it.
    for (name, expected) in files {
        let source: Vec<u8> = match name.strip_prefix("lib/lib.") {
            Some(lib) => libs
                .get(&format!("lib.{lib}"))
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("{name} is not in libs.json"))
                .as_bytes()
                .to_vec(),
            None => std::fs::read(compiler.join(name.strip_prefix("lib/").unwrap_or(name)))
                .unwrap_or_else(|error| panic!("{name}: {error}")),
        };
        let digest: String = Sha256::digest(&source)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            &digest,
            expected.as_str().unwrap(),
            "{name} differs from its provenance"
        );
    }
    for (name, source) in &libs {
        assert!(
            files.contains_key(&format!("lib/{name}")),
            "{name} is bundled but not recorded"
        );
        for reference in source.as_str().unwrap().split("<reference lib=\"").skip(1) {
            let lib = reference.split('"').next().unwrap();
            assert!(
                libs.contains_key(&format!("lib.{lib}.d.ts")),
                "{name} references lib {lib}, which is not bundled"
            );
        }
    }
}

#[test]
fn serialized_rust_payloads_satisfy_the_handwritten_sdk() {
    use yinz_types::input::{
        actions::Action,
        git_plugin::{GitPlugin, GitRevision},
    };
    use yinz_types::internal_ids::*;
    use yinz_types::{authoring::*, data::*, input::layout::*};
    let fixture = |name: &str, ty: &str, value: serde_json::Value| {
        format!("const {name} = {value} satisfies SDK.{ty};\n")
    };
    let mut source = "import type * as SDK from 'yinz:plugin';\n".to_owned();
    macro_rules! fixture {
        ($name:literal, $ty:literal, $value:expr) => {
            source.push_str(&fixture($name, $ty, serde_json::to_value($value).unwrap()));
        };
    }
    fixture!(
        "stream_command",
        "CommandObject",
        PluginCommand::RunCommand(
            vec!["true".into()],
            Default::default(),
            "/host".into(),
            Default::default(),
            Some(4),
            Some(true)
        )
    );
    fixture!(
        "subscribe_output",
        "CommandObject",
        PluginCommand::SubscribePaneOutput(Some(std::collections::BTreeMap::from([(
            "work".into(),
            "one".into()
        )])))
    );
    // Live output is the one event whose runtime shape is not its serde shape: the host
    // delivers `data` as a Uint8Array (see `RuntimeInstance::call_event`), so the fixtures
    // spell the wire form the guest actually sees.
    source.push_str(
        "const pane_output = {PaneOutput: {pane_id: 7, data: new Uint8Array([0, 128, 255]), context: {}}} satisfies SDK.Event;\n",
    );
    source.push_str(
        "const command_output = {RunCommandOutput: {data: new Uint8Array([0, 128, 255]), stderr: true, context: {}}} satisfies SDK.Event;\n",
    );
    fixture!("mode", "Event", Event::ModeUpdate(ModeInfo::default()));
    fixture!("tabs", "Event", Event::TabUpdate(vec![TabInfo::default()]));
    fixture!("panes", "Event", Event::PaneUpdate(PaneManifest::default()));
    fixture!("pane", "PaneInfo", PaneInfo::default());
    fixture!(
        "pane_restart_refused",
        "PaneInfo",
        PaneInfo {
            restart_error: Some("Cannot restart agent".into()),
            ..Default::default()
        }
    );
    fixture!(
        "project_config",
        "ProjectConfig",
        yinz_config::project_config::parse("agent_exit {}").unwrap()
    );
    fixture!("style", "Style", yinz_types::style::Style::default());
    fixture!("layout", "TiledPaneLayout", TiledPaneLayout::default());
    for (name, revision) in [
        ("branch", Some(GitRevision::Branch("main".into()))),
        ("tag", Some(GitRevision::Tag("v1".into()))),
        ("commit", Some(GitRevision::Commit("abc1234".into()))),
        ("default", None),
    ] {
        let git = GitPlugin {
            repo: "https://example.com/plugin.git".into(),
            entrypoint: "src/main.ts".into(),
            revision,
        };
        let location = RunPluginLocation::Git(git.clone());
        let run = RunPlugin {
            location: location.clone(),
            ..Default::default()
        };
        let action = Action::StartOrReloadPlugin {
            plugin: RunPluginOrAlias::RunPlugin(run.clone()),
        };
        for (suffix, ty, value) in [
            ("git", "GitPlugin", serde_json::to_value(&git).unwrap()),
            (
                "location",
                "RunPluginLocation",
                serde_json::to_value(location).unwrap(),
            ),
            ("run", "RunPlugin", serde_json::to_value(run).unwrap()),
            ("action", "Action", serde_json::to_value(&action).unwrap()),
            (
                "keybinds",
                "Keybinds",
                serde_json::to_value(vec![(
                    InputMode::Normal,
                    vec![("Ctrl g".parse::<KeyWithModifier>().unwrap(), vec![action])],
                )])
                .unwrap(),
            ),
        ] {
            source.push_str(&fixture(&format!("{name}_{suffix}"), ty, value));
        }
        if let Some(revision) = git.revision {
            source.push_str(&fixture(
                &format!("{name}_revision"),
                "GitRevision",
                serde_json::to_value(revision).unwrap(),
            ));
        }
    }
    fixture!(
        "floating",
        "FloatingPaneLayout",
        FloatingPaneLayout::default()
    );
    fixture!(
        "config",
        "CommandObject",
        PluginCommand::ProjectConfigWrite {
            directory: "/host".into(),
            source: "agent_exit {}".into(),
            expected: None
        }
    );
    fixture!(
        "check",
        "CommandObject",
        PluginCommand::CheckPlugin {
            name: "example.ts".into(),
            source: "export default {};".into(),
            mode: PluginCheckMode::Prepare
        }
    );
    fixture!(
        "report",
        "PluginCheckReport",
        PluginCheckReport {
            formatted_source: Some("export default {};\n".into()),
            permissions: vec![PermissionType::ReadFiles],
            outline: vec![OutlineEntry {
                line: 1,
                kind: OutlineKind::Permission,
                text: "ReadFiles".into()
            }],
            ..Default::default()
        }
    );
    fixture!(
        "fs",
        "FsEntry",
        FsEntry {
            path: "/host/file".into(),
            metadata: Some(FileMetadata {
                is_dir: false,
                is_file: true,
                is_symlink: false,
                len: u64::MAX
            })
        }
    );
    fixture!(
        "hook",
        "Event",
        Event::Hook {
            point: yinz_types::input::hooks::HookPoint::PaneExit,
            payload: HookPayload::PaneExit {
                pane_id: TerminalId::new(1),
                exit_code: Some(0),
                cwd: Some("/host".into()),
                command: Some("sh".into()),
                held: true
            }
        }
    );
    fixture!(
        "rows",
        "DataRows",
        DataRows {
            columns: vec!["int".into(), "real".into(), "blob".into(), "null".into()],
            rows: vec![vec![
                DataValue::Int(i64::MAX),
                DataValue::Real(1.5),
                DataValue::Blob(vec![0, 128, 255]),
                DataValue::Null
            ]],
            truncated: true,
            next_cursor: Some(DataRowId::new(i64::MAX))
        }
    );
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("fixtures.ts"), &source).unwrap();
    insta::assert_snapshot!("sdk_rust_fixtures", source);
}

#[test]
fn host_wire_definitions_require_explicit_sdk_review() {
    // Handwritten declarations have no production generator. Keep a readable structural
    // snapshot so adding/changing a wire field forces a payload review as well as key coverage.
    let mut wire = String::new();
    for (name, source) in [
        (
            "commands",
            include_str!("../../../../yinz-plugin-api/src/plugin_command.proto"),
        ),
        (
            "events",
            include_str!("../../../../yinz-plugin-api/src/event.proto"),
        ),
        (
            "data",
            include_str!("../../../../yinz-plugin-api/src/plugin_data.proto"),
        ),
        (
            "actions",
            include_str!("../../../../yinz-plugin-api/src/action.proto"),
        ),
    ] {
        wire.push_str(&format!("{name}\n"));
        for line in source
            .lines()
            .map(|line| line.split("//").next().unwrap().trim())
            .filter(|line| !line.is_empty())
        {
            wire.push_str(line);
            wire.push('\n');
        }
    }
    insta::assert_snapshot!("sdk_wire_definitions", wire);
}

#[test]
fn sdk_rpc_declarations_require_one_destination_and_json() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("rpc.ts"), r#"
      import yinz, {type Json, type RpcCall, type CancellablePromise} from 'yinz:plugin';
      yinz.rpc.handle('echo', async (payload: Json, call: RpcCall) => {const id: number = call.source.Plugin; call.progress({status:'waiting'}); return payload;});
      yinz.rpc.handle('nil', () => {});
      const request: CancellablePromise<Json> = yinz.rpc.call({pluginId:7,name:'echo',payload:null,timeoutMs:10,onProgress: (payload: Json) => {}});
      request.cancel();
      yinz.rpc.call({pluginUrl:'file:/plugin.ts',pluginConfig:{city:'Oslo'},name:'echo'});
      // @ts-expect-error destination required
      yinz.rpc.call({name:'echo'});
      // @ts-expect-error only one destination
      yinz.rpc.call({pluginId:7,pluginUrl:'x',name:'echo'});
      // @ts-expect-error config requires URL
      yinz.rpc.call({pluginId:7,pluginConfig:{},name:'echo'});
      // @ts-expect-error JSON only
      yinz.rpc.call({pluginId:7,name:'echo',payload:1n});
      // @ts-expect-error numeric timeout
      yinz.rpc.call({pluginId:7,name:'echo',timeoutMs:'10'});
      // @ts-expect-error progress is a callback
      yinz.rpc.call({pluginId:7,name:'echo',onProgress:true});
      // @ts-expect-error progress requires JSON
      yinz.rpc.handle('bad-progress', (_, call) => {call.progress(1n);});
      // @ts-expect-error unknown option
      yinz.rpc.call({pluginId:7,name:'echo',timeout:10});
      // @ts-expect-error JSON results
      yinz.rpc.handle('bad', () => new Uint8Array());
    "#).unwrap();
}

#[test]
fn sdk_agent_and_completion_types_check_rust_completion_fixtures() {
    // agent-proto's wire tests compare these fixtures to serialized Rust structs.
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../default-plugins/agent-proto/src/completion-fixtures.json"
    ))
    .unwrap();
    let source = format!(
        r#"
      import yinz, {{type CompletionMessage, type CompletionOptions, type CompletionResult, type PaneId, type AgentInfo}} from 'yinz:plugin';
      const request = {} satisfies {{messages: CompletionMessage[]}} & Record<string, unknown>;
      const reply = {};
      const options: CompletionOptions = {{messages:request.messages, system:request.system, model:request.model, tools:request.tools, toolChoice:request.tool_choice, temperature:request.temperature, maxTokens:request.max_tokens}};
      const result: CompletionResult = {{content:reply.content, toolCalls:reply.tool_calls, finishReason:reply.finish_reason, usage:reply.usage, model:reply.model}};
      yinz.llm.complete(options).cancel();
      yinz.llm.complete({{messages:[{{role:'assistant',tool_calls:result.toolCalls}},{{role:'tool',tool_call_id:'call_1',content:'clear'}}]}});
      const pane: PaneId = yinz.agent.spawn({{kind:'codex',permission_mode:'plan',sourceDirectory:'/host',startSuppressed:true}});
      const info: AgentInfo | null = yinz.agent.info(7);
      yinz.agent.info({{Terminal:7}});
      yinz.agent.on(e => {{const hook:string=e.hook;}})();
      const sent: void = yinz.agent.send(7,'draft',{{enter:false}});
      // @ts-expect-error paste helpers do not set a timeout
      yinz.agent.send(7,'go',{{timeoutMs:30}});
      // @ts-expect-error turn waits do not set a timeout
      yinz.agent.prompt(7,'go',{{timeoutMs:30}});
      // @ts-expect-error turn waits do not set a timeout
      yinz.agent.waitForTurn(7,{{timeoutMs:30}});
      yinz.agent.waitForState(7,['idle','Busy'],{{after:'Waiting'}}).then(p => p.agent?.session_id);
      yinz.agent.prompt(7,'go',{{fallback:'idle',enter:false}}).then(t => {{const message:string|null=t.message;}});
      // @ts-expect-error plugin IDs are not agents
      yinz.agent.send({{Plugin:7}},'go');
      // @ts-expect-error invalid states
      yinz.agent.waitForState(7,'running');
      // @ts-expect-error unsupported fallback
      yinz.agent.waitForTurn(7,{{fallback:'SessionStart'}});
      // @ts-expect-error tool messages require a call ID
      yinz.llm.complete({{messages:[{{role:'tool',content:'answer'}}]}});
      // @ts-expect-error unknown role
      yinz.llm.complete({{messages:[{{role:'developer'}}]}});
      // @ts-expect-error maxTokens is numeric
      yinz.llm.complete({{messages:[],system:'hello',maxTokens:'12'}});
      // @ts-expect-error tools are JSON
      yinz.llm.complete({{messages:[],system:'hello',tools:[()=>null]}});
      // @ts-expect-error nested wire field spelling is preserved
      const bad: CompletionMessage = {{role:'assistant',toolCalls:result.toolCalls}};
    "#,
        fixtures["request"], fixtures["reply"]
    );
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("agents.ts"), &source).unwrap();
}

#[test]
fn pane_sequence_types_check_steps_options_results_and_primitives() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("sequence.ts"), r#"
      import z, {type PaneSequenceResult, type PaneExit} from 'yinz:plugin';
      const seq=z.paneSequence({steps:[{path:'cat',stdin:'hello',timeoutMs:10,cwd:'inherit',target:{Ssh:{host:'host'}}}],
        floating:true,stopOnFailure:false,closeOnSuccess:true,onCancel:'close',keepScrollback:false,killGraceMs:50,
        onStep(n,step,code){const value:number|null=code;seq.push({path:'true'});},onDone(result){const r:PaneSequenceResult=result;}});
      seq.cancel();seq.done.then(r=>r.failedStep);const pane=seq.paneId;
      z.waitForPaneExit(7).then((r:PaneExit)=>r.closed);z.runInPane({Terminal:7},{path:'true'}).cancel();
      // @ts-expect-error timeout is milliseconds and numeric
      z.paneSequence({steps:[{path:'cat',timeoutMs:'10'}]});
      // @ts-expect-error stdin is text
      z.runInPane(7,{stdin:4});
      // @ts-expect-error terminal panes only
      z.waitForPaneExit({Plugin:7});
      // @ts-expect-error unknown cancellation policy
      z.paneSequence({steps:[],onCancel:'kill'});
      // @ts-expect-error result codes can be unavailable
      seq.done.then(r=>{const codes:number[]=r.exitCodes;});
    "#).unwrap();
}

#[test]
fn sdk_trace_accepts_only_optional_booleans() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.ts");
    check_semantic_source(
        &path,
        "import z from 'yinz:plugin'; z.trace(); z.trace(true); z.trace(false);",
    )
    .unwrap();
    for argument in ["1", "'true'", "null", "{}"] {
        assert!(check_semantic_source(
            &path,
            &format!("import z from 'yinz:plugin'; z.trace({argument});")
        )
        .is_err());
    }
}

#[test]
fn notification_policy_and_observer_contracts_type_check() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(
        &dir.path().join("policy.ts"),
        r#"
import yinz, {type NotificationDecision} from 'yinz:plugin';
yinz.on('NotificationRequested', notification => {
  const id: number | bigint = notification.id;
  const decision: NotificationDecision = {Replace: {body: notification.body, delivery: 'Both'}};
  yinz.host.resolveNotification({id, decision});
});
yinz.on('NotificationResolved', result => {
  const source: string = result.original.source;
  const suppressed: boolean = result.suppressed;
});
const history = yinz.host.listNotifications();
const body: string | undefined = history[0]?.outcome?.body;
"#,
    )
    .unwrap();
}

#[test]
fn sdk_checks_unload_and_define_plugin_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("unload.ts");
    check_semantic_source(&entry, r#"
      import yinz, {definePlugin, type Plugin} from 'yinz:plugin';
      const sync: Plugin = {unload() {}};
      const async: Plugin = {async unload() {await yinz.sleep(1);}};
      export default definePlugin({defaults:{path:'tmp'}, load(config) {config.path.toUpperCase();}, async unload() {
        await yinz.runCommand({command:['true']});
      }});
      // @ts-expect-error unload must be callable
      const invalid: Plugin = {unload: 42};
      // @ts-expect-error async unload cannot return a string
      const wrong: Plugin = {async unload() {return 'wrong';}};
    "#).unwrap();
}

#[test]
fn import_meta_types_are_global_and_keep_the_es2022_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.ts");
    for prefix in ["", "import 'yinz:plugin';"] {
        check_semantic_source(&path, &format!(
            "{prefix} export const url: string = import.meta.url; export const sibling: string = import.meta.resolve('./missing.ts');"
        )).unwrap();
    }
    for (source, diagnostic) in [
        ("import.meta.resolve(1);", "TS2345"),
        ("import.meta.resolve('./main.ts', './');", "TS2554"),
        ("const url: number = import.meta.url;", "TS2322"),
        (
            "const url: number = import.meta.resolve('./main.ts');",
            "TS2322",
        ),
        ("new URL(import.meta.url);", "TS2304"),
        ("document.title;", "TS2584"),
    ] {
        let error = format!(
            "{:#}",
            check_semantic_source(&path, &format!("export {{}}; {source}")).unwrap_err()
        );
        assert!(error.contains(diagnostic), "{source}: {error}");
    }
}

#[test]
fn palette_provider_types_require_exactly_one_source() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("palette.ts"), r#"
      import yinz, {type PaletteAction, type PaletteView} from 'yinz:plugin';
      const action: PaletteAction = {id:'pdf',label:'Open PDF…',aliases:['reader','document'],run(){}};
      yinz.commandPalette.registerProvider({id:'static',title:'Static',actions:[action]});
      yinz.commandPalette.registerProvider({id:'dynamic',title:'Dynamic',provideActions: async request => [action]});
      const nested: PaletteView[] = [
        {type:'actions',title:'Static',actions:[action]},
        {type:'actions',title:'Dynamic',actions:request => [action]}
      ];
      // @ts-expect-error exactly one source
      yinz.commandPalette.registerProvider({id:'both',title:'Both',actions:[],provideActions:()=>[]});
      // @ts-expect-error source required
      yinz.commandPalette.registerProvider({id:'neither',title:'Neither'});
      // @ts-expect-error aliases must be strings
      const bad: PaletteAction = {...action,aliases:[1]};
      // @ts-expect-error aliases must be an array
      const badArray: PaletteAction = {...action,aliases:'reader'};
    "#).unwrap();
}

#[test]
fn sdk_picker_and_instance_info_contract() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("picker.ts"), r#"
import z, {type CancellablePromise, type PluginInfo} from 'yinz:plugin';
const self: PluginInfo = z.getPluginInfo();
const other: PluginInfo | null = z.getPluginInfo(2);
const request: CancellablePromise<string | null> = z.pick({kind:'file', roots:['/tmp'], extensions:['pdf'], hidden:true, recursive:false, timeoutMs:50});
request.cancel();
// @ts-expect-error kind is required
z.pick({roots:['/tmp']});
// @ts-expect-error extensions must be strings
z.pick({kind:'file',extensions:[7]});
"#).unwrap();
}
