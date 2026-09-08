use super::*;

#[test]
fn shipped_async_example_typechecks_offline() {
    check_plugin(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins/hello.ts"))
        .unwrap();
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
