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
