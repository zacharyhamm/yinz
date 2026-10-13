use super::*;

#[test]
fn cancelled_git_transfer_does_not_publish_a_selection() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connected = yinz_test_support::Watched::new(false);
    let notify = connected.clone();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (_socket, _) = listener.accept().unwrap();
        *notify.lock() = true;
        let _ = released.recv_timeout(Duration::from_secs(20));
    });
    let data = tempfile::tempdir().unwrap();
    let plugin = GitPlugin {
        repo: format!("git://127.0.0.1:{port}/"),
        entrypoint: "main.ts".into(),
        revision: None,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = {
        let plugin = plugin.clone();
        let data = data.path().to_owned();
        let cancel = cancel.clone();
        std::thread::spawn(move || resolve(&data, &plugin, &[], &cancel))
    };
    connected.wait_until(
        "Git transfer connection",
        Duration::from_secs(15),
        |value| value.then_some(()),
    );
    cancel.store(true, Ordering::Release);
    assert!(format!("{:#}", worker.join().unwrap().unwrap_err()).contains("cancelled"));
    release.send(()).unwrap();
    server.join().unwrap();
    assert!(status(data.path(), &plugin).unwrap().is_none());
}
use std::{
    net::TcpListener,
    os::unix::process::CommandExt,
    process::{Child, Stdio},
    sync::Arc,
};

struct Remote {
    directory: tempfile::TempDir,
    server: Option<Child>,
    url: String,
}

impl Remote {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fixture_git(directory.path(), &["init", "-b", "main"]);
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("main.ts"), "export default {};\n").unwrap();
        fs::write(directory.path().join("src/main.ts"), "export default {};\n").unwrap();
        fs::write(
            directory.path().join("shared.ts"),
            "export const value = 1;\n",
        )
        .unwrap();
        fixture_git(directory.path(), &["add", "."]);
        fixture_git(directory.path(), &["commit", "-m", "first"]);
        fixture_git(directory.path(), &["tag", "v1"]);
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let server = Command::new("git")
            .process_group(0)
            .args([
                "daemon",
                "--listen=127.0.0.1",
                &format!("--port={port}"),
                "--export-all",
                "--reuseaddr",
                &format!("--base-path={}", directory.path().display()),
                directory.path().to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let remote = Self {
            directory,
            server: Some(server),
            url: format!("git://127.0.0.1:{port}/"),
        };
        // Wait on readiness in another thread so Watched wakes on the condition.
        let ready = yinz_test_support::Watched::new(false);
        let probe = ready.clone();
        let waiter = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
                assert!(Instant::now() < deadline, "Git daemon did not start");
                std::thread::yield_now();
            }
            *probe.lock() = true;
        });
        ready.wait_until("Git daemon readiness", Duration::from_secs(15), |ready| {
            ready.then_some(())
        });
        waiter.join().unwrap();
        remote
    }

    fn plugin(&self, revision: Option<GitRevision>) -> GitPlugin {
        GitPlugin {
            repo: self.url.clone(),
            entrypoint: "main.ts".into(),
            revision,
        }
    }

    fn commit(&self, source: &str) {
        fs::write(self.directory.path().join("main.ts"), source).unwrap();
        fixture_git(self.directory.path(), &["add", "."]);
        fixture_git(self.directory.path(), &["commit", "-m", "change"]);
    }

    fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            // Git spawns git-daemon, which can spawn connection workers.
            unsafe { libc::kill(-(server.id() as i32), libc::SIGKILL) };
            let _ = server.wait();
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn git_remote_cleanup_closes_listener_and_connection_workers() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    for stop_first in [false, true] {
        let mut remote = Remote::new();
        let address = remote
            .url
            .strip_prefix("git://")
            .unwrap()
            .trim_end_matches('/');
        let mut connection = TcpStream::connect(address).unwrap();
        let address = connection.peer_addr().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let request = format!("git-upload-pack /\0host={address}\0");
        write!(connection, "{:04x}{request}", request.len() + 4).unwrap();
        // Receiving an advertisement proves a connection worker has started.
        connection.read_exact(&mut [0; 4]).unwrap();
        if stop_first {
            remote.stop();
            remote.stop();
        }
        drop(remote);
        let result = connection.read_to_end(&mut Vec::new());
        assert!(
            result.is_ok()
                || matches!(result, Err(ref error) if error.kind() == std::io::ErrorKind::ConnectionReset),
            "Git connection worker survived cleanup: {result:?}"
        );
        let closed = yinz_test_support::Watched::new(false);
        let notify = closed.clone();
        let probe = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while TcpStream::connect(address).is_ok() {
                assert!(Instant::now() < deadline, "Git listener survived cleanup");
                std::thread::yield_now();
            }
            *notify.lock() = true;
        });
        closed.wait_until("Git listener cleanup", Duration::from_secs(15), |closed| {
            closed.then_some(())
        });
        probe.join().unwrap();
    }
}

fn fixture_git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Plugin test",
            "-c",
            "user.email=plugin@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn git_install_update_pinning_offline_and_failed_update() {
    let mut remote = Remote::new();
    let data = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let default = remote.plugin(None);
    assert!(status(data.path(), &default).unwrap().is_none());
    assert!(!data.path().join("plugin-repositories").exists());
    let first = resolve(data.path(), &default, &[], &cancel).unwrap();
    assert!(crate::write(&first.root, "main.ts", "export default {};", true, false).is_err());
    assert!(!first.root.join("main.bak.ts").exists());
    let branch = remote.plugin(Some(GitRevision::Branch("main".into())));
    let tag = remote.plugin(Some(GitRevision::Tag("v1".into())));
    let commit = remote.plugin(Some(GitRevision::Commit(first.commit.clone().unwrap())));
    for plugin in [&branch, &tag, &commit] {
        assert_eq!(
            resolve(data.path(), plugin, &[], &cancel).unwrap().commit,
            first.commit
        );
    }
    remote.commit("export default { changed: true };\n");
    assert_eq!(resolve(data.path(), &default, &[], &cancel).unwrap(), first);
    let next = install(
        data.path(),
        &default,
        std::slice::from_ref(&default),
        true,
        &cancel,
    )
    .unwrap();
    assert_ne!(next.commit, first.commit);
    assert_eq!(
        fs::read_to_string(&first.entrypoint).unwrap(),
        "export default {};\n"
    );
    assert_eq!(
        install(data.path(), &branch, &[], true, &cancel)
            .unwrap()
            .commit,
        next.commit
    );
    for plugin in [&tag, &commit] {
        assert_eq!(
            install(data.path(), plugin, &[], true, &cancel)
                .unwrap()
                .commit,
            first.commit
        );
    }
    // A sibling alias's entrypoint must exist before a shared selection advances.
    let mut nested = default.clone();
    nested.entrypoint = "src/main.ts".into();
    fs::remove_file(remote.directory.path().join("src/main.ts")).unwrap();
    remote.commit("export default { broken: true };\n");
    assert!(install(
        data.path(),
        &default,
        &[default.clone(), nested],
        true,
        &cancel
    )
    .is_err());
    assert_eq!(status(data.path(), &default).unwrap().unwrap(), next);
    fs::remove_file(remote.directory.path().join("main.ts")).unwrap();
    fixture_git(remote.directory.path(), &["add", "."]);
    fixture_git(
        remote.directory.path(),
        &["commit", "-m", "missing entrypoint"],
    );
    assert!(install(data.path(), &default, &[], true, &cancel).is_err());
    assert_eq!(resolve(data.path(), &default, &[], &cancel).unwrap(), next);
    fixture_git(remote.directory.path(), &["branch", "feature", "v1"]);
    let feature = remote.plugin(Some(GitRevision::Branch("feature".into())));
    let feature_install = resolve(data.path(), &feature, &[], &cancel).unwrap();
    fixture_git(remote.directory.path(), &["branch", "-D", "feature"]);
    assert!(install(data.path(), &feature, &[], true, &cancel).is_err());
    assert_eq!(
        status(data.path(), &feature).unwrap().unwrap(),
        feature_install
    );
    remote.stop();
    assert_eq!(resolve(data.path(), &default, &[], &cancel).unwrap(), next);
    assert_eq!(
        install(data.path(), &tag, &[], true, &cancel)
            .unwrap()
            .commit,
        first.commit
    );
}

#[test]
fn failed_tag_installs_recover_through_install_and_update_and_stay_pinned() {
    let remote = Remote::new();
    let cancel = AtomicBool::new(false);
    for update in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let name = format!("late-{update}");
        let plugin = remote.plugin(Some(GitRevision::Tag(name.clone())));
        assert!(install(data.path(), &plugin, &[], false, &cancel).is_err());
        assert!(status(data.path(), &plugin).unwrap().is_none());
        fixture_git(remote.directory.path(), &["tag", &name]);
        let installed = install(data.path(), &plugin, &[], update, &cancel).unwrap();
        remote.commit(&format!("export const changed = '{name}';\n"));
        fixture_git(remote.directory.path(), &["tag", "-f", &name]);
        for update in [false, true] {
            assert_eq!(
                install(data.path(), &plugin, &[], update, &cancel).unwrap(),
                installed
            );
        }
    }
}

#[test]
fn failed_default_branch_install_refreshes_the_remote_head() {
    let remote = Remote::new();
    let data = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let mut plugin = remote.plugin(None);
    plugin.entrypoint = "late.ts".into();
    assert!(resolve(data.path(), &plugin, &[], &cancel).is_err());
    fixture_git(remote.directory.path(), &["checkout", "-b", "new-default"]);
    fs::write(
        remote.directory.path().join("late.ts"),
        "export default {};\n",
    )
    .unwrap();
    remote.commit("export const changed = true;\n");
    let installed = resolve(data.path(), &plugin, &[], &cancel).unwrap();
    assert_eq!(
        installed.commit.unwrap(),
        fixture_git(remote.directory.path(), &["rev-parse", "HEAD"])
    );
}

#[test]
fn concurrent_git_installs_and_cancelled_lock_wait_leave_selection_intact() {
    let remote = Remote::new();
    let data = tempfile::tempdir().unwrap();
    let plugin = remote.plugin(None);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let plugin = plugin.clone();
            let data = data.path().to_owned();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                resolve(&data, &plugin, &[], &AtomicBool::new(false)).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let installs = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert!(installs.iter().all(|value| value == &installs[0]));
    let lock = File::options()
        .read(true)
        .write(true)
        .open(installation(data.path(), &plugin).join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let entered = yinz_test_support::Watched::new(false);
    let worker = {
        let cancel = cancel.clone();
        let entered = entered.clone();
        let plugin = plugin.clone();
        let data = data.path().to_owned();
        std::thread::spawn(move || {
            *entered.lock() = true;
            install(&data, &plugin, &[], true, &cancel)
        })
    };
    entered.wait_until("update started", Duration::from_secs(10), |value| {
        value.then_some(())
    });
    cancel.store(true, Ordering::Release);
    assert!(worker
        .join()
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert_eq!(status(data.path(), &plugin).unwrap().unwrap(), installs[0]);
}

#[test]
fn local_git_plugins_are_read_only_and_reject_symlink_escapes() {
    let repository = tempfile::tempdir().unwrap();
    fixture_git(repository.path(), &["init", "-b", "main"]);
    fs::write(repository.path().join("main.ts"), "dirty plugin").unwrap();
    let before = fixture_git(repository.path(), &["status", "--porcelain"]);
    let plugin = GitPlugin {
        repo: repository.path().to_string_lossy().into_owned(),
        entrypoint: "main.ts".into(),
        revision: None,
    };
    let data = tempfile::tempdir().unwrap();
    let installed = install(data.path(), &plugin, &[], true, &AtomicBool::new(false)).unwrap();
    assert_eq!(installed.root, repository.path().canonicalize().unwrap());
    assert_eq!(
        before,
        fixture_git(repository.path(), &["status", "--porcelain"])
    );
    assert!(!data.path().join("plugin-repositories").exists());
    fs::remove_file(&installed.entrypoint).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("main.ts"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path().join("main.ts"), &installed.entrypoint).unwrap();
    assert!(resolve(data.path(), &plugin, &[], &AtomicBool::new(false)).is_err());
}
