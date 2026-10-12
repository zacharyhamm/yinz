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
    let mother = Declaration {
        mother: Some(vec!["/host/agent".into()]),
        ..Default::default()
    };
    let claude = Declaration {
        claude: Some(ClaudeState {
            root: "/host/.claude".into(),
            global: "/host/.claude.json".into(),
            home: "/host".into(),
        }),
        ..Default::default()
    };
    for name in ["CLAUDE_CONFIG_DIR", "XDG_STATE_HOME", "DISABLE_AUTOUPDATER"] {
        let env = BTreeMap::from([(name.into(), "custom".into())]);
        project.validate_spawn_env(&env).unwrap();
        mother.validate_spawn_env(&env).unwrap();
        assert!(claude.validate_spawn_env(&env).is_err());
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

#[test]
fn mother_directories_are_private_persistent_and_separate() {
    let cache = tempfile::tempdir().unwrap();
    let first = mother_directory(cache.path(), "../session/one").unwrap();
    let second = mother_directory(cache.path(), "../session/two").unwrap();
    assert_ne!(first, second);
    assert!(first.starts_with(cache.path().canonicalize().unwrap().join("mother")));
    fs::write(first.join("kept"), "conversation notes").unwrap();
    assert_eq!(
        mother_directory(cache.path(), "../session/one").unwrap(),
        first
    );
    assert_eq!(
        fs::read_to_string(first.join("kept")).unwrap(),
        "conversation notes"
    );
    assert_eq!(
        fs::metadata(&first).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(mother_directory(cache.path(), "").is_err());
    let blocked = cache.path().join("file");
    fs::write(&blocked, "not a directory").unwrap();
    assert!(mother_directory(&blocked, "session").is_err());
    fs::remove_dir(&second).unwrap();
    std::os::unix::fs::symlink(&first, &second).unwrap();
    assert!(mother_directory(cache.path(), "../session/two").is_err());
}

#[test]
fn mother_state_overrides_cannot_grant_home_or_metadata_writes() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let (temp, env) = fixture();
    std::env::set_var("HOME", temp.path());
    std::env::set_var("CODEX_HOME", temp.path().join("codex-state"));
    assert_eq!(
        mother_state("codex").unwrap(),
        [temp.path().join("codex-state")]
    );
    std::env::set_var("PI_CODING_AGENT_DIR", "~/pi-state");
    assert_eq!(mother_state("pi").unwrap(), [temp.path().join("pi-state")]);
    for unsafe_path in [temp.path(), Path::new("/"), &env.root, &env.metadata] {
        assert!(
            env.validate_mother_state(unsafe_path).is_err(),
            "{unsafe_path:?}"
        );
    }
    assert!(env
        .validate_mother_state(&temp.path().join("codex-state"))
        .is_ok());
    assert!(mother_state("plugin-provider").is_err());
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::set_var("XDG_CACHE_HOME", temp.path().join("cache"));
    assert_eq!(
        ClaudeState::resolve().unwrap().root,
        temp.path().join(".claude")
    );
    std::env::set_var("CLAUDE_CONFIG_DIR", temp.path().join("claude-state"));
    assert_eq!(
        ClaudeState::resolve().unwrap().root,
        temp.path().join("claude-state")
    );
    // The override cannot make the whole home writable.
    std::env::set_var("XDG_DATA_HOME", temp.path().join("data"));
    std::env::set_var("CLAUDE_CONFIG_DIR", temp.path());
    assert!(env.accept_mother("claude").is_err());
}

#[test]
fn unavailable_runtime_does_not_hide_invalid_claude_configuration() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("PATH", home.join("no-bwrap"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "owner").unwrap());
    let profile = Environment::at_root(&work.root.with_file_name("profile-claude"));
    let assert_failure = || {
        let error = work.prepare_mother("claude").err().unwrap();
        assert!(error.is::<MotherSetupError>(), "{error:#}");
        assert!(!profile.metadata.join("accepted.json").exists());
    };
    assert_failure();
    std::env::set_var("CLAUDE_CONFIG_DIR", "relative-config");
    assert_failure();
}

#[cfg(target_os = "linux")]
#[test]
fn supported_mother_keeps_claude_setup_errors_fatal() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    yinz_test_support::write_warm_script(&bin.join("bwrap"), "echo 'bubblewrap 0.10.0'");
    let unavailable = bin.join("unavailable");
    yinz_test_support::write_warm_script(&unavailable, "exit 1");
    std::env::set_var("PATH", &bin);
    fs::create_dir(home.join(".claude")).unwrap();
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "owner").unwrap());
    for contents in [None, Some("invalid")] {
        if let Some(contents) = contents {
            fs::write(home.join(".claude.json"), contents).unwrap();
        }
        let error = work.prepare_mother("claude").err().unwrap();
        assert!(error.is::<MotherSetupError>(), "{error:#}");
    }
    fs::rename(unavailable, bin.join("bwrap")).unwrap();
    assert!(work
        .prepare_mother("claude")
        .err()
        .unwrap()
        .is::<MotherSetupError>());
    fs::write(home.join(".claude.json"), "{}").unwrap();
    let profile = work.prepare_mother("claude").unwrap();
    assert!(matches!(
        profile.check_launch(&work.root, Path::new("sh")).unwrap(),
        LaunchCheck::MotherFallback(_)
    ));
}

#[test]
fn claude_mother_trust_preserves_settings_at_each_effective_location() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    let work = mother_directory(&home.join("space \"quote\".dot"), "owner").unwrap();
    for (override_dir, legacy) in [(false, false), (true, false), (false, true), (true, true)] {
        let root = home.join(if override_dir { "custom" } else { ".claude" });
        fs::create_dir_all(&root).unwrap();
        if override_dir {
            std::env::set_var("CLAUDE_CONFIG_DIR", &root);
        } else {
            std::env::remove_var("CLAUDE_CONFIG_DIR");
        }
        let global = if override_dir {
            root.join(".claude.json")
        } else {
            home.join(".claude.json")
        };
        let path = if legacy {
            root.join(".config.json")
        } else {
            global.clone()
        };
        let before = serde_json::json!({
            "oauthAccount": {"accountUuid": "keep"}, "mcpServers": {"keep": {}},
            "projects": {"/other": {"hasTrustDialogAccepted": false}, work.to_str().unwrap(): {"allowedTools": ["keep"]}}
        });
        fs::write(&global, "{}").unwrap();
        fs::write(&path, serde_json::to_vec(&before).unwrap()).unwrap();
        let state = ClaudeState::resolve().unwrap();
        state.trust_mother(&work).unwrap();
        let mut expected = before;
        expected["projects"][work.to_str().unwrap()]["hasTrustDialogAccepted"] = true.into();
        let contents = fs::read(&path).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&contents).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&state.settings().unwrap().0).unwrap(),
            expected
        );
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        state.trust_mother(&work).unwrap();
        assert_eq!(fs::read(&path).unwrap(), contents);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        if legacy {
            assert_eq!(fs::read_to_string(&global).unwrap(), "{}");
        }
    }
}

#[test]
fn claude_mother_trust_fails_without_changing_bad_or_unwritable_settings() {
    let temp = tempfile::tempdir().unwrap();
    let state = ClaudeState {
        root: temp.path().into(),
        global: temp.path().join(".claude.json"),
        home: temp.path().into(),
    };
    let lock = state.global.with_extension("json.lock");
    for contents in [
        "invalid",
        "[]",
        r#"{"projects":null}"#,
        r#"{"projects":[]}"#,
    ] {
        fs::write(&state.global, contents).unwrap();
        assert!(state.trust_mother(temp.path()).is_err());
        assert_eq!(fs::read_to_string(&state.global).unwrap(), contents);
        assert!(!lock.exists());
    }
    let invalid_project = serde_json::json!({"projects": {temp.path().canonicalize().unwrap().to_str().unwrap(): false}});
    fs::write(&state.global, invalid_project.to_string()).unwrap();
    assert!(state.trust_mother(temp.path()).is_err());
    assert_eq!(
        fs::read_to_string(&state.global).unwrap(),
        invalid_project.to_string()
    );

    let destination = temp.path().join("read-only");
    fs::create_dir(&destination).unwrap();
    let settings = destination.join("settings.json");
    fs::write(&settings, "{}").unwrap();
    fs::remove_file(&state.global).unwrap();
    std::os::unix::fs::symlink(&settings, &state.global).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o500)).unwrap();
    let result = state.trust_mother(temp.path());
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(format!("{:#}", result.unwrap_err()).contains("create Claude trust settings"));
    assert_eq!(fs::read_to_string(&settings).unwrap(), "{}");
    assert!(!lock.exists());
    state.trust_mother(temp.path()).unwrap();
    assert!(state.global.is_symlink());
}

#[test]
fn claude_config_lock_recovers_expired_locks() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.lock");
    fs::create_dir(&path).unwrap();
    File::open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(20))
        .unwrap();

    let lock = ClaudeConfigLock::acquire(path.clone(), Duration::ZERO).unwrap();
    assert!(path.is_dir());
    assert!(ClaudeConfigLock::acquire(path.clone(), Duration::ZERO).is_err());
    drop(lock);
    assert!(!path.exists());
}

#[test]
fn claude_config_lock_preserves_fresh_and_future_locks() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.lock");
    fs::create_dir(&path).unwrap();
    for offset in [Duration::ZERO, Duration::from_secs(60)] {
        File::open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + offset)
            .unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(ClaudeConfigLock::acquire(path.clone(), Duration::from_millis(30)).is_err());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn claude_config_lock_preserves_nonempty_expired_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.lock");
    fs::create_dir(&path).unwrap();
    let contents = path.join("keep");
    fs::write(&contents, "preserve").unwrap();
    File::open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(20))
        .unwrap();

    assert!(ClaudeConfigLock::acquire(path, Duration::ZERO).is_err());
    assert_eq!(fs::read_to_string(contents).unwrap(), "preserve");
}

#[test]
fn claude_mother_trust_recovers_expired_config_lock() {
    let temp = tempfile::tempdir().unwrap();
    let state = ClaudeState {
        root: temp.path().into(),
        global: temp.path().join(".claude.json"),
        home: temp.path().into(),
    };
    fs::write(&state.global, r#"{"unrelated":"keep"}"#).unwrap();
    let path = state.global.with_extension("json.lock");
    fs::create_dir(&path).unwrap();
    File::open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(20))
        .unwrap();

    state.trust_mother(temp.path()).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&state.settings().unwrap().0).unwrap();
    assert_eq!(settings["unrelated"], "keep");
    let work = temp.path().canonicalize().unwrap();
    assert_eq!(
        settings["projects"][work.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
    assert!(!path.exists());
}

#[test]
fn claude_config_lock_bounds_contention_and_trust_reads_after_release() {
    let temp = tempfile::tempdir().unwrap();
    let state = ClaudeState {
        root: temp.path().into(),
        global: temp.path().join(".claude.json"),
        home: temp.path().into(),
    };
    fs::write(&state.global, "{}").unwrap();
    let path = state.global.with_extension("json.lock");
    let lock = ClaudeConfigLock::acquire(path.clone(), Duration::ZERO).unwrap();
    assert!(ClaudeConfigLock::acquire(path.clone(), Duration::from_millis(30)).is_err());
    std::thread::scope(|scope| {
        let pending = scope.spawn(|| state.trust_mother(temp.path()));
        fs::write(&state.global, r#"{"concurrent":"keep"}"#).unwrap();
        drop(lock);
        pending.join().unwrap().unwrap();
    });
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&state.settings().unwrap().0).unwrap()
            ["concurrent"],
        "keep"
    );
    assert!(!path.exists());
}

#[test]
fn claude_mother_accepts_safe_host_aliases_and_relative_config() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let alias = home.join("home-alias");
    std::os::unix::fs::symlink(&home, &alias).unwrap();
    std::env::set_var("HOME", &alias);
    std::env::set_var("XDG_DATA_HOME", alias.join("data"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    fs::create_dir(home.join(".claude")).unwrap();
    fs::create_dir(home.join("transcripts")).unwrap();
    fs::write(home.join("settings.json"), "{}").unwrap();
    for name in [
        ".claude.json",
        ".claude/.config.json",
        ".claude/.claude.json",
    ] {
        std::os::unix::fs::symlink(home.join("settings.json"), home.join(name)).unwrap();
    }
    std::os::unix::fs::symlink(home.join("transcripts"), home.join(".claude/projects")).unwrap();
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "owner").unwrap());
    let profile = work.accept_mother("claude").unwrap();
    assert!(profile.home.starts_with(home.join("data")));
    assert_eq!(work.accept_mother("claude").unwrap().root(), profile.root());
    let reloaded = Environment::at_root(profile.root());
    let accepted = reloaded.accepted().unwrap();
    reloaded
        .validate_launch_cwd(&work.root, &accepted.declaration)
        .unwrap();
    for name in ["CLAUDE_CONFIG_DIR", "XDG_STATE_HOME", "DISABLE_AUTOUPDATER"] {
        assert!(reloaded
            .validate_spawn_env(&BTreeMap::from([(name.into(), "custom".into())]))
            .is_err());
    }
    std::env::set_current_dir(&home).unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", "data/../.claude");
    let relative = work.accept_mother("claude").unwrap();
    assert_eq!(
        relative
            .accepted()
            .unwrap()
            .declaration
            .claude
            .unwrap()
            .root,
        home.join(".claude")
    );
    for name in [".config.json", ".claude.json", "projects"] {
        let link = home.join(".claude").join(name);
        fs::remove_file(&link).unwrap();
        for unsafe_path in [&work.root, &profile.home, &profile.metadata] {
            std::os::unix::fs::symlink(unsafe_path, &link).unwrap();
            assert!(
                work.accept_mother("claude").is_err(),
                "{link:?} -> {unsafe_path:?}"
            );
            fs::remove_file(&link).unwrap();
        }
        let target = if name == "projects" {
            "transcripts"
        } else {
            "settings.json"
        };
        std::os::unix::fs::symlink(home.join(target), &link).unwrap();
    }
    let private = profile.home.join("projects");
    fs::remove_dir(&private).unwrap();
    std::os::unix::fs::symlink(home.join("transcripts"), &private).unwrap();
    assert!(profile.accepted().is_err());
}

#[test]
fn claude_mother_configuration_is_host_authoritative_and_never_seeded() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("XDG_CACHE_HOME", home.join("xdg-cache"));
    std::env::set_var("XDG_STATE_HOME", home.join("xdg-state"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    fs::create_dir(home.join(".claude")).unwrap();
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "owner").unwrap());
    let profile = work.accept_mother("claude").unwrap();
    let error = profile
        .check_launch(&work.root, Path::new("sh"))
        .unwrap_err();
    assert!(error.is::<MotherSetupError>());
    assert!(format!("{error:#}").contains("complete setup first"));
    assert!(!home.join(".claude/.claude.json").exists());
    let global = home.join(".claude.json");
    fs::write(
        &global,
        r#"{"oauthAccount":"first","mcpServers":{"host":{}}}"#,
    )
    .unwrap();
    let profile = work.accept_mother("claude").unwrap();
    let accepted = profile.accepted().unwrap();
    assert_eq!(
        accepted.declaration.mother,
        Some(vec![profile.home.join("projects")])
    );
    assert!(!home.join(".claude/.claude.json").exists());
    let claude = accepted.declaration.claude.as_ref().unwrap();
    for contents in [r#"{"oauthAccount":"second","mcpServers":{}}"#, "{}"] {
        fs::write(&global, contents).unwrap();
        assert_eq!(claude.settings().unwrap().0, contents);
        assert_eq!(work.accept_mother("claude").unwrap().root(), profile.root());
    }
    fs::write(&global, "invalid").unwrap();
    assert!(profile.accepted().is_ok());
    assert!(work.accept_mother("claude").is_ok());
    assert!(claude.settings().unwrap_err().is::<MotherSetupError>());
    fs::write(home.join(".claude/.config.json"), r#"{"legacy":true}"#).unwrap();
    assert_eq!(claude.settings().unwrap().0, r#"{"legacy":true}"#);
    fs::remove_file(home.join(".claude/.config.json")).unwrap();
    fs::remove_file(&global).unwrap();
    assert!(profile.accepted().is_ok());
    assert!(profile.check_launch(&work.root, Path::new("sh")).is_err());
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join("custom"));
    fs::create_dir(home.join("custom")).unwrap();
    fs::write(home.join("custom/.claude.json"), "{}").unwrap();
    let overridden = work.accept_mother("claude").unwrap();
    assert_eq!(
        overridden
            .accepted()
            .unwrap()
            .declaration
            .claude
            .unwrap()
            .global,
        home.join("custom/.claude.json")
    );
    assert!(!global.exists());
    std::env::set_var("CLAUDE_CONFIG_DIR", &work.root);
    assert!(work.accept_mother("claude").is_err());
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join("custom"));
    assert_eq!(
        fs::read_to_string(home.join("custom/.claude.json")).unwrap(),
        "{}"
    );
    std::env::set_var("XDG_CACHE_HOME", home.join("xdg-cache"));
    let link = home.join("custom/projects");
    std::os::unix::fs::symlink(&work.root, &link).unwrap();
    assert!(work.accept_mother("claude").is_err());
    fs::remove_file(link).unwrap();
    std::env::set_var("HOME", "");
    assert!(ClaudeState::resolve().is_err());
}

#[test]
fn mother_provider_profiles_preserve_recorded_targets_and_legacy_profiles() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("CODEX_HOME", home.join("codex-state"));
    std::env::set_var("PI_CODING_AGENT_DIR", home.join("pi-state"));
    let work = mother_directory(&home.join("cache"), "owner").unwrap();
    let legacy = Environment::at_root(&work);
    fs::create_dir(home.join("codex-state")).unwrap();
    {
        let _lock = legacy.lock(true).unwrap();
        legacy
            .save(&Accepted {
                root: work.clone(),
                declaration: Declaration {
                    mother: Some(vec![home.join("codex-state")]),
                    ..Default::default()
                },
                ready: true,
            })
            .unwrap();
    }
    let legacy_record = fs::read(legacy.metadata.join("accepted.json")).unwrap();
    let codex = legacy.accept_mother("codex").unwrap();
    let codex_record = fs::read(codex.metadata.join("accepted.json")).unwrap();
    let pi = legacy.accept_mother("pi").unwrap();
    assert_ne!(codex.root(), pi.root());
    assert_ne!(codex.root(), legacy.root());
    assert!(!codex.root().starts_with(&work));
    assert_eq!(legacy.accept_mother("codex").unwrap().root(), codex.root());
    assert_eq!(
        fs::read(codex.metadata.join("accepted.json")).unwrap(),
        codex_record
    );
    assert_eq!(
        fs::read(legacy.metadata.join("accepted.json")).unwrap(),
        legacy_record
    );
    for (target, state) in [
        (codex.root(), "codex-state"),
        (pi.root(), "pi-state"),
        (legacy.root(), "codex-state"),
    ] {
        let reloaded = Environment::at_root(target);
        let accepted = reloaded.accepted().unwrap();
        assert_eq!(accepted.declaration.mother, Some(vec![home.join(state)]));
        assert_eq!(reloaded.writable_root(&accepted.declaration), work);
        reloaded
            .validate_launch_cwd(&work, &accepted.declaration)
            .unwrap();
        assert!(reloaded
            .validate_launch_cwd(&home, &accepted.declaration)
            .is_err());
    }
    assert_eq!(
        fs::metadata(codex.root()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::remove_dir(pi.root()).unwrap();
    std::os::unix::fs::symlink(codex.root(), pi.root()).unwrap();
    assert!(legacy.accept_mother("pi").is_err());
}

#[test]
fn mother_acceptance_reuses_ready_profiles_under_a_shared_lease() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("CODEX_HOME", home.join("codex-state"));
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "lease").unwrap());
    let profile = work.accept_mother("codex").unwrap();
    let record = profile.metadata.join("accepted.json");
    use std::os::unix::fs::MetadataExt;
    let inode = fs::metadata(&record).unwrap().ino();
    let contents = fs::read(&record).unwrap();
    let lease = profile.lock(false).unwrap();
    for _ in 0..2 {
        assert_eq!(work.accept_mother("codex").unwrap().root(), profile.root());
        assert_eq!(fs::metadata(&record).unwrap().ino(), inode);
    }
    std::env::set_var("CODEX_HOME", home.join("changed-state"));
    assert!(work.accept_mother("codex").is_err());
    assert_eq!(fs::read(&record).unwrap(), contents);
    std::env::set_var("CODEX_HOME", &home);
    assert!(work.accept_mother("codex").is_err());
    drop(lease);
    std::env::set_var("CODEX_HOME", home.join("changed-state"));
    let updated = work.accept_mother("codex").unwrap();
    assert_eq!(
        updated.accepted().unwrap().declaration.mother,
        Some(vec![home.join("changed-state")])
    );
    for invalid in [b"invalid".as_slice(), b"{}"] {
        fs::write(&record, invalid).unwrap();
        let _lease = profile.lock(false).unwrap();
        assert!(work.accept_mother("codex").is_err());
        assert_eq!(fs::read(&record).unwrap(), invalid);
    }
    work.accept_mother("codex").unwrap();
    let mut accepted = profile.accepted().unwrap();
    accepted.ready = false;
    profile.save(&accepted).unwrap();
    let _lease = profile.lock(false).unwrap();
    assert!(work.accept_mother("codex").is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn mother_environment_skips_non_utf8_names_and_values_in_both_modes() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use std::os::unix::ffi::OsStrExt;
    let (_temp, env) = fixture();
    let invalid_name = std::ffi::OsStr::from_bytes(b"MOTHER_INVALID_\xff");
    std::env::set_var(invalid_name, "valid-value");
    std::env::set_var("MOTHER_INVALID_VALUE", std::ffi::OsStr::from_bytes(b"\xff"));
    std::env::set_var("MOTHER_VALID", "preserved");
    std::env::set_var("YINZ_PRIVATE_SENTINEL", "hidden");
    std::env::set_var("YINZ_SESSION_NAME", "encoding-test");
    std::env::remove_var("YINZ_PANE_REF");
    let declaration = Declaration {
        mother: Some(vec![]),
        ..Default::default()
    };
    for runtime in [false, true] {
        let command = env.command(&declaration, &env.root, runtime).unwrap();
        let args: Vec<_> = command.get_args().collect();
        let index = args.iter().position(|arg| *arg == "--args").unwrap();
        let encoded = fs::read(format!(
            "/proc/self/fd/{}",
            args[index + 1].to_str().unwrap()
        ))
        .unwrap();
        let words: Vec<_> = encoded.split(|byte| *byte == 0).collect();
        assert!(words
            .windows(3)
            .any(|triple| triple == [b"--setenv".as_slice(), b"MOTHER_VALID", b"preserved"]));
        assert!(!words.contains(&b"MOTHER_INVALID_VALUE".as_slice()));
        assert!(!words.contains(&invalid_name.as_bytes()));
        assert!(!words.contains(&b"YINZ_PRIVATE_SENTINEL".as_slice()));
        assert!(words
            .windows(3)
            .any(|triple| triple == [b"--setenv".as_slice(), b"YINZ_BUBBLEWRAP", b"1"]));
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces; run explicitly on Linux"]
fn real_mother_bubblewrap_preserves_resolver() {
    use std::io::{BufRead, BufReader};
    const CONTENTS: &str = "nameserver 127.0.0.53\n";
    const UPDATED: &str = "nameserver 192.0.2.53\n";
    const SOURCE: &str = "BWRAP_RESOLVER_TEST_SOURCE";
    if let Some(source) = std::env::var_os(SOURCE) {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = temp.path().canonicalize().unwrap();
        let env = Environment::at(root.clone(), &root.join("metadata"));
        let declaration = Declaration {
            mother: Some(vec![]),
            ..Default::default()
        };
        let mut child = env
            .command(&declaration, &root, false)
            .unwrap()
            .args([
                "--",
                "sh",
                "-eu",
                "-c",
                r#"
                cat /etc/resolv.conf
                read -r reply
                cat /etc/resolv.conf
                if echo changed >> /etc/resolv.conf; then exit 1; fi
                test ! -e /run/unrelated
                test ! -e /tmp/unrelated
                "#,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                send.send(line.unwrap()).unwrap();
            }
        });
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(30)).unwrap(),
            CONTENTS.trim_end()
        );
        let source = PathBuf::from(source);
        let replacement = source.with_extension("new");
        fs::write(&replacement, UPDATED).unwrap();
        fs::rename(replacement, source).unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"continue\n")
            .unwrap();
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(30)).unwrap(),
            UPDATED.trim_end()
        );
        let output = child.wait_with_output().unwrap();
        reader.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    // An outer namespace supplies each host layout without touching the real /etc or /run.
    let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    for layout in [
        "stub",
        "resolved",
        "network-manager",
        "multi-hop",
        "relative",
        "directory-link",
        "tmp",
        "plain",
    ] {
        let root = temp.path().canonicalize().unwrap().join(layout);
        let etc = root.join("etc");
        let run = root.join("run");
        let tmp = root.join("tmp");
        fs::create_dir_all(&etc).unwrap();
        fs::create_dir_all(run.join("systemd/resolve")).unwrap();
        fs::create_dir_all(run.join("resolvconf")).unwrap();
        fs::create_dir_all(run.join("NetworkManager")).unwrap();
        fs::create_dir_all(tmp.join("resolver")).unwrap();
        fs::write(run.join("unrelated"), "private").unwrap();
        fs::write(tmp.join("unrelated"), "private").unwrap();
        let target = match layout {
            "plain" => "etc/resolv.conf",
            "resolved" => "run/systemd/resolve/resolv.conf",
            "network-manager" => "run/NetworkManager/resolv.conf",
            "tmp" => "tmp/resolver/resolv.conf",
            _ => "run/systemd/resolve/stub-resolv.conf",
        };
        let source = root.join(target);
        fs::write(&source, CONTENTS).unwrap();
        let link = match layout {
            "multi-hop" | "relative" => {
                std::os::unix::fs::symlink(
                    if layout == "relative" {
                        "../systemd/resolve/stub-resolv.conf"
                    } else {
                        "/run/systemd/resolve/stub-resolv.conf"
                    },
                    run.join("resolvconf/resolv.conf"),
                )
                .unwrap();
                "/run/resolvconf/resolv.conf".to_owned()
            },
            "directory-link" => {
                std::os::unix::fs::symlink("systemd/resolve", run.join("resolver")).unwrap();
                "/run/resolver/stub-resolv.conf".to_owned()
            },
            _ => format!("../{target}"),
        };
        if layout != "plain" {
            std::os::unix::fs::symlink(link, etc.join("resolv.conf")).unwrap();
        }
        let lock = claude_view::mount_lock().unwrap();
        let lock_path = fs::read_link(format!("/proc/self/fd/{}", lock.as_raw_fd())).unwrap();
        fs::write(tmp.join(lock_path.file_name().unwrap()), "").unwrap();
        let mut outer = Command::new("bwrap");
        claude_view::guard_setup(&mut outer).unwrap();
        let output = outer
            .args([
                "--unshare-all",
                "--share-net",
                "--die-with-parent",
                "--ro-bind",
                "/",
                "/",
                "--bind",
            ])
            .arg(&root)
            .arg(&root)
            .arg("--ro-bind")
            .arg(&etc)
            .arg("/etc")
            .arg("--ro-bind")
            .arg(&run)
            .arg("/run")
            .arg("--ro-bind")
            .arg(&tmp)
            .arg("/tmp")
            .arg("--ro-bind")
            .arg(&lock_path)
            .arg(&lock_path)
            .args(["--dev", "/dev", "--proc", "/proc"])
            .arg("--chdir")
            .arg(&root)
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bubblewrap::tests::real_mother_bubblewrap_preserves_resolver",
                "--ignored",
                "--nocapture",
            ])
            .env(SOURCE, &source)
            .env("TMPDIR", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "layout {layout}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn mother_resolver_mount_follows_private_run() {
    let resolver = Path::new("/etc/resolv.conf");
    if !resolver.exists() {
        return;
    }
    let (_temp, env) = fixture();
    let declaration = Declaration {
        mother: Some(vec![]),
        ..Default::default()
    };
    let command = env.command(&declaration, &env.root, false).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let private_run = args
        .windows(2)
        .position(|args| args == ["--tmpfs", "/run"])
        .unwrap();
    let mut mounts = Command::new("bwrap");
    bind_mother_resolver(&mut mounts, resolver).unwrap();
    let mounts: Vec<_> = mounts.get_args().collect();
    if !mounts.is_empty() {
        assert!(
            args.windows(mounts.len())
                .position(|args| args == mounts)
                .unwrap()
                > private_run
        );
    }
    assert!(!args
        .windows(3)
        .any(|args| args[0] == "--ro-bind" && args[1] == resolver));
}

#[cfg(target_os = "linux")]
#[test]
fn mother_resolver_mounts_directories_for_every_symlink_hop() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path().canonicalize().unwrap();
    let etc = root.join("etc");
    let middle = root.join("resolvconf");
    let target = root.join("systemd/resolve");
    for directory in [&etc, &middle, &target] {
        fs::create_dir_all(directory).unwrap();
    }
    let resolver = etc.join("resolv.conf");
    std::os::unix::fs::symlink("../resolvconf/resolv.conf", &resolver).unwrap();
    std::os::unix::fs::symlink(target.join("stub-resolv.conf"), middle.join("resolv.conf"))
        .unwrap();
    fs::write(target.join("stub-resolv.conf"), "nameserver 127.0.0.53").unwrap();
    let mut command = Command::new("bwrap");
    bind_mother_resolver(&mut command, &resolver).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let expected: Vec<_> = [&etc, &middle, &target]
        .into_iter()
        .flat_map(|path| {
            [
                std::ffi::OsStr::new("--ro-bind"),
                path.as_os_str(),
                path.as_os_str(),
            ]
        })
        .collect();
    assert_eq!(args, expected);
    fs::remove_file(target.join("stub-resolv.conf")).unwrap();
    std::os::unix::fs::symlink(&resolver, target.join("stub-resolv.conf")).unwrap();
    assert!(bind_mother_resolver(&mut Command::new("bwrap"), &resolver).is_err());
    for path in ["/run/resolv.conf", "/tmp/resolv.conf"] {
        assert!(bind_mother_resolver(&mut Command::new("bwrap"), Path::new(path)).is_err());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces; run explicitly on Linux"]
fn real_claude_mother_supports_symlinked_storage_and_settings() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let home = temp.path().canonicalize().unwrap();
    let alias = home.join("alias");
    std::os::unix::fs::symlink(&home, &alias).unwrap();
    std::env::set_var("HOME", &alias);
    std::env::set_var("XDG_DATA_HOME", alias.join("data"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    fs::create_dir(home.join(".claude")).unwrap();
    fs::create_dir(home.join("host-projects")).unwrap();
    fs::write(home.join("host-projects/host.jsonl"), "host transcript").unwrap();
    fs::write(home.join("settings.json"), r#"{"host":true}"#).unwrap();
    for name in [
        ".claude.json",
        ".claude/.config.json",
        ".claude/.claude.json",
    ] {
        std::os::unix::fs::symlink(home.join("settings.json"), home.join(name)).unwrap();
    }
    std::os::unix::fs::symlink(home.join("host-projects"), home.join(".claude/projects")).unwrap();
    let work = Environment::at_root(&mother_directory(&home.join("cache"), "owner").unwrap());
    let mut profile = work.prepare_mother("claude").unwrap();
    let accepted = profile.accepted().unwrap();
    profile.settings = Some(
        accepted
            .declaration
            .claude
            .as_ref()
            .unwrap()
            .settings()
            .unwrap(),
    );
    let trusted_settings = profile.settings.as_ref().unwrap().0.clone();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&trusted_settings).unwrap()["projects"]
            [work.root.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
    profile
        .validate_launch_cwd(&work.root, &accepted.declaration)
        .unwrap();
    for _ in 0..2 {
        let output = profile
            .command(&accepted.declaration, &work.root, false)
            .unwrap()
            .args(["--setenv", "EXPECTED_CLAUDE_SETTINGS", &trusted_settings])
            .args([
                "--",
                "/bin/sh",
                "-eu",
                "-c",
                r#"
                for name in .claude.json .config.json; do
                    test "$(cat "$CLAUDE_CONFIG_DIR/$name")" = "$EXPECTED_CLAUDE_SETTINGS"
                    echo private > "$CLAUDE_CONFIG_DIR/$name.tmp"
                    mv "$CLAUDE_CONFIG_DIR/$name.tmp" "$CLAUDE_CONFIG_DIR/$name"
                    test "$(cat "$CLAUDE_CONFIG_DIR/$name")" = private
                done
                test ! -e "$CLAUDE_CONFIG_DIR/projects/host.jsonl"
                echo private >> "$CLAUDE_CONFIG_DIR/projects/private.jsonl"
                if echo changed > "$HOME/settings.json"; then exit 1; fi
                "#,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).unwrap(),
            trusted_settings
        );
        assert_eq!(
            fs::read_to_string(home.join("host-projects/host.jsonl")).unwrap(),
            "host transcript"
        );
        assert!(!home.join("host-projects/private.jsonl").exists());
        for name in [
            ".claude/.config.json",
            ".claude/.claude.json",
            ".claude/projects",
        ] {
            assert!(home.join(name).is_symlink());
        }
    }
    assert_eq!(
        fs::read_to_string(profile.home.join("projects/private.jsonl")).unwrap(),
        "private\nprivate\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires real Bubblewrap user namespaces; run explicitly on Linux"]
fn real_mother_bubblewrap_limits_writes_and_preserves_agent_environment() {
    if yinz_test_support::reexec_test() {
        return;
    }
    // Keep the host fixture outside /tmp, which the sandbox replaces with private storage.
    let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("metadata"));
    std::env::set_var("CODEX_HOME", home.join("agent-state"));
    std::env::set_var("PI_CODING_AGENT_DIR", home.join("pi-state"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::set_var("XDG_CACHE_HOME", home.join("xdg-cache"));
    std::env::set_var("XDG_STATE_HOME", home.join("xdg-state"));
    fs::create_dir_all(home.join(".claude/skills")).unwrap();
    for file in [
        "settings.json",
        ".credentials.json",
        "CLAUDE.md",
        "skills/example",
    ] {
        fs::write(home.join(".claude").join(file), "host").unwrap();
    }
    fs::write(home.join(".claude.json"), r#"{"host":true}"#).unwrap();
    std::env::set_var("MOTHER_AUTH_SENTINEL", "test-auth");
    std::env::set_var("YINZ_SESSION_NAME", "mother-test");
    std::env::set_var("YINZ_SOCKET_DIR", home.join("sockets"));
    fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let root = mother_directory(&home.join("cache"), "test").unwrap();
    let work = Environment::at_root(&root);
    let codex = work.prepare_mother("codex").unwrap();
    let _lease = codex.lock(false).unwrap();
    assert_eq!(work.prepare_mother("codex").unwrap().root(), codex.root());
    let pi = codex.for_agent("pi", Some(&root)).unwrap();
    assert!(codex.for_agent("cursor", Some(&root)).is_err());
    assert_ne!(codex.root(), pi.root());
    let env = Environment::at_root(codex.root());
    assert_eq!(
        env.check_launch(&root, Path::new("sh")).unwrap(),
        LaunchCheck::Ready(None),
        "{}",
        fs::read_to_string(env.metadata.join("launch.log")).unwrap_or_default()
    );
    fs::write(home.join("project"), "unchanged").unwrap();
    let accepted = env.accepted().unwrap();
    let output = env
        .command(&accepted.declaration, &root, false)
        .unwrap()
        .args([
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
            echo own > own-file
            echo state > "$CODEX_HOME/state"
            if touch "$PI_CODING_AGENT_DIR/codex-must-not-write"; then exit 1; fi
            echo private > /tmp/mother-private
            test "$MOTHER_AUTH_SENTINEL" = test-auth
            test "$YINZ_BUBBLEWRAP" = 1
            test "$(cat "$HOME/project")" = unchanged
            if echo changed > "$HOME/project"; then exit 1; fi
            if touch "$HOME/metadata/new"; then exit 1; fi
            command -v yinz
        "#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("own-file")).unwrap(), "own\n");
    assert_eq!(
        fs::read_to_string(home.join("agent-state/state")).unwrap(),
        "state\n"
    );
    assert_eq!(
        fs::read_to_string(home.join("project")).unwrap(),
        "unchanged"
    );
    assert!(!root.join(".git").exists());
    let accepted = pi.accepted().unwrap();
    let output = pi
        .command(&accepted.declaration, &root, false)
        .unwrap()
        .args([
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
            echo pi > "$PI_CODING_AGENT_DIR/state"
            test "$(cat own-file)" = own
            if touch "$CODEX_HOME/pi-must-not-write"; then exit 1; fi
        "#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Claude replaces its settings file by rename, which needs a writable directory.
    let mut claude = codex.for_agent("claude", Some(&root)).unwrap();
    let accepted = claude.accepted().unwrap();
    claude.settings = Some(
        accepted
            .declaration
            .claude
            .as_ref()
            .unwrap()
            .settings()
            .unwrap(),
    );
    let output = claude
        .command(&accepted.declaration, &root, false)
        .unwrap()
        .args([
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
            test "$CLAUDE_CONFIG_DIR" = "$HOME/.claude"
            test "$DISABLE_AUTOUPDATER" = 1
            test "$(cat "$CLAUDE_CONFIG_DIR/.claude.json")" = '{"host":true}'
            echo sandbox > "$CLAUDE_CONFIG_DIR/.claude.json.tmp"
            mv "$CLAUDE_CONFIG_DIR/.claude.json.tmp" "$CLAUDE_CONFIG_DIR/.claude.json"
            echo log > "$XDG_CACHE_HOME/claude-cli-nodejs/log"
            mkdir -p "$XDG_STATE_HOME/claude/locks" "$XDG_CACHE_HOME/claude/staging"
            echo lock > "$XDG_STATE_HOME/claude/locks/test"
            echo staging > "$XDG_CACHE_HOME/claude/staging/test"
            for file in settings.json .credentials.json CLAUDE.md skills/example; do
                echo sandbox > "$CLAUDE_CONFIG_DIR/$file.tmp"
                mv "$CLAUDE_CONFIG_DIR/$file.tmp" "$CLAUDE_CONFIG_DIR/$file"
            done
            echo transcript > "$CLAUDE_CONFIG_DIR/projects/test.jsonl"
            if echo changed > "$HOME/.claude.json"; then exit 1; fi
            if touch "$CODEX_HOME/claude-must-not-write"; then exit 1; fi
        "#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(home.join(".claude.json")).unwrap(),
        r#"{"host":true}"#
    );
    assert!(!home.join(".claude/.claude.json").exists());
    for file in [
        "settings.json",
        ".credentials.json",
        "CLAUDE.md",
        "skills/example",
    ] {
        assert_eq!(
            fs::read_to_string(home.join(".claude").join(file)).unwrap(),
            if file == ".credentials.json" {
                "sandbox\n"
            } else {
                "host"
            }
        );
    }
    assert!(!home.join(".claude/projects").exists());
    assert!(!home.join("xdg-state/claude").exists());
    assert!(!home.join("xdg-cache/claude").exists());
    // Hold one overlay open while another starts, with a pipe handshake rather than timing.
    let mut running_command = claude.command(&accepted.declaration, &root, false).unwrap();
    let mut running = running_command
        .args([
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
            echo private > "$CLAUDE_CONFIG_DIR/launch-marker"
            echo ready
            read release
        "#,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(running.stdout.take().unwrap()),
        &mut ready,
    )
    .unwrap();
    assert_eq!(ready, "ready\n");
    // A prepared launch retains its snapshot despite a later host rewrite.
    fs::write(home.join(".claude.json"), r#"{"host":"changed"}"#).unwrap();
    fs::write(home.join(".claude/.credentials.json"), "new account").unwrap();
    fs::remove_file(home.join(".claude/skills/example")).unwrap();
    let output = claude
        .command(&accepted.declaration, &root, false)
        .unwrap()
        .args([
            "--",
            "sh",
            "-eu",
            "-c",
            r#"
            test "$(cat "$CLAUDE_CONFIG_DIR/.claude.json")" = '{"host":true}'
            test "$(cat "$CLAUDE_CONFIG_DIR/.credentials.json")" = 'new account'
            test "$(cat "$CLAUDE_CONFIG_DIR/settings.json")" = host
            test ! -e "$CLAUDE_CONFIG_DIR/skills/example"
            test ! -e "$CLAUDE_CONFIG_DIR/launch-marker"
            test "$(cat "$CLAUDE_CONFIG_DIR/projects/test.jsonl")" = transcript
            test ! -e "$XDG_STATE_HOME/claude/locks/test"
        "#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    writeln!(running.stdin.take().unwrap(), "release").unwrap();
    assert!(running.wait().unwrap().success());
}

#[test]
fn mother_state_cannot_expose_metadata_through_a_storage_symlink() {
    let (temp, mut env) = fixture();
    let state = temp.path().join("agent-state");
    let metadata = state.join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    let alias = temp.path().join("metadata-alias");
    std::os::unix::fs::symlink(&metadata, &alias).unwrap();
    env.metadata = alias;
    let declaration = Declaration {
        mother: Some(vec![state.canonicalize().unwrap()]),
        ..Default::default()
    };
    assert!(env.validate_storage(&declaration).is_err());
    let state_alias = temp.path().join("state-alias");
    std::os::unix::fs::symlink(&state, &state_alias).unwrap();
    let declaration = Declaration {
        mother: Some(vec![state_alias]),
        ..Default::default()
    };
    assert!(env.validate_storage(&declaration).is_err());
}

#[test]
fn settings_snapshot_recovers_partial_writes_and_bounds_corruption() {
    let mut attempts = 0;
    let snapshot = settings_snapshot(|| {
        attempts += 1;
        match attempts {
            1 => Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                .context("settings absent during rewrite"),
            2 => Ok(Vec::new()),
            3 => Ok(br#"{"partial":"#.to_vec()),
            _ => Ok(br#"{"complete":true}"#.to_vec()),
        }
    })
    .unwrap();
    assert_eq!(attempts, 4);
    assert_eq!(snapshot.0, r#"{"complete":true}"#);
    for invalid in ["", "{", "null", "[]", "broken"] {
        let mut attempts = 0;
        let error = settings_snapshot(|| {
            attempts += 1;
            Ok(invalid.as_bytes().to_vec())
        })
        .unwrap_err();
        assert_eq!(attempts, 5);
        assert!(error.is::<MotherSetupError>());
    }
    let command = yinz_types::input::command::RunCommand {
        claude_settings: Some(snapshot),
        ..Default::default()
    };
    assert!(!serde_json::to_string(&command)
        .unwrap()
        .contains("complete"));
    assert!(!format!("{command:?}").contains("complete"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires /dev/fuse and fusermount3; run explicitly on Linux"]
fn real_claude_supervision_preserves_exit_and_stops_on_service_failure() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use std::os::unix::process::ExitStatusExt;
    let host = tempfile::tempdir().unwrap();
    for script in [
        "exit 23",
        "kill -TERM $$",
        "flock -x \"$2\" fusermount3 -u \"$1\"; exec sleep 30",
    ] {
        let view = claude_view::View::mount(host.path(), b"{}").unwrap();
        let mount = view.path().to_path_buf();
        let mut sandbox = SandboxCommand {
            command: Command::new("/bin/sh"),
            view: Some(view),
        };
        let lock = claude_view::mount_lock().unwrap();
        let lock_path = fs::read_link(format!("/proc/self/fd/{}", lock.as_raw_fd())).unwrap();
        sandbox
            .args(["-c", script, "sh"])
            .arg(&mount)
            .arg(lock_path);
        let result = supervise(&mut sandbox);
        if script == "exit 23" {
            assert_eq!(result.unwrap().code(), Some(23));
        } else if script == "kill -TERM $$" {
            assert_eq!(result.unwrap().signal(), Some(libc::SIGTERM));
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("filesystem service stopped"));
        }
        drop(sandbox);
        assert!(!mount.exists());
    }
}
