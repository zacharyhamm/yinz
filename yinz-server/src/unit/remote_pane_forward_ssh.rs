use super::*;
use std::net::{SocketAddr, TcpListener};

#[test]
#[ignore]
fn configured_local_forward_allows_pane_creation_and_reconnection() {
    if yinz_test_support::reexec_test() {
        return;
    }
    test_env();
    let root = tempfile::Builder::new()
        .prefix("yf-")
        .tempdir_in("/tmp")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let configured_address = listener.local_addr().unwrap();
    let config_path = root.path().join("ssh_config");
    let fixture_config = remote_test_dir().join("ssh_config");
    let config = std::fs::read_to_string(&fixture_config).unwrap();
    std::fs::write(
        &config_path,
        format!("{config}\nHost {HOST}\n    LocalForward {configured_address} 127.0.0.1:2222\n"),
    )
    .unwrap();
    let shim = std::fs::read_to_string(remote_test_dir().join("bin/ssh")).unwrap();
    yinz_test_support::write_warm_script(
        &root.path().join("ssh"),
        &shim.replace(
            fixture_config.to_str().unwrap(),
            &shell_quote(config_path.to_str().unwrap()),
        ),
    );
    let mut path = vec![root.path().to_path_buf()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    // Only this isolated test process reads the overridden environment.
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(path).unwrap());
        std::env::set_var("YINZ_SOCKET_DIR", root.path().join("sockets"));
    }
    proxy_set_enabled(true);
    drop(listener);
    ssh::bootstrap(HOST).expect("bootstrap must own the configured listener first");
    assert!(TcpStream::connect(configured_address).is_ok());

    assert_pane_forward_lifecycle(Some(configured_address));
}

#[test]
#[ignore]
fn relative_socket_directory_allows_pane_creation_and_reconnection() {
    if yinz_test_support::reexec_test() {
        return;
    }
    test_env();
    let root = tempfile::Builder::new()
        .prefix("yf-")
        .tempdir_in("/tmp")
        .unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    // Set before YINZ_SOCK_DIR initializes, in this isolated test process.
    unsafe { std::env::set_var("YINZ_SOCKET_DIR", "./sockets") };

    assert_pane_forward_lifecycle(None);
}

fn assert_pane_forward_lifecycle(configured_address: Option<SocketAddr>) {
    let hosts = start();
    let marker = marker("forward");
    let pane = open_pane(
        &hosts,
        &format!(
            "stty -echo; echo {marker}-ready; while read line; do echo {marker}-got:$line; done"
        ),
    );
    pane.wait_for_output(&format!("{marker}-ready"));
    let pid = remote_pids(&marker);
    assert_eq!(pid.len(), 1);
    let socket = hosts.forward_socket_path(HOST);
    assert!(UnixStream::connect(&socket).is_ok());
    let env = TestEnv {
        sockets: PathBuf::from(std::env::var_os("YINZ_SOCKET_DIR").unwrap())
            .canonicalize()
            .unwrap(),
    };
    assert_eq!(local_forward_count(&env), 1);

    with_link_down(|| {
        wait_for_remote("the private SSH tunnel to exit", || {
            local_forward_count(&env) == 0
        });
    });
    pane.wait_for_output_within("── reconnected ──", Duration::from_secs(30));
    pane.send_line("after-reconnect");
    pane.wait_for_output(&format!("{marker}-got:after-reconnect"));
    assert_eq!(remote_pids(&marker), pid);
    assert_eq!(local_forward_count(&env), 1);

    pane.close();
    wait_for_remote("the closed pane to exit", || {
        remote_pids(&marker).is_empty()
    });
    hosts.shutdown();
    assert!(!socket.exists(), "the private forwarding socket leaked");
    assert_eq!(
        local_forward_count(&env),
        0,
        "the private SSH tunnel leaked"
    );
    if let Some(configured_address) = configured_address {
        assert!(
            TcpStream::connect(configured_address).is_ok(),
            "closing the private tunnel must leave the bootstrap master's listener intact"
        );
    }

    for entry in std::fs::read_dir(socket.parent().unwrap()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "ctl") {
            let output = Command::new("ssh")
                .arg("-S")
                .arg(&path)
                .args(["-O", "exit", HOST])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "failed to close the bootstrap master: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
