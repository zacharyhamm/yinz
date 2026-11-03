use super::*;

#[test]
fn session_generation_keeps_missing_builtins_missing_after_publication() {
    let data = tempfile::tempdir().unwrap();
    fs::create_dir(data.path().join("plugins")).unwrap();
    let generation = pin_session_generation(data.path()).unwrap();
    let entry = data.path().join("plugins/builtins/tab-bar.scm");
    assert!(!source_path(&entry).unwrap().exists());
    fs::create_dir(entry.parent().unwrap()).unwrap();
    fs::write(&entry, "new release").unwrap();
    let retained = source_path(&entry).unwrap();
    assert_ne!(retained, entry);
    assert!(!retained.exists());
    drop(generation);
    let _new_server = pin_session_generation(data.path()).unwrap();
    assert_eq!(
        fs::read_to_string(source_path(&entry).unwrap()).unwrap(),
        "new release"
    );
}

#[test]
fn session_generation_preserves_live_edits_then_pins_old_sources_after_exchange() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("plugins/builtins");
    fs::create_dir_all(&root).unwrap();
    let entry = root.join("tab-bar.scm");
    fs::write(&entry, "old").unwrap();
    fs::write(root.join("dependency.scm"), "dependency old").unwrap();
    let generation = pin_session_generation(data.path()).unwrap();
    assert_eq!(source_path(&entry).unwrap(), entry);
    fs::write(&entry, "edited old").unwrap();
    // Ordinary editor writes still reload through the live installation.
    assert_eq!(
        fs::read_to_string(source_path(&entry).unwrap()).unwrap(),
        "edited old"
    );
    let backup = data.path().join("retired");
    fs::rename(&root, &backup).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(&entry, "new").unwrap();
    fs::write(root.join("dependency.scm"), "dependency new").unwrap();
    fs::remove_dir_all(backup).unwrap();
    let retained = source_path(&entry).unwrap();
    assert_ne!(retained, entry);
    assert_eq!(fs::read_to_string(&retained).unwrap(), "edited old");
    assert_eq!(
        fs::read_to_string(retained.parent().unwrap().join("dependency.scm")).unwrap(),
        "dependency old"
    );
    assert_eq!(builtin_path(data.path(), "tab-bar").unwrap(), retained);
    drop(generation);
    assert!(!retained.exists());
    assert_eq!(source_path(&entry).unwrap(), entry);
    let _new_server = pin_session_generation(data.path()).unwrap();
    assert_eq!(
        fs::read_to_string(source_path(&entry).unwrap()).unwrap(),
        "new"
    );
}

#[test]
fn session_generation_remembers_removed_builtin_entrypoints_and_rejects_escapes() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("plugins/builtins");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("tab-bar.scm"), "old").unwrap();
    let _generation = pin_session_generation(data.path()).unwrap();
    fs::rename(&root, data.path().join("retired")).unwrap();
    fs::create_dir(&root).unwrap();
    assert_eq!(
        fs::read_to_string(builtin_path(data.path(), "tab-bar").unwrap()).unwrap(),
        "old"
    );
    assert!(source_path(&root.join("../secret.scm")).is_err());
}

#[test]
fn session_generation_reads_read_only_installation_without_sidecar() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    let plugins = data.path().join("plugins");
    let root = plugins.join("builtins");
    fs::create_dir_all(&root).unwrap();
    let entry = root.join("tab-bar.scm");
    fs::write(&entry, "read-only builtin").unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o444)).unwrap();
    for directory in [&root, &plugins, data.path()] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
    }
    // Restore directory permissions even if pinning failed, so cleanup can run.
    let result = pin_session_generation(data.path()).and_then(|generation| {
        let contents = fs::read_to_string(builtin_path(data.path(), "tab-bar")?)?;
        Ok((generation, contents))
    });
    for directory in [data.path(), &plugins, &root] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (_generation, contents) = result.unwrap();
    assert_eq!(contents, "read-only builtin");
    assert!(!plugins.join(".yinz-update.lock").exists());
}
