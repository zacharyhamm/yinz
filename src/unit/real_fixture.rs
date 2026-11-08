//! Process-owned real-server fixtures. The guard outlives a killed libtest process.
//!
//! Session claims cover unlinked endpoints and workers; the inherited shared lease covers
//! daemon startup and final process exit. PTY supervisors close inherited descriptors, so
//! their pane groups additionally require the existing confirmed orphan sweep. No PID is signalled.
//!
//! Each guard runs outside nextest's process group and receives EOF on its CLOEXEC lifeline
//! when its test exits, unwinds, or is killed. Cleanup has one 30-second budget. A failed or
//! killed guard leaves its private trees and diagnostics in place; there is no prefix/age/PID
//! janitor. In particular, TestRun/OwnedDir reclamation cannot safely delete these trees while
//! servers are alive, and Drop cannot recover a guard that is itself SIGKILLed.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions, TryLockError},
    io::{BufRead, Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use yinz_runtime::errors::prelude::*;

const GUARD_ROOT: &str = "YINZ_REAL_FIXTURE_GUARD";
const GUARD_SOCKETS: &str = "YINZ_REAL_FIXTURE_SOCKETS";
const RESPONSE: &str = "YINZ_FIXTURE_RESULT ";
const BUDGET: Duration = Duration::from_secs(30);

pub(super) struct RealFixture {
    path: PathBuf,
    sockets: PathBuf,
    users: Option<Arc<File>>,
    guard: Child,
    result: Mutex<mpsc::Receiver<Result<(), String>>>,
    stages: yinz_test_support::Watched<Vec<String>>,
}

impl RealFixture {
    pub(super) fn new(prefix: &str) -> Self {
        Self::create(prefix, None).expect("create owned real-server fixture")
    }

    pub(super) fn new_in(prefix: &str, parent: &Path) -> Self {
        Self::create(
            prefix,
            Some(&parent.join(format!("yz-fixtures-{}", nix::unistd::Uid::current()))),
        )
        .expect("create owned real-server fixture")
    }

    fn create(prefix: &str, parent: Option<&Path>) -> Result<Self> {
        // Siblings of TestRun, not children: nextest may finish while an interrupted test's
        // guard is still cleaning up. Honor the managed location without letting TestRun
        // remove a live fixture. /tmp remains the short fallback for direct cargo test.
        let parent = parent.map(Path::to_path_buf).unwrap_or_else(|| {
            std::env::var_os(yinz_runtime::owned_dir::TEST_CACHE_ROOT_ENV)
                .and_then(|path| PathBuf::from(path).parent().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(format!("yz-fixtures-{}", nix::unistd::Uid::current()))
        });
        yinz_runtime::shared::ensure_secure_dir(&parent).context("secure fixture registry")?;
        let root = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .context("create fixture directory")?;
        let path = root.path().canonicalize().context("canonicalize fixture")?;
        // Unix endpoints need short paths even when managed storage is on a long mount path.
        let sockets = tempfile::Builder::new()
            .prefix("yf-")
            .tempdir_in("/tmp")
            .context("create short fixture socket namespace")?;
        let socket_path = sockets
            .path()
            .canonicalize()
            .context("canonicalize fixture sockets")?;
        std::os::unix::fs::symlink(&socket_path, path.join("sockets"))
            .context("link fixture socket namespace")?;
        let users = File::create(path.join(".users")).context("create fixture lifetime lease")?;
        users.lock_shared().context("lock fixture lifetime lease")?;
        let log = File::create(path.join("teardown.log")).context("create guard log")?;
        let worker = format!(
            "{}::guard_worker",
            module_path!()
                .split_once("::")
                .context("fixture module name")?
                .1
        );
        let mut guard = Command::new(std::env::current_exe().context("find fixture test binary")?)
            .args(["--exact", &worker, "--ignored", "--nocapture"])
            .env(GUARD_ROOT, &path)
            .env(GUARD_SOCKETS, &socket_path)
            // Nextest may terminate the test's process group. The guard must survive it.
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn()
            .context("start fixture guard")?;
        // Once the guard exists it owns deletion, including constructor failure/unwind.
        let _retained = root.into_path();
        let _retained_sockets = sockets.into_path();
        let stages = yinz_test_support::Watched::new(Vec::new());
        let observed = stages.clone();
        let stdout = guard.stdout.take().context("fixture guard stdout")?;
        let (send, result) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = (|| {
                for line in std::io::BufReader::new(stdout).lines() {
                    let line = line.context("read fixture guard response")?;
                    if let Some(stage) = line.strip_prefix("YINZ_FIXTURE_STAGE ") {
                        observed.lock().push(stage.to_owned());
                    }
                    if let Some(json) = line.strip_prefix(RESPONSE) {
                        return serde_json::from_str(json).context("decode fixture guard result");
                    }
                }
                bail!("fixture guard exited without a result")
            })();
            let _ = send.send(outcome.unwrap_or_else(|error| Err(format!("{error:#}"))));
        });
        stages.wait_until("fixture guard to claim cleanup", BUDGET, |stages| {
            stages.iter().any(|stage| stage == "ready").then_some(())
        });
        eprintln!(
            "real-server fixture: {} (sockets {})",
            path.display(),
            socket_path.display()
        );
        Ok(Self {
            path,
            sockets: socket_path,
            users: Some(Arc::new(users)),
            guard,
            result: Mutex::new(result),
            stages,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn socket_dir(&self) -> &Path {
        &self.sockets
    }

    /// Pass only this fixture's lease, including through daemonization and re-exec.
    /// The parent's descriptor stays CLOEXEC so independent fixtures cannot pin each other.
    pub(super) fn inherit(&self, command: &mut Command) {
        let users = self.users.as_ref().expect("fixture is active").clone();
        // SAFETY: the closure only changes descriptor flags with fcntl after fork. The Arc
        // keeps the descriptor valid until spawn; no shared parent flags are changed.
        unsafe {
            command.pre_exec(move || {
                if nix::libc::fcntl(users.as_raw_fd(), nix::libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    fn finish(&mut self) -> Result<()> {
        drop(self.users.take());
        drop(self.guard.stdin.take());
        let outcome = self
            .result
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .recv_timeout(BUDGET + Duration::from_secs(5))
            .with_context(|| format!("wait for fixture guard; stages: {:?}", self.stages.get()))?;
        // A result is emitted immediately before process::exit; reap the guard itself too.
        let status = self.guard.wait().context("reap fixture guard")?;
        outcome.map_err(|error| anyhow!(error))?;
        anyhow::ensure!(status.success(), "fixture guard failed: {status}");
        Ok(())
    }
}

impl Drop for RealFixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            diagnostics(&self.path);
        }
        if let Err(error) = self.finish() {
            diagnostics(&self.path);
            let message = format!(
                "fixture teardown failed; retained {}: {error:#}",
                self.path.display()
            );
            if std::thread::panicking() {
                let _ = writeln!(std::io::stderr(), "{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

fn diagnostics(root: &Path) {
    // Diagnostics must never panic while unwinding, even if tmp or the whole tree is missing.
    for path in [
        root.join("teardown.log"),
        root.join(format!(
            "tmp/yinz-{}/yinz-log/yinz.log",
            nix::unistd::Uid::current()
        )),
    ] {
        if let Ok(log) = fs::read_to_string(&path) {
            let _ = writeln!(std::io::stderr(), "{}:\n{log}", path.display());
        }
    }
}

#[test]
#[ignore = "fixture cleanup subprocess; invoked with a private root and stdin lifeline"]
fn guard_worker() {
    let path = PathBuf::from(std::env::var_os(GUARD_ROOT).expect("private fixture root"));
    let sockets = PathBuf::from(std::env::var_os(GUARD_SOCKETS).expect("private socket namespace"));
    let guard_lease = File::create(path.join(".guard")).expect("create guard ownership lease");
    guard_lease.lock().expect("claim fixture guard");
    stage("ready");
    // EOF follows normal Drop, unwind, or test death. The write end is never exec-inherited.
    let result = std::io::stdin()
        .read_to_end(&mut Vec::new())
        .map_err(|e| e.to_string());
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = result.and_then(|_| cleanup(&path, &sockets).map_err(|e| format!("{e:#}")));
        let _ = send.send(result);
    });
    let result = match receive.recv_timeout(BUDGET) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!("fixture cleanup exceeded {BUDGET:?}")),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("fixture cleanup worker exited without a result".into())
        },
    };
    if let Err(error) = &result {
        eprintln!("fixture cleanup retained its tree: {error}");
    }
    let _ = writeln!(
        std::io::stdout(),
        "{RESPONSE}{}",
        serde_json::to_string(&result).expect("encode guard result")
    );
    let _ = std::io::stdout().flush();
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

fn entries(path: &Path) -> Result<Vec<fs::DirEntry>> {
    match fs::read_dir(path) {
        Ok(entries) => entries
            .collect::<std::io::Result<Vec<_>>>()
            .context("read fixture entries"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read fixture directory {}", path.display())),
    }
}

fn stage(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
    let _ = writeln!(std::io::stdout(), "YINZ_FIXTURE_STAGE {message}");
}

fn try_lock(file: &File) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error).context("lock fixture ownership"),
    }
}

fn cleanup(root: &Path, sockets: &Path) -> Result<()> {
    // The guard receives these exact created paths before any fixture commands run.
    // The diagnostic symlink at root/sockets is never used to choose shutdown targets.
    let directory = sockets.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    let mut errors = Vec::new();
    // Also covers servers in daemon startup, final client flushing, and pane descendants.
    // This is a kernel ownership barrier, not a quiet-period or socket-existence heuristic.
    let users = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".users"))
        .context("open fixture lifetime lease")?;
    let mut users_locked = false;
    let mut claims = BTreeMap::<PathBuf, (File, bool)>::new();
    let mut kills =
        BTreeMap::<(u64, u64), (PathBuf, std::thread::JoinHandle<std::io::Result<()>>)>::new();
    // Avoid duplicate requests after completion, but recognize a replacement at the same path.
    let mut attempted = BTreeSet::new();
    let mut drained = false;
    let mut first_pass = true;
    loop {
        // A daemon can bind after EOF or while another shutdown is still in flight. Keep
        // discovering without blocking on either ownership or individual completion replies.
        if !drained {
            for entry in entries(&directory)? {
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error).context("inspect fixture endpoint"),
                };
                let identity = (metadata.dev(), metadata.ino());
                if yinz_ipc::consts::is_ipc_socket(&metadata.file_type())
                    && attempted.insert(identity)
                {
                    let path = entry.path();
                    let endpoint = path.clone();
                    let kill = std::thread::spawn(move || {
                        yinz_ipc::ipc::kill_session_with_completion(&endpoint)
                    });
                    kills.insert(identity, (path, kill));
                }
            }
        }
        // Claims survive rename/unlink. Keep acquired locks until removal, and discover new
        // claims as well: a daemon may not have claimed its endpoint on the first pass.
        for claim in entries(&directory.join(yinz_ipc::consts::SESSION_OWNERSHIP_DIR))? {
            if let std::collections::btree_map::Entry::Vacant(entry) = claims.entry(claim.path()) {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(claim.path())
                    .with_context(|| format!("open fixture claim {}", claim.path().display()))?;
                stage(&format!("wait claim {}", claim.file_name().display()));
                entry.insert((file, false));
            }
        }
        let mut claims_locked = true;
        for (path, (file, locked)) in &mut claims {
            if !*locked {
                *locked = try_lock(file)
                    .with_context(|| format!("wait for fixture claim {}", path.display()))?;
            }
            claims_locked &= *locked;
        }
        if !users_locked {
            users_locked = try_lock(&users).context("wait for fixture processes to exit")?;
        }
        drained = users_locked && claims_locked;
        if first_pass {
            stage("wait processes");
            first_pass = false;
        }
        let finished: Vec<_> = kills
            .iter()
            .filter(|(_, (_, kill))| kill.is_finished())
            .map(|(identity, _)| *identity)
            .collect();
        for identity in finished {
            let (path, kill) = kills
                .remove(&identity)
                .context("finished fixture shutdown")?;
            let result = kill
                .join()
                .map_err(|_| anyhow!("fixture shutdown thread panicked"))?;
            if let Err(error) = result {
                // Missing/refused endpoints are retried while ownership remains held; they
                // never authorize deletion. Preserve other errors even if ownership drains.
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) {
                    attempted.remove(&identity);
                } else {
                    let message = format!("shutdown {}: {error}", path.display());
                    eprintln!("{message}");
                    errors.push(message);
                }
            }
        }
        if drained && kills.is_empty() {
            break;
        }
        // The guard's outer BUDGET bounds discovery, ownership waits, and shutdown together.
        std::thread::sleep(Duration::from_millis(25));
    }
    // Supervisors deliberately close inherited descriptors and can survive a dead server.
    // Query only this private namespace, then use the existing conditional, confirmed sweep.
    let pty = directory.join("pty");
    let mut sessions = std::collections::BTreeSet::new();
    for entry in entries(&pty)? {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "sock")
        {
            if yinz_process::pty_host::client::socket_is_dead(&entry.path()) {
                continue;
            }
            let status = yinz_process::pty_host::client::query(&entry.path())
                .with_context(|| format!("query fixture supervisor {}", entry.path().display()))?;
            anyhow::ensure!(
                !status.attached,
                "fixture supervisor still attached: {status:?}"
            );
            sessions.insert(
                status
                    .session_name
                    .context("fixture supervisor has no session identity")?,
            );
        }
    }
    for session in sessions {
        stage(&format!("reclaim detached panes for {session}"));
        yinz_process::pty_host::cli::terminate_orphans_of_session_in(&pty, session.as_str())
            .context("confirm fixture orphan shutdown")?;
    }
    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    stage("remove tree");
    remove_trees(root, sockets, |path| fs::remove_dir_all(path))
}

fn remove_trees(
    root: &Path,
    sockets: &Path,
    remove: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<()> {
    remove(sockets)
        .with_context(|| format!("remove fixture socket namespace {}", sockets.display()))?;
    remove(root).with_context(|| format!("remove fixture tree {}", root.display()))
}

#[test]
fn cleanup_discovers_late_servers_while_ownership_or_shutdown_is_pending() {
    use interprocess::local_socket::traits::Listener;
    use yinz_ipc::{
        ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg},
        session_ownership::SessionOwnership,
    };

    for pending_shutdown in [false, true] {
        let fixture = RealFixture::new_in("ylb-", Path::new("/tmp"));
        let root = fixture.path().to_owned();
        let sockets = fixture.socket_dir().to_owned();
        let directory = sockets.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
        fs::create_dir_all(&directory).unwrap();
        // This is the daemon's inherited lease before it has claimed or bound an endpoint.
        let daemon_lease = fixture.users.as_ref().unwrap().clone();
        let (late_stopped, wait_late) = mpsc::channel();
        let pending = pending_shutdown.then(|| {
            let owner = SessionOwnership::acquire(&directory.join("pending"))
                .unwrap()
                .unwrap();
            let listener = owner.bind().unwrap();
            std::thread::spawn(move || {
                let mut receiver =
                    IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
                assert!(matches!(
                    receiver.recv_client_msg(),
                    IpcRecv::Msg(ClientToServerMsg::KillSessionWithCompletion { .. }, _)
                ));
                // Completion depends on stopping the late daemon. Joining this request in
                // the discovery loop would prevent that daemon from ever being discovered.
                wait_late.recv_timeout(BUDGET).unwrap();
                receiver
                    .get_sender::<ServerToClientMsg>()
                    .unwrap()
                    .send_server_msg(ServerToClientMsg::KillSessionComplete { error: None })
                    .unwrap();
                owner.shutdown().unwrap();
            })
        });
        let stages = fixture.stages.clone();
        let cleanup = std::thread::spawn(move || drop(fixture));
        stages.wait_until("cleanup's initial discovery pass", BUDGET, |stages| {
            stages
                .iter()
                .any(|stage| stage == "wait processes")
                .then_some(())
        });
        // Both the endpoint and its claim appear only after the initial discovery pass.
        let owner = SessionOwnership::acquire(&directory.join("late"))
            .unwrap()
            .unwrap();
        let listener = owner.bind().unwrap();
        let stopped = yinz_test_support::Watched::new(false);
        let observed = stopped.clone();
        let daemon = std::thread::spawn(move || {
            let mut receiver =
                IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::KillSessionWithCompletion { .. }, _)
            ));
            receiver
                .get_sender::<ServerToClientMsg>()
                .unwrap()
                .send_server_msg(ServerToClientMsg::KillSessionComplete { error: None })
                .unwrap();
            owner.shutdown().unwrap();
            drop(owner);
            drop(daemon_lease);
            if pending_shutdown {
                late_stopped.send(()).unwrap();
            }
            *observed.lock() = true;
        });
        stopped.wait_until("late daemon shutdown", BUDGET, |stopped| {
            stopped.then_some(())
        });
        daemon.join().unwrap();
        if let Some(pending) = pending {
            pending.join().unwrap();
        }
        cleanup.join().unwrap();
        assert!(!root.exists());
        assert!(!sockets.exists());
    }
}

#[test]
fn unlinked_and_relocated_claims_hold_tree_until_delayed_checkpoint_finishes() {
    use yinz_ipc::session_ownership::SessionOwnership;
    let fixture = RealFixture::new("ycu-");
    let root = fixture.path().to_owned();
    let sockets = root.join("sockets").canonicalize().unwrap();
    let directory = sockets.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    fs::create_dir_all(&directory).unwrap();
    let owner = SessionOwnership::acquire(&directory.join("private-update"))
        .unwrap()
        .unwrap();
    let listener = owner.bind().unwrap();
    owner.relocate(&directory.join("retired-source")).unwrap();
    owner.shutdown().unwrap();
    drop(listener);
    let other = SessionOwnership::acquire(&directory.join("replacement"))
        .unwrap()
        .unwrap();
    let stages = fixture.stages.clone();
    let finished = yinz_test_support::Watched::new(false);
    let completed = finished.clone();
    let cleanup = std::thread::spawn(move || {
        drop(fixture);
        *completed.lock() = true;
    });
    stages.wait_until("cleanup to wait on retained ownership", BUDGET, |stages| {
        stages
            .iter()
            .any(|stage| stage.starts_with("wait claim"))
            .then_some(())
    });
    assert!(root.exists());
    assert!(!finished.get());
    // A synchronous checkpoint resumes after the endpoint has been unlinked. It must finish
    // while the tree still exists, before the final ownership holder allows deletion.
    let checkpoint = root.join("cache/session");
    yinz_runtime::session_checkpoint::write_checkpoint(
        &checkpoint,
        "delayed metadata",
        &("layout { pane; }".into(), Default::default()),
    )
    .unwrap();
    assert!(checkpoint.join("current").exists());
    drop(owner);
    drop(other);
    cleanup.join().unwrap();
    assert!(finished.get());
    assert!(!root.exists());
}

#[test]
fn unwinding_with_missing_diagnostic_directory_still_removes_tree() {
    let fixture = RealFixture::new("ysr-");
    let root = fixture.path().to_owned();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _fixture = fixture;
        panic!("failing test body");
    }));
    assert!(panic.is_err());
    assert!(!root.exists());
}

#[test]
fn removal_errors_report_the_path_and_retain_the_tree() {
    let fixture = RealFixture::new("ysr-");
    let root = fixture.path().to_owned();
    let error = remove_trees(&root, fixture.socket_dir(), |path| {
        if path == root {
            Err(std::io::Error::other("injected removal failure"))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("injected removal failure"));
    assert!(message.contains(root.to_str().unwrap()));
    assert!(root.exists());
    drop(fixture);
    assert!(!root.exists());
}

#[test]
fn cleanup_failure_fails_normal_drop_and_preserves_the_original_unwind() {
    for unwind in [false, true] {
        let fixture = RealFixture::new("ysr-");
        let root = fixture.path().to_owned();
        let sockets = fixture.socket_dir().to_owned();
        use interprocess::local_socket::traits::Listener;
        use yinz_ipc::ipc::{
            ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg,
        };
        let directory = sockets.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
        fs::create_dir_all(&directory).unwrap();
        let endpoint = directory.join("failed-shutdown");
        let listener = yinz_ipc::consts::ipc_bind(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let mut receiver =
                IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::KillSessionWithCompletion { .. }, _)
            ));
            receiver
                .get_sender::<ServerToClientMsg>()
                .unwrap()
                .send_server_msg(ServerToClientMsg::KillSessionComplete {
                    error: Some("deliberate shutdown failure".into()),
                })
                .unwrap();
        });
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _fixture = fixture;
            if unwind {
                panic!("original test failure");
            }
        }))
        .unwrap_err();
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(message.contains(if unwind {
            "original test failure"
        } else {
            "fixture teardown failed; retained"
        }));
        assert!(root.exists());
        assert!(sockets.exists());
        server.join().unwrap();
        assert!(fs::read_to_string(root.join("teardown.log"))
            .unwrap()
            .contains("deliberate shutdown failure"));
        cleanup(&root, &sockets).unwrap();
        assert!(!root.exists());
        assert!(!sockets.exists());
    }
}

#[test]
fn diagnostic_socket_symlink_cannot_redirect_cleanup() {
    let fixture = RealFixture::new("ysr-");
    let root = fixture.path().to_owned();
    let sockets = fixture.socket_dir().to_owned();
    let unrelated = tempfile::tempdir().unwrap();
    fs::write(unrelated.path().join("keep"), "unrelated fixture").unwrap();
    fs::remove_file(root.join("sockets")).unwrap();
    std::os::unix::fs::symlink(unrelated.path(), root.join("sockets")).unwrap();
    drop(fixture);
    assert!(!root.exists());
    assert!(!sockets.exists());
    assert_eq!(
        fs::read_to_string(unrelated.path().join("keep")).unwrap(),
        "unrelated fixture"
    );
}
