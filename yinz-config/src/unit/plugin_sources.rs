use super::*;
use std::os::unix::{ffi::OsStrExt, fs::symlink};

#[test]
fn special_sources_are_rejected_without_waiting_for_a_fifo_writer() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("blocked.scm");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let link = dir.path().join("link.scm");
    symlink(&fifo, &link).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let errors: Vec<_> = [fifo, link, dir.path().to_owned(), "/dev/null".into()]
            .iter()
            .map(|path| format!("{:#}", read_source_file(path).unwrap_err()))
            .collect();
        sender.send(errors).unwrap();
    });
    let errors = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("source validation must not wait for a FIFO writer");
    for error in errors {
        assert!(error.contains("not a regular file"), "{error}");
    }
}

#[test]
fn source_reader_follows_regular_symlinks_and_keeps_the_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.scm");
    let link = dir.path().join("current.scm");
    fs::write(&source, "(define value 1)").unwrap();
    symlink(&source, &link).unwrap();
    assert_eq!(
        read_source_file(&link).unwrap().as_deref(),
        Some("(define value 1)")
    );
    fs::File::create(&source)
        .unwrap()
        .set_len(MAX_PLUGIN_SOURCE_BYTES as u64 + 1)
        .unwrap();
    assert!(format!("{:#}", read_source_file(&link).unwrap_err()).contains("source-file budget"));
    fs::remove_file(source).unwrap();
    assert_eq!(read_source_file(&link).unwrap(), None);
}
