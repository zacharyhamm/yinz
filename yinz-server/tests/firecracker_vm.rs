#![cfg(target_os = "linux")]

//! A real provider, Firecracker process, SSH connection and remote pane.
//! Run only through `cargo x firecracker-test --test`.
//!
//! Every test boots its own VM under its own session name, and a guard stops that
//! session's VMs on the way out, so a failing assertion never leaves a guest running.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    thread::JoinHandle,
    time::Duration,
};

use yinz_server::{
    firecracker::{EnsureResponse, Provider},
    os_input_output::AsyncReader,
    pty_host::remote::{
        ssh::{RemoteHosts, SshOptions},
        RemoteConn, RemoteReader,
    },
};
use yinz_test_support::Watched;
use yinz_types::command::PaneTarget;
use yinz_types::input::command::RunCommand;

const TIMEOUT: Duration = Duration::from_secs(90);
const IMAGE: &str = "yinz-dev";

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
    let provider_path = repo_root().join("target/debug/yinz-firecracker");
    Provider::new(provider_path, Duration::from_secs(120)).unwrap()
}

#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn project_workflows_observe_and_sync_a_real_guest() {
    let status = std::process::Command::new("python3")
        .arg("extensions/tests/smoke.py")
        .arg("--firecracker-only")
        .current_dir(repo_root())
        .status()
        .expect("start the workflow acceptance runner");
    assert!(
        status.success(),
        "the typed Firecracker workflow acceptance failed"
    );
}

#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn a_provider_boots_a_real_firecracker_remote_pane() {
    let provider = provider();
    let session = "firecracker-test";
    let name = "pane";
    let project = repo_root();
    let ensured = provider
        .ensure(
            session,
            name,
            Some(IMAGE),
            Some(&project),
            &AtomicBool::new(false),
        )
        .unwrap();
    let _guard = SessionGuard {
        provider: provider.clone(),
        sessions: vec![session],
    };
    let listing = provider.list(Some(session)).unwrap();
    assert_eq!(listing.vms.len(), 1);
    assert!(listing.vms[0].vm_id.is_some());
    let before = listing.vms[0].metrics.as_ref().unwrap();
    assert!(!before.generation.is_empty());
    assert!(before.memory_bytes.unwrap() > 0);
    assert!(before.disk_allocated_bytes.unwrap() > 0);
    assert!(before.disk_logical_bytes.unwrap() > 0);

    let text = run_in_guest(
        &ensured,
        "set -e; test -f Cargo.toml; rustc --version >/dev/null; printf 'rustc-ok '; cargo --list | grep -q '^    x '; printf 'cargo-x-ok '; getent ahostsv4 index.crates.io >/dev/null; printf 'dns-ok '; curl -4 -fsS --connect-timeout 10 --max-time 30 https://index.crates.io/config.json >/dev/null; printf 'https-ok '; codex --version >/dev/null; printf 'codex-ok '; claude --version >/dev/null; printf 'claude-ok firecracker-pane-ok '; yinz --version",
        "rustc-ok cargo-x-ok dns-ok https-ok codex-ok claude-ok firecracker-pane-ok yinz",
    );
    assert!(text.contains("firecracker-pane-ok yinz"), "{text}");
    let listing = provider.list(Some(session)).unwrap();
    let after = listing.vms[0].metrics.as_ref().unwrap();
    assert_eq!(before.generation, after.generation);
    assert_eq!(before.source, after.source);
    assert!(after.sample_time_us > before.sample_time_us);
    assert!(after.cpu_usage_us.unwrap() > before.cpu_usage_us.unwrap());
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

    // Two credential files the provider must copy in with mode 0600. The second is spelled
    // the way a Claude Code credential is, which is what asks for the guest's first-run
    // state to be seeded beside it.
    let credentials = tempfile::tempdir().unwrap();
    let host_credential = credentials.path().join("auth.json");
    fs::write(&host_credential, "{\"token\":\"firecracker-test-secret\"}").unwrap();
    let host_claude_credential = credentials.path().join("claude.json");
    fs::write(&host_claude_credential, "{\"claudeAiOauth\":{}}").unwrap();
    // A third, into a home that is not the account the provider logs in as. Root reading
    // everything is not the same as the guest account being given anything, so this one is
    // what says the copy ends up where `agent` can reach it.
    let host_agent_credential = credentials.path().join("agent.json");
    fs::write(&host_agent_credential, "{\"claudeAiOauth\":{}}").unwrap();
    let _config = ConfigWithCredential::new(&[
        (host_credential.as_path(), "/root/.yinz-test/auth.json"),
        (
            host_claude_credential.as_path(),
            "/root/.claude/.credentials.json",
        ),
        (
            host_agent_credential.as_path(),
            "/home/agent/.claude/.credentials.json",
        ),
    ]);

    let ensured = provider
        .ensure(
            session,
            name,
            Some(IMAGE),
            Some(&worktree_path),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(ensured.cwd.as_deref(), Some(Path::new("/workspace")));
    let listing = provider.list(Some(session)).unwrap();
    assert_eq!(listing.vms.len(), 1, "{listing:?}");
    assert_eq!(listing.vms[0].state, "running");
    assert_eq!(listing.vms[0].name, name);

    // Inside the guest: change every kind of file, read the credential back, and report a
    // hook event the way a Claude hook in the guest does - both by hand, and through the
    // script provisioning installed, which is what an agent started in the pane actually runs.
    let text = run_in_guest(
        &ensured,
        concat!(
            "set -e; test \"$(cat a.txt)\" = a; echo guest >> a.txt; echo new > new.txt; ",
            "rm gone.txt guest-deleted.txt; echo guest-both > both.txt; ",
            "printf 'cred-mode=%s ' \"$(stat -c %a /root/.yinz-test/auth.json)\"; ",
            "printf 'cred-owner=%s ' \"$(stat -c %U:%G /root/.yinz-test/auth.json)\"; ",
            "printf 'cred-dir-mode=%s ' \"$(stat -c %a /root/.yinz-test)\"; ",
            "grep -q firecracker-test-secret /root/.yinz-test/auth.json && printf 'cred-ok '; ",
            "grep -q '\"hasCompletedOnboarding\":true' /root/.claude.json && printf 'onboarded '; ",
            "grep -q '\"hasTrustDialogAccepted\":true' /root/.claude.json && printf 'trusted '; ",
            "printf '{\"sentinel\":\"guest-wrote-this\"}' > /root/.claude.json; ",
            "printf '{\"hook_event_name\":\"Stop\",\"session_id\":\"fc-test\"}' | yinz --pty-broker report claude && printf 'hook-reported '; ",
            // Provisioning wired every agent. Codex routes by conversation because
            // its shared daemon cannot inherit one pane's broker environment.
            "test -x /root/.claude/hocus/hocus-hook.sh; ",
            "test -x /root/.codex/hocus/hocus-hook.sh; ",
            "test -f /root/.config/opencode/plugin/hocus.js; ",
            "grep -q 'pty-broker report claude' /root/.claude/hocus/hocus-hook.sh; ",
            "grep -q 'hocus codex-report' /root/.codex/hocus/hocus-hook.sh; ",
            "grep -q '\"--pty-broker\", \"report\", \"opencode\"' /root/.config/opencode/plugin/hocus.js; ",
            "grep -q hocus-hook /root/.claude/settings.json; ",
            "grep -q hocus-hook /root/.codex/hooks.json; ",
            "printf 'hooks-installed '; ",
            // The installed script itself, with nothing but the event on stdin. It swallows
            // the broker's own output and always exits 0, so what this proves is the half
            // the line above cannot: the script runs clean and puts nothing on stdout,
            // which some events would inject into the agent's context. Acceptance is the
            // direct `--pty-broker report` above.
            "out=$(printf '{\"hook_event_name\":\"Stop\",\"session_id\":\"fc-test\"}' ",
            "| /root/.claude/hocus/hocus-hook.sh); ",
            "test -z \"$out\" && printf 'script-silent '; ",
            // The account for the third mapping, made only now: the first copy landed in a
            // /home/agent that nothing owned, and the next one has a home to answer to.
            "rm -rf /home/agent; useradd -m -s /bin/sh agent; ",
            "printf 'guest-edits-done'"
        ),
        "guest-edits-done",
    );
    assert!(text.contains("cred-mode=600"), "{text}");
    // Root because root owns the directory it went into, not because root is the login:
    // `-a` alone would have carried the host's own answer across instead.
    assert!(text.contains("cred-owner=root:root"), "{text}");
    assert!(text.contains("cred-dir-mode=700"), "{text}");
    assert!(text.contains("cred-ok"), "{text}");
    // A credential nothing can reach is the same as no credential: without these two an
    // agent in the guest sits on the first-run wizard and asks to log in.
    assert!(text.contains("onboarded trusted"), "{text}");
    assert!(
        text.contains("hook-reported"),
        "the guest's hook report was not accepted by its broker: {text:?}"
    );
    assert!(
        text.contains("hooks-installed"),
        "provisioning did not install the coding-agent hooks in the guest: {text:?}"
    );
    assert!(
        text.contains("script-silent"),
        "the installed hook script did not run cleanly in the guest: {text:?}"
    );

    fs::write(&host_credential, "{\"token\":\"refreshed\"}").unwrap();
    let refreshed = provider
        .ensure(
            session,
            name,
            Some(IMAGE),
            Some(&worktree_path),
            &AtomicBool::new(false),
        )
        .unwrap();
    let text = run_in_guest(&refreshed, "cat /root/.yinz-test/auth.json", "refreshed");
    assert!(text.contains("refreshed"), "{text}");

    // The guest replaced its Claude Code state with something of its own while it was
    // running. Seeding it again puts back the two flags an agent needs and keeps what the
    // guest wrote - the account it fetched and the projects it knows live in this file, and
    // a second pane joining the VM must not cost it them.
    let text = run_in_guest(&refreshed, "cat /root/.claude.json", "sentinel");
    assert!(text.contains("guest-wrote-this"), "{text}");
    assert!(text.contains("\"hasCompletedOnboarding\":true"), "{text}");
    assert!(text.contains("\"hasTrustDialogAccepted\":true"), "{text}");

    // `agent` owns its home now, so this copy had somebody to belong to. Everything the
    // provider put there follows the home rather than the login: without that the file is
    // root's, 0600, inside a 0700 directory, and the account that has to read it cannot.
    let text = run_in_guest(
        &refreshed,
        concat!(
            "set -e; ",
            "printf 'agent-cred=%s ' \"$(stat -c %U:%G /home/agent/.claude/.credentials.json)\"; ",
            "printf 'agent-dir=%s ' \"$(stat -c %U:%G /home/agent/.claude)\"; ",
            "printf 'agent-state=%s ' \"$(stat -c %U:%G /home/agent/.claude.json)\"; ",
            "su -s /bin/sh agent -c 'cat /home/agent/.claude/.credentials.json' > /dev/null && printf 'agent-can-read '; ",
            "printf 'agent-home-checked'"
        ),
        "agent-home-checked",
    );
    assert!(text.contains("agent-cred=agent:agent"), "{text}");
    assert!(text.contains("agent-dir=agent:agent"), "{text}");
    assert!(text.contains("agent-state=agent:agent"), "{text}");
    assert!(text.contains("agent-can-read"), "{text}");

    // Something in the guest where its Claude Code state belongs that cannot be read as
    // JSON at all - here a directory, so even the read fails. Seeding says so and stops.
    // Propagating that failure is what used to terminate a booted VM and retain it as
    // `provision-failed`, over a file whose absence only costs a pane a wizard.
    run_in_guest(
        &refreshed,
        "rm -rf /root/.claude.json && mkdir /root/.claude.json && printf 'state-replaced'",
        "state-replaced",
    );
    let survived = provider
        .ensure(
            session,
            name,
            Some(IMAGE),
            Some(&worktree_path),
            &AtomicBool::new(false),
        )
        .unwrap();
    let listing = provider.list(Some(session)).unwrap();
    assert_eq!(listing.vms.len(), 1, "{listing:?}");
    assert_eq!(listing.vms[0].state, "running", "{listing:?}");
    let text = run_in_guest(
        &survived,
        concat!(
            "set -e; test -d /root/.claude.json; ",
            "test -s /root/.claude/.credentials.json; ",
            "printf 'state-left-alone'"
        ),
        "state-left-alone",
    );
    assert!(text.contains("state-left-alone"), "{text}");
    // Put back something the rest of the test can read, so the guest is left as it was.
    run_in_guest(
        &survived,
        "rm -rf /root/.claude.json && printf '{}' > /root/.claude.json && printf 'state-restored'",
        "state-restored",
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
/// stops on the last. Joining is also the only moment a VM provisioned by an older Yinz
/// meets this one, so it is where the guest's hooks are brought up to date.
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
        .ensure(
            first,
            name,
            Some(IMAGE),
            Some(&worktree_path),
            &AtomicBool::new(false),
        )
        .unwrap();

    // A VM outlives the server that made it. Take its hooks away, the way a VM that has
    // been running since before this Yinz has none, and let the join put them back:
    // nothing else installs them, and a `claude` spawned into the VM is handed no inline
    // settings to fall back on.
    let cleared = run_in_guest(
        &ensured,
        "set -e; rm -f /root/.claude/settings.json; printf 'hooks-cleared'",
        "hooks-cleared",
    );
    assert!(cleared.contains("hooks-cleared"), "{cleared}");

    // By name alone, which is `yinz action new-pane --firecracker shared`: no worktree and
    // no image, because a pane that joins a VM names neither. The provider has to find the
    // VM by its name and answer with the worktree it mirrors, which is the only way the
    // pane, the layout it serializes and the agent daemon learn which checkout it got.
    let joined = provider
        .ensure(second, name, None, None, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        joined.ssh_host, ensured.ssh_host,
        "the second session got its own VM"
    );
    assert_eq!(
        joined.worktree.as_deref(),
        Some(worktree_path.as_path()),
        "the join was not told which host checkout its guest folder came from"
    );

    // The same from the session that provisioned it: a second pane in the first session
    // names only the VM too.
    let rejoined = provider
        .ensure(first, name, None, None, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(rejoined.ssh_host, ensured.ssh_host);
    assert_eq!(rejoined.worktree.as_deref(), Some(worktree_path.as_path()));
    let restored = run_in_guest(
        &joined,
        "set -e; grep -q hocus-hook /root/.claude/settings.json; printf 'hooks-restored'",
        "hooks-restored",
    );
    assert!(
        restored.contains("hooks-restored"),
        "joining a running VM left it without the coding-agent hooks: {restored:?}"
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
        .ensure(
            first,
            "other",
            Some(IMAGE),
            Some(&worktree_path),
            &AtomicBool::new(false),
        )
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
        .open_pane_with_ssh_options(
            &ensured.ssh_host,
            &command,
            None,
            yinz_server::TerminalId::new(1),
            options,
        )
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
/// provider's `YINZ_FIRECRACKER_CONFIG` override for as long as this value lives. The
/// harness runs tests one at a time, so the process-wide variable is not contended.
struct ConfigWithCredential {
    _dir: tempfile::TempDir,
}

impl ConfigWithCredential {
    fn new(mappings: &[(&Path, &str)]) -> Self {
        let harness = harness_config_path();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(&harness).unwrap()).unwrap();
        config["credential_files"] = mappings
            .iter()
            .map(|(host, guest)| serde_json::json!({ "host": host, "guest": guest }))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firecracker.json");
        fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        std::env::set_var("YINZ_FIRECRACKER_CONFIG", &path);
        Self { _dir: dir }
    }
}

impl Drop for ConfigWithCredential {
    fn drop(&mut self) {
        std::env::remove_var("YINZ_FIRECRACKER_CONFIG");
    }
}

fn harness_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("YINZ_FIRECRACKER_CONFIG") {
        return PathBuf::from(path);
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .expect("cargo x firecracker-test sets XDG_CONFIG_HOME");
    config_home.join("yinz/firecracker.json")
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
