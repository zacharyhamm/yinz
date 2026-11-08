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
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
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
        Self::create(prefix).expect("create owned real-server fixture")
    }

    fn create(prefix: &str) -> Result<Self> {
        // Siblings of TestRun, not children: nextest may finish while an interrupted test's
        // guard is still cleaning up. Honor the managed location without letting TestRun
        // remove a live fixture. /tmp remains the short fallback for direct cargo test.
        let parent = std::env::var_os(yinz_runtime::owned_dir::TEST_CACHE_ROOT_ENV)
            .and_then(|path| PathBuf::from(path).parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(format!("yz-fixtures-{}", nix::unistd::Uid::current()));
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

fn cleanup(root: &Path, sockets: &Path) -> Result<()> {
    // The guard receives these exact created paths before any fixture commands run.
    // The diagnostic symlink at root/sockets is never used to choose shutdown targets.
    let directory = sockets.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    let claims = entries(&directory.join(yinz_ipc::consts::SESSION_OWNERSHIP_DIR))?;
    let mut errors = Vec::new();
    // Dispatch to all instances before waiting: retired sources and private destinations can
    // depend on one another. Claim files survive rename/unlink and must also be waited on.
    let mut kills = Vec::new();
    for entry in entries(&directory)? {
        if yinz_ipc::consts::is_ipc_socket(&entry.file_type().context("inspect fixture endpoint")?)
        {
            let path = entry.path();
            kills.push(std::thread::spawn(move || {
                let result = yinz_ipc::ipc::kill_session_with_completion(&path);
                (path, result)
            }));
        }
    }
    for kill in kills {
        let (path, result) = kill
            .join()
            .map_err(|_| anyhow!("fixture shutdown thread panicked"))?;
        if let Err(error) = result {
            // It may already be shutting down. Ownership/lifetime release below is still
            // required; a missing/refused endpoint alone never authorizes deletion.
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                let message = format!("shutdown {}: {error}", path.display());
                eprintln!("{message}");
                errors.push(message);
            }
        }
    }
    for claim in claims {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(claim.path())
            .with_context(|| format!("open fixture claim {}", claim.path().display()))?;
        stage(&format!("wait claim {}", claim.file_name().display()));
        file.lock()
            .with_context(|| format!("wait for fixture claim {}", claim.path().display()))?;
    }
    // Also covers servers in daemon startup, final client flushing, and pane descendants.
    // This is a kernel ownership barrier, not a quiet-period or socket-existence heuristic.
    let users = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".users"))
        .context("open fixture lifetime lease")?;
    stage("wait processes");
    users.lock().context("wait for fixture processes to exit")?;
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
