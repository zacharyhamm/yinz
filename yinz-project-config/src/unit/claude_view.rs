use super::*;

fn read(tree: &mut Tree, fh: FileHandle) -> String {
    let mut bytes = vec![0; 1024];
    let len = tree
        .file(fh, false)
        .unwrap()
        .read_at(&mut bytes, 0)
        .unwrap();
    String::from_utf8(bytes[..len].to_vec()).unwrap()
}

#[test]
fn credentials_follow_host_replacements_and_keep_open_descriptors() {
    let host = tempfile::tempdir().unwrap();
    let path = host.path().join(NAME);
    fs::write(&path, "initial").unwrap();
    let mut first = Tree::new(host.path(), b"{}").unwrap();
    let mut second = Tree::new(host.path(), b"{}").unwrap();
    let old = first.open(CREDENTIAL, libc::O_RDWR).unwrap();
    first
        .file(old, true)
        .unwrap()
        .write_all_at(b"updated", 0)
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated");
    let (ino, stage) = second
        .create(ROOT, OsStr::new("replacement"), libc::O_RDWR, 0o666)
        .unwrap();
    second
        .file(stage, true)
        .unwrap()
        .write_all_at(b"replaced", 0)
        .unwrap();
    second
        .rename(
            ROOT,
            OsStr::new("replacement"),
            ROOT,
            OsStr::new(NAME),
            RenameFlags::empty(),
        )
        .unwrap();
    assert_eq!(second.attr(ino, None).unwrap().size, 8);
    assert_eq!(fs::read_to_string(&path).unwrap(), "replaced");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(read(&mut first, old), "updated");
    second
        .file(stage, true)
        .unwrap()
        .write_all_at(b"R", 0)
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "Replaced");
    fs::write(host.path().join("host-new"), "host replacement").unwrap();
    fs::rename(host.path().join("host-new"), &path).unwrap();
    assert_eq!(first.lookup(ROOT, OsStr::new(NAME)).unwrap(), CREDENTIAL);
    let new = first.open(CREDENTIAL, libc::O_RDONLY).unwrap();
    assert_eq!(read(&mut first, new), "host replacement");
    assert_eq!(read(&mut second, stage), "Replaced");
}

#[test]
fn missing_credentials_create_and_unsafe_links_fail_without_host_mutations() {
    let host = tempfile::tempdir().unwrap();
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    assert_eq!(
        tree.open(CREDENTIAL, libc::O_RDONLY).unwrap_err(),
        Errno::ENOENT
    );
    let (_, fh) = tree
        .create(ROOT, OsStr::new(NAME), libc::O_RDWR, 0o777)
        .unwrap();
    tree.file(fh, true)
        .unwrap()
        .write_all_at(b"new", 0)
        .unwrap();
    let credential = host.path().join(NAME);
    assert_eq!(fs::read_to_string(&credential).unwrap(), "new");
    assert_eq!(fs::metadata(&credential).unwrap().mode() & 0o777, 0o600);
    tree.remove(ROOT, OsStr::new(NAME), false).unwrap();
    let unrelated = host.path().join("unrelated");
    fs::write(&unrelated, "safe").unwrap();
    std::os::unix::fs::symlink(&unrelated, &credential).unwrap();
    assert_eq!(
        tree.open(CREDENTIAL, libc::O_RDWR).unwrap_err(),
        Errno::ELOOP
    );
    assert!(Tree::new(host.path(), b"{}").is_err());
    fs::remove_file(&credential).unwrap();
    fs::hard_link(&unrelated, &credential).unwrap();
    assert_eq!(
        tree.open(CREDENTIAL, libc::O_RDWR).unwrap_err(),
        Errno::EPERM
    );
    let (_, stage) = tree
        .create(ROOT, OsStr::new("stage"), libc::O_RDWR, 0o600)
        .unwrap();
    tree.file(stage, true)
        .unwrap()
        .write_all_at(b"bad", 0)
        .unwrap();
    assert!(tree
        .rename(
            ROOT,
            OsStr::new("stage"),
            ROOT,
            OsStr::new(NAME),
            RenameFlags::empty()
        )
        .is_err());
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "safe");
    for name in ["..", ".", "../unrelated", "/unrelated", "sub/file", "x\0"] {
        assert!(tree
            .create(ROOT, OsStr::new(name), libc::O_RDWR, 0o600)
            .is_err());
    }
    fs::remove_file(&credential).unwrap();
    fs::create_dir(&credential).unwrap();
    assert!(tree.open(CREDENTIAL, libc::O_RDWR).is_err());
}

#[test]
fn ordinary_changes_are_private_and_copy_on_write() {
    let host = tempfile::tempdir().unwrap();
    fs::write(host.path().join("settings.json"), "host").unwrap();
    fs::create_dir(host.path().join("nested")).unwrap();
    fs::write(host.path().join("nested/file"), "nested").unwrap();
    let mut tree = Tree::new(host.path(), br#"{"snapshot":true}"#).unwrap();
    let ino = tree.lookup(ROOT, OsStr::new("settings.json")).unwrap();
    assert!(matches!(
        tree.nodes[&ino],
        Node::File { private: false, .. }
    ));
    let reader = tree.open(ino, libc::O_RDONLY).unwrap();
    let writer = tree.open(ino, libc::O_RDWR).unwrap();
    tree.file(writer, true)
        .unwrap()
        .write_all_at(b"ours", 0)
        .unwrap();
    assert_eq!(read(&mut tree, reader), "ours");
    assert_eq!(
        fs::read_to_string(host.path().join("settings.json")).unwrap(),
        "host"
    );
    tree.remove(ROOT, OsStr::new("settings.json"), false)
        .unwrap();
    assert_eq!(
        tree.lookup(ROOT, OsStr::new("settings.json")).unwrap_err(),
        Errno::ENOENT
    );
    assert_eq!(read(&mut tree, reader), "ours");
    tree.rename(
        ROOT,
        OsStr::new("nested"),
        ROOT,
        OsStr::new("moved"),
        RenameFlags::empty(),
    )
    .unwrap();
    let moved = tree.lookup(ROOT, OsStr::new("moved")).unwrap();
    let file = tree.lookup(moved, OsStr::new("file")).unwrap();
    let fh = tree.open(file, libc::O_RDONLY).unwrap();
    assert_eq!(read(&mut tree, fh), "nested");
    assert!(host.path().join("nested/file").exists());
    assert!(!host.path().join("moved").exists());
}

#[test]
fn credential_write_errors_are_synchronous() {
    let host = tempfile::tempdir().unwrap();
    fs::write(host.path().join(NAME), "original").unwrap();
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    let fh = tree.open(CREDENTIAL, libc::O_RDONLY).unwrap();
    assert_eq!(tree.file(fh, true).unwrap_err(), Errno::EBADF);
    let writable = tree.open(CREDENTIAL, libc::O_RDWR).unwrap();
    fs::hard_link(host.path().join(NAME), host.path().join("unsafe-link")).unwrap();
    assert_eq!(tree.file(writable, true).unwrap_err(), Errno::EPERM);
    assert_eq!(
        fs::read_to_string(host.path().join(NAME)).unwrap(),
        "original"
    );
}

#[test]
#[ignore = "requires /dev/fuse and fusermount3; run explicitly on Linux"]
fn real_fuse_credentials_and_private_configuration() {
    let host = tempfile::tempdir().unwrap();
    fs::write(host.path().join(NAME), "initial").unwrap();
    fs::write(host.path().join("settings.json"), "host").unwrap();
    let first = View::mount(host.path(), b"{}").unwrap();
    let second = View::mount(host.path(), b"{}").unwrap();
    fs::write(first.path().join(NAME), "first").unwrap();
    assert_eq!(fs::read_to_string(host.path().join(NAME)).unwrap(), "first");
    let old = File::open(first.path().join(NAME)).unwrap();
    fs::write(second.path().join("new"), "second").unwrap();
    fs::rename(second.path().join("new"), second.path().join(NAME)).unwrap();
    let mut bytes = [0; 5];
    old.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"first");
    assert_eq!(
        fs::read_to_string(first.path().join(NAME)).unwrap(),
        "second"
    );
    fs::write(host.path().join("new"), "from-host").unwrap();
    fs::rename(host.path().join("new"), host.path().join(NAME)).unwrap();
    assert_eq!(
        fs::read_to_string(first.path().join(NAME)).unwrap(),
        "from-host"
    );
    OpenOptions::new()
        .append(true)
        .open(first.path().join(NAME))
        .unwrap()
        .write_all(b"+append")
        .unwrap();
    assert_eq!(
        fs::read_to_string(host.path().join(NAME)).unwrap(),
        "from-host+append"
    );
    fs::write(first.path().join("settings.json"), "private").unwrap();
    assert_eq!(
        fs::read_to_string(second.path().join("settings.json")).unwrap(),
        "host"
    );
    assert_eq!(
        fs::read_to_string(host.path().join("settings.json")).unwrap(),
        "host"
    );
    assert!(fs::hard_link(first.path().join(NAME), first.path().join("link")).is_err());
    let mount = first.path().to_path_buf();
    drop(old);
    drop(first);
    assert!(!mount.exists());
}

#[test]
#[ignore = "requires /dev/fuse and fusermount3; run explicitly on Linux"]
fn real_fuse_cleanup_watcher_releases_mount_on_lost_lease() {
    let host = tempfile::tempdir().unwrap();
    let mut view = View::mount(host.path(), b"{}").unwrap();
    let path = view.path().to_path_buf();
    // A killed launcher closes this pipe without sending the graceful-exit message.
    drop(view.cleanup.stdin.take());
    assert!(view.cleanup.wait().unwrap().success());
    assert!(!path.exists());
    drop(view);
}

#[test]
#[ignore = "requires /dev/fuse and fusermount3; run explicitly on Linux"]
fn real_fuse_missing_credentials_write_failures_and_unsafe_replacements() {
    let host = tempfile::tempdir().unwrap();
    let view = View::mount(host.path(), b"{}").unwrap();
    let credential = host.path().join(NAME);
    assert!(!view.path().join(NAME).exists());
    fs::write(view.path().join(NAME), "new").unwrap();
    assert_eq!(fs::read_to_string(&credential).unwrap(), "new");
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(fs::write(view.path().join(NAME), "denied").is_err());
    assert_eq!(fs::read_to_string(&credential).unwrap(), "new");
    fs::write(view.path().join("stage"), "replacement").unwrap();
    fs::set_permissions(host.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = fs::rename(view.path().join("stage"), view.path().join(NAME));
    fs::set_permissions(host.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&credential).unwrap(), "new");
    fs::remove_file(&credential).unwrap();
    fs::write(host.path().join("unrelated"), "untouched").unwrap();
    std::os::unix::fs::symlink(host.path().join("unrelated"), &credential).unwrap();
    assert!(fs::read(view.path().join(NAME)).is_err());
    assert!(fs::write(view.path().join(NAME), "denied").is_err());
    assert!(fs::rename(view.path().join("stage"), view.path().join(NAME)).is_err());
    assert_eq!(
        fs::read_to_string(host.path().join("unrelated")).unwrap(),
        "untouched"
    );
}

#[test]
#[ignore = "requires Bubblewrap user namespaces; run explicitly on Linux"]
fn real_mount_guard_releases_before_agent_execution() {
    use std::io::Read;
    use std::sync::mpsc;

    let lock = MountMutation::acquire().unwrap();
    let mut command = Command::new("bwrap");
    guard_setup(&mut command).unwrap();
    command
        .args([
            "--unshare-user",
            "--ro-bind",
            "/",
            "/",
            "--",
            "/bin/sh",
            "-c",
            "printf ready; read finished",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let (entered, starting) = mpsc::channel();
    let (spawned, child) = mpsc::channel();
    let (finished, finish) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        entered.send(()).unwrap();
        spawned.send(command.spawn().unwrap()).unwrap();
        finish.recv().unwrap();
    });
    starting.recv_timeout(Duration::from_secs(5)).unwrap();
    // The behavior under test is blocking spawn while a mount mutation owns the lock.
    assert!(matches!(
        child.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    drop(lock);
    let mut child = child.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut ready = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(&ready, b"ready");
    let lock = mount_lock().unwrap();
    lock.lock().unwrap();
    child.stdin.take().unwrap().write_all(b"done\n").unwrap();
    assert!(child.wait().unwrap().success());
    finished.send(()).unwrap();
    thread.join().unwrap();
}

#[test]
fn detached_files_wait_for_all_lookups_and_handles() {
    for forget_first in [false, true] {
        for replace in [false, true] {
            let host = tempfile::tempdir().unwrap();
            let mut tree = Tree::new(host.path(), b"{}").unwrap();
            let baseline = tree.nodes.len();
            let name = OsStr::new("temporary");
            let (ino, fh) = tree.create(ROOT, name, libc::O_RDWR, 0o600).unwrap();
            tree.file(fh, true)
                .unwrap()
                .write_all_at(b"old", 0)
                .unwrap();
            tree.entry_attr(ino, Some(fh)).unwrap();
            tree.entry_attr(ino, None).unwrap();
            let other = tree.open(ino, libc::O_RDONLY).unwrap();
            let fd = tree.file(fh, false).unwrap().as_raw_fd();
            if replace {
                let (_, replacement) = tree
                    .create(ROOT, OsStr::new("replacement"), libc::O_RDWR, 0o600)
                    .unwrap();
                tree.rename(
                    ROOT,
                    OsStr::new("replacement"),
                    ROOT,
                    name,
                    RenameFlags::empty(),
                )
                .unwrap();
                tree.release(replacement);
            } else {
                tree.remove(ROOT, name, false).unwrap();
            }
            tree.collect_unused();
            assert_eq!(read(&mut tree, fh), "old");
            tree.forget(ino, 1);
            tree.release(other);
            tree.collect_unused();
            assert!(tree.nodes.contains_key(&ino));
            if forget_first {
                tree.forget(ino, 1);
            } else {
                tree.release(fh);
            }
            tree.collect_unused();
            assert!(tree.nodes.contains_key(&ino));
            if forget_first {
                assert_eq!(read(&mut tree, fh), "old");
                tree.release(fh);
            } else {
                assert_eq!(tree.attr(ino, None).unwrap().size, 3);
                tree.forget(ino, 1);
            }
            tree.collect_unused();
            assert!(!tree.nodes.contains_key(&ino));
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
            assert_eq!(tree.nodes.len(), baseline + usize::from(replace));
        }
    }
}

#[test]
fn repeated_atomic_replacements_reclaim_old_nodes() {
    let host = tempfile::tempdir().unwrap();
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    let baseline = tree.nodes.len();
    for destination in ["settings.json", NAME] {
        for _ in 0..256 {
            let (ino, fh) = tree
                .create(ROOT, OsStr::new("stage"), libc::O_RDWR, 0o600)
                .unwrap();
            tree.entry_attr(ino, Some(fh)).unwrap();
            tree.rename(
                ROOT,
                OsStr::new("stage"),
                ROOT,
                OsStr::new(destination),
                RenameFlags::empty(),
            )
            .unwrap();
            tree.file(fh, true)
                .unwrap()
                .write_all_at(b"new", 0)
                .unwrap();
            tree.release(fh);
            tree.forget(ino, 1);
            tree.collect_unused();
            assert_eq!(tree.nodes.len(), baseline + 1);
            assert!(tree.handles.is_empty());
            assert_eq!(tree.lifetimes.len(), tree.nodes.len() - 1);
        }
    }
    assert_eq!(fs::read_to_string(host.path().join(NAME)).unwrap(), "new");
}

#[test]
fn scanning_large_directories_does_not_retain_child_descriptors() {
    let host = tempfile::tempdir().unwrap();
    let nested = host.path().join("nested");
    fs::create_dir(&nested).unwrap();
    for index in 0..256 {
        fs::write(nested.join(index.to_string()), "host").unwrap();
    }
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    let baseline = tree.nodes.len();
    for _ in 0..3 {
        let ino = tree.lookup(ROOT, OsStr::new("nested")).unwrap();
        tree.entry_attr(ino, None).unwrap();
        let fh = tree.opendir(ino).unwrap();
        tree.forget(ino, 1);
        tree.collect_unused();
        assert_eq!(tree.directories[&fh].entries.len(), 258);
        assert_eq!(tree.nodes.len(), baseline + 1);
        tree.releasedir(fh);
        tree.collect_unused();
        assert_eq!(tree.nodes.len(), baseline);
    }
}

#[test]
fn forgetting_cached_descendants_reclaims_their_ancestors() {
    let host = tempfile::tempdir().unwrap();
    fs::create_dir(host.path().join("nested")).unwrap();
    fs::write(host.path().join("nested/file"), "host").unwrap();
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    let baseline = tree.nodes.len();
    let parent = tree.lookup(ROOT, OsStr::new("nested")).unwrap();
    tree.entry_attr(parent, None).unwrap();
    let child = tree.lookup(parent, OsStr::new("file")).unwrap();
    tree.entry_attr(child, None).unwrap();
    tree.forget(parent, 1);
    tree.collect_unused();
    assert!(tree.nodes.contains_key(&parent));
    tree.forget(child, 1);
    tree.collect_unused();
    assert_eq!(tree.nodes.len(), baseline);
    let reloaded = tree.lookup(ROOT, OsStr::new("nested")).unwrap();
    assert_ne!(reloaded, parent);
    let child = tree.lookup(reloaded, OsStr::new("file")).unwrap();
    let fh = tree.open(child, libc::O_RDONLY).unwrap();
    assert_eq!(read(&mut tree, fh), "host");
}

#[test]
fn private_changes_and_tombstones_survive_forget() {
    let host = tempfile::tempdir().unwrap();
    fs::create_dir(host.path().join("nested")).unwrap();
    fs::write(host.path().join("nested/file"), "host").unwrap();
    fs::write(host.path().join("nested/deleted"), "host").unwrap();
    let mut tree = Tree::new(host.path(), b"{}").unwrap();
    let parent = tree.lookup(ROOT, OsStr::new("nested")).unwrap();
    tree.entry_attr(parent, None).unwrap();
    let child = tree.lookup(parent, OsStr::new("file")).unwrap();
    tree.entry_attr(child, None).unwrap();
    let fh = tree.open(child, libc::O_RDWR).unwrap();
    tree.file(fh, true)
        .unwrap()
        .write_all_at(b"ours", 0)
        .unwrap();
    tree.remove(parent, OsStr::new("deleted"), false).unwrap();
    tree.release(fh);
    tree.forget(child, 1);
    tree.forget(parent, 1);
    tree.collect_unused();
    assert_eq!(tree.lookup(ROOT, OsStr::new("nested")).unwrap(), parent);
    assert_eq!(tree.lookup(parent, OsStr::new("file")).unwrap(), child);
    assert_eq!(
        tree.lookup(parent, OsStr::new("deleted")),
        Err(Errno::ENOENT)
    );
    tree.rename(
        ROOT,
        OsStr::new("nested"),
        ROOT,
        OsStr::new("moved"),
        RenameFlags::empty(),
    )
    .unwrap();
    tree.collect_unused();
    assert_eq!(tree.lookup(ROOT, OsStr::new("nested")), Err(Errno::ENOENT));
    assert_eq!(tree.lookup(ROOT, OsStr::new("moved")).unwrap(), parent);
    let fh = tree.open(child, libc::O_RDONLY).unwrap();
    assert_eq!(read(&mut tree, fh), "ours");
    assert_eq!(
        fs::read_to_string(host.path().join("nested/file")).unwrap(),
        "host"
    );
    assert!(host.path().join("nested/deleted").exists());
}

#[test]
fn failed_operations_release_temporary_nodes_and_handles() {
    let host = tempfile::tempdir().unwrap();
    fs::write(host.path().join("file"), "host").unwrap();
    fs::write(host.path().join(NAME), "credential").unwrap();
    let view = ConfigFs(Mutex::new(Tree::new(host.path(), b"{}").unwrap()));
    let baseline = view.0.lock().unwrap().nodes.len();
    assert_eq!(
        view.with(|tree| {
            let ino = tree.lookup(ROOT, OsStr::new("file"))?;
            tree.open(ino, libc::O_RDONLY | libc::O_TRUNC)
        }),
        Err(Errno::EBADF)
    );
    assert_eq!(
        view.with(|tree| tree.open(CREDENTIAL, libc::O_RDONLY | libc::O_TRUNC)),
        Err(Errno::EBADF)
    );
    let tree = view.0.lock().unwrap();
    assert!(tree.handles.is_empty());
    assert_eq!(tree.nodes.len(), baseline);
    drop(tree);
    assert_eq!(
        view.with(|tree| {
            let ino = tree.lookup(ROOT, OsStr::new("file"))?;
            tree.opendir(ino)
        }),
        Err(Errno::ENOTDIR)
    );
    let tree = view.0.lock().unwrap();
    assert!(tree.directories.is_empty());
    assert_eq!(tree.nodes.len(), baseline);
}
