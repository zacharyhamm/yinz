use super::*;

#[test]
fn sdk_inventories_match_rust_commands_events_permissions_and_decoded_replies() {
    use strum::IntoEnumIterator;
    use zellij_types::data::{CommandType, EventType, PermissionType};
    let commands = CommandType::iter()
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
    let source = format!(
        r#"
      import type {{CommandArgs, CommandReplies, CommandResults, EventType, Permission}} from 'zellij:plugin';
      type Equal<A, B> = [A, B] extends [B, A] ? true : false;
      type Assert<T extends true> = T;
      type Commands = Assert<Equal<keyof CommandArgs, {}>>;
      type Results = Assert<Equal<keyof CommandResults, keyof CommandArgs>>;
      type Events = Assert<Equal<EventType, {}>>;
      type Permissions = Assert<Equal<Permission, {}>>;
      type Replies = Assert<Equal<keyof CommandReplies, {}>>;
    "#,
        quoted(commands),
        quoted(events),
        quoted(permissions),
        quoted(replies)
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
}

#[test]
fn sdk_raw_commands_reject_unknown_names_and_wrong_payloads() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("commands.ts"), r#"
      import zellij, {type Integer, type PaneId, type DataRows} from 'zellij:plugin';
      zellij.command('RunCommand', [['true'], {}, '/host', {}, null]);
      zellij.command({RunCommand: [['true'], {}, '/host', {}, 0, true]});
      zellij.command('RunCommand', [['true'], {}, '/host', {}, null, false]);
      zellij.command({RunCommand: [['true'], {}, '/host', {}, null]});
      zellij.command('SubscribePaneOutput', {work:'one'});
      zellij.command({SubscribePaneOutput: null});
      zellij.on('PaneOutput', p=>{const id:number=p.pane_id; const data:number[]=p.data; const context:string=p.context.work;});
      zellij.on('RunCommandOutput', p=>{const stderr:boolean=p.stderr; const data:number[]=p.data;});
      // @ts-expect-error streaming flag is boolean or null
      zellij.command('RunCommand', [['true'], {}, '/host', {}, null, 1]);
      // @ts-expect-error streaming flag is boolean or null in object form too
      zellij.command({RunCommand: [['true'], {}, '/host', {}, null, 'yes']});
      // @ts-expect-error context values are strings
      zellij.command('SubscribePaneOutput', {work:1});
      // @ts-expect-error raw output has JSON bytes
      zellij.on('PaneOutput', p=>{const bytes:Uint8Array=p.data;});
      // @ts-expect-error command output has no pane ID
      zellij.on('RunCommandOutput', p=>p.pane_id);
      const pid: number = zellij.command('GetPluginIds').plugin_id;
      const objectPid: number = zellij.command({GetPluginIds: null}).plugin_id;
      const pane: PaneId | null = zellij.command('OpenFile', [{path: '/host/file'}, {}]);
      const bytes: Uint8Array = zellij.command({FsRead: {path: '/host/file', max_bytes: 9223372036854775807n}});
      const nothing: null = zellij.command('RenameTab', [0, 'renamed']);
      const job: Integer = zellij.command('EnqueueQueueJob', {queue:'q',plugin_url:'x',config:{},context:{}});
      zellij.command('RunAction', [{MoveFocus: {direction: 'Left'}}, {}]);
      // @ts-expect-error unknown command
      zellij.command('MadeUp');
      // @ts-expect-error unknown object command
      zellij.command({MadeUp: {}});
      // @ts-expect-error unknown keys beside a known command
      zellij.command({SetTimeout: 1, MadeUp: {}});
      // @ts-expect-error one command per request
      zellij.command({SetTimeout: 1, SwitchTabTo: 0});
      // @ts-expect-error required tuple payload
      zellij.command('RenameTab');
      // @ts-expect-error wrong tuple position
      zellij.command('RenameTab', ['name', 0]);
      // @ts-expect-error unit command has no payload
      zellij.command('GetPluginIds', {});
      // @ts-expect-error wrong nested action
      zellij.command('RunAction', [{MoveFocus: {direction: 'Diagonal'}}, {}]);
      // @ts-expect-error raw byte read does not return a string
      const text: string = zellij.command('FsRead', {path:'x', max_bytes:null});
      // @ts-expect-error real values cannot be bigint
      zellij.command('SetTimeout', 1n);
      const file: Uint8Array = zellij.fs.read('/host/file', {maxBytes: 8n});
      zellij.fs.write('/host/file', file, {append:true, createDirs:true});
      const absent: import('zellij:plugin').FileMetadata | null = zellij.fs.metadata('/host/missing');
      zellij.projectConfig.write('.', 'agent_exit {}', null);
      const d = zellij.data;
      d.define('note', {text:'Text', score:{kind:'Real', default:d.real(1)}});
      d.apply([d.put('note', {text:'hello', bytes:new Uint8Array([255])}, {uid:'a', expectRev:1n})]);
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[]}}]);
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2], [3n, 4n], [5, 6n], [7n, 8]]}}]);
      zellij.command('DataApply', {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[]}}]});
      zellij.command({DataApply: {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2], [3n, 4n], [5, 6n], [7n, 8]]}}]}});
      // @ts-expect-error snapshot pairs require an ID and revision
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1]]}}]);
      // @ts-expect-error snapshot pairs cannot have extra elements
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[[1, 2, 3]]}}]);
      // @ts-expect-error snapshot IDs cannot be text
      d.apply([{AssertRows: {entity:'note', filter:d.and(), rows:[['1', 2]]}}]);
      // @ts-expect-error snapshot revisions cannot be text
      zellij.command('DataApply', {namespace:null, mutations:[{AssertRows: {entity:'note', filter:d.and(), rows:[[1, '2']]}}]});
      // @ts-expect-error snapshot assertions require a filter
      d.apply([{AssertRows: {entity:'note', rows:[]}}]);
      // @ts-expect-error raw snapshot assertions also require a filter
      zellij.command({DataApply: {namespace:null, mutations:[{AssertRows: {entity:'note', rows:[]}}]}});
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
      const projected: import('zellij:plugin').DataRecord = {uid:'a'};
      const revision: import('zellij:plugin').DataRecord = {rev:1n};
      // @ts-expect-error a record revision cannot be text
      const textRevision: import('zellij:plugin').DataRecord = {rev:'1'};
      // @ts-expect-error a record revision cannot be null
      const nullRevision: import('zellij:plugin').DataRecord = {rev:null};
      // @ts-expect-error a record revision cannot be boolean
      const boolRevision: import('zellij:plugin').DataRecord = {rev:true};
      // @ts-expect-error update revisions cannot be text
      d.update('note', d.eq('uid', 'a'), {}, {expectRev:'1'});
      const watch = d.watch({entity:'note'}, change => {const raw: DataRows = change.raw;});
      const watchId: Integer = watch.watchId;
      watch.dispose();
      // @ts-expect-error optimistic source guard is required
      zellij.projectConfig.write('.', 'agent_exit {}');
      // @ts-expect-error helpers use camelCase options
      zellij.fs.write('/host/file', 'text', {create_dirs:true});
      // @ts-expect-error file payloads are text or Uint8Array
      zellij.fs.write('/host/file', [1,2]);
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
    use zellij_types::data::CommandType;
    let source = include_str!("../../../../../zellij-tile/src/shim.rs");
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
    for (name, source) in zellij_config::consts::BUILTIN_DENO_PLUGINS {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../zellij-config/assets/plugins/{name}.ts"));
        check_semantic_source(&path, source).unwrap();
    }
}

#[test]
fn shipped_async_example_typechecks_offline() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins");
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        assert_ne!(path.extension().and_then(|s| s.to_str()), Some("lua"));
        if path.extension().and_then(|s| s.to_str()) == Some("ts") {
            check_plugin(&path).unwrap();
        }
    }
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
            .execute_script("zellij:watchdog-test", "while (true) {}")
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
    check_semantic_source(&entry, "import zellij, {definePlugin, type EventType} from 'zellij:plugin';\nconst event: EventType = 'PaneUpdate';\nzellij.subscribe([event]);\nexport default definePlugin({render: () => 'ok'});").unwrap();
    for (source, expected) in [
        ("import zellij from 'zellij:plugin'; zellij.subscribee(['PaneUpdate']);", "subscribee"),
        ("import type {EventType} from 'zellij:plugin'; const event: EventType = 'PaneUpdat';", "PaneUpdat"),
        ("import {definePlugin} from 'zellij:plugin'; export default definePlugin({render: () => 42});", "TS2322"),
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
        import zellij, {definePlugin, type CancellablePromise, type RunCommandResult,
            type WebRequestResult, type PipeMessage} from 'zellij:plugin';
        export default definePlugin({defaults: {greeting: 'hello'}, async load(config) {
            const command: CancellablePromise<RunCommandResult> = zellij.runCommand({
                command: ['echo', config.greeting], cwd: '/host', envVariables: {LANG: 'C'},
                maxBytesReturned: 1024, context: {tag: 'one'},
                onOutput(output) {const bytes:Uint8Array=output.data; const stderr:boolean=output.stderr; const tag:string=output.context.tag;},
            });
            const web: CancellablePromise<WebRequestResult> = zellij.webRequest({
                url: 'https://example.com', verb: 'Post', body: zellij.encodeUtf8('hello'),
                headers: {'content-type': 'text/plain'}, proxy: 'http://localhost:8080', context: {}, streamId: 'stream',
            });
            const [result, response] = await Promise.all([command, web]);
            const exit: number | null = result.exitCode;
            const bytes: Uint8Array = result.stderr;
            const status: number = response.status;
            const text: string = zellij.decodeUtf8(response.body);
            const context: Record<string, string> = result.context;
            const pipe: CancellablePromise<PipeMessage> = zellij.waitForPipe('reply', msg => msg.payload !== null);
            const event = zellij.waitForEvent('RunCommandResult', ([code]) => code === 0);
            const raw: number[] = (await event)[1];
            const timer = zellij.every(100, async () => { await zellij.sleep(1); return true; });
            const once = zellij.after(0, () => {});
            for (const handle of [command, web, pipe, event, timer, once, zellij.sleep(1)]) handle.cancel();
            // @ts-expect-error command argv must be strings
            zellij.runCommand({command: [1]});
            // @ts-expect-error command is required
            zellij.runCommand({cwd: '/host'});
            // @ts-expect-error maximum is numeric
            zellij.runCommand({command: ['true'], maxBytesReturned: '1'});
            // @ts-expect-error context values must be strings
            zellij.runCommand({command: ['true'], context: {tag: 1}});
            // @ts-expect-error only host verbs are supported
            zellij.webRequest({url: 'x', verb: 'Patch'});
            // @ts-expect-error only strings or Uint8Array bodies
            zellij.webRequest({url: 'x', body: [1]});
            // @ts-expect-error stream IDs must be strings
            zellij.webRequest({url: 'x', streamId: 1});
            // @ts-expect-error wrapper uses camelCase
            zellij.webRequest({url: 'x', stream_id: 'stream'});
            // @ts-expect-error exitCode can be null
            const code: number = result.exitCode;
            // @ts-expect-error helper output is bytes
            const output: string = result.stdout;
            // @ts-expect-error pipe names are strings
            zellij.waitForPipe(1);
            // @ts-expect-error predicates are synchronous
            zellij.waitForPipe('reply', async () => true);
            // @ts-expect-error callbacks only return boolean or void
            zellij.after(0, () => 'render');
            // @ts-expect-error chained promises have no cancel method
            command.then(result => result).cancel();
            // @ts-expect-error permission promises are not cancellable
            zellij.waitForPermissions([]).cancel();
            // @ts-expect-error callback must be callable
            zellij.runCommand({command:['true'], onOutput:true});
            // @ts-expect-error callback receives binary bytes
            zellij.runCommand({command:['true'], onOutput(output) {const text:string=output.data;}});
            // @ts-expect-error decoding requires bytes
            zellij.decodeUtf8('hello');
        }});
        // @ts-expect-error defaults must be strings
        definePlugin({defaults: {count: 1}});
    "#).unwrap();
}

#[test]
fn serialized_rust_payloads_satisfy_the_handwritten_sdk() {
    use zellij_types::internal_ids::*;
    use zellij_types::{authoring::*, data::*, input::layout::*};
    let fixture = |name: &str, ty: &str, value: serde_json::Value| {
        format!("const {name} = {value} satisfies SDK.{ty};\n")
    };
    let mut source = "import type * as SDK from 'zellij:plugin';\n".to_owned();
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
    fixture!(
        "pane_output",
        "Event",
        Event::PaneOutput {
            pane_id: 7,
            data: vec![0, 128, 255],
            context: Default::default()
        }
    );
    fixture!(
        "command_output",
        "Event",
        Event::RunCommandOutput {
            data: vec![0, 128, 255],
            stderr: true,
            context: Default::default()
        }
    );
    fixture!("mode", "Event", Event::ModeUpdate(ModeInfo::default()));
    fixture!("tabs", "Event", Event::TabUpdate(vec![TabInfo::default()]));
    fixture!("panes", "Event", Event::PaneUpdate(PaneManifest::default()));
    fixture!("pane", "PaneInfo", PaneInfo::default());
    fixture!("style", "Style", zellij_types::style::Style::default());
    fixture!("layout", "TiledPaneLayout", TiledPaneLayout::default());
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
            mode: PluginCheckMode::Semantic
        }
    );
    fixture!(
        "report",
        "PluginCheckReport",
        PluginCheckReport {
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
            point: zellij_types::input::hooks::HookPoint::PaneExit,
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
            include_str!("../../../../../zellij-plugin-api/src/plugin_command.proto"),
        ),
        (
            "events",
            include_str!("../../../../../zellij-plugin-api/src/event.proto"),
        ),
        (
            "data",
            include_str!("../../../../../zellij-plugin-api/src/plugin_data.proto"),
        ),
        (
            "actions",
            include_str!("../../../../../zellij-plugin-api/src/action.proto"),
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
      import zellij, {type Json, type RpcCall, type CancellablePromise} from 'zellij:plugin';
      zellij.rpc.handle('echo', async (payload: Json, call: RpcCall) => {const id: number = call.source.Plugin; return payload;});
      zellij.rpc.handle('nil', () => {});
      const request: CancellablePromise<Json> = zellij.rpc.call({pluginId:7,name:'echo',payload:null,timeoutMs:10});
      request.cancel();
      zellij.rpc.call({pluginUrl:'file:/plugin.ts',pluginConfig:{city:'Oslo'},name:'echo'});
      // @ts-expect-error destination required
      zellij.rpc.call({name:'echo'});
      // @ts-expect-error only one destination
      zellij.rpc.call({pluginId:7,pluginUrl:'x',name:'echo'});
      // @ts-expect-error config requires URL
      zellij.rpc.call({pluginId:7,pluginConfig:{},name:'echo'});
      // @ts-expect-error JSON only
      zellij.rpc.call({pluginId:7,name:'echo',payload:1n});
      // @ts-expect-error numeric timeout
      zellij.rpc.call({pluginId:7,name:'echo',timeoutMs:'10'});
      // @ts-expect-error unknown option
      zellij.rpc.call({pluginId:7,name:'echo',timeout:10});
      // @ts-expect-error JSON results
      zellij.rpc.handle('bad', () => new Uint8Array());
    "#).unwrap();
}

#[test]
fn sdk_agent_and_completion_types_check_rust_completion_fixtures() {
    // hocus-proto's wire tests compare these fixtures to serialized Rust structs.
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../default-plugins/hocus-proto/src/completion-fixtures.json"
    ))
    .unwrap();
    let source = format!(
        r#"
      import zellij, {{type CompletionMessage, type CompletionOptions, type CompletionResult, type PaneId, type AgentInfo}} from 'zellij:plugin';
      const request = {} satisfies {{messages: CompletionMessage[]}} & Record<string, unknown>;
      const reply = {};
      const options: CompletionOptions = {{messages:request.messages, system:request.system, model:request.model, tools:request.tools, toolChoice:request.tool_choice, temperature:request.temperature, maxTokens:request.max_tokens}};
      const result: CompletionResult = {{content:reply.content, toolCalls:reply.tool_calls, finishReason:reply.finish_reason, usage:reply.usage, model:reply.model}};
      zellij.hocus.complete(options).cancel();
      zellij.hocus.complete({{messages:[{{role:'assistant',tool_calls:result.toolCalls}},{{role:'tool',tool_call_id:'call_1',content:'clear'}}]}});
      const pane: PaneId = zellij.agent.spawn({{kind:'codex',permission_mode:'plan',sourceDirectory:'/host',startSuppressed:true}});
      const info: AgentInfo | null = zellij.agent.info(7);
      zellij.agent.info({{Terminal:7}});
      zellij.agent.on(e => {{const hook:string=e.hook;}})();
      zellij.agent.waitForState(7,['idle','Busy'],{{after:'Waiting'}}).then(p => p.agent?.session_id);
      zellij.agent.prompt(7,'go',{{fallback:'idle',enter:false}}).then(t => {{const message:string|null=t.message;}});
      // @ts-expect-error plugin IDs are not agents
      zellij.agent.send({{Plugin:7}},'go');
      // @ts-expect-error invalid states
      zellij.agent.waitForState(7,'running');
      // @ts-expect-error unsupported fallback
      zellij.agent.waitForTurn(7,{{fallback:'SessionStart'}});
      // @ts-expect-error tool messages require a call ID
      zellij.hocus.complete({{messages:[{{role:'tool',content:'answer'}}]}});
      // @ts-expect-error unknown role
      zellij.hocus.complete({{messages:[{{role:'developer'}}]}});
      // @ts-expect-error maxTokens is numeric
      zellij.hocus.complete({{messages:[],system:'hello',maxTokens:'12'}});
      // @ts-expect-error tools are JSON
      zellij.hocus.complete({{messages:[],system:'hello',tools:[()=>null]}});
      // @ts-expect-error nested wire field spelling is preserved
      const bad: CompletionMessage = {{role:'assistant',toolCalls:result.toolCalls}};
    "#,
        fixtures["request"], fixtures["reply"]
    );
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("agents.ts"), &source).unwrap();
}

#[test]
fn embedded_workflow_view_has_semantic_types_without_a_sibling_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("view.ts");
    check_semantic_source(&path, "import {view} from 'zellij:workflow-view'; view.attach({directory:'/work'}); view.render(8,24);").unwrap();
    assert!(check_semantic_source(
        &path,
        "import {view} from 'zellij:workflow-view'; view.attach({directory:42});"
    )
    .is_err());
}

#[test]
fn pane_sequence_types_check_steps_options_results_and_primitives() {
    let dir = tempfile::tempdir().unwrap();
    check_semantic_source(&dir.path().join("sequence.ts"), r#"
      import z, {type PaneSequenceResult, type PaneExit} from 'zellij:plugin';
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
        "import z from 'zellij:plugin'; z.trace(); z.trace(true); z.trace(false);",
    )
    .unwrap();
    for argument in ["1", "'true'", "null", "{}"] {
        assert!(check_semantic_source(
            &path,
            &format!("import z from 'zellij:plugin'; z.trace({argument});")
        )
        .is_err());
    }
}
