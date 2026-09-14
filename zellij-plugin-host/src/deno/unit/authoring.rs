use super::*;
use zellij_types::authoring::PluginCheckMode::{Format, Prepare, Semantic, Syntax};

fn report(
    dir: &Path,
    source: &str,
    mode: PluginCheckMode,
) -> zellij_types::authoring::PluginCheckReport {
    crate::plugin_authoring::check_source(&dir.join("main.ts"), source, mode)
}

#[test]
fn formats_drafts_without_writing_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let first = report(dir.path(), "export const label='λ'", Prepare);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let formatted = first.formatted_source.unwrap();
    assert_eq!(formatted, "export const label = \"λ\";\n");
    assert!(!dir.path().join("main.ts").exists());
    let again = report(dir.path(), &formatted, Format);
    assert_eq!(again.formatted_source.as_deref(), Some(formatted.as_str()));
    let checked = report(dir.path(), &formatted, Semantic);
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.formatted_source.is_none());
}

#[test]
fn strict_compiler_and_type_aware_rules_reject_unsafe_drafts() {
    let dir = tempfile::tempdir().unwrap();
    // A surrounding project cannot weaken the policy or supply configuration code.
    std::fs::write(
        dir.path().join(".oxlintrc.json"),
        r#"{"rules":{"typescript/no-explicit-any":"off"}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("oxlint.config.ts"),
        "throw new Error('project config executed');",
    )
    .unwrap();
    for (source, expected) in [
        ("export const value: any = 1;", "no-explicit-any"),
        ("export const values: string[] = []; export const value = values[0]!;", "no-non-null-assertion"),
        ("export const value = JSON.parse('1');", "no-unsafe-assignment"),
        ("export function value() { return JSON.parse('1'); }", "no-unsafe-return"),
        ("JSON.parse('1').call();", "no-unsafe-call"),
        ("export const value = JSON.parse('1').field;", "no-unsafe-member-access"),
        ("export const values: string[] = []; export const value: string = values[0];", "undefined"),
        ("export const value: {name?: string} = {name: undefined};", "exactOptionalPropertyTypes"),
        ("export const values: Record<string,string> = {}; export const value = values.name;", "index signature"),
        ("const unused = 1; export {};", "never read"),
        ("export function value(flag: boolean) { if (flag) return 1; }", "return"),
        ("export const value = (1 as unknown) as string;", "no-unsafe-type-assertion"),
        ("import z from 'zellij:plugin'; void z.waitForPermissions(['ReadFiles']);", "no-floating-promises"),
        ("export const value: () => void = async () => { throw new Error('rejected'); };", "no-misused-promises"),
        ("class Base { value(): void {} } export class Child extends Base { value(): void {} }", "override"),
        ("export function value(x: number): number { switch(x) { case 0: x += 1; case 1: return x; default: return 2; } }", "Fallthrough"),
        ("export function value(x: 'a' | 'b'): number { switch (x) { case 'a': return 1; } return 0; }", "switch-exhaustiveness-check"),
    ] {
        let result = report(dir.path(), source, Semantic);
        assert!(result.diagnostics.join("\n").contains(expected), "{source}: {:?}", result.diagnostics);
        assert!(result.formatted_source.is_none());
    }
}

#[test]
fn stages_only_confined_dependencies_and_never_formats_them() {
    let dir = tempfile::tempdir().unwrap();
    let dependency = "export const label='λ'";
    std::fs::write(dir.path().join("dep.ts"), dependency).unwrap();
    let result = report(dir.path(), "export {label} from './dep.ts';", Format);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
        dependency
    );
    std::fs::write(dir.path().join("dep.ts"), "export const value: string = 1;").unwrap();
    let result = report(dir.path(), "export {value} from './dep.ts';", Prepare);
    assert!(
        result.diagnostics.join("\n").contains("dep.ts"),
        "{:?}",
        result.diagnostics
    );
    for source in [
        "import 'npm:bad';",
        "import '../escape.ts';",
        "/// <reference types=\"node\" />\nexport {};",
        "export const = ;",
    ] {
        assert!(
            !report(dir.path(), source, Format).diagnostics.is_empty(),
            "{source}"
        );
    }
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.ts"), "export const secret = 1;").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.ts"),
        dir.path().join("escape.ts"),
    )
    .unwrap();
    assert!(
        report(dir.path(), "export {secret} from './escape.ts';", Prepare)
            .diagnostics
            .join("\n")
            .contains("escapes")
    );
}

#[test]
fn encoded_traversal_aliases_leave_no_entries_outside_owned_scratch() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    let cache = dir.path().join("cache");
    std::fs::create_dir(&plugins).unwrap();
    std::fs::write(plugins.join("dep.ts"), "export const value = 1;\n").unwrap();
    let name = dir.path().file_name().unwrap().to_str().unwrap();
    for traversal in ["..%2f..%2f", "%2e%2e%2f%2e%2e%2f"] {
        // The loader resolves this back into plugins, but the decoded alias has two parent
        // components. Staging it verbatim would create a sibling of the owned directory.
        let source = format!("export {{value}} from './{traversal}{name}/plugins/dep.ts';");
        std::fs::write(plugins.join("main.ts"), &source).unwrap();
        for operation in ["check", "format"] {
            let output = Command::new(std::env::var_os("ZELLIJ_COMPILATION_WORKER").unwrap())
                .env("ZELLIJ_TEST_CACHE_ROOT", &cache)
                .args(["plugins", operation, "--file"])
                .arg(plugins.join("main.ts"))
                .output()
                .unwrap();
            // Keep even a regressed run's side effects inside this test's temporary directory.
            for process in std::fs::read_dir(&cache).unwrap() {
                let registry = process.unwrap().path().join("cache/authoring-tmp");
                if registry.exists() {
                    for entry in std::fs::read_dir(registry).unwrap() {
                        assert_eq!(entry.unwrap().file_name(), ".registry.lock");
                    }
                }
            }
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{operation}: {diagnostic}");
            assert!(
                diagnostic.contains("checked file escapes plugin root"),
                "{diagnostic}"
            );
            assert_eq!(
                std::fs::read_to_string(plugins.join("main.ts")).unwrap(),
                source
            );
        }
    }
}

#[test]
fn symlinked_entrypoints_keep_the_original_dependency_root() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(plugins.join("sub")).unwrap();
    let source = "export {value} from '../dep.ts';";
    std::fs::write(plugins.join("sub/main.ts"), source).unwrap();
    std::fs::write(plugins.join("dep.ts"), "export const value = 1;\n").unwrap();
    std::os::unix::fs::symlink("sub/main.ts", plugins.join("main.ts")).unwrap();
    for mode in [Semantic, Prepare] {
        let checked = report(&plugins, source, mode);
        assert!(
            checked.diagnostics.is_empty(),
            "{mode:?}: {:?}",
            checked.diagnostics
        );
    }
    std::fs::write(dir.path().join("outside.ts"), "export const value = 2;\n").unwrap();
    for mode in [Semantic, Prepare] {
        let checked = report(&plugins, "export {value} from '../../outside.ts';", mode);
        assert!(
            checked
                .diagnostics
                .join("\n")
                .contains("escapes plugin directory"),
            "{checked:?}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(plugins.join("sub/main.ts")).unwrap(),
        source
    );
}

#[test]
fn bare_relative_and_absolute_files_check_and_format_sibling_imports() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    let source = "export {value} from './dep.ts'";
    let dependency = "export const value=1";
    std::fs::write(dir.path().join("dep.ts"), dependency).unwrap();
    for path in [
        PathBuf::from("main.ts"),
        PathBuf::from("./main.ts"),
        entry.clone(),
    ] {
        std::fs::write(&entry, source).unwrap();
        for args in [
            vec!["plugins", "check", "--file"],
            vec!["plugins", "format", "--file"],
            vec!["plugins", "format", "--check", "--file"],
        ] {
            let output = Command::new(std::env::var_os("ZELLIJ_COMPILATION_WORKER").unwrap())
                .current_dir(dir.path())
                .env("HOME", dir.path().join("empty-home"))
                .env("XDG_CONFIG_HOME", dir.path().join("empty-config"))
                .args(args)
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            std::fs::read_to_string(&entry).unwrap(),
            "export { value } from \"./dep.ts\";\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("main.bak.ts")).unwrap(),
            source
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
            dependency
        );
    }
}

#[test]
fn suppression_policy_rejects_blanket_unexplained_and_unused_directives() {
    let dir = tempfile::tempdir().unwrap();
    for source in [
        "// oxlint-disable\nexport const x = 1;",
        "// oxlint-disable-next-line typescript/no-explicit-any\nexport const x: any = 1;",
        "// @ts-nocheck\nexport const x: string = 1;",
        "/* eslint typescript/no-explicit-any: off */\nexport const x: any = 1;",
        "// prettier-ignore\nexport const x = 1;",
        "/* global injected */\nexport const x = injected;",
        "// oxlint-disable-next-line typescript/no-explicit-any -- no unsafe value here\nexport const x = 1;",
    ] {
        assert!(!report(dir.path(), source, Semantic).diagnostics.is_empty(), "{source}");
    }
    let source = "export const text = '// oxlint-disable';";
    assert!(report(dir.path(), source, Syntax).diagnostics.is_empty());
}

#[test]
fn saved_format_check_is_read_only_and_format_keeps_the_previous_version() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.ts");
    let source = "export const label='λ'";
    std::fs::write(&file, source).unwrap();
    assert!(crate::plugin_authoring::format_file(&file, true).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
    assert!(!dir.path().join("main.bak.ts").exists());
    crate::plugin_authoring::format_file(&file, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.bak.ts")).unwrap(),
        source
    );
    crate::plugin_authoring::format_file(&file, true).unwrap();
}

#[test]
fn missing_and_incompatible_bundles_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        format!("{:#}", bundle::verify_dir(dir.path()).unwrap_err()).contains("missing bundled")
    );
    std::fs::write(dir.path().join("oxlint"), "wrong version").unwrap();
    assert!(format!("{:#}", bundle::verify_dir(dir.path()).unwrap_err()).contains("hash mismatch"));
}

#[test]
fn declarations_aliases_and_unicode_diagnostics_use_original_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("dep.d.ts"),
        "export interface Label { text: string; }\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("dep.d.ts", dir.path().join("alias.d.ts")).unwrap();
    let checked = report(
        dir.path(),
        "import type {Label} from './alias.d.ts'; export const label: Label = {text: 'λ'};",
        Prepare,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let source = "export const λ = 'λ'; export const number: number = λ;";
    let checked = report(dir.path(), source, Semantic);
    let diagnostic = checked.diagnostics.join("\n");
    assert!(
        diagnostic.contains(
            &dir.path()
                .canonicalize()
                .unwrap()
                .join("main.ts")
                .display()
                .to_string()
        ),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("authoring-tmp"), "{diagnostic}");
    assert!(diagnostic.contains("(1,36)"), "{diagnostic}");
}

#[test]
fn failed_write_preserves_source_and_backup() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.ts");
    let backup = dir.path().join("main.bak.ts");
    std::fs::write(&file, "export const original = 1;\n").unwrap();
    std::fs::write(&backup, "previous backup").unwrap();
    let result = crate::plugin_authoring::write(
        dir.path(),
        "main.ts",
        "export const broken: string = 1;",
        true,
        true,
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "export const original = 1;\n"
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "previous backup");
    crate::plugin_authoring::write(
        dir.path(),
        "main.ts",
        "export const updated='λ'",
        true,
        true,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "export const updated = \"λ\";\n"
    );
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        "export const original = 1;\n"
    );
}

#[test]
fn native_resolution_preserves_exact_extensions_and_import_locations() {
    let dir = tempfile::tempdir().unwrap();
    for (extension, value) in [("js", "1"), ("ts", "'ts'"), ("mjs", "2"), ("mts", "'mts'")] {
        std::fs::write(
            dir.path().join(format!("foo.{extension}")),
            format!("export const value = {value};\n"),
        )
        .unwrap();
    }
    std::os::unix::fs::symlink("foo.js", dir.path().join("alias.js")).unwrap();
    let source = r#"import { value as js } from "./alias.js";
import { value as ts } from "./foo.ts";
import { value as mjs } from "./foo.mjs";
import { value as mts } from "./foo.mts";
export { value } from "./foo.js";
export const values: [number, string, number, string] = [js, ts, mjs, mts];
export const typed: typeof import("./foo.js").value = 1;
"#;
    for mode in [Semantic, Prepare] {
        let result = report(dir.path(), source, mode);
        assert!(
            result.diagnostics.is_empty(),
            "{mode:?}: {:?}",
            result.diagnostics
        );
        if mode == Prepare {
            assert_eq!(result.formatted_source.as_deref(), Some(source));
        }
    }
    for source in [
        "import './foo.js'; export const λ = JSON.parse('1');",
        "import './foo.\\\njs'; export const λ = JSON.parse('1');",
        "export const emoji = '😀'; import './foo.js'; export const λ = JSON.parse('1');",
    ] {
        let result = report(dir.path(), source, Semantic);
        let diagnostic = result.diagnostics.join("\n");
        let before = source.split('λ').next().unwrap();
        let row = before.chars().filter(|&ch| ch == '\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap().len() + 1;
        assert!(
            diagnostic.contains(&format!("main.ts:{row}:{column}:")),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("authoring-tmp"), "{diagnostic}");
    }
}

#[test]
fn formatting_accepts_filesystem_names_and_refuses_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let source = "export const value=1";
    for name in ["space name".to_owned(), "λ插件".to_owned(), "x".repeat(200)] {
        let file = dir.path().join(format!("{name}.ts"));
        let backup = dir.path().join(format!("{name}.bak.ts"));
        std::fs::write(&file, source).unwrap();
        crate::plugin_authoring::format_file(&file, false).unwrap();
        crate::plugin_authoring::format_file(&file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), source);
        std::fs::remove_file(&backup).unwrap();
        std::os::unix::fs::symlink(dir.path().join("untouched"), &backup).unwrap();
        std::fs::write(&file, source).unwrap();
        assert!(crate::plugin_authoring::format_file(&file, false)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        assert!(!dir.path().join("untouched").exists());
    }
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("main.ts");
    std::fs::write(&file, source).unwrap();
    std::fs::write(outside.path().join("main.bak.ts"), "previous").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(&file, dir.path().join("linked.ts")).unwrap();
    for path in [
        dir.path().join("linked/main.ts"),
        dir.path().join("linked.ts"),
    ] {
        assert!(crate::plugin_authoring::format_file(&path, false)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
    }
    assert_eq!(std::fs::read_to_string(file).unwrap(), source);
    assert_eq!(
        std::fs::read_to_string(outside.path().join("main.bak.ts")).unwrap(),
        "previous"
    );
}
