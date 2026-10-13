use super::*;
use yinz_types::authoring::PluginCheckMode::{Format, Prepare, Semantic, Syntax};

fn report(
    dir: &Path,
    source: &str,
    mode: PluginCheckMode,
) -> yinz_types::authoring::PluginCheckReport {
    crate::plugin_authoring::check_source(&dir.join("main.ts"), source, mode)
}

fn check_and_write(dir: &Path, name: &str, source: &str) {
    let entry = dir.join(name);
    for mode in [Semantic, Prepare] {
        let result = crate::plugin_authoring::check_source(&entry, source, mode);
        assert!(
            result.diagnostics.is_empty(),
            "{name}, {mode:?}: {:?}",
            result.diagnostics
        );
        assert_eq!(
            result.formatted_source.as_deref(),
            (mode == Prepare).then_some(source)
        );
        assert!(!entry.exists());
    }
    crate::plugin_authoring::write(dir, name, source, false, true).unwrap();
    assert_eq!(std::fs::read_to_string(entry).unwrap(), source);
}

fn reject_check_and_write(dir: &Path, source: &str, expected: &str) {
    let file = dir.join("main.ts");
    let backup = dir.join("main.bak.ts");
    let original = "export const original = 1;\n";
    std::fs::write(&file, original).unwrap();
    std::fs::write(&backup, "previous backup").unwrap();
    let checked = report(dir, source, Semantic);
    assert!(
        checked.diagnostics.join("\n").contains(expected),
        "{source}: {checked:?}"
    );
    let error = crate::plugin_authoring::write(dir, "main.ts", source, true, true).unwrap_err();
    assert!(
        format!("{error:#}").contains(expected),
        "{source}: {error:#}"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), original);
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "previous backup");
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

fn reject_unsafe_draft(source: &str, expected: &str) {
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
    let started = std::time::Instant::now();
    eprintln!("strict-authoring start: {expected}");
    let result = report(dir.path(), source, Semantic);
    eprintln!("strict-authoring done: {expected} {:?}", started.elapsed());
    assert!(
        result.diagnostics.join("\n").contains(expected),
        "{source}: {:?}",
        result.diagnostics
    );
    assert!(result.formatted_source.is_none());
}

macro_rules! unsafe_drafts {
    ($($name:ident: ($source:expr, $expected:expr),)*) => {
        $(#[test] fn $name() { reject_unsafe_draft($source, $expected); })*
    };
}

unsafe_drafts! {
    strict_rejects_explicit_any: ("export const value: any = 1;", "no-explicit-any"),
    strict_rejects_non_null_assertion: ("export const values: string[] = []; export const value = values[0]!;", "no-non-null-assertion"),
    strict_rejects_unsafe_assignment: ("export const value = JSON.parse('1');", "no-unsafe-assignment"),
    strict_rejects_unsafe_return: ("export function value() { return JSON.parse('1'); }", "no-unsafe-return"),
    strict_rejects_unsafe_call: ("JSON.parse('1').call();", "no-unsafe-call"),
    strict_rejects_unsafe_member_access: ("export const value = JSON.parse('1').field;", "no-unsafe-member-access"),
    strict_rejects_unchecked_index: ("export const values: string[] = []; export const value: string = values[0];", "undefined"),
    strict_rejects_exact_optional_properties: ("export const value: {name?: string} = {name: undefined};", "exactOptionalPropertyTypes"),
    strict_rejects_index_signature: ("export const values: Record<string,string> = {}; export const value = values.name;", "index signature"),
    strict_rejects_unused_local: ("const unused = 1; export {};", "never read"),
    strict_rejects_missing_return: ("export function value(flag: boolean) { if (flag) return 1; }", "return"),
    strict_rejects_unsafe_type_assertion: ("export const value = (1 as unknown) as string;", "no-unsafe-type-assertion"),
    strict_rejects_floating_promise: ("import z from 'yinz:plugin'; void z.waitForPermissions(['ReadFiles']);", "no-floating-promises"),
    strict_rejects_misused_promise: ("export const value: () => void = async () => { throw new Error('rejected'); };", "no-misused-promises"),
    strict_rejects_missing_override: ("class Base { value(): void {} } export class Child extends Base { value(): void {} }", "override"),
    strict_rejects_switch_fallthrough: ("export function value(x: number): number { switch(x) { case 0: x += 1; case 1: return x; default: return 2; } }", "Fallthrough"),
    strict_rejects_exhaustive_switch: ("export function value(x: 'a' | 'b'): number { switch (x) { case 'a': return 1; } return 0; }", "switch-exhaustiveness-check"),
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
fn authoring_stages_embedded_modules_without_adjacent_files() {
    let dir = tempfile::tempdir().unwrap();
    let result = report(
        dir.path(),
        "import {view} from 'yinz:workflow-view'; export const render = view.render;",
        Format,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result
        .formatted_source
        .unwrap()
        .contains("yinz:workflow-view"));
    assert!(!dir.path().join("workflow-view.ts").exists());
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
            let output = Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
                .env("YINZ_TEST_CACHE_ROOT", &cache)
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
            let output = Command::new(std::env::var_os("YINZ_COMPILATION_WORKER").unwrap())
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
fn explained_typescript_exceptions_check_and_write_entrypoints_and_dependencies() {
    for prefix in ["//", "///"] {
        let dir = tempfile::tempdir().unwrap();
        // Even a one-character explanation is nonempty. @ts-check remains permitted.
        let source = format!(
            "{prefix} @ts-check\n{prefix} @ts-expect-error -- x\nexport const value: string = 1;\n"
        );
        check_and_write(dir.path(), "dep.ts", &source);
        check_and_write(
            dir.path(),
            "main.ts",
            "export { value } from \"./dep.ts\";\n",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
            source
        );
    }
}

const FORBIDDEN_COMPILER_DIRECTIVES: &[&str] = &[
    "// @ts-nocheck",
    "/// @ts-nocheck",
    "///@ts-nocheck",
    "// @TS-NoCheck",
    "///\u{feff}@ts-nocheck",
    "// @ts-ignore",
    "/// @ts-ignore -- Still forbidden.",
    "// @ts-expect-error",
    "/// @ts-expect-error",
    "// @ts-expect-error --",
    "/// @ts-expect-error -- \t ",
    "// @ts-expect-error -- \u{a0} ",
    "// @ts-expect-error An explanation without the separator.",
    "/* @ts-expect-error -- Block comments are forbidden. */",
    "/** @ts-expect-error -- JSDoc is a block comment. */",
    "/* Explanation\n * @ts-expect-error -- Last-line directives also suppress errors. */",
    "/* Explanation\r * @ts-ignore */",
    "/* Explanation\u{2028} * @ts-ignore */",
    "/* @ts-nocheck */",
];

#[test]
fn compiler_suppression_spellings_cannot_bypass_entrypoint_checks_or_writes() {
    let dir = tempfile::tempdir().unwrap();
    for directive in FORBIDDEN_COMPILER_DIRECTIVES {
        let source = format!("{directive}\nexport const value: string = 1;\n");
        let checked = report(dir.path(), &source, Syntax);
        assert!(
            checked.diagnostics.join("\n").contains("only explained"),
            "{source}: {checked:?}"
        );
        reject_check_and_write(dir.path(), &source, "only explained");
    }
}

#[test]
fn compiler_suppression_spellings_cannot_bypass_dependency_checks_or_writes() {
    let dir = tempfile::tempdir().unwrap();
    let dependency = dir.path().join("dep.ts");
    for directive in FORBIDDEN_COMPILER_DIRECTIVES {
        let source = format!("{directive}\nexport const value: string = 1;\n");
        std::fs::write(&dependency, &source).unwrap();
        reject_check_and_write(
            dir.path(),
            "export { value } from \"./dep.ts\";\n",
            "dep.ts: only explained",
        );
        assert_eq!(std::fs::read_to_string(&dependency).unwrap(), source);
        assert!(!dir.path().join("dep.bak.ts").exists());
    }
}

#[test]
fn unused_typescript_exceptions_fail_semantics_and_writes_but_not_syntax_or_format() {
    for prefix in ["//", "///"] {
        let dir = tempfile::tempdir().unwrap();
        let source = format!(
            "{prefix} @ts-expect-error -- This line has no type error.\nexport const value = 1;\n"
        );
        for mode in [Syntax, Format] {
            let checked = report(dir.path(), &source, mode);
            assert!(checked.diagnostics.is_empty(), "{mode:?}: {checked:?}");
        }
        reject_check_and_write(dir.path(), &source, "Unused '@ts-expect-error' directive");
        std::fs::write(dir.path().join("dep.ts"), &source).unwrap();
        reject_check_and_write(
            dir.path(),
            "export { value } from \"./dep.ts\";\n",
            "Unused '@ts-expect-error' directive",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
            source
        );
    }
}

#[test]
fn typescript_exceptions_leave_other_type_errors_and_lint_findings() {
    let dir = tempfile::tempdir().unwrap();
    for (body, expected) in [
        (
            "export const value: string = 1;\nexport const other: number = 'wrong';\n",
            "not assignable to type 'number'",
        ),
        ("export const value: any[] = 1;\n", "no-explicit-any"),
    ] {
        let source = format!("// @ts-expect-error -- Deliberate type mismatch.\n{body}");
        reject_check_and_write(dir.path(), &source, expected);
    }
}

#[test]
fn compiler_directive_text_in_strings_is_harmless() {
    let dir = tempfile::tempdir().unwrap();
    check_and_write(
        dir.path(),
        "main.ts",
        "export const text = \"// @ts-ignore; /// @ts-nocheck; /* @ts-expect-error */\";\n",
    );
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
    reject_check_and_write(
        dir.path(),
        "export const broken: string = 1;",
        "not assignable to type 'string'",
    );
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
fn native_resolution_rewrites_module_augmentations_and_path_references() {
    for source in [
        r#"import type { Label as ImportedLabel } from "./dep.ts";
declare module "./dep.ts" {
  interface Label {
    count: number;
  }
}
export const label: ImportedLabel = { text: "ok", count: 1 };
"#,
        r#"// oxlint-disable-next-line typescript/triple-slash-reference -- Verify path reference staging.
/// <reference path='./dep.ts' />
// oxlint-disable-next-line typescript/triple-slash-reference -- Verify path reference staging.
/// <reference path="./types.d.ts" />
import type { Label } from "./dep.ts";
export const label: Label = { text: "ok" };
export const count: Count = 1;
"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let dependency = "export interface Label {text:string}";
        let declaration = "type Count=number";
        std::fs::write(dir.path().join("dep.ts"), dependency).unwrap();
        std::fs::write(dir.path().join("types.d.ts"), declaration).unwrap();
        check_and_write(dir.path(), "main.ts", source);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
            dependency
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("types.d.ts")).unwrap(),
            declaration
        );
    }
}

#[test]
fn native_resolution_ignores_typescript_jsdoc_imports() {
    let dir = tempfile::tempdir().unwrap();
    // Inactive comments must neither load this invalid dependency nor resolve the missing one.
    let dependency = "export const invalid: string = 1;";
    std::fs::write(dir.path().join("dep.ts"), dependency).unwrap();
    let source = r#"/** @import { Label } from "./dep.ts" */
/** @type {import("./dep.ts").Label} */
export const present = 1;
/** @import { Missing } from "./missing.ts" */
/** @type {import("./missing.ts").Missing} */
export const missing = 2;
"#;
    for name in ["main.ts", "main.mts"] {
        check_and_write(dir.path(), name, source);
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("dep.ts")).unwrap(),
        dependency
    );
    assert!(!dir.path().join("missing.ts").exists());
}

#[test]
fn native_resolution_rewrites_javascript_jsdoc_imports() {
    let dir = tempfile::tempdir().unwrap();
    let dependency = "export interface Label { text: string; }\n";
    std::fs::write(dir.path().join("dep.d.ts"), dependency).unwrap();
    let source = r#"/** @import { Label } from "./dep.d.ts" */
/** @type {Label} */
export const importedLabel = { text: "ok" };
/** @type {import("./dep.d.ts").Label} */
export const label = { text: "ok" };
"#;
    for name in ["main.js", "main.mjs"] {
        check_and_write(dir.path(), name, source);
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("dep.d.ts")).unwrap(),
        dependency
    );
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

#[test]
fn import_meta_passes_strict_authoring_without_resolving_targets() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"export const url: string = import.meta.url;
export const sibling: string = import.meta.resolve("./missing.ts");
"#;
    for mode in [Semantic, Prepare] {
        let checked = report(dir.path(), source, mode);
        assert!(
            checked.diagnostics.is_empty(),
            "{mode:?}: {:?}",
            checked.diagnostics
        );
        if mode == Prepare {
            assert_eq!(checked.formatted_source.as_deref(), Some(source));
        }
    }
    assert!(!dir.path().join("missing.ts").exists());
}

#[test]
fn strict_authoring_accepts_test_declarations() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"import {test, assertEquals, session} from 'yinz:testing';
test('unit', () => { assertEquals(1, 1); });
test('integration', () => { session.start(); session.resize(80, 24); });
throw new Error('source must never execute');
"#;
    for name in ["main.test.ts", "main.integration.test.ts"] {
        let path = dir.path().join(name);
        for mode in [Semantic, Prepare, Format] {
            let checked = crate::plugin_authoring::check_source(&path, source, mode);
            assert!(
                checked.diagnostics.is_empty(),
                "{name} {mode:?}: {checked:?}"
            );
            assert_eq!(checked.formatted_source.is_some(), mode != Semantic);
            assert!(!path.exists());
        }
    }
}

#[test]
fn strict_test_checks_keep_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.test.ts");
    for (source, expected) in [
        ("export const value: string = 1;", "not assignable"),
        ("export const value: any = 1;", "no-explicit-any"),
        ("export const values: string[] = []; export const value: string = values[0];", "undefined"),
        ("import {assertRejects} from 'yinz:testing'; assertRejects(() => Promise.reject(new Error('no')));", "no-floating-promises"),
        ("// @ts-ignore\nexport const value: string = 1;", "suppression"),
    ] {
        let checked = crate::plugin_authoring::check_source(&path, source, Semantic);
        let diagnostic = checked.diagnostics.join("\n");
        assert!(diagnostic.contains(expected), "{source}: {diagnostic}");
        assert!(!diagnostic.contains("authoring-tmp"), "{diagnostic}");
    }
    std::fs::write(dir.path().join("dep.ts"), "export const value = 1;").unwrap();
    let source = "export const emoji = '😀'; import './dep.ts'; import {assertEquals} from 'yinz:testing'; assertEquals(1, 1); export const λ = JSON.parse('1');";
    let checked = crate::plugin_authoring::check_source(&path, source, Semantic);
    let diagnostic = checked.diagnostics.join("\n");
    let column = source.split('λ').next().unwrap().len() + 1;
    assert!(
        diagnostic.contains(&format!("main.test.ts:1:{column}:")),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("authoring-tmp"), "{diagnostic}");
}

#[test]
fn test_declarations_do_not_escape_authoring_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let source = "import {assertEquals} from 'yinz:testing'; assertEquals(1, 1);";
    for name in ["main.ts", "main.test.js", "main.test.mts"] {
        let checked =
            crate::plugin_authoring::check_source(&dir.path().join(name), source, Semantic);
        assert!(!checked.diagnostics.is_empty(), "{name}");
    }
    std::fs::write(dir.path().join("dep.test.ts"), source).unwrap();
    assert!(checker::check_plugin(&dir.path().join("dep.test.ts")).is_err());
    assert!(!report(dir.path(), "import './dep.test.ts';", Semantic)
        .diagnostics
        .is_empty());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.ts"), "export const value = 1;").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("outside.ts"),
        dir.path().join("escape.ts"),
    )
    .unwrap();
    std::fs::write(dir.path().join("inside.ts"), "export const value = 1;").unwrap();
    for import in [
        "../outside.ts".to_owned(),
        "./escape.ts".to_owned(),
        "npm:bad".to_owned(),
        "https://example.com/a.ts".to_owned(),
        format!("file:{}", dir.path().join("inside.ts").display()),
    ] {
        let source = format!("import {import:?};");
        let checked = crate::plugin_authoring::check_source(
            &dir.path().join("main.test.ts"),
            &source,
            Semantic,
        );
        assert!(!checked.diagnostics.is_empty(), "{import}");
    }
    assert!(crate::plugin_authoring::check_source(
        &dir.path().join("main.test.ts"),
        source,
        Syntax
    )
    .diagnostics
    .is_empty());
}
