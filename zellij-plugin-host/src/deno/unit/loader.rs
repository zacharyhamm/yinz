use super::*;

#[test]
fn deno_source_and_graph_budgets_apply_before_parsing() {
    let (dir, loader) = fixture();
    let oversized = " ".repeat(limits::MAX_SOURCE_BYTES + 1);
    assert!(PluginModuleLoader::new(&dir.path().join("main.ts"), &oversized).is_err());
    assert!(super::super::check_syntax(&dir.path().join("main.ts"), &oversized).is_err());
    let path = dir.path().join("large.ts");
    std::fs::write(&path, &oversized).unwrap();
    assert!(loader
        .read_source(&file_specifier(&path).unwrap(), true)
        .is_err());
    let source = " ".repeat(limits::MAX_SOURCE_BYTES);
    for index in 0..8 {
        let path = dir.path().join(format!("dep{index}.ts"));
        std::fs::write(&path, &source).unwrap();
        let url = file_specifier(&path).unwrap();
        assert_eq!(loader.read_source(&url, true).unwrap().len(), source.len());
        assert!(
            loader.read_source(&url, true).is_ok(),
            "reading a cached module must not double-charge"
        );
    }
    assert!(loader
        .read_source(loader.entrypoint(), true)
        .unwrap_err()
        .to_string()
        .contains("module-graph budget"));
}

fn fixture() -> (tempfile::TempDir, PluginModuleLoader) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.ts"), "export const n: number = 1;").unwrap();
    let loader =
        PluginModuleLoader::new(&dir.path().join("main.ts"), "export const n: number = 1;")
            .unwrap();
    (dir, loader)
}

#[test]
fn explicit_local_modules_only() {
    let (dir, loader) = fixture();
    for name in ["dep.js", "dep.mjs", "dep.ts", "dep.mts"] {
        std::fs::write(dir.path().join(name), "export const n = 1;").unwrap();
        let url = loader
            .resolve(
                &format!("./{name}"),
                loader.entrypoint().as_str(),
                ResolutionKind::Import,
            )
            .unwrap();
        assert!(loader.load_source(&url).is_ok());
    }
    for name in [
        "react",
        "node:fs",
        "npm:x",
        "https://example.com/x.ts",
        "data:text/javascript,1",
        "/dep.ts",
        "file:///dep.ts",
        "./dep",
        "./dep.ts?q",
        "./dep.ts#x",
        "./dep\\x.ts",
    ] {
        assert!(
            loader
                .resolve(name, loader.entrypoint().as_str(), ResolutionKind::Import)
                .is_err(),
            "{name}"
        );
        assert!(
            loader
                .resolve(
                    name,
                    loader.entrypoint().as_str(),
                    ResolutionKind::DynamicImport
                )
                .is_err(),
            "{name}"
        );
    }
    for name in [
        "dep.tsx",
        "dep.jsx",
        "dep.json",
        "dep.cjs",
        "dep.cts",
        "dep.d.mts",
        "dep.d.ts",
    ] {
        std::fs::write(dir.path().join(name), "").unwrap();
        assert!(
            loader
                .resolve(
                    &format!("./{name}"),
                    loader.entrypoint().as_str(),
                    ResolutionKind::Import
                )
                .is_err(),
            "{name}"
        );
    }
    let declarations = loader
        .resolve_local("./dep.d.ts", loader.entrypoint().as_str(), true)
        .unwrap();
    assert!(loader.load_source(&declarations).is_err());
    let sdk = loader
        .resolve(
            SDK_SPECIFIER,
            loader.entrypoint().as_str(),
            ResolutionKind::Import,
        )
        .unwrap();
    assert!(loader.load_source(&sdk).is_ok());
}

#[test]
fn transpilation_keeps_source_maps_and_source_lines() {
    let (_dir, loader) = fixture();
    let module = loader.load_source(loader.entrypoint()).unwrap();
    let ModuleSourceCode::String(code) = module.code else {
        panic!("expected text")
    };
    assert!(!code.as_str().contains(": number"));
    let map = loader.get_source_map(loader.entrypoint().as_str()).unwrap();
    let map: serde_json::Value = serde_json::from_slice(&map).unwrap();
    assert_eq!(map["sourcesContent"][0], "export const n: number = 1;");
    assert_eq!(
        loader
            .get_source_mapped_source_line(loader.entrypoint().as_str(), 0)
            .as_deref(),
        Some("export const n: number = 1;")
    );
}

#[cfg(unix)]
#[test]
fn traversal_and_symlink_escapes_are_rejected_at_resolve_and_load() {
    use std::os::unix::fs::symlink;
    let (dir, loader) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.ts");
    std::fs::write(&secret, "export const secret = 1;").unwrap();
    symlink(&secret, dir.path().join("escape.ts")).unwrap();
    symlink(outside.path(), dir.path().join("escape-dir")).unwrap();
    for name in ["./escape.ts", "./escape-dir/secret.ts"] {
        assert!(loader
            .resolve_local(name, loader.entrypoint().as_str(), true)
            .is_err());
    }
    assert!(PluginModuleLoader::new(&dir.path().join("escape.ts"), "export {};").is_err());
    assert!(loader
        .load_source(&file_specifier(&secret.canonicalize().unwrap()).unwrap())
        .is_err());
    let local = dir.path().join("local.ts");
    std::fs::write(&local, "export {};").unwrap();
    symlink(&local, dir.path().join("alias.ts")).unwrap();
    assert_eq!(
        loader
            .resolve_local("./alias.ts", loader.entrypoint().as_str(), false)
            .unwrap(),
        file_specifier(&local.canonicalize().unwrap()).unwrap()
    );
    let url = loader
        .resolve_local("./local.ts", loader.entrypoint().as_str(), false)
        .unwrap();
    std::fs::remove_file(&local).unwrap();
    symlink(&secret, &local).unwrap();
    assert!(loader.load_source(&url).is_err());
    let outside_name = outside.path().file_name().unwrap().to_str().unwrap();
    assert!(loader
        .resolve_local(
            &format!("../{outside_name}/secret.ts"),
            loader.entrypoint().as_str(),
            false
        )
        .is_err());
}

#[test]
fn invalid_syntax_is_rejected_without_evaluation() {
    let (dir, loader) = fixture();
    for (name, source) in [
        ("invalid.ts", "export const = ;"),
        ("invalid.js", "const n: number = 1"),
        ("jsx.ts", "const x = <div />"),
    ] {
        std::fs::write(dir.path().join(name), source).unwrap();
        let url = loader
            .resolve_local(&format!("./{name}"), loader.entrypoint().as_str(), false)
            .unwrap();
        assert!(loader.load_source(&url).is_err());
    }
}

#[tokio::test]
async fn runtime_loads_sdk_local_graph_and_the_supplied_buffer() {
    use deno_core::{JsRuntime, RuntimeOptions};
    use std::rc::Rc;
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    std::fs::write(&entry, "throw new Error('stale source');").unwrap();
    std::fs::write(dir.path().join("dep.mts"), "export const n: number = 42;").unwrap();
    let loader = Rc::new(PluginModuleLoader::new(&entry,
        "import {n} from './dep.mts'; import zellij, {definePlugin} from 'zellij:plugin';\nif (definePlugin(n) !== 42 || zellij !== globalThis.zellij) throw new Error('bad SDK');"
    ).unwrap());
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        module_loader: Some(loader.clone()),
        ..Default::default()
    })
    .unwrap();
    runtime
        .execute_script(
            "zellij:test-sdk",
            "globalThis.zellij = { definePlugin: x => x };",
        )
        .unwrap();
    let module = runtime
        .load_main_es_module(loader.entrypoint())
        .await
        .unwrap();
    let evaluated = runtime.mod_evaluate(module);
    runtime.run_event_loop(Default::default()).await.unwrap();
    evaluated.await.unwrap();
}

#[tokio::test]
async fn runtime_errors_point_to_original_typescript() {
    use deno_core::{JsRuntime, RuntimeOptions};
    use std::rc::Rc;
    let dir = tempfile::tempdir().unwrap();
    let source = "enum Status { Ready }\n\nfunction boom() {\n  throw new Error('mapped failure');\n}\nboom();";
    let loader = Rc::new(PluginModuleLoader::new(&dir.path().join("main.ts"), source).unwrap());
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        module_loader: Some(loader.clone()),
        ..Default::default()
    })
    .unwrap();
    let module = runtime
        .load_main_es_module(loader.entrypoint())
        .await
        .unwrap();
    let evaluated = runtime.mod_evaluate(module);
    let error = match runtime.run_event_loop(Default::default()).await {
        Err(error) => error.to_string(),
        Ok(()) => evaluated.await.unwrap_err().to_string(),
    };
    assert!(error.contains("main.ts:4:"), "{error}");
}
