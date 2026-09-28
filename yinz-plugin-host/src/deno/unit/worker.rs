use super::super::checker;
use yinz_plugin_authoring::analysis;
use yinz_types::authoring::PluginCheckMode;

fn recursive_source(extension: &str, source: String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("deep.{extension}"));
    let error = checker::check_syntax(&path, &source).unwrap_err();
    assert!(
        format!("{error:#}").contains("compilation worker"),
        "{error:#}"
    );
    assert!(checker::check_semantic_source(&path, &source).is_err());
    for mode in [PluginCheckMode::Syntax, PluginCheckMode::Semantic] {
        let report = crate::plugin_authoring::check_source(&path, &source, mode);
        assert!(!report.diagnostics.is_empty());
    }
    checker::check_syntax(&path, "export default {};").unwrap();
    checker::check_semantic_source(&path, "export default {};").unwrap();
    assert!(
        analysis::check(&path, "export default {};", PluginCheckMode::Semantic)
            .diagnostics
            .is_empty()
    );
}

fn nested(prefix: &str, suffix: &str, depth: usize) -> String {
    format!("{}0{}", prefix.repeat(depth), suffix.repeat(depth))
}

#[test]
fn recursive_js_arrays_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("[", "]", 10_000)),
    );
}
#[test]
fn recursive_ts_arrays_are_contained() {
    recursive_source(
        "ts",
        format!("export default {};", nested("[", "]", 10_000)),
    );
}
#[test]
fn recursive_objects_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("{a:", "}", 10_000)),
    );
}
#[test]
fn recursive_unary_expressions_are_contained() {
    recursive_source(
        "js",
        format!("export default {};", nested("!", "", 100_000)),
    );
}
#[test]
fn recursive_binary_chains_are_contained() {
    recursive_source(
        "ts",
        format!("export default {};", nested("1+", "", 100_000)),
    );
}
#[test]
fn recursive_types_are_contained() {
    recursive_source("ts", format!("type Deep = {};", nested("{a:", "}", 10_000)));
}

#[test]
fn semantic_dependency_native_failure_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let dep = dir.path().join("dep.ts");
    let main = dir.path().join("main.ts");
    let source = "import './dep.ts'; export default {};";
    std::fs::write(
        &dep,
        format!(
            "export default {}0{};",
            "[".repeat(10_000),
            "]".repeat(10_000)
        ),
    )
    .unwrap();
    let error = checker::check_semantic_source(&main, source).unwrap_err();
    assert!(format!("{error:#}").contains("dep.ts"), "{error:#}");
    std::fs::write(&dep, "export default {};").unwrap();
    checker::check_semantic_source(&main, source).unwrap();
}
