#![cfg(target_os = "linux")]

//! A real provider, Firecracker process, SSH connection and remote pane.
//! Run only through `cargo x firecracker-test --test`.

use std::{path::PathBuf, sync::Arc, thread::JoinHandle, time::Duration};

use zellij_server::{
    firecracker::Provider,
    os_input_output::AsyncReader,
    pty_host::remote::{
        ssh::{RemoteHosts, SshOptions},
        RemoteConn, RemoteReader,
    },
};
use zellij_test_support::Watched;
use zellij_utils::input::command::RunCommand;

const TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Default)]
struct Output {
    bytes: Vec<u8>,
    ended: bool,
    error: Option<String>,
}

struct Pane {
    conn: Arc<RemoteConn>,
    output: Watched<Output>,
    pump: Option<JoinHandle<()>>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let _ = self.conn.terminate(Duration::from_secs(5));
    }
}

struct VmGuard {
    provider: Provider,
    session: &'static str,
    name: &'static str,
}

impl Drop for VmGuard {
    fn drop(&mut self) {
        let _ = self.provider.stop(self.session, self.name);
    }
}

#[test]
#[ignore = "requires /dev/kvm and a built Firecracker test image"]
fn a_provider_boots_a_real_firecracker_remote_pane() {
    assert!(PathBuf::from("/dev/kvm").exists(), "/dev/kvm is missing");
    let provider_path = repo_root().join("target/debug/zellij-firecracker");
    let provider = Provider::new(provider_path, Duration::from_secs(120)).unwrap();
    let session = "firecracker-test";
    let name = "pane";
    let project = repo_root();
    let ensured = provider
        .ensure(session, name, Some("zellij-dev"), Some(&project))
        .unwrap();
    let _vm = VmGuard {
        provider: provider.clone(),
        session,
        name,
    };
    assert_eq!(provider.list(Some(session)).unwrap().len(), 1);

    let options = SshOptions {
        identity_file: ensured.ssh_identity_file,
        known_hosts_file: ensured.ssh_known_hosts_file,
    };
    let command = RunCommand {
        command: PathBuf::from("/bin/sh"),
        args: vec![
            "-lc".to_owned(),
            "set -e; test -f Cargo.toml; rustc --version >/dev/null; printf 'rustc-ok '; cargo x --help >/dev/null; printf 'cargo-x-ok '; codex --version >/dev/null; printf 'codex-ok '; claude --version >/dev/null; printf 'claude-ok firecracker-pane-ok '; zellij --version".to_owned(),
        ],
        cwd: ensured.cwd,
        remote_host: Some(ensured.ssh_host.clone()),
        ..Default::default()
    };
    let hosts = Arc::new(RemoteHosts::new());
    let (conn, reader_socket) = hosts
        .open_pane_with_ssh_options(&ensured.ssh_host, &command, None, 1, options)
        .unwrap();
    let reader = RemoteReader::new(conn.clone(), reader_socket).unwrap();
    let pane = pump(conn, reader);
    pane.output.wait_until_described(
        "the Firecracker pane to print Zellij's version and exit",
        TIMEOUT,
        |output| {
            let text = String::from_utf8_lossy(&output.bytes);
            (output.ended
                && text
                    .contains("rustc-ok cargo-x-ok codex-ok claude-ok firecracker-pane-ok zellij"))
            .then(|| text.into_owned())
        },
        |output| format!("{output:?}"),
    );
    drop(pane);
    provider.stop(session, name).unwrap();
    assert!(provider.list(Some(session)).unwrap().is_empty());
}

fn pump(conn: Arc<RemoteConn>, mut reader: RemoteReader) -> Pane {
    let output: Watched<Output> = Watched::default();
    let pump_output = output.clone();
    let pump = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut buffer = [0; 8192];
            loop {
                match reader.read(&mut buffer).await {
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
                }
            }
        });
    });
    Pane {
        conn,
        output,
        pump: Some(pump),
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
