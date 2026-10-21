use super::*;
use std::{fs, os::unix::fs::symlink};

#[test]
fn layout_directories_preserve_symlinks_before_parent_segments() {
    let cwd = std::env::current_dir().unwrap();
    let root = tempfile::tempdir_in(&cwd).unwrap();
    for dir in ["cfg/layouts", "data/session", "data/layouts"] {
        fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    symlink("../data/session", root.path().join("cfg/current")).unwrap();
    let expected = "layout { tab name=\"Symlink destination\" { pane; }; }";
    fs::write(root.path().join("data/layouts/custom.kdl"), expected).unwrap();
    fs::write(
        root.path().join("cfg/layouts/custom.kdl"),
        "layout { tab name=\"Lexical destination\" { pane; }; }",
    )
    .unwrap();

    let relative_root = root.path().strip_prefix(&cwd).unwrap();
    for base in [root.path(), relative_root] {
        let config_dir = base.join("cfg/current/..");
        for configured in [None, Some(config_dir.join("layouts"))] {
            let resolved = layout_dir_or_default(configured, Some(config_dir.clone())).unwrap();
            assert!(resolved.is_absolute());
            assert_eq!(
                fs::read_to_string(resolved.join("custom.kdl")).unwrap(),
                expected,
            );
        }
    }
}

#[test]
fn layout_directories_are_absolute_and_keep_override_precedence() {
    let cwd = std::env::current_dir().unwrap();
    for (configured, config_dir, expected) in [
        (
            None,
            "relative/./config",
            cwd.join("relative/./config/layouts"),
        ),
        (
            Some("relative/../custom-layouts"),
            "/config",
            cwd.join("relative/../custom-layouts"),
        ),
        (None, "/config", PathBuf::from("/config/layouts")),
        (
            Some("/custom/../layouts"),
            "relative/config",
            PathBuf::from("/custom/../layouts"),
        ),
        (Some(""), "/config", cwd.clone()),
    ] {
        assert_eq!(
            layout_dir_or_default(configured.map(PathBuf::from), Some(config_dir.into())),
            Some(expected),
        );
    }
    assert_eq!(layout_dir_or_default(None, None), None);
}

#[test]
fn layout_directory_resolution_neither_requires_nor_creates_directories() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert_eq!(
        layout_dir_or_default(None, Some(missing.clone())),
        Some(missing.join("layouts")),
    );
    assert_eq!(
        layout_dir_or_default(Some(missing.clone()), None),
        Some(missing.clone()),
    );
    assert!(!missing.exists());
}
