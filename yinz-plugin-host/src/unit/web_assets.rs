use super::*;
#[test]
fn asset_freezing_confines_paths_and_refuses_links_special_files() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("static")).unwrap();
    let source = temp.path().join("test.scm");
    std::fs::write(&source, "").unwrap();
    std::fs::write(temp.path().join("static/app.js"), "old").unwrap();
    std::fs::write(temp.path().join("static/ignored.exe"), "skip").unwrap();
    let files = load_directory(temp.path(), &source, "static").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files["app.js"].etag.len(), 34);
    std::fs::write(temp.path().join("static/app.js"), "new").unwrap();
    assert_eq!(&*files["app.js"].bytes, b"old");
    for path in ["/tmp", "../static", "static/../static"] {
        assert!(load_directory(temp.path(), &source, path).is_err());
    }
    symlink("static", temp.path().join("link")).unwrap();
    assert!(load_directory(temp.path(), &source, "link").is_err());
    symlink("app.js", temp.path().join("static/leaf.js")).unwrap();
    assert!(load_directory(temp.path(), &source, "static").is_err());
    std::fs::remove_file(temp.path().join("static/leaf.js")).unwrap();
    let fifo =
        std::ffi::CString::new(temp.path().join("static/fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(load_directory(temp.path(), &source, "static").is_err());
}
#[test]
fn asset_file_tree_and_depth_limits() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("big.js"), vec![0; 2 * 1024 * 1024 + 1]).unwrap();
    assert!(load_tree(open_root(temp.path()).unwrap()).is_err());
    std::fs::remove_file(temp.path().join("big.js")).unwrap();
    for i in 0..257 {
        std::fs::write(temp.path().join(format!("{i}.js")), "").unwrap();
    }
    assert!(load_tree(open_root(temp.path()).unwrap()).is_err());
    let deep = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(deep.path().join("a/b/c/d/e/f/g")).unwrap();
    assert!(load_tree(open_root(deep.path()).unwrap()).is_err());
}

#[test]
fn builtin_assets_remain_in_the_retained_generation_after_exchange() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(root.join("web/static")).unwrap();
    std::fs::write(root.join("tab-bar.scm"), "old source").unwrap();
    std::fs::write(root.join("web/static/app.js"), "old bytes").unwrap();
    let _generation =
        yinz_runtime::builtin_generation::pin_session_generation(data.path()).unwrap();
    std::fs::rename(&root, data.path().join("backup")).unwrap();
    std::fs::create_dir_all(root.join("web/static")).unwrap();
    std::fs::write(root.join("web/static/app.js"), "new bytes").unwrap();
    std::fs::remove_dir_all(data.path().join("backup")).unwrap();
    let user = data.path().join("user.scm");
    std::fs::write(&user, "").unwrap();
    for _ in 0..2 {
        // Both initial load and reload must resolve the same session generation.
        let files = load_directory(data.path(), &user, "builtin:web/static").unwrap();
        assert_eq!(&*files["app.js"].bytes, b"old bytes");
    }
}

#[test]
fn aggregate_asset_bytes_are_bounded_and_exact_limit_is_accepted() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..4 {
        std::fs::File::create(temp.path().join(format!("{i}.js")))
            .unwrap()
            .set_len(2 * 1024 * 1024)
            .unwrap();
    }
    assert_eq!(load_tree(open_root(temp.path()).unwrap()).unwrap().len(), 4);
    std::fs::write(temp.path().join("extra.js"), "x").unwrap();
    assert!(load_tree(open_root(temp.path()).unwrap()).is_err());
}
