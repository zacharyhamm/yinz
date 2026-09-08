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
            PermissionType::ReadApplicationState
        ]
    );
    assert_eq!(report.warnings.len(), 2, "{report:?}");
    assert!(report.outline.iter().any(|entry| entry.text == "load(...)"));
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
