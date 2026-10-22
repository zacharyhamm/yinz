use super::*;

#[test]
fn fresh_install_omits_retired_notes_and_todos() {
    let data = tempfile::tempdir().unwrap();
    install(&asset_dir(), data.path()).unwrap();
    let root = data.path().join("plugins/builtins");
    assert!(!root.join("todos.scm").exists());
    assert!(!root.join("project-store/notes.scm").exists());
    assert!(builtin_path(data.path(), "todos").is_err());
    assert!(builtin_path(data.path(), "project-store").is_ok());
}

#[test]
fn install_replaces_shipped_files_and_preserves_unrelated_files() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("shared")).unwrap();
    std::fs::write(source.path().join("shared/module.scm"), "first").unwrap();
    std::fs::write(source.path().join("agent-exit.scm"), "entry").unwrap();
    std::fs::write(source.path().join("agent-exit.test.scm"), "suite").unwrap();
    install(source.path(), data.path()).unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::write(root.join("custom.scm"), "unrelated").unwrap();
    std::fs::write(data.path().join("plugin-data.db"), "database").unwrap();
    std::fs::write(source.path().join("shared/module.scm"), "second").unwrap();
    install(source.path(), data.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("shared/module.scm")).unwrap(),
        "second"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("custom.scm")).unwrap(),
        "unrelated"
    );
    assert_eq!(
        std::fs::read_to_string(data.path().join("plugin-data.db")).unwrap(),
        "database"
    );
    assert!(root.join("agent-exit.test.scm").is_file());
    assert_eq!(
        builtin_path(data.path(), "agent-exit").unwrap(),
        root.join("agent-exit.scm")
    );
    assert!(builtin_path(data.path(), "../agent-exit").is_err());
    assert!(builtin_path(data.path(), "unknown").is_err());
    let empty = tempfile::tempdir().unwrap();
    let error = builtin_path(empty.path(), "agent-exit")
        .unwrap_err()
        .to_string();
    assert!(error.contains(
        &empty
            .path()
            .join("plugins/builtins/agent-exit.scm")
            .display()
            .to_string()
    ));
    assert!(error.contains("reinstall"));
}

#[test]
fn install_failure_is_reported_without_removing_other_files() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("plugins"), "keep").unwrap();
    assert!(install(&asset_dir(), data.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(data.path().join("plugins")).unwrap(),
        "keep"
    );
}

#[test]
fn reinstall_replaces_file_symlinks_without_writing_their_targets() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "unrelated").unwrap();
    std::fs::write(source.path().join("agent-exit.scm"), "shipped").unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("agent-exit.scm");
    std::os::unix::fs::symlink(outside.path(), &target).unwrap();
    install(source.path(), data.path()).unwrap();
    assert!(!target.is_symlink());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "shipped");
    assert_eq!(
        std::fs::read_to_string(outside.path()).unwrap(),
        "unrelated"
    );
}
