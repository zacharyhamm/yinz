use super::*;

#[test]
fn pending_cancellation_prevents_ssh_spawning() {
    let cancelled = AtomicBool::new(true);
    let error = bootstrap_output(
        &mut Command::new("/no/bootstrap/executable"),
        Duration::from_secs(30),
        Some(&cancelled),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Interrupted"));
    let error = HostForward::start(
        "example",
        &SshOptions::default(),
        Path::new("/no/forward/directory/socket"),
        "/remote/socket",
        Some(&cancelled),
    )
    .err()
    .expect("cancelled forward must fail before spawning");
    assert!(error.to_string().contains("Interrupted"));
}

#[test]
fn remote_shell_arguments_are_literal() {
    for value in [
        "",
        "a space",
        "a'quote",
        "$(false); echo unsafe",
        "line\nbreak",
        "--option",
    ] {
        let command = format!("printf '%s' {}", shell_argument(value).unwrap());
        let output = Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, value.as_bytes());
    }
    assert!(shell_argument("bad\0argument").is_err());
}

#[test]
fn forwarding_is_owned_bounded_and_uses_keepalives() {
    let args = ssh_forward_args(&SshOptions::default());
    for option in [
        "BatchMode=yes",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
        "ForkAfterAuthentication=no",
        "ExitOnForwardFailure=no",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
    ] {
        assert!(args.iter().any(|arg| arg == option), "{option}");
    }
    assert_eq!(
        forward_path(Path::new("/tmp/a:b\\c")).unwrap(),
        "/tmp/a\\:b\\\\c"
    );
    assert!(forward_path(Path::new("relative")).is_err());
    assert!(forward_path(Path::new("/tmp/bad\npath")).is_err());
}

#[test]
fn forwarding_paths_escape_percent_tokens() {
    for (path, expected) in [
        ("/tmp/%h", "/tmp/%%h"),
        ("/tmp/%q", "/tmp/%%q"),
        ("/tmp/trailing%", "/tmp/trailing%%"),
        ("/tmp/%%", "/tmp/%%%%"),
        ("/tmp/%%%h", "/tmp/%%%%%%h"),
        (r"/tmp/%h:a\b${HOME}", r"/tmp/%%h\:a\\b$\{HOME}"),
        (r"/tmp/\%q:${%h}%", r"/tmp/\\%%q\:$\{%%h}%%"),
    ] {
        assert_eq!(forward_path(Path::new(path)).unwrap(), expected, "{path}");
    }
}

#[test]
fn dropping_a_forward_reaps_its_child_and_removes_only_its_socket() {
    let dir = tempfile::tempdir().unwrap();
    let ours = dir.path().join("ours.sock");
    let other = dir.path().join("other.sock");
    let _ours = std::os::unix::net::UnixListener::bind(&ours).unwrap();
    let _other = std::os::unix::net::UnixListener::bind(&other).unwrap();
    let ssh = Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .spawn()
        .unwrap();
    let pid = nix::unistd::Pid::from_raw(ssh.id() as i32);
    drop(HostForward {
        host: "example".into(),
        local_socket: ours.clone(),
        ssh,
        stderr: None,
    });
    assert!(!ours.exists());
    assert!(other.exists());
    assert_eq!(
        nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
        Err(nix::errno::Errno::ECHILD)
    );
}

#[test]
fn existing_forward_socket_is_never_claimed_or_removed() {
    let dir = tempfile::Builder::new()
        .prefix("yssh-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("existing.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert!(HostForward::start(
        "example",
        &SshOptions::default(),
        &socket,
        "/remote/socket",
        None
    )
    .is_err());
    assert!(socket.exists());
    assert_eq!(
        forward_path(Path::new("/tmp/${HOME}")).unwrap(),
        "/tmp/$\\{HOME}"
    );
    for host in [
        "-option",
        "host;id",
        "$(id)",
        "user@host\n",
        "user@host`id`",
    ] {
        assert!(validate_ssh_host(host).is_err());
    }
}
