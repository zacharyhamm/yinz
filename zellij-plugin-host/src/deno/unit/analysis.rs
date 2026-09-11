use super::*;

#[test]
fn literal_grants_use_sdk_bindings_and_ignore_shadowed_names_comments_and_imported_code() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("dependency.ts"),
        "zellij.requestPermission(['FullHdAccess']);",
    )
    .unwrap();
    let report = check(
        &dir.path().join("example.ts"),
        r#"
import api, {zellij as alias} from 'zellij:plugin';
import * as sdk from 'zellij:plugin';
import './dependency.ts';
// zellij.requestPermission(['RunCommands']);
const text = "zellij.requestPermission(['FullHdAccess'])";
api.requestPermission(['ReadFiles']);
alias.waitForPermissions(['WriteFiles']);
sdk.zellij.command('RequestPluginPermissions', ['PluginData']);
sdk.default.command({RequestPluginPermissions: ['SharedPluginData']});
function shadow(zellij, api, alias, sdk, globalThis) {
  zellij.requestPermission(['RunCommands']); api.requestPermission(['FullHdAccess']);
  alias.waitForPermissions(['WebAccess']); sdk.zellij.requestPermission(['OpenFiles']);
  globalThis.zellij.requestPermission(['AuthorPlugins']);
}
zellij.waitForPermissions(computed);
zellij.requestPermission(['ReadFiles', ...computed]);
globalThis.zellij.requestPermission(['ReadApplicationState']);
zellij.host.requestPluginPermissions(['Reconfigure']);
sdk.zellij.host.requestPluginPermissions(['WriteToStdin']);
function shadowHost(host, zellij) { host.requestPluginPermissions(['WebAccess']); zellij.host.requestPluginPermissions(['OpenFiles']); }
zellij.registerDefaultKeybindings([{mode: 'normal', key: 'Ctrl g', actions: ['NewPane']}]);
export default {load() {}, render() {return ''}};
"#,
        PluginCheckMode::Syntax,
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(
        report.permissions,
        [
            PermissionType::ReadFiles,
            PermissionType::WriteFiles,
            PermissionType::PluginData,
            PermissionType::SharedPluginData,
            PermissionType::ReadApplicationState,
            PermissionType::Reconfigure,
            PermissionType::WriteToStdin,
        ]
    );
    assert_eq!(report.warnings.len(), 2, "{report:?}");
    assert!(report.outline.iter().any(|entry| entry.text == "load(...)"));
    assert!(report
        .outline
        .iter()
        .any(|entry| entry.kind == OutlineKind::Handler && entry.text == "default keybindings"));
    assert!(report.outline.iter().all(|entry| entry.line > 1));
}

#[test]
fn runtime_check_reports_syntax_semantics_and_confined_dependency_failures() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.ts");
    assert!(!check(&path, "const broken = ;", PluginCheckMode::Syntax)
        .diagnostics
        .is_empty());
    let source = "import {value} from './dep.ts'; const n: number = value;";
    assert!(check(&path, source, PluginCheckMode::Syntax)
        .diagnostics
        .is_empty());
    assert!(!check(&path, source, PluginCheckMode::Semantic)
        .diagnostics
        .is_empty());
    std::fs::write(dir.path().join("dep.ts"), "export const value = 1;").unwrap();
    assert!(check(&path, source, PluginCheckMode::Semantic)
        .diagnostics
        .is_empty());
    std::fs::write(dir.path().join("dep.ts"), "export const value = 'text';").unwrap();
    assert!(!check(&path, source, PluginCheckMode::Semantic)
        .diagnostics
        .is_empty());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("escape.ts"), "export const value = 1;").unwrap();
    std::fs::remove_file(dir.path().join("dep.ts")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("escape.ts"), dir.path().join("dep.ts"))
        .unwrap();
    let report = check(&path, source, PluginCheckMode::Semantic);
    assert!(
        report.diagnostics.join("\n").contains("escapes"),
        "{report:?}"
    );
}

#[test]
fn hocus_tool_outlines_and_permission_warnings_respect_sdk_identity() {
    let dir = tempfile::tempdir().unwrap();
    for (source, warns) in [
        ("import {zellij as z} from 'zellij:plugin'; z.hocus.tools({forecast:{handler(){}}});", true),
        ("zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']); zellij.hocus.tools({forecast:{handler(){}}});", false),
    ] {
        let report = check(&dir.path().join("weather.ts"), source, PluginCheckMode::Syntax);
        assert!(report.outline.iter().any(|e| e.kind == OutlineKind::Tool && e.text == "forecast"));
        assert_eq!(report.warnings.iter().any(|w| w.contains("registration requires")), warns);
    }
    let report = check(
        &dir.path().join("shadow.ts"),
        "function f(zellij) { zellij.hocus.tools({wrong:{}}); }",
        PluginCheckMode::Syntax,
    );
    assert!(!report.outline.iter().any(|e| e.kind == OutlineKind::Tool));
}

fn warnings_of(source: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let report = check(&dir.path().join("lint.ts"), source, PluginCheckMode::Syntax);
    assert!(report.diagnostics.is_empty(), "{report:?}");
    report.warnings
}

#[test]
fn a_permission_typo_suggests_the_nearest_typescript_spelling() {
    let warnings = warnings_of("zellij.requestPermission(['ReadFile', 'SharedData', 'weather']);");
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(
        warnings[0].contains("\"ReadFile\" grants nothing - did you mean \"ReadFiles\"?"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains("did you mean \"SharedPluginData\"?"),
        "{warnings:?}"
    );
    assert!(
        warnings[2].contains("\"weather\" grants nothing") && !warnings[2].contains("did you mean"),
        "{warnings:?}"
    );
}

#[test]
fn using_the_data_store_without_plugin_data_is_a_finding_in_every_spelling() {
    for source in [
        "zellij.requestPermission(['ReadFiles']); export default { async load() { await zellij.waitForPermissions(['ReadFiles']); zellij.data.fetch({entity: 'note'}); } };",
        "import z from 'zellij:plugin'; const D = z.data; export default { async load() { await z.waitForPermissions([]); D.count({entity: 'note'}); } };",
        "export default { async load() { await zellij.waitForPermissions([]); zellij.command('DataCatalog', {namespace: null}); } };",
        "export default { async load() { await zellij.waitForPermissions([]); zellij.command({DataCount: {request: {entity: 'note'}}}); } };",
        "export default { async load() { await zellij.waitForPermissions([]); zellij.host.dataCatalog({namespace: null}); } };",
    ] {
        let warnings = warnings_of(source);
        assert_eq!(warnings.len(), 1, "{source}\n{warnings:?}");
        assert!(warnings[0].contains("requestPermission(['PluginData'])"), "{warnings:?}");
    }
    for source in [
        "zellij.requestPermission(['PluginData']); export default { async load() { await zellij.waitForPermissions(['PluginData']); zellij.data.fetch({entity: 'note'}); } };",
        // Predicates and converters touch no store; a shadowed name is somebody else's data.
        "const p = zellij.data.eq('a', 1); zellij.data.int(4); function f(zellij) { zellij.data.fetch({entity: 'x'}); }",
        "const data = {fetch() {}}; data.fetch({entity: 'x'});",
        "// zellij.data.fetch({entity: 'x'})\nconst s = \"zellij.data.fetch({entity: 'x'})\";",
    ] {
        assert!(warnings_of(source).is_empty(), "{source}");
    }
}

#[test]
fn a_shared_namespace_needs_shared_plugin_data_in_every_spelling() {
    let granted = "zellij.requestPermission(['PluginData']); export default { async load() { await zellij.waitForPermissions(['PluginData']); ";
    for call in [
        "zellij.data.define('note', {text: 'text'}, {namespace: 'shared:notes'});",
        "zellij.data.fetch({namespace: 'shared:notes', entity: 'note'});",
        "zellij.data.apply([], {namespace: 'shared:notes'});",
        "zellij.data.catalog('shared:notes');",
        "zellij.data.watch({namespace: 'shared:notes', entity: 'note'}, () => {});",
        "zellij.command('DataFetch', {request: {namespace: 'shared:notes', entity: 'note'}});",
        "const D = zellij.data; D.fetch({namespace: 'shared:notes', entity: 'note'} as const);",
    ] {
        let warnings = warnings_of(&format!("{granted}{call} }} }};"));
        assert_eq!(warnings.len(), 1, "{call}\n{warnings:?}");
        assert!(
            warnings[0].contains("shared: namespace needs SharedPluginData"),
            "{warnings:?}"
        );
    }
    // Said once, however many calls; quiet once requested; quiet when the word is not a namespace.
    let warnings = warnings_of(&format!("{granted}zellij.data.fetch({{namespace: 'shared:a', entity: 'n'}}); zellij.data.count({{namespace: 'shared:b', entity: 'n'}}); }} }};"));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    for source in [
        "zellij.requestPermission(['PluginData', 'SharedPluginData']); export default { async load() { await zellij.waitForPermissions(['PluginData']); zellij.data.fetch({namespace: 'shared:notes', entity: 'note'}); } };",
        "zellij.requestPermission(['PluginData']); export default { async load() { await zellij.waitForPermissions(['PluginData']); /* shared:notes */ const label = 'shared:notes'; zellij.data.fetch({entity: 'note'}); zellij.hocus.tools({t: {description: 'reads shared:notes', handler() {}}}); } };",
    ] {
        let warnings = warnings_of(source);
        assert!(!warnings.iter().any(|w| w.contains("shared:")), "{source}\n{warnings:?}");
    }
    // An invalid namespace is named too.
    let warnings = warnings_of(&format!(
        "{granted}zellij.data.fetch({{namespace: 'shared:', entity: 'note'}}); }} }};"
    ));
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("line ") && w.contains("namespace name is empty")),
        "{warnings:?}"
    );
}

#[test]
fn a_reserved_attribute_declared_in_define_is_a_finding() {
    let granted = "zellij.requestPermission(['PluginData']); export default { async load() { await zellij.waitForPermissions(['PluginData']); ";
    let warnings = warnings_of(&format!("{granted}zellij.data.define('note', {{id: 'text', text: 'text', created_at: 'int'}}); }} }};"));
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("\"id\" is on every row already"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains("\"created_at\" is on every row already"),
        "{warnings:?}"
    );
    for source in [
        // An entity may be called `id`; only the attribute list is read, and only literal keys.
        &format!("{granted}zellij.data.define('id', {{text: 'text'}}); }} }};"),
        &format!(
            "{granted}const id = 'text'; zellij.data.define('note', {{[id]: 'text'}}); }} }};"
        ),
        &format!("{granted}zellij.data.fetch({{entity: 'note', id: 1}}); }} }};"),
    ] {
        assert!(warnings_of(source).is_empty(), "{source}");
    }
}

#[test]
fn a_data_call_in_load_before_awaiting_permissions_is_a_finding() {
    let asked = "zellij.requestPermission(['PluginData']); ";
    let warning = "must await zellij.waitForPermissions";
    for source in [
        "export default { load() { zellij.data.fetch({entity: 'note'}); } };",
        "export default { async load() { zellij.data.fetch({entity: 'note'}); await zellij.waitForPermissions(['PluginData']); } };",
        "export default { load: async () => { zellij.data.fetch({entity: 'note'}); } };",
        "export default { load: async function () { zellij.data.fetch({entity: 'note'}); } };",
        "zellij.data.fetch({entity: 'note'});",
        "export default { async load() { await zellij.sleep(1); zellij.data.fetch({entity: 'note'}); } };",
    ] {
        let warnings = warnings_of(&format!("{asked}{source}"));
        assert!(warnings.iter().any(|w| w.contains(warning)), "{source}\n{warnings:?}");
        assert_eq!(warnings.iter().filter(|w| w.contains(warning)).count(), 1, "{warnings:?}");
    }
    for source in [
        "export default { async load() { await zellij.waitForPermissions(['PluginData']); zellij.data.fetch({entity: 'note'}); } };",
        "export default { async load() { await zellij.waitForEvent('PermissionRequestResult'); zellij.data.fetch({entity: 'note'}); } };",
        "export default { load() { zellij.on('PermissionRequestResult', () => { zellij.data.fetch({entity: 'note'}); }); } };",
        "export default { load() {}, update() { zellij.data.fetch({entity: 'note'}); } };",
        "export default { load() {}, async render() { await zellij.sleep(1); zellij.data.fetch({entity: 'note'}); return ''; } };",
        "function loader(zellij) { zellij.data.fetch({entity: 'note'}); }",
    ] {
        let warnings = warnings_of(&format!("{asked}{source}"));
        assert!(!warnings.iter().any(|w| w.contains(warning)), "{source}\n{warnings:?}");
    }
}

/// The reference examples are what Hocus shows its model; a finding there is a finding in
/// every plugin written from them. The builtins are analysed too, but only for diagnostics: a
/// builtin is granted everything without asking, so the permission rules do not apply to it.
#[test]
fn the_shipped_typescript_examples_are_quiet_and_the_builtins_analyse() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut checked = 0;
    for (directory, quiet) in [
        ("example-plugins", true),
        ("zellij-config/assets/plugins", false),
    ] {
        for entry in std::fs::read_dir(root.join(directory)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("ts") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let report = check(&path, &source, PluginCheckMode::Syntax);
            assert!(
                report.diagnostics.is_empty(),
                "{}: {report:?}",
                path.display()
            );
            if quiet {
                assert!(
                    report.warnings.is_empty(),
                    "{}: {:#?}",
                    path.display(),
                    report.warnings
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked} files checked");
}
