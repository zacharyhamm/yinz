use super::*;

fn fixture() -> (tempfile::TempDir, Environment) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    fs::create_dir(root.join(".yinz")).unwrap();
    fs::write(root.join(".yinz/bubblewrap.kdl"), "").unwrap();
    let env = Environment::at(root.canonicalize().unwrap(), &temp.path().join("data"));
    (temp, env)
}

#[test]
fn strict_declarations_and_substitutions() {
    let (_temp, env) = fixture();
    fs::write(env.root.join("tool"), "tool").unwrap();
    let parse_source =
        |source: &str| parse(&source.parse().unwrap(), &env.root, Path::new("/host/home"));
    let declaration = parse_source(
        r#"
        ro-bind "tool" "/opt/tool"
        path "${SANDBOX_HOME}/.cargo/bin"
        env "PROJECT" "${REPOSITORY}"
        env "HOST" "${HOST_HOME}"
        seed "tool" "${SANDBOX_HOME}/.config/tool"
        setup "sh" "-c" "echo $HOME"
    "#,
    )
    .unwrap();
    assert_eq!(declaration.mounts[0].0, env.root.join("tool"));
    assert_eq!(declaration.paths, ["/home/yinz/.cargo/bin"]);
    assert_eq!(declaration.env["HOST"], "/host/home");
    assert_eq!(declaration.env["PROJECT"], env.root.to_str().unwrap());
    assert_eq!(declaration.setup[0][2], "echo $HOME");
    for source in [
        "unknown \"x\"",
        "setup",
        "path relative=\"x\"",
        "path \"relative\"",
        "path \"/x:/y\"",
        "env \"HOME\" \"/host\"",
        "env \"CARGO_TARGET_DIR\" \"/shared\"",
        "env \"YINZ_BUBBLEWRAP\" \"0\"",
        "env \"X\" \"a\"; inherit-env \"X\"",
        "seed \"tool\" \"/etc/config\"",
        "ro-bind \"missing\" \"/opt/tool\"",
        "ro-bind \"tool\" \"/run/override\"",
        "ro-bind \"tool\" \"/usr\"",
        "ro-bind \"tool\" \"/\"",
        "ro-bind \"tool\" \"/opt/../run/override\"",
        "check \"true\" {}",
        "ro-bind \"tool\" \"/opt/tool\"; ro-bind \"tool\" \"/opt/tool/subdir\"",
    ] {
        assert!(parse_source(source).is_err(), "accepted {source}");
    }
}

#[test]
fn global_replaces_project_and_rejects_duplicate_canonical_paths() {
    let (temp, env) = fixture();
    fs::write(env.root.join(".yinz/bubblewrap.kdl"), "this is invalid").unwrap();
    let config = temp.path().join("config");
    fs::create_dir(&config).unwrap();
    let body = format!(
        "project {:?} {{ env \"SOURCE\" \"global\"; }}",
        env.root.to_str().unwrap()
    );
    fs::write(config.join("bubblewrap.kdl"), &body).unwrap();
    assert_eq!(
        selected(&env.root, Some(&config), temp.path()).unwrap().env["SOURCE"],
        "global"
    );
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&env.root, &alias).unwrap();
    fs::write(
        config.join("bubblewrap.kdl"),
        format!("{body}\nproject {:?} {{}}", alias.to_str().unwrap()),
    )
    .unwrap();
    assert!(selected(&env.root, Some(&config), temp.path())
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
}

#[test]
fn accepted_declaration_and_leases_are_independent_of_repository_edits() {
    let (_temp, env) = fixture();
    let exclusive = env.lock(true).unwrap();
    assert!(env.lock(false).is_err());
    assert!(env.lock(true).is_err());
    env.save(&Accepted {
        root: env.root.clone(),
        declaration: Declaration::default(),
        ready: false,
    })
    .unwrap();
    assert!(env.accepted().is_err());
    env.save(&Accepted {
        root: env.root.clone(),
        declaration: Declaration::default(),
        ready: true,
    })
    .unwrap();
    fs::write(env.root.join(".yinz/bubblewrap.kdl"), "bad").unwrap();
    assert!(env.accepted().unwrap().declaration.setup.is_empty());
    drop(exclusive);
    let first = env.lock(false).unwrap();
    let second = env.lock(false).unwrap();
    assert!(env.lock(true).is_err());
    drop((first, second));
    assert!(env.lock(true).is_ok());
}

#[test]
fn worktrees_share_environment_and_external_git_metadata_is_rejected() {
    let (temp, env) = fixture();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&env.root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial"
        ])
        .status()
        .unwrap()
        .success());
    let worktree = env.root.join(".worktrees/branch");
    assert!(Command::new("git")
        .arg("-C")
        .arg(&env.root)
        .args(["worktree", "add", "-qb", "branch"])
        .arg(&worktree)
        .status()
        .unwrap()
        .success());
    assert_eq!(crate::root(&worktree).unwrap(), env.root);
    env.validate_cwd(&worktree).unwrap();
    let external = temp.path().join("external");
    assert!(Command::new("git")
        .arg("-C")
        .arg(&env.root)
        .args(["worktree", "add", "-qb", "external"])
        .arg(&external)
        .status()
        .unwrap()
        .success());
    assert!(env.validate_cwd(&external).is_err());
    let nested = env.root.join("nested");
    assert!(Command::new("git")
        .arg("init")
        .arg("--separate-git-dir")
        .arg(temp.path().join("git-outside"))
        .arg(&nested)
        .status()
        .unwrap()
        .success());
    assert!(env.validate_cwd(&nested).is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces; run explicitly on Linux"]
fn real_bubblewrap_isolation_preparation_retry_and_private_caches() {
    let (temp, env) = fixture();
    fs::write(
        env.root.join(".yinz/bubblewrap.kdl"),
        r#"
        setup "sh" "-c" "mkdir -p $HOME/.cache; echo private > $HOME/.cache/provisioned; exit 7"
    "#,
    )
    .unwrap();
    assert!(env.prepare(None).is_err());
    assert!(env.accepted().is_err());
    assert!(env.home.join(".cache/provisioned").exists());
    fs::write(temp.path().join("secret"), "host secret").unwrap();
    std::os::unix::fs::symlink(temp.path().join("secret"), env.root.join("escape")).unwrap();
    fs::write(
        env.root.join(".yinz/bubblewrap.kdl"),
        r#"
        setup "sh" "-c" "test -f $HOME/.cache/provisioned; echo ready > prepared"
        check "sh" "-c" "test -f prepared"
    "#,
    )
    .unwrap();
    for source in ["{", "{\"future_format\":true}"] {
        fs::write(env.metadata.join("accepted.json"), source).unwrap();
        env.prepare(None).unwrap();
    }
    let accepted = env.accepted().unwrap();
    assert!(env.root.join("prepared").exists());
    fs::write(env.root.join(".yinz/bubblewrap.kdl"), "malformed").unwrap();
    env.preflight().unwrap();
    let script = format!(
        r#"
        set -eu
        test "$HOME" = /home/yinz
        test -z "${{CARGO_TARGET_DIR+x}}"
        test -f "$HOME/.cache/provisioned"
        test ! -e escape
        test ! -e '{}'
        test ! -e '{}'
        ! touch /usr/bin/yinz-bubblewrap-write-test
        id -un >/dev/null
        grep -q ":$(id -u):$(id -g)::/home/yinz:" /etc/passwd
        for tool in awk cc; do
            if [ -L "/usr/bin/$tool" ]; then test -e "/usr/bin/$tool"; fi
        done
        ! cat /proc/1/root'{}'
        mkdir -p nested
        git init -q nested
        git -C nested status --porcelain
        echo written > written
        test ! -e /tmp/other-agent
        touch /tmp/other-agent
    "#,
        temp.path().join("secret").display(),
        env.metadata.display(),
        temp.path().join("secret").display()
    );
    for _ in 0..2 {
        env.execute(
            &accepted.declaration,
            &env.root,
            &["sh".into(), "-c".into(), script.clone()],
            None,
        )
        .unwrap();
    }
    assert!(env.root.join("written").exists());
    let _running = env.lock(false).unwrap();
    assert!(env.prepare(None).is_err());
}

#[test]
fn spawn_env_cannot_override_the_sandbox_boundary() {
    let project = Declaration::default();
    for name in [
        "HOME",
        "PATH",
        "YINZ_BUBBLEWRAP",
        "YINZ_SOCKET_DIR",
        "CARGO_TARGET_DIR",
    ] {
        assert!(project
            .validate_spawn_env(&BTreeMap::from([(name.into(), "bad".into())]))
            .is_err());
    }
    project
        .validate_spawn_env(&BTreeMap::from([("YINZ_AGENT_PANE".into(), "1".into())]))
        .unwrap();
    for name in ["CLAUDE_CONFIG_DIR", "XDG_STATE_HOME", "DISABLE_AUTOUPDATER"] {
        let env = BTreeMap::from([(name.into(), "custom".into())]);
        project.validate_spawn_env(&env).unwrap();
        parse(
            &format!("env {name:?} \"custom\"").parse().unwrap(),
            Path::new("/project"),
            Path::new("/host"),
        )
        .unwrap();
    }
}

#[test]
fn checkout_built_binary_is_not_mounted_over_the_writable_project() {
    let binds = |binary: &str| {
        let mut command = Command::new("bwrap");
        bind_binary(&mut command, Path::new("/project"), Path::new(binary));
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let inside = "/project/target/debug/yinz";
    assert_eq!(binds(inside), ["--ro-bind", inside, "/run/yinz-bin/yinz"]);
    let outside = "/project-tools/bin/yinz";
    assert_eq!(
        binds(outside),
        [
            "--ro-bind",
            outside,
            outside,
            "--ro-bind",
            outside,
            "/run/yinz-bin/yinz"
        ]
    );
}

#[test]
fn accepted_mounts_cannot_be_redirected_to_host_secrets() {
    let (temp, env) = fixture();
    let source = env.root.join("tool");
    fs::write(&source, "tool").unwrap();
    let declaration = parse(
        &"ro-bind \"tool\" \"/opt/tool\"".parse().unwrap(),
        &env.root,
        temp.path(),
    )
    .unwrap();
    fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &source).unwrap();
    assert!(declaration.validate(&env.root).is_err());
    let declaration = Declaration {
        mounts: vec![(temp.path().to_path_buf(), "/opt/host".into())],
        ..Default::default()
    };
    let _lock = env.lock(true).unwrap();
    assert!(env.validate_storage(&declaration).is_err());
}

#[test]
fn storage_aliases_cannot_expose_metadata_through_repository_or_home() {
    let (temp, mut env) = fixture();
    let alias = temp.path().join("data-alias");
    std::os::unix::fs::symlink(&env.root, &alias).unwrap();
    env.metadata = alias.join("metadata");
    let lock = env.lock(true).unwrap();
    assert!(env.validate_storage(&Declaration::default()).is_err());
    drop(lock);
    env.metadata = temp.path().join("private-metadata");
    let _lock = env.lock(true).unwrap();
    fs::create_dir_all(env.home.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&env.metadata, &env.home).unwrap();
    assert!(env.validate_storage(&Declaration::default()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces"]
fn real_bubblewrap_pins_sources_before_startup() {
    let (temp, env) = fixture();
    env.prepare(None).unwrap();
    let source = env.root.join("tool");
    fs::write(&source, "accepted").unwrap();
    let declaration = parse(
        &"ro-bind \"tool\" \"/opt/tool\"".parse().unwrap(),
        &env.root,
        temp.path(),
    )
    .unwrap();
    let mut command = env.command(&declaration, &env.root, false).unwrap();
    fs::rename(&source, env.root.join("original-tool")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &source).unwrap();
    let output = command.args(["--", "cat", "/opt/tool"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"accepted");
    assert!(env.command(&declaration, &env.root, false).is_err());

    // Sources that cannot be opened for reading still mount: a socket, a FIFO (which
    // would block an ordinary open), and a file the owner may only execute.
    let _listener = std::os::unix::net::UnixListener::bind(env.root.join("agent.sock")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(env.root.join("pipe"))
        .status()
        .unwrap()
        .success());
    fs::write(env.root.join("exec-only"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(
        env.root.join("exec-only"),
        fs::Permissions::from_mode(0o111),
    )
    .unwrap();
    let declaration = parse(
        &r#"
        ro-bind "agent.sock" "/opt/agent.sock"
        ro-bind "pipe" "/opt/pipe"
        ro-bind "exec-only" "/opt/exec-only"
        "#
        .parse()
        .unwrap(),
        &env.root,
        temp.path(),
    )
    .unwrap();
    let output = env
        .command(&declaration, &env.root, false)
        .unwrap()
        .args([
            "--",
            "sh",
            "-c",
            "test -S /opt/agent.sock && test -p /opt/pipe && test -x /opt/exec-only",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn environment_arguments_are_private_and_preserve_values() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use std::os::unix::fs::MetadataExt;

    let (_temp, env) = fixture();
    let inherited = "inherited-secret \t\n=a=b";
    std::env::set_var("BWRAP_TEST_INHERITED", inherited);
    let declaration = Declaration {
        env: BTreeMap::from([
            ("DECLARED".into(), "declared-secret \t\n=a=b".into()),
            ("EMPTY".into(), String::new()),
            ("TERM".into(), "declared-terminal".into()),
        ]),
        inherit: vec!["BWRAP_TEST_INHERITED".into()],
        ..Default::default()
    };
    let command = env.command(&declaration, &env.root, false).unwrap();
    std::env::remove_var("BWRAP_TEST_INHERITED");
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert!(!args.contains(&"--setenv"));
    for secret in ["declared-secret", "inherited-secret", "declared-terminal"] {
        assert!(!format!("{command:?}").contains(secret));
    }
    let index = args.iter().position(|arg| *arg == "--args").unwrap();
    assert_eq!(args[index - 1], "--clearenv");
    let fd: i32 = args[index + 1].parse().unwrap();
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let path = format!("/proc/self/fd/{fd}");
    let metadata = fs::metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 0);
    let encoded = fs::read(&path).unwrap();
    assert_eq!(encoded.last(), Some(&0));
    let words: Vec<_> = encoded[..encoded.len() - 1]
        .split(|byte| *byte == 0)
        .collect();
    let (triples, remainder) = words.as_chunks::<3>();
    assert!(remainder.is_empty());
    let mut delivered = BTreeMap::new();
    for triple in triples {
        assert_eq!(triple[0], b"--setenv");
        assert!(delivered.insert(triple[1], triple[2]).is_none());
    }
    for (name, value) in &declaration.env {
        assert_eq!(delivered[name.as_bytes()], value.as_bytes());
    }
    assert_eq!(
        delivered[b"BWRAP_TEST_INHERITED".as_slice()],
        inherited.as_bytes()
    );
    assert_eq!(delivered[b"HOME".as_slice()], HOME.as_bytes());
    assert_eq!(delivered[b"YINZ_BUBBLEWRAP".as_slice()], b"1");
    drop(command);
    assert!(
        !Path::new(&path).exists(),
        "command owns the private descriptor"
    );
}

#[test]
fn environment_arguments_reject_nuls_and_missing_inherited_variables() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let (_temp, env) = fixture();
    for (name, value) in [("TOKEN", "secret\0value"), ("secret\0name", "value")] {
        let declaration = Declaration {
            env: BTreeMap::from([(name.into(), value.into())]),
            ..Default::default()
        };
        let error = env.command(&declaration, &env.root, false).unwrap_err();
        assert!(error.to_string().contains("NUL"));
        assert!(!format!("{error:#}").contains("secret"));
    }
    let declaration = Declaration {
        inherit: vec!["BWRAP_TEST_MISSING_VARIABLE".into()],
        ..Default::default()
    };
    std::env::remove_var("BWRAP_TEST_MISSING_VARIABLE");
    assert!(env
        .command(&declaration, &env.root, false)
        .unwrap_err()
        .to_string()
        .contains("missing inherited variable BWRAP_TEST_MISSING_VARIABLE"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces; run explicitly on Linux"]
fn real_bubblewrap_delivers_environment_without_secrets_in_argv() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use std::io::Read;
    use std::time::Duration;

    let (_temp, env) = fixture();
    env.prepare(None).unwrap();
    let declared = "declared-sentinel-secret \t\n=a=b";
    let inherited = "inherited-sentinel-secret \t\n=c=d";
    std::env::set_var("BWRAP_TEST_LIVE_INHERITED", inherited);
    let declaration = Declaration {
        env: BTreeMap::from([
            ("DECLARED".into(), declared.into()),
            ("EMPTY".into(), String::new()),
        ]),
        inherit: vec!["BWRAP_TEST_LIVE_INHERITED".into()],
        ..Default::default()
    };
    let mut command = env.command(&declaration, &env.root, false).unwrap();
    std::env::remove_var("BWRAP_TEST_LIVE_INHERITED");
    command.env("BWRAP_TEST_UNDECLARED", "host-only-secret");
    command.args([
        "--", "sh", "-eu", "-c",
        r#"test -z "${BWRAP_TEST_UNDECLARED+x}"; test "${EMPTY+x}" = x; printf '%s\0' "$DECLARED" "$BWRAP_TEST_LIVE_INHERITED" "$EMPTY"; read -r reply; test "$reply" = continue"#,
    ]);
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    // Reuse the builder to check that each child rewinds the descriptor before Bubblewrap reads it.
    for _ in 0..2 {
        let mut child = command.spawn().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let expected = format!("{declared}\0{inherited}\0\0").into_bytes();
        let length = expected.len();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut bytes = vec![0; length];
            sender
                .send(stdout.read_exact(&mut bytes).map(|()| bytes))
                .unwrap();
        });
        // Receiving the environment proves the sandbox is ready and waiting for stdin.
        let delivered = receiver.recv_timeout(Duration::from_secs(30));
        let argv = fs::read(format!("/proc/{}/cmdline", child.id()));
        let released = child.stdin.take().unwrap().write_all(b"continue\n");
        if delivered.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().unwrap();
        reader.join().unwrap();
        assert_eq!(delivered.unwrap().unwrap(), expected);
        released.unwrap();
        assert!(status.success());
        let argv = String::from_utf8(argv.unwrap()).unwrap();
        assert!(
            argv.contains("--args"),
            "expected a live Bubblewrap process: {argv:?}"
        );
        for secret in [declared, inherited, "host-only-secret"] {
            assert!(!argv.contains(secret));
        }
        assert!(!argv.contains("--setenv"));
    }
}

#[test]
fn missing_global_projects_preserve_overrides_and_local_fallback() {
    let (temp, env) = fixture();
    let config = temp.path().join("config");
    fs::create_dir(&config).unwrap();
    let missing = format!("project {:?} {{}}", temp.path().join("missing"));
    let matching = format!("project {:?} {{ env \"SOURCE\" \"global\"; }}", env.root);
    for source in [
        format!("{missing}\n{matching}"),
        format!("{matching}\n{missing}"),
    ] {
        fs::write(config.join("bubblewrap.kdl"), source).unwrap();
        assert_eq!(
            selected(&env.root, Some(&config), temp.path()).unwrap().env["SOURCE"],
            "global"
        );
    }
    fs::write(config.join("bubblewrap.kdl"), &missing).unwrap();
    assert!(selected(&env.root, Some(&config), temp.path())
        .unwrap()
        .env
        .is_empty());
    for source in [
        format!("project {:?}", temp.path().join("missing")),
        "project \"relative\" {}".into(),
        "unknown \"/missing\" {}".into(),
    ] {
        fs::write(config.join("bubblewrap.kdl"), source).unwrap();
        assert!(selected(&env.root, Some(&config), temp.path()).is_err());
    }
    let link = temp.path().join("loop");
    std::os::unix::fs::symlink(&link, &link).unwrap();
    fs::write(
        config.join("bubblewrap.kdl"),
        format!("project {link:?} {{}}"),
    )
    .unwrap();
    assert!(selected(&env.root, Some(&config), temp.path()).is_err());
}

#[test]
fn corrupt_acceptance_is_invalidated_even_when_preparation_fails() {
    let (_temp, env) = fixture();
    let lock = env.lock(true).unwrap();
    drop(lock);
    fs::write(env.root.join(".yinz/bubblewrap.kdl"), "invalid").unwrap();
    for source in ["{", "{\"future_format\":true}"] {
        fs::write(env.metadata.join("accepted.json"), source).unwrap();
        assert!(env.prepare(None).is_err());
        let accepted: Accepted =
            serde_json::from_slice(&fs::read(env.metadata.join("accepted.json")).unwrap()).unwrap();
        assert!(!accepted.ready);
        assert_eq!(accepted.root, env.root);
        assert!(env.accepted().is_err());
    }
    fs::remove_file(env.metadata.join("accepted.json")).unwrap();
    fs::create_dir(env.metadata.join("accepted.json")).unwrap();
    assert!(env.prepare(None).is_err());
    assert!(!env.home.exists());
}

#[test]
fn minimum_bubblewrap_version() {
    use std::os::unix::process::ExitStatusExt;
    for text in [
        "bubblewrap 0.10.0",
        "bubblewrap 0.10.1-1ubuntu2",
        "bubblewrap 1.0.0",
    ] {
        validate_version(&std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: text.as_bytes().to_vec(),
            stderr: vec![],
        })
        .unwrap();
    }
    for text in [
        "bubblewrap 0.8.0",
        "bubblewrap 0.9.9",
        "",
        "0.10.0",
        "bubblewrap x",
        "bubblewrap 0.10",
        "bubblewrap 0.10.0.1",
    ] {
        assert!(
            validate_version(&std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: text.as_bytes().to_vec(),
                stderr: vec![],
            })
            .is_err(),
            "{text}"
        );
    }
    assert!(validate_version(&std::process::Output {
        status: std::process::ExitStatus::from_raw(256),
        stdout: b"bubblewrap 0.10.0".to_vec(),
        stderr: vec![],
    })
    .is_err());
}

// Only the descendant owns the FIFO writer: EOF proves it died, not just its parent.
fn blocked_descendant(directory: &Path) -> (Vec<String>, std::sync::mpsc::Receiver<&'static str>) {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;

    let ready = directory.join("descendant-ready");
    let gate = directory.join("descendant-gate");
    for path in [&ready, &gate] {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader_path = ready.clone();
    std::thread::spawn(move || {
        let mut reader = File::open(reader_path).unwrap();
        let mut message = [0; 5];
        reader.read_exact(&mut message).unwrap();
        assert_eq!(&message, b"ready");
        let _ = sender.send("ready");
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
        let _ = sender.send("closed");
    });
    (
        vec![
            "sh".into(),
            "-c".into(),
            r#"sh -c 'exec 3>"$1"; printf ready >&3; read -r reply <"$2"' sh "$1" "$2" & wait"#
                .into(),
            "sh".into(),
            ready.to_str().unwrap().into(),
            gate.to_str().unwrap().into(),
        ],
        receiver,
    )
}

fn assert_descendant_killed(events: std::sync::mpsc::Receiver<&'static str>, directory: &Path) {
    assert_eq!(
        events.recv_timeout(Duration::from_secs(30)).unwrap(),
        "ready"
    );
    let closed = events.recv_timeout(Duration::from_secs(30));
    if closed.is_err() {
        // Release a surviving descendant before reporting a broken process-group kill.
        let _ = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(directory.join("descendant-gate"))
            .and_then(|mut gate| gate.write_all(b"release\n"));
    }
    assert_eq!(closed.unwrap(), "closed");
}

#[test]
fn check_wait_handles_success_failure_and_kills_descendants() {
    let mut success = Command::new("/usr/bin/true");
    success.process_group(0);
    wait_check(&mut success, Instant::now() + Duration::from_secs(2)).unwrap();
    let mut failure = Command::new("sh");
    failure.args(["-c", "exit 7"]).process_group(0);
    assert!(
        wait_check(&mut failure, Instant::now() + Duration::from_secs(2))
            .unwrap_err()
            .to_string()
            .contains("7")
    );
    let temp = tempfile::tempdir().unwrap();
    let (args, events) = blocked_descendant(temp.path());
    let mut hanging = Command::new(&args[0]);
    hanging.args(&args[1..]).process_group(0);
    assert!(
        wait_check(&mut hanging, Instant::now() + Duration::from_secs(10))
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert_descendant_killed(events, temp.path());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces"]
fn real_bubblewrap_checks_run_once_capture_output_and_share_deadline() {
    if let Ok(root) = std::env::var("BWRAP_RUN_TEST_ROOT") {
        let env = Environment::at(
            PathBuf::from(root),
            Path::new(&std::env::var("BWRAP_RUN_TEST_DATA").unwrap()),
        );
        // The server's half of a launch, then the pane's.
        let result = env
            .check_launch(&std::env::current_dir().unwrap(), Path::new("sh"))
            .and_then(|_| env.run(&["sh".into(), "-c".into(), "touch agent-ran".into()]));
        let error = result.unwrap_err();
        eprintln!("{error:#}");
        assert!(std::env::var_os("BWRAP_RUN_EXPECT_FAILURE").is_some());
        assert!(format!("{error:#}").contains("check exited"));
        return;
    }
    let (_temp, env) = fixture();
    fs::write(
        env.root.join(".yinz/bubblewrap.kdl"),
        r#"
        check "sh" "-c" "echo checked >> checks; echo private-output; echo private-error >&2"
    "#,
    )
    .unwrap();
    env.prepare(None).unwrap();
    env.preflight().unwrap();
    assert_eq!(
        fs::read_to_string(env.root.join("checks")).unwrap(),
        "checked\n"
    );
    let launch = || {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact", "bubblewrap::tests::real_bubblewrap_checks_run_once_capture_output_and_share_deadline",
            "--ignored", "--nocapture",
        ]).env("BWRAP_RUN_TEST_ROOT", &env.root)
          .env("BWRAP_RUN_TEST_DATA", env.metadata.parent().unwrap().parent().unwrap())
          .current_dir(&env.root);
        command
    };
    for _ in 0..2 {
        let output = launch().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(env.root.join("agent-ran").exists());
        fs::remove_file(env.root.join("agent-ran")).unwrap();
    }
    let log = OpenOptions::new()
        .append(true)
        .open(env.metadata.join("launch.log"))
        .unwrap();
    assert_eq!(
        fs::metadata(env.metadata.join("launch.log"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_to_string(env.root.join("checks")).unwrap(),
        "checked\nchecked\nchecked\n"
    );
    let output = fs::read_to_string(env.metadata.join("launch.log")).unwrap();
    assert_eq!(output.matches("private-output").count(), 2);
    assert_eq!(output.matches("private-error").count(), 2);
    // A program the sandbox cannot resolve is told apart from a failed check.
    let missing = env
        .check_launch(&env.root, Path::new("yinz-no-such-program"))
        .unwrap_err();
    assert!(
        missing.downcast_ref::<ProgramNotFound>().is_some(),
        "{missing:#}"
    );
    env.save(&Accepted {
        root: env.root.clone(),
        declaration: Declaration {
            checks: vec![vec![
                "sh".into(),
                "-c".into(),
                "echo hidden-secret; exit 7".into(),
            ]],
            ..Default::default()
        },
        ready: true,
    })
    .unwrap();
    let output = launch()
        .env("BWRAP_RUN_EXPECT_FAILURE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("launch.log"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("hidden-secret"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("hidden-secret"));
    assert!(!env.root.join("agent-ran").exists());
    assert!(env.accepted().is_ok());
    assert!(fs::read_to_string(env.metadata.join("launch.log"))
        .unwrap()
        .contains("hidden-secret"));
    let shared = Declaration {
        checks: vec![
            vec!["sleep".into(), "0.15".into()],
            vec!["sleep".into(), "0.15".into()],
        ],
        ..Default::default()
    };
    assert!(format!(
        "{:#}",
        env.checks(
            &shared,
            &env.root,
            &log,
            Instant::now() + Duration::from_millis(250)
        )
        .unwrap_err()
    )
    .contains("timed out"));
    let (args, events) = blocked_descendant(&env.root);
    let declaration = Declaration {
        checks: vec![args],
        ..Default::default()
    };
    let lease = env.lock(false).unwrap();
    let error = env
        .checks(
            &declaration,
            &env.root,
            &log,
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    drop(lease);
    assert!(env.lock(true).is_ok());
    assert_descendant_killed(events, &env.root);
}

#[cfg(target_os = "linux")]
#[test]
fn unsupported_version_stops_empty_declarations_before_provisioning() {
    const TEST: &str =
        "bubblewrap::tests::unsupported_version_stops_empty_declarations_before_provisioning";
    if let Ok(root) = std::env::var("BWRAP_VERSION_TEST_ROOT") {
        let env = Environment::at(
            PathBuf::from(root),
            Path::new(&std::env::var("BWRAP_VERSION_TEST_DATA").unwrap()),
        );
        assert!(env.preflight().is_err());
        assert!(env.prepare(None).is_err());
        assert!(!env.home.exists());
        return;
    }
    let (temp, env) = fixture();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink("/usr/bin/false", bin.join("bwrap")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("BWRAP_VERSION_TEST_ROOT", &env.root)
        .env("BWRAP_VERSION_TEST_DATA", temp.path().join("data"))
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(bin.join("bwrap")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("BWRAP_VERSION_TEST_ROOT", &env.root)
        .env("BWRAP_VERSION_TEST_DATA", temp.path().join("data"))
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
