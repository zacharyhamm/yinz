use super::*;

#[test]
fn repository_imports_share_the_runtime_and_semantic_boundary() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("src")).unwrap();
    let path = directory.path().join("src/main.ts");
    let source =
        "import {value} from '../shared.ts'; export default {render(): string {return value;}};";
    std::fs::write(&path, source).unwrap();
    std::fs::write(
        directory.path().join("shared.ts"),
        "export const value: string = 'shared';",
    )
    .unwrap();
    let loader = PluginModuleLoader::new_with_root(&path, source, Some(directory.path())).unwrap();
    let url = loader
        .resolve(
            "../shared.ts",
            loader.entrypoint().as_str(),
            ResolutionKind::Import,
        )
        .unwrap();
    assert!(loader.load_source(&url).is_ok());
    let installed = crate::plugin_authoring::git::InstalledPlugin {
        entrypoint: path,
        root: directory.path().canonicalize().unwrap(),
        commit: None,
    };
    let report = crate::plugin_authoring::check_repository(&installed).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("shared.ts"),
        "export const value = 'escape';",
    )
    .unwrap();
    std::fs::remove_file(directory.path().join("shared.ts")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("shared.ts"),
        directory.path().join("shared.ts"),
    )
    .unwrap();
    // Use a fresh loader because sources are intentionally cached within an instance.
    let confined =
        PluginModuleLoader::new_with_root(&installed.entrypoint, source, Some(directory.path()))
            .unwrap();
    assert!(confined
        .resolve(
            "../shared.ts",
            confined.entrypoint().as_str(),
            ResolutionKind::Import
        )
        .is_err());
    assert!(!crate::plugin_authoring::check_repository(&installed)
        .unwrap()
        .diagnostics
        .is_empty());
}

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
    // An imported builtin module is pinned the same way.
    let view = PluginModuleLoader::embedded(
        "view-test",
        "import hook from './agent-exit.ts'; export default hook;",
    )
    .unwrap();
    let url = view
        .resolve_local("./agent-exit.ts", view.entrypoint().as_str(), false)
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
        "import hook from './agent-exit.ts'; export default hook;",
    )
    .unwrap();
    let url = loader
        .resolve_local("./agent-exit.ts", loader.entrypoint().as_str(), false)
        .unwrap();
    assert_eq!(
        loader.read_source(&url, false).unwrap(),
        yinz_deno_builtins::BUILTIN_DENO_FILES
            .get_file("agent-exit.ts")
            .unwrap()
            .contents_utf8()
            .unwrap()
    );
    assert!(loader.load_source(&url).is_ok());
    assert_eq!(
        loader
            .resolve_local("./agent-exit.ts", loader.entrypoint().as_str(), false)
            .unwrap(),
        url
    );
    assert_eq!(
        loader
            .resolve_local("./agent-exit.ts", url.as_str(), false)
            .unwrap(),
        url
    );
    assert_eq!(
        loader
            .resolve_local("./%61gent-exit.ts", url.as_str(), false)
            .unwrap(),
        url
    );
    let nested = PluginModuleLoader::embedded("nested/main", "export default {};").unwrap();
    assert_eq!(
        nested
            .resolve_local("../agent-exit.ts", nested.entrypoint().as_str(), false)
            .unwrap(),
        url
    );
    let dir = tempfile::tempdir().unwrap();
    let local_module = dir.path().join("missing.ts");
    std::fs::write(&local_module, "export const present = true;").unwrap();
    for specifier in [
        "./missing.ts",
        "./agent-exit.ts?x",
        "../Cargo.toml",
        "file:///tmp/agent-exit.ts",
        "yinz-builtin:///agent-exit.ts",
        "yinz:agent-exit?x",
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
    assert!(maps.contains_key(&Path::new(EMBEDDED_ROOT).join("agent-exit.ts")));
}

async fn import_meta_exports(loader: PluginModuleLoader) -> HashMap<String, String> {
    use deno_core::{v8, JsRuntime, RuntimeOptions};
    use std::rc::Rc;
    let loader = Rc::new(loader);
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
    runtime.run_event_loop(Default::default()).await.unwrap();
    evaluated.await.unwrap();
    let namespace = runtime.get_module_namespace(module).unwrap();
    deno_core::scope!(scope, runtime);
    let namespace = v8::Local::new(scope, namespace);
    deno_core::serde_v8::from_v8(scope, namespace.into()).unwrap()
}

#[tokio::test]
async fn import_meta_runtime_urls_round_trip_and_enforce_confinement() {
    use yinz_runtime::plugin_location::parse_plugin_location;
    use yinz_types::input::layout::RunPluginLocation;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("space λ%#?,");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    let sibling = root.join("shared λ%#?,.ts");
    std::fs::write(&sibling, "throw new Error('resolution must not evaluate');").unwrap();
    std::fs::write(dir.path().join("outside.ts"), "export {};").unwrap();
    std::fs::write(root.join("asset.json"), "{}").unwrap();
    std::os::unix::fs::symlink(&sibling, root.join("alias.ts")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.ts"), root.join("escape.ts")).unwrap();
    for repository in [false, true] {
        let (entry, prefix) = if repository {
            (root.join("nested/main λ%#?,.ts"), "../")
        } else {
            (root.join("main λ%#?,.ts"), "./")
        };
        let rejected = serde_json::to_string(&[
            format!("{prefix}missing.ts"),
            format!("{prefix}../outside.ts"),
            format!("{prefix}escape.ts"),
            format!("{prefix}asset.json"),
            format!("{prefix}alias.ts?query"),
            format!("{prefix}alias.ts#fragment"),
            "node:fs".into(),
            "npm:package".into(),
            "https://example.com/main.ts".into(),
            file_specifier(&sibling).unwrap().to_string(),
        ])
        .unwrap();
        let source = format!(
            r#"
export const url: string = import.meta.url;
export const sibling: string = import.meta.resolve("{prefix}shared%20%CE%BB%25%23%3F%2C.ts");
export const alias: string = import.meta.resolve("{prefix}alias.ts");
for (const specifier of {rejected}) {{
  let rejected = false;
  try {{ import.meta.resolve(specifier); }}
  catch (error) {{
    if (!(error instanceof TypeError)) throw error;
    rejected = true;
  }}
  if (!rejected) throw new Error(`accepted ${{specifier}}`);
}}
"#
        );
        std::fs::write(&entry, &source).unwrap();
        let exports = import_meta_exports(
            PluginModuleLoader::new_with_root(
                &entry,
                &source,
                repository.then_some(root.as_path()),
            )
            .unwrap(),
        )
        .await;
        for (key, path) in [("url", &entry), ("sibling", &sibling), ("alias", &sibling)] {
            let canonical = path.canonicalize().unwrap();
            assert_eq!(exports[key], file_specifier(&canonical).unwrap().as_str());
            assert_eq!(
                parse_plugin_location(&exports[key], None).unwrap(),
                RunPluginLocation::File(canonical)
            );
        }
    }
}

#[tokio::test]
async fn import_meta_embedded_urls_have_no_filesystem_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("missing.ts");
    std::fs::write(&local, "export {};").unwrap();
    let file_url = serde_json::to_string(file_specifier(&local).unwrap().as_str()).unwrap();
    let source = format!(
        r#"
export const url: string = import.meta.url;
export const sibling: string = import.meta.resolve("./agent-exit.ts");
for (const specifier of ["./missing.ts", {file_url}]) {{
  let rejected = false;
  try {{ import.meta.resolve(specifier); }}
  catch (error) {{
    if (!(error instanceof TypeError)) throw error;
    rejected = true;
  }}
  if (!rejected) throw new Error(`accepted ${{specifier}}`);
}}
"#
    );
    let exports =
        import_meta_exports(PluginModuleLoader::embedded("meta-test", &source).unwrap()).await;
    assert_eq!(exports["url"], "yinz-builtin:///meta-test.ts");
    assert_eq!(exports["sibling"], "yinz-builtin:///agent-exit.ts");
}
