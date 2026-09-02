#![cfg(target_os = "linux")]

//! A real provider, Firecracker process, SSH connection and remote pane.
//! Run only through `cargo x firecracker-test --test`.
//!
//! Every test boots its own VM under its own session name, and a guard stops that
//! session's VMs on the way out, so a failing assertion never leaves a guest running.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};

use zellij_server::{
    firecracker::{EnsureResponse, Provider},
    os_input_output::AsyncReader,
    pty_host::remote::{
        ssh::{RemoteHosts, SshOptions},
        RemoteConn, RemoteReader,
    },
};
use zellij_test_support::Watched;
use zellij_utils::input::command::{PaneTarget, RunCommand};

const TIMEOUT: Duration = Duration::from_secs(90);
const IMAGE: &str = "zellij-dev";

#[derive(Debug, Default)]
struct Output {
    bytes: Vec<u8>,
    ended: bool,
    error: Option<String>,
}

struct Pane {
    conn: Arc<RemoteConn>,
    output: Watched<Output>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pump: Option<JoinHandle<()>>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let _ = self.conn.terminate();
    }
}

/// Stops every VM the named sessions still hold when the test ends, however it ends.
struct SessionGuard {
    provider: Provider,
    sessions: Vec<&'static str>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        for session in &self.sessions {
            let _ = self.provider.stop_session(session);
        }
    }
}

fn provider() -> Provider {
    assert!(PathBuf::from("/dev/kvm").exists(), "/dev/kvm is missing");
    let provider_path = repo_root().join("target/debug/zellij-firecracker");
    Provider::new(provider_path, Duration::from_secs(120)).unwrap()
}

#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn a_provider_boots_a_real_firecracker_remote_pane() {
    let provider = provider();
    let session = "firecracker-test";
    let name = "pane";
    let project = repo_root();
    let ensured = provider
        .ensure(session, name, Some(IMAGE), Some(&project))
        .unwrap();
    let _guard = SessionGuard {
        provider: provider.clone(),
        sessions: vec![session],
    };
    assert_eq!(provider.list(Some(session)).unwrap().vms.len(), 1);

    let text = run_in_guest(
        &ensured,
        "set -e; test -f Cargo.toml; rustc --version >/dev/null; printf 'rustc-ok '; cargo --list | grep -q '^    x '; printf 'cargo-x-ok '; getent ahostsv4 index.crates.io >/dev/null; printf 'dns-ok '; curl -4 -fsS --connect-timeout 10 --max-time 30 https://index.crates.io/config.json >/dev/null; printf 'https-ok '; codex --version >/dev/null; printf 'codex-ok '; claude --version >/dev/null; printf 'claude-ok firecracker-pane-ok '; zellij --version",
        "rustc-ok cargo-x-ok dns-ok https-ok codex-ok claude-ok firecracker-pane-ok zellij",
    );
    assert!(text.contains("firecracker-pane-ok zellij"), "{text}");
    assert_eq!(provider.stop(session, name).unwrap(), Vec::<String>::new());
    assert!(provider.list(Some(session)).unwrap().vms.is_empty());
}

/// The worktree lifecycle end to end: the copy in, a credential file, hook reporting from
/// the guest, a manual sync that merges both sides, a session rename, and the final
/// `stop_session` that finds nothing left to merge.
#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn a_worktree_vm_syncs_merges_renames_and_stops() {
    let provider = provider();
    let session = "fc-sync-a";
    let renamed = "fc-sync-b";
    let name = "sync";
    let _guard = SessionGuard {
        provider: provider.clone(),
        sessions: vec![session, renamed],
    };

    let worktree = tempfile::tempdir().unwrap();
    let worktree_path = worktree.path().canonicalize().unwrap();
    fs::write(worktree_path.join("a.txt"), "a\n").unwrap();
    fs::write(worktree_path.join("both.txt"), "both\n").unwrap();
    fs::write(worktree_path.join("gone.txt"), "gone\n").unwrap();
    fs::write(worktree_path.join("host-only.txt"), "host-only\n").unwrap();
    fs::write(worktree_path.join("guest-deleted.txt"), "keep me\n").unwrap();

    // A credential file the provider must copy in with mode 0600.
    let credentials = tempfile::tempdir().unwrap();
    let host_credential = credentials.path().join("auth.json");
    fs::write(&host_credential, "{\"token\":\"firecracker-test-secret\"}").unwrap();
    let _config = ConfigWithCredential::new(&host_credential, "/root/.zellij-test/auth.json");

    let ensured = provider
        .ensure(session, name, Some(IMAGE), Some(&worktree_path))
        .unwrap();
    assert_eq!(ensured.cwd.as_deref(), Some(Path::new("/workspace")));
    let listing = provider.list(Some(session)).unwrap();
    assert_eq!(listing.vms.len(), 1, "{listing:?}");
    assert_eq!(listing.vms[0].state, "running");
    assert_eq!(listing.vms[0].name, name);

    // Inside the guest: change every kind of file, read the credential back, and report a
    // hook event the way a Claude hook in the guest does.
    let text = run_in_guest(
        &ensured,
        concat!(
            "set -e; test \"$(cat a.txt)\" = a; echo guest >> a.txt; echo new > new.txt; ",
            "rm gone.txt guest-deleted.txt; echo guest-both > both.txt; ",
            "printf 'cred-mode=%s ' \"$(stat -c %a /root/.zellij-test/auth.json)\"; ",
            "grep -q firecracker-test-secret /root/.zellij-test/auth.json && printf 'cred-ok '; ",
            "printf '{\"hook_event_name\":\"Stop\",\"session_id\":\"fc-test\"}' | zellij --pty-broker report claude; ",
            "printf 'guest-edits-done'"
        ),
        "guest-edits-done",
    );
    assert!(text.contains("cred-mode=600 cred-ok"), "{text}");
    assert!(
        text.contains("\x1b]777;zellij-agent-report;claude;"),
        "the guest's hook report did not reach the pane: {text:?}"
    );

    // Meanwhile on the host: one file both sides changed, one the host alone changed, and
    // one the host changed that the guest deleted.
    fs::write(worktree_path.join("both.txt"), "host-both\n").unwrap();
    fs::write(worktree_path.join("host-only.txt"), "host-changed\n").unwrap();
    fs::write(worktree_path.join("guest-deleted.txt"), "host edited\n").unwrap();

    let warnings = provider.sync(session, name).unwrap();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    let both = worktree_path.join("both.txt");
    let both_guest = worktree_path.join("both.txt.guest");
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with(&both.display().to_string())
                && w.contains("changed on both sides")
                && w.contains(&both_guest.display().to_string())),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.starts_with(
            &worktree_path
                .join("guest-deleted.txt")
                .display()
                .to_string()
        )),
        "{warnings:?}"
    );
    assert_eq!(read(&worktree_path, "a.txt"), "a\nguest\n");
    assert_eq!(read(&worktree_path, "new.txt"), "new\n");
    assert!(!worktree_path.join("gone.txt").exists());
    assert_eq!(read(&worktree_path, "host-only.txt"), "host-changed\n");
    assert_eq!(read(&worktree_path, "both.txt"), "host-both\n");
    assert_eq!(read(&worktree_path, "both.txt.guest"), "guest-both\n");
    assert_eq!(read(&worktree_path, "guest-deleted.txt"), "host edited\n");

    // The sync pushed the merged tree back, so the guest sees the host's decisions.
    let text = run_in_guest(
        &ensured,
        "set -e; test \"$(cat both.txt)\" = host-both; test \"$(cat both.txt.guest)\" = guest-both; test \"$(cat host-only.txt)\" = host-changed; test ! -e gone.txt; printf 'guest-merged-ok'",
        "guest-merged-ok",
    );
    assert!(text.contains("guest-merged-ok"), "{text}");

    provider.rename_session(session, renamed).unwrap();
    assert!(provider.list(Some(session)).unwrap().vms.is_empty());
    let listing = provider.list(Some(renamed)).unwrap();
    assert_eq!(listing.vms.len(), 1, "{listing:?}");
    assert_eq!(listing.vms[0].session, renamed);

    // Nothing changed since the sync, so the release has nothing to warn about, and the
    // guest disk is not retained.
    assert_eq!(
        provider.stop_session(renamed).unwrap(),
        Vec::<String>::new()
    );
    let listing = provider.list(Some(renamed)).unwrap();
    assert!(listing.vms.is_empty(), "{listing:?}");
    assert!(
        listing
            .recoveries
            .iter()
            .all(|r| r.name.as_deref() != Some(name)),
        "the released disk was retained: {listing:?}"
    );
    assert_eq!(read(&worktree_path, "a.txt"), "a\nguest\n");
}

/// Two sessions on the same worktree share one VM; it survives the first release and
/// stops on the last.
#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn two_sessions_share_a_worktree_vm_until_the_last_one_leaves() {
    let provider = provider();
    let first = "fc-share-1";
    let second = "fc-share-2";
    let name = "shared";
    let _guard = SessionGuard {
        provider: provider.clone(),
        sessions: vec![first, second],
    };
    let worktree = tempfile::tempdir().unwrap();
    let worktree_path = worktree.path().canonicalize().unwrap();
    fs::write(worktree_path.join("shared.txt"), "shared\n").unwrap();

    let ensured = provider
        .ensure(first, name, Some(IMAGE), Some(&worktree_path))
        .unwrap();
    let joined = provider
        .ensure(second, name, Some(IMAGE), Some(&worktree_path))
        .unwrap();
    assert_eq!(
        joined.ssh_host, ensured.ssh_host,
        "the second session got its own VM"
    );

    let sessions_of = |provider: &Provider| -> Vec<String> {
        provider
            .list(None)
            .unwrap()
            .vms
            .into_iter()
            .filter(|vm| vm.name == name)
            .map(|vm| vm.session)
            .collect()
    };
    assert_eq!(
        sessions_of(&provider),
        vec![first.to_owned(), second.to_owned()]
    );

    // A different name for the same worktree is refused while the VM runs.
    let refused = provider
        .ensure(first, "other", Some(IMAGE), Some(&worktree_path))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("already running"), "{refused}");

    assert_eq!(provider.stop(first, name).unwrap(), Vec::<String>::new());
    assert_eq!(sessions_of(&provider), vec![second.to_owned()]);
    let text = run_in_guest(
        &joined,
        "set -e; test -f shared.txt; echo still-here >> shared.txt; printf 'shared-vm-alive'",
        "shared-vm-alive",
    );
    assert!(text.contains("shared-vm-alive"), "{text}");

    assert_eq!(provider.stop(second, name).unwrap(), Vec::<String>::new());
    assert!(sessions_of(&provider).is_empty());
    assert_eq!(read(&worktree_path, "shared.txt"), "shared\nstill-here\n");
}

/// Open a login-shell pane in the guest, run `script` with `sh -lc` in the VM's working
/// directory, and return everything the pane printed once it has ended with `marker`.
fn run_in_guest(ensured: &EnsureResponse, script: &str, marker: &str) -> String {
    let options = SshOptions {
        identity_file: ensured.ssh_identity_file.clone(),
        known_hosts_file: ensured.ssh_known_hosts_file.clone(),
    };
    let command = RunCommand {
        command: PathBuf::from("/bin/sh"),
        args: vec!["-lc".to_owned(), script.to_owned()],
        cwd: ensured.cwd.clone(),
        target: PaneTarget::Ssh {
            host: ensured.ssh_host.clone(),
        },
        ..Default::default()
    };
    let hosts = Arc::new(RemoteHosts::new());
    let (conn, reader_socket) = hosts
        .open_pane_with_ssh_options(&ensured.ssh_host, &command, None, 1, options)
        .unwrap();
    let reader = RemoteReader::new(conn.clone(), reader_socket).unwrap();
    let pane = pump(conn, reader);
    let text = pane.output.wait_until_described(
        &format!("the Firecracker pane to print {marker:?} and exit"),
        TIMEOUT,
        |output| {
            let text = String::from_utf8_lossy(&output.bytes);
            (output.ended && text.contains(marker)).then(|| text.into_owned())
        },
        |output| format!("{output:?}"),
    );
    drop(pane);
    text
}

fn pump(conn: Arc<RemoteConn>, mut reader: RemoteReader) -> Pane {
    let output: Watched<Output> = Watched::default();
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let pump_output = output.clone();
    let pump = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut buffer = [0; 8192];
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => return,
                    read = reader.read(&mut buffer) => match read {
                        Ok(0) => {
                            pump_output.lock().ended = true;
                            return;
                        },
                        Ok(read) => pump_output.lock().bytes.extend_from_slice(&buffer[..read]),
                        Err(error) => {
                            let mut output = pump_output.lock();
                            output.error = Some(error.to_string());
                            output.ended = true;
                            return;
                        },
                    },
                }
            }
        });
    });
    Pane {
        conn,
        output,
        cancel: Some(cancel_tx),
        pump: Some(pump),
    }
}

/// The harness's provider config plus one `credential_files` entry, selected through the
/// provider's `ZELLIJ_FIRECRACKER_CONFIG` override for as long as this value lives. The
/// harness runs tests one at a time, so the process-wide variable is not contended.
struct ConfigWithCredential {
    _dir: tempfile::TempDir,
}

impl ConfigWithCredential {
    fn new(host: &Path, guest: &str) -> Self {
        let harness = harness_config_path();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(&harness).unwrap()).unwrap();
        config["credential_files"] = serde_json::json!([{ "host": host, "guest": guest }]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firecracker.json");
        fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        std::env::set_var("ZELLIJ_FIRECRACKER_CONFIG", &path);
        Self { _dir: dir }
    }
}

impl Drop for ConfigWithCredential {
    fn drop(&mut self) {
        std::env::remove_var("ZELLIJ_FIRECRACKER_CONFIG");
    }
}

fn harness_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ZELLIJ_FIRECRACKER_CONFIG") {
        return PathBuf::from(path);
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .expect("cargo x firecracker-test sets XDG_CONFIG_HOME");
    config_home.join("zellij/firecracker.json")
}

fn read(worktree: &Path, file: &str) -> String {
    fs::read_to_string(worktree.join(file)).unwrap_or_else(|error| panic!("{file}: {error}"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
