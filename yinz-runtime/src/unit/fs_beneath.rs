use super::*;
#[test]
fn confined_components_links_and_nonblocking_special_files() {
    use std::{
        io::Read,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("dir")).unwrap();
    std::fs::write(temp.path().join("dir/file"), "ok").unwrap();
    symlink("dir", temp.path().join("link")).unwrap();
    symlink("dir/file", temp.path().join("leaf")).unwrap();
    let fifo = std::ffi::CString::new(temp.path().join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let root = open_root(temp.path()).unwrap();
    for path in ["../dir/file", "/etc/passwd", "link/file", "leaf"] {
        assert!(open_beneath(&root, Path::new(path)).is_err(), "{path}");
    }
    assert!(open_root(&temp.path().join("link")).is_err());
    let file = open_beneath(&root, Path::new("fifo")).unwrap();
    assert!(!file.metadata().unwrap().is_file());
    let mut text = String::new();
    open_beneath(&root, Path::new("dir/file"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "ok");
}
