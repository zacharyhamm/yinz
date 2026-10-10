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
    env.prepare(None).unwrap();
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
fn wrapper_preserves_agent_identity_and_rejects_boundary_env_overrides() {
    let (_temp, env) = fixture();
    let mut command = yinz_types::command::CommandToRun {
        path: "env".into(),
        args: vec![
            "YINZ_AGENT_PANE=1".into(),
            "yinz".into(),
            "agent".into(),
            "codex".into(),
            "--command".into(),
            "codex".into(),
            "--".into(),
            "--model".into(),
            "model".into(),
        ],
        cwd: Some(env.root.clone()),
        ..Default::default()
    };
    env.wrap(&mut command).unwrap();
    assert_eq!(
        yinz_types::agent::agent_kind_of(Path::new("yinz"), &command.args).as_deref(),
        Some("codex")
    );
    for name in [
        "HOME",
        "PATH",
        "YINZ_BUBBLEWRAP",
        "YINZ_SOCKET_DIR",
        "CARGO_TARGET_DIR",
    ] {
        assert!(validate_spawn_env(&BTreeMap::from([(name.into(), "bad".into())])).is_err());
    }
    validate_spawn_env(&BTreeMap::from([("YINZ_AGENT_PANE".into(), "1".into())])).unwrap();
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
}
