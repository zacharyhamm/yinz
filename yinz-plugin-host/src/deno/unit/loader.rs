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
        // A `.js` file is parsed by the worker like any other: the cache remembers only
        // successful emissions, so a syntax error costs a spawn each time and is still
        // reported here, before V8 ever sees the module.
        let spawns = spawn_count();
        assert!(loader.load_source(&url).is_err());
        assert_eq!(spawn_count() - spawns, 1, "{name}");
        assert!(loader.load_source(&url).is_err());
        assert_eq!(spawn_count() - spawns, 2, "{name}");
    }
}

fn spawn_count() -> usize {
    super::super::worker::spawn_count()
}

#[test]
fn identical_source_is_transpiled_once_and_an_edit_misses() {
    let (dir, mut loader) = fixture();
    let spawns = spawn_count();
    loader.load_source(loader.entrypoint()).unwrap();
    assert_eq!(spawn_count() - spawns, 1);
    loader.load_source(loader.entrypoint()).unwrap();
    assert_eq!(
        spawn_count() - spawns,
        1,
        "the same content at the same path is served from the cache"
    );
    loader.sources.borrow_mut().clear();
    loader
        .set_entry_source("export const n: number = 2;")
        .unwrap();
    let module = loader.load_source(loader.entrypoint()).unwrap();
    let ModuleSourceCode::String(code) = module.code else {
        panic!("expected text")
    };
    assert!(code.as_str().contains("= 2"), "{}", code.as_str());
    assert_eq!(spawn_count() - spawns, 2, "an edit misses");
    // The same text under another name is another module: its source map must name it.
    let other = dir.path().join("other.ts");
    std::fs::write(&other, "export const n: number = 2;").unwrap();
    let url = loader
        .resolve_local("./other.ts", loader.entrypoint().as_str(), false)
        .unwrap();
    loader.load_source(&url).unwrap();
    assert_eq!(spawn_count() - spawns, 3, "the path is part of the key");
    let map = loader.get_source_map(url.as_str()).unwrap();
    let map: serde_json::Value = serde_json::from_slice(&map).unwrap();
    assert!(
        map["sources"][0].as_str().unwrap().ends_with("other.ts"),
        "{map}"
    );
}

#[test]
fn embedded_builtins_stay_cached_under_eviction_pressure() {
    let source = "export const value: number = 42;";
    let loader = PluginModuleLoader::embedded("context-menu", source).unwrap();
    let spawns = spawn_count();
    loader.load_source(loader.entrypoint()).unwrap();
    assert_eq!(spawn_count() - spawns, 1);
    cache_fill_for_test(MAX_CACHE_ENTRIES + 1);
    let (entries, bytes) = cache_stats();
    assert!(entries <= MAX_CACHE_ENTRIES, "{entries} entries");
    assert!(bytes <= MAX_CACHE_BYTES, "{bytes} bytes");
    let again = PluginModuleLoader::embedded("context-menu", source).unwrap();
    again.load_source(again.entrypoint()).unwrap();
    assert_eq!(
        spawn_count() - spawns,
        1,
        "a compiled-in builtin is pinned through eviction"
    );
    // The registered workflow module is pinned the same way.
    let view = PluginModuleLoader::embedded(
        "view-test",
        "import {view} from 'yinz:workflow-view'; export default view;",
    )
    .unwrap();
    let url = view
        .resolve_local(WORKFLOW_SPECIFIER, view.entrypoint().as_str(), false)
        .unwrap();
    let before = spawn_count();
    view.load_source(&url).unwrap();
    let first = spawn_count() - before;
    cache_fill_for_test(MAX_CACHE_ENTRIES + 1);
    view.load_source(&url).unwrap();
    assert_eq!(spawn_count() - before, first);
}

#[test]
fn transpile_cache_evicts_least_recently_used_unpinned_entries_within_bounds() {
    fn emitted(text: &str) -> Arc<deno_ast::EmittedSourceText> {
        Arc::new(deno_ast::EmittedSourceText {
            text: text.to_owned(),
            source_map: None,
        })
    }
    let key = |name: &str| cache_key(Path::new(name), name, false);
    let mut cache = TranspileCache::default();
    cache.insert(key("pinned"), emitted("pinned"), true);
    for index in 0..MAX_CACHE_ENTRIES {
        cache.insert(key(&format!("entry{index}")), emitted("x"), false);
    }
    assert_eq!(cache.entries.len(), MAX_CACHE_ENTRIES);
    assert!(cache.get(&key("pinned")).is_some(), "pinned survives");
    assert!(
        cache.get(&key("entry0")).is_none(),
        "the oldest unpinned entry went first"
    );
    // Touching an entry makes it recent; the next eviction takes the untouched one.
    assert!(cache.get(&key("entry1")).is_some());
    cache.insert(key("newest"), emitted("x"), false);
    assert!(cache.get(&key("entry1")).is_some());
    assert!(cache.get(&key("entry2")).is_none());
    // Bytes bound: an entry larger than the whole budget is not stored at all, and a large
    // entry evicts by bytes before the count is reached.
    cache.insert(
        key("huge"),
        emitted(&"h".repeat(MAX_CACHE_BYTES + 1)),
        false,
    );
    assert!(cache.get(&key("huge")).is_none());
    cache.insert(
        key("large"),
        emitted(&"l".repeat(MAX_CACHE_BYTES - 8)),
        false,
    );
    assert!(
        cache.total_bytes <= MAX_CACHE_BYTES,
        "{}",
        cache.total_bytes
    );
    assert!(cache.get(&key("large")).is_some());
    assert!(cache.get(&key("pinned")).is_some());
    assert!(cache.entries.len() < MAX_CACHE_ENTRIES);
    // Replacing a key does not double count.
    let before = cache.total_bytes;
    cache.insert(key("pinned"), emitted("pinned"), true);
    assert_eq!(cache.total_bytes, before);
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
        "import {n} from './dep.mts'; import yinz, {definePlugin} from 'yinz:plugin';\nif (definePlugin(n) !== 42 || yinz !== globalThis.yinz) throw new Error('bad SDK');"
    ).unwrap());
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        module_loader: Some(loader.clone()),
        ..Default::default()
    })
    .unwrap();
    runtime
        .execute_script(
            "yinz:test-sdk",
            "globalThis.yinz = { definePlugin: x => x };",
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

#[test]
fn embedded_source_maps_keep_the_private_module_url_and_typescript_source() {
    let source = "export const value: number = 42;";
    let loader = PluginModuleLoader::embedded("context-menu", source).unwrap();
    loader.load_source(loader.entrypoint()).unwrap();
    let map = loader.get_source_map(loader.entrypoint().as_str()).unwrap();
    let map: serde_json::Value = serde_json::from_slice(&map).unwrap();
    assert_eq!(map["sources"][0], "yinz-builtin:///context-menu.ts");
    assert_eq!(map["sourcesContent"][0], source);
    assert_eq!(
        loader
            .get_source_mapped_source_line(loader.entrypoint().as_str(), 0)
            .as_deref(),
        Some(source)
    );
    for specifier in [
        "./sibling.ts",
        "../escape.ts",
        "file:///tmp/a.ts",
        "npm:x",
        "https://example.com/a.ts",
        "yinz-builtin:///context-menu.ts",
    ] {
        for kind in [ResolutionKind::Import, ResolutionKind::DynamicImport] {
            assert!(loader
                .resolve(specifier, loader.entrypoint().as_str(), kind)
                .is_err());
        }
    }
}

#[test]
fn embedded_modules_resolve_relative_imports_without_filesystem_fallback() {
    let loader = PluginModuleLoader::embedded(
        "view-test",
        "import {view} from 'yinz:workflow-view'; export default view;",
    )
    .unwrap();
    let url = loader
        .resolve_local(WORKFLOW_SPECIFIER, loader.entrypoint().as_str(), false)
        .unwrap();
    assert_eq!(
        loader.read_source(&url, false).unwrap(),
        yinz_deno_builtins::BUILTIN_DENO_FILES
            .get_file("workflow-view.ts")
            .unwrap()
            .contents_utf8()
            .unwrap()
    );
    assert!(loader.load_source(&url).is_ok());
    assert_eq!(
        loader
            .resolve_local("./workflow-view.ts", loader.entrypoint().as_str(), false)
            .unwrap(),
        url
    );
    assert_eq!(
        loader
            .resolve_local("./workflow-view.ts", url.as_str(), false)
            .unwrap(),
        url
    );
    assert_eq!(
        loader
            .resolve_local("./%77orkflow-view.ts", url.as_str(), false)
            .unwrap(),
        url
    );
    let nested = PluginModuleLoader::embedded("nested/main", "export default {};").unwrap();
    assert_eq!(
        nested
            .resolve_local("../workflow-view.ts", nested.entrypoint().as_str(), false)
            .unwrap(),
        url
    );
    let dir = tempfile::tempdir().unwrap();
    let local_module = dir.path().join("missing.ts");
    std::fs::write(&local_module, "export const present = true;").unwrap();
    for specifier in [
        "./missing.ts",
        "./workflow-view.ts?x",
        "../Cargo.toml",
        "file:///tmp/workflow-view.ts",
        "yinz-builtin:///workflow-view.ts",
        "yinz:workflow-view?x",
        "yinz:missing",
        local_module.to_str().unwrap(),
    ] {
        assert!(loader
            .resolve_local(specifier, loader.entrypoint().as_str(), false)
            .is_err());
        assert!(loader
            .resolve_local(specifier, url.as_str(), false)
            .is_err());
    }
    let maps = loader.checked_sources().unwrap();
    assert!(maps.contains_key(&Path::new(EMBEDDED_ROOT).join("workflow-view.ts")));
}
