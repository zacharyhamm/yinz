use super::*;
use crate::Runtime;

#[test]
fn entry_reads_use_the_pinned_root_and_refuse_a_replaced_symlink() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let entry = root.path().join("main.scm");
    let external = outside.path().join("external.scm");
    std::fs::write(&entry, "42").unwrap();
    std::fs::write(&external, "secret").unwrap();
    let sources = SourceRoot::file(&entry, None).unwrap();
    assert_eq!(sources.read_entry().unwrap(), "42");
    std::fs::remove_file(&entry).unwrap();
    std::os::unix::fs::symlink(&external, &entry).unwrap();
    assert!(sources.read_entry().is_err());
}

#[test]
fn embedded_modules_resolve_from_each_importers_directory() {
    let mut root = SourceRoot::embedded(
        "main.scm",
        BTreeMap::from([
            (
                "left/main.scm".into(),
                "(require \"./value.scm\") (provide left) (define left value)".into(),
            ),
            (
                "left/value.scm".into(),
                "(provide value) (define value 17)".into(),
            ),
            (
                "right/main.scm".into(),
                "(require \"./value.scm\") (provide right) (define right value)".into(),
            ),
            (
                "right/value.scm".into(),
                "(provide value) (define value 25)".into(),
            ),
        ]),
    )
    .unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let result = runtime
        .eval_with_source_root(
            "(require \"./left/main.scm\" \"./right/main.scm\") (+ left right)",
            &mut root,
        )
        .unwrap();
    assert_eq!(result.values.last().unwrap(), "42");
}

#[test]
fn require_selectors_preserve_renames_and_quoted_data() {
    let mut root = SourceRoot::embedded(
        "main.scm",
        BTreeMap::from([(
            "values.scm".into(),
            "(provide answer other) (define answer 42) (define other 10)".into(),
        )]),
    )
    .unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let result = runtime.eval_with_source_root("(require (prefix-in item: (only-in \"./values.scm\" answer))) (list item:answer '(require \"./not-an-import.scm\"))", &mut root).unwrap();
    assert_eq!(
        result.values.last().unwrap(),
        "(42 (require \"./not-an-import.scm\"))"
    );
}

#[test]
fn case_datums_are_inert_but_key_and_clause_imports_are_confined() {
    let mut requested = Vec::new();
    let (_, modules) = prepare(
        r#"(case (begin (require "./key.scm") 'value)
             [((require "./inert.scm")) (require "./active.scm")]
             [else (require "./fallback.scm")])"#,
        "main",
        &mut |_, specifier| {
            requested.push(specifier.to_owned());
            Ok(ModuleSource {
                id: specifier.to_owned(),
                source: "(void)".into(),
            })
        },
    )
    .unwrap();
    assert_eq!(requested, ["./key.scm", "./active.scm", "./fallback.scm"]);
    assert_eq!(modules.len(), 3);

    let mut root = SourceRoot::embedded("main.scm", BTreeMap::new()).unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let output = runtime
        .eval_with_source_root(
            r#"(case '(require "./missing.scm") [((require "./missing.scm")) 7])"#,
            &mut root,
        )
        .unwrap();
    assert_eq!(output.values.last().unwrap(), "7");
    assert!(runtime
        .eval_with_source_root(
            r#"(case 'value [(value) (require "../escape.scm")])"#,
            &mut root,
        )
        .is_err());
}

#[test]
fn quasiquoted_import_data_is_inert_but_executable_unquotes_are_confined() {
    let mut requested = Vec::new();
    let (_, modules) = prepare(
        r#"`((require "./inert.scm") `(require "./nested.scm")
             ,(begin (require "./active.scm") 42))"#,
        "main",
        &mut |_, specifier| {
            requested.push(specifier.to_owned());
            Ok(ModuleSource {
                id: "active".into(),
                source: "(void)".into(),
            })
        },
    )
    .unwrap();
    assert_eq!(requested, ["./active.scm"]);
    assert_eq!(modules.len(), 1);
}

#[test]
fn file_sources_confine_symlinks_and_explicit_extensions() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("value.scm"),
        "(provide x) (define x 3)",
    )
    .unwrap();
    fs::write(
        outside.path().join("value.scm"),
        "(provide x) (define x 99)",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("value.scm"),
        directory.path().join("escape.scm"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        directory.path().join("value.scm"),
        directory.path().join("inside.scm"),
    )
    .unwrap();
    let mut root = SourceRoot::file(&directory.path().join("unsaved.scm"), None).unwrap();
    let origin = root.entry_id().to_owned();
    assert!(root
        .load(&origin, "./inside.scm")
        .unwrap()
        .source
        .contains("3"));
    for specifier in [
        "./escape.scm",
        "../outside.scm",
        "/etc/passwd",
        "https://host/evil.scm",
        "./value",
        "./value.txt",
        "value.scm",
        "./value.scm?x",
        "./value.scm#x",
    ] {
        assert!(
            root.load(&origin, specifier).is_err(),
            "accepted {specifier}"
        );
    }
    assert!(root.load("forged-origin", "./value.scm").is_err());
}

#[test]
fn imported_modules_are_checked_without_running_initializers() {
    let mut root = SourceRoot::embedded(
        "main.scm",
        BTreeMap::from([(
            "value.scm".into(),
            "(provide x) (yinz-command \"must-not-run\") (define x 3)".into(),
        )]),
    )
    .unwrap();
    let mut runtime = Runtime::new(|_| panic!("check ran imported initializer")).unwrap();
    runtime
        .check_with_source_root("(require \"./value.scm\") x", &mut root)
        .unwrap();
}

#[test]
fn file_sources_reject_special_files_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pipe.scm");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut root = SourceRoot::file(&directory.path().join("main.scm"), None).unwrap();
    assert!(root
        .resolve("./pipe.scm")
        .unwrap_err()
        .to_string()
        .contains("regular file"));
    let origin = root.entry_id().to_owned();
    assert!(root
        .load(&origin, "./pipe.scm")
        .unwrap_err()
        .to_string()
        .contains("regular file"));
    assert!(SourceRoot::file(&path, Some(&path)).is_err());
}

#[test]
fn module_file_graph_and_import_depth_limits_are_enforced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("big.scm");
    let file = File::create(&path).unwrap();
    file.set_len(crate::MAX_SOURCE_BYTES as u64 + 1).unwrap();
    let mut root = SourceRoot::file(&directory.path().join("main.scm"), None).unwrap();
    let origin = root.entry_id().to_owned();
    assert!(root
        .load(&origin, "./big.scm")
        .unwrap_err()
        .to_string()
        .contains("8 MiB"));

    file.set_len(crate::MAX_SOURCE_BYTES as u64).unwrap();
    root.begin(0).unwrap();
    for index in 0..9 {
        let module = format!("{index}.scm");
        fs::hard_link(&path, directory.path().join(&module)).unwrap();
        let loaded = root.load(&origin, &format!("./{module}"));
        if index < 8 {
            assert!(loaded.is_ok());
        } else {
            assert!(loaded.unwrap_err().to_string().contains("64 MiB"));
        }
    }

    let sources = (0..66)
        .map(|index| {
            (
                format!("{index}.scm"),
                format!("(require \"./{}.scm\")", index + 1),
            )
        })
        .collect();
    let mut root = SourceRoot::embedded("main.scm", sources).unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let error = runtime
        .check_with_source_root("(require \"./0.scm\")", &mut root)
        .unwrap_err();
    assert!(error.to_string().contains("depth"), "{error:#}");
}

#[test]
fn plugin_locations_preserve_urls_and_refuse_symlinks_or_escapes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let entry = root.join("main.scm");
    let sibling = root.join("日本 space.scm");
    fs::write(&entry, "").unwrap();
    fs::write(&sibling, "(error \"resolution must not evaluate\")").unwrap();
    std::os::unix::fs::symlink(&sibling, root.join("link.scm")).unwrap();
    let sources = SourceRoot::file(&entry, None).unwrap();
    assert_eq!(
        sources.location().unwrap(),
        url::Url::from_file_path(&entry).unwrap().as_str()
    );
    assert_eq!(
        sources.resolve("./日本 space.scm").unwrap(),
        url::Url::from_file_path(sibling).unwrap().as_str()
    );
    for invalid in [
        "./link.scm",
        "../main.scm",
        "/tmp/main.scm",
        "./main.txt",
        "./missing.scm",
    ] {
        assert!(sources.resolve(invalid).is_err(), "{invalid}");
    }
    let mut sources = sources;
    let mut runtime =
        Runtime::new(|_| panic!("location lookup must not dispatch a host command")).unwrap();
    runtime.eval_with_source_root("(define location (yinz-plugin-location)) (define sibling (yinz-plugin-resolve \"./日本 space.scm\"))", &mut sources).unwrap();
    assert_eq!(
        runtime.value("location").unwrap(),
        sources.location().unwrap()
    );
    assert_eq!(
        runtime.value("sibling").unwrap(),
        sources.resolve("./日本 space.scm").unwrap()
    );
}

#[test]
fn repository_and_embedded_targets_retain_source_identity() {
    use yinz_types::input::{
        git_plugin::{GitPlugin, GitRevision},
        layout::RunPluginLocation,
    };
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("plugins")).unwrap();
    fs::write(directory.path().join("plugins/main.scm"), "").unwrap();
    fs::write(directory.path().join("plugins/finder.scm"), "").unwrap();
    let repository = GitPlugin {
        repo: "https://example.org/reader.git".into(),
        entrypoint: "plugins/main.scm".into(),
        revision: Some(GitRevision::Commit("a".repeat(40))),
    };
    let sources = SourceRoot::file(
        &directory.path().join("plugins/main.scm"),
        Some(directory.path()),
    )
    .unwrap()
    .with_location(RunPluginLocation::Git(repository.clone()));
    assert_eq!(sources.location().unwrap(), repository.descriptor());
    let resolved = GitPlugin::from_descriptor(&sources.resolve("./finder.scm").unwrap()).unwrap();
    assert_eq!(resolved.repo, repository.repo);
    assert_eq!(resolved.revision, repository.revision);
    assert_eq!(resolved.entrypoint, Path::new("plugins/finder.scm"));
    let sources = SourceRoot::embedded(
        "main.scm",
        BTreeMap::from([
            ("main.scm".into(), "".into()),
            ("other.scm".into(), "".into()),
            ("import-only.ss".into(), "".into()),
        ]),
    )
    .unwrap();
    assert_eq!(sources.location().unwrap(), "yinz:main");
    assert_eq!(sources.resolve("./other.scm").unwrap(), "yinz:other");
    assert!(sources.resolve("./missing.scm").is_err());
    assert!(sources.resolve("./import-only.ss").is_err());
}

#[test]
fn plugin_copy_keeps_original_sources_and_a_confined_deduplicated_graph() {
    let directory = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("lib")).unwrap();
    let entry_source = "(require \"./lib/left.scm\" \"./lib/right.scm\") (+ left right)";
    for (path, source) in [
        ("main.scm", entry_source),
        (
            "lib/left.scm",
            "(require \"./shared.scm\") (provide left) (define left value)",
        ),
        (
            "lib/right.scm",
            "(require \"./shared.scm\") (provide right) (define right value)",
        ),
        ("lib/shared.scm", "(provide value) (define value 21)"),
        ("asset.txt", "explicit fixture required"),
    ] {
        fs::write(directory.path().join(path), source).unwrap();
    }
    let mut sources = SourceRoot::file(&directory.path().join("main.scm"), None).unwrap();
    let copy = destination.path().join("日本 copy");
    let entry = sources
        .copy_to(destination.path(), Path::new("日本 copy"))
        .unwrap();
    assert_eq!(fs::read_to_string(&entry).unwrap(), entry_source);
    assert!(!copy.join("asset.txt").exists());
    assert_eq!(fs::read_dir(copy.join("lib")).unwrap().count(), 3);
    assert!(sources
        .copy_to(destination.path(), Path::new("日本 copy"))
        .is_err());
    std::os::unix::fs::symlink(directory.path(), destination.path().join("link")).unwrap();
    assert!(sources
        .copy_to(destination.path(), Path::new("link/escape"))
        .is_err());
    assert!(!directory.path().join("escape").exists());
    assert!(sources
        .copy_to(destination.path(), Path::new("../escape"))
        .is_err());
    let mut copied = SourceRoot::file(&entry, None).unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let evaluated = runtime
        .eval_with_source_root(&copied.read_entry().unwrap(), &mut copied)
        .unwrap();
    assert_eq!(evaluated.values.last().unwrap(), "42");
    fs::remove_file(directory.path().join("lib/shared.scm")).unwrap();
    std::os::unix::fs::symlink(
        copy.join("lib/shared.scm"),
        directory.path().join("lib/shared.scm"),
    )
    .unwrap();
    assert!(sources
        .copy_to(destination.path(), Path::new("rejected"))
        .is_err());
    assert!(!destination.path().join("rejected").exists());
}
