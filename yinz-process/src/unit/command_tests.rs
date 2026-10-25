use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

#[test]
fn pane_command_preparation_preserves_arguments_and_only_uses_existing_directories() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    std::fs::write(&file, "not a directory").unwrap();
    for cwd in [
        None,
        Some(root.path().to_owned()),
        Some(root.path().join("missing")),
        Some(file),
    ] {
        let command = RunCommand {
            command: "/usr/bin/true".into(),
            args: vec!["argument with spaces".into(), "--literal=$value".into()],
            cwd: cwd.clone(),
            ..Default::default()
        };
        let prepared = prepare_command(&command).unwrap();
        assert_eq!(prepared.get_program(), command.command.as_os_str());
        assert_eq!(
            prepared.get_args().collect::<Vec<_>>(),
            command
                .args
                .iter()
                .map(std::ffi::OsStr::new)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prepared.get_current_dir(),
            cwd.as_deref().filter(|path| path.is_dir())
        );
    }
}

#[test]
fn agent_launchers_use_the_execution_hosts_binary_outside_path() {
    let root = tempfile::tempdir().unwrap();
    let host_bin = root.path().join("host yinz");
    yinz_test_support::write_warm_script(&host_bin, "printf '%s\\n' \"$@\"");
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    symlink("/usr/bin/env", bin.join("env")).unwrap();
    for wrap in [
        yinz_runtime::agent::launch::wrap_codex as fn(String, Vec<String>) -> (String, Vec<String>),
        yinz_runtime::agent::launch::wrap_muse,
    ] {
        let (program, args) = wrap(
            "/opt/agent cli".into(),
            vec![
                "resume".into(),
                "--last".into(),
                "prompt with spaces=kept".into(),
            ],
        );
        assert!(resolve_command_with_path(
            &RunCommand {
                command: program.clone().into(),
                ..Default::default()
            },
            Some(bin.as_os_str()),
        )
        .is_none());

        for with_env in [false, true] {
            let mut cmd = RunCommand {
                command: program.clone().into(),
                args: args.clone(),
                ..Default::default()
            };
            let mut expected = cmd.clone();
            expected.command = host_bin.clone();
            if with_env {
                cmd.command = "env".into();
                cmd.args.splice(
                    ..0,
                    [
                        format!("PATH={}", bin.display()),
                        "SETTING=value with spaces=kept".into(),
                        program.clone(),
                    ],
                );
                expected = cmd.clone();
                expected.args[2] = host_bin.to_str().unwrap().into();
            }
            resolve_agent_launcher(&host_bin, &mut cmd.command, &mut cmd.args).unwrap();
            assert_eq!(cmd, expected);
            let output = std::process::Command::new(&cmd.command)
                .args(&cmd.args)
                .env("PATH", &bin)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!("{}\n", args.join("\n"))
            );
        }
    }
}

#[test]
fn agent_launcher_resolution_leaves_other_commands_and_explicit_paths_unchanged() {
    for (program, args) in [
        ("codex", vec![]),
        ("muse", vec![]),
        ("yinz", vec!["action", "new-pane"]),
        ("yinz", vec!["agent"]),
        ("yinz", vec!["agent", "hooks", "install"]),
        ("env", vec!["PATH=/remote/bin", "codex"]),
        (
            "env",
            vec!["PATH=/remote/bin", "yinz", "action", "new-pane"],
        ),
        ("env", vec!["PATH=/remote/bin"]),
        ("/other/yinz", vec!["agent", "codex", "--"]),
        (
            "env",
            vec!["PATH=/remote/bin", "/other/yinz", "agent", "codex", "--"],
        ),
    ] {
        let requested = RunCommand {
            command: program.into(),
            args: args.into_iter().map(String::from).collect(),
            ..Default::default()
        };
        let mut resolved = requested.clone();
        resolve_agent_launcher(
            Path::new("/remote/bin/yinz"),
            &mut resolved.command,
            &mut resolved.args,
        )
        .unwrap();
        assert_eq!(resolved, requested);
    }
}

#[test]
fn command_lookup_agrees_with_execution_in_the_child_directory() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let bin = cwd.join("bin");
    let blocked = cwd.join("blocked");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&blocked).unwrap();
    symlink("/usr/bin/true", bin.join("run-me")).unwrap();
    symlink("/usr/bin/true", cwd.join("local-only")).unwrap();
    std::fs::write(blocked.join("run-me"), b"not executable").unwrap();
    std::fs::set_permissions(
        blocked.join("run-me"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    for (command, path, expected) in [
        ("run-me", "bin", Some(bin.join("run-me"))),
        ("run-me", "blocked:bin", Some(bin.join("run-me"))),
        ("local-only", "", Some(cwd.join("local-only"))),
        ("local-only", "bin", None),
        ("./run-me", "bin", None),
        ("./bin/run-me", "blocked", Some(bin.join("run-me"))),
        ("run-me", "blocked", None),
    ] {
        let cmd = RunCommand {
            command: command.into(),
            cwd: Some(cwd.clone()),
            ..Default::default()
        };
        assert_eq!(
            resolve_command_with_path(&cmd, Some(path.as_ref())),
            expected,
            "command={command}, PATH={path}"
        );
        let executed = std::process::Command::new(command)
            .current_dir(&cwd)
            .env("PATH", path)
            .status();
        assert_eq!(
            executed.is_ok(),
            expected.is_some(),
            "command={command}, PATH={path}"
        );
        if let Ok(status) = executed {
            assert!(status.success());
        }
    }
}

#[test]
fn command_lookup_works_after_the_current_directory_is_removed() {
    const CHILD_ENV: &str = "YINZ_TEST_REMOVED_COMMAND_CWD";
    if env::var_os(CHILD_ENV).is_none() {
        // Isolate the process-wide cwd change even when run with cargo test.
        let output = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "command::tests::command_lookup_works_after_the_current_directory_is_removed",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().canonicalize().unwrap().join("missing");
    env::set_current_dir(dir.path()).unwrap();
    dir.close().unwrap();
    assert!(env::current_dir().is_err());

    for cwd in [None, Some(missing)] {
        for path in ["/usr/bin", "relative:/usr/bin", ":/usr/bin"] {
            let cmd = RunCommand {
                command: "true".into(),
                cwd: cwd.clone(),
                ..Default::default()
            };
            let mut child = std::process::Command::new(&cmd.command);
            // Match both spawn paths' fallback when the requested cwd is missing.
            if let Some(cwd) = cmd.cwd.as_ref().filter(|cwd| cwd.is_dir()) {
                child.current_dir(cwd);
            }
            assert!(child.env("PATH", path).status().unwrap().success());
            assert_eq!(
                resolve_command_with_path(&cmd, Some(path.as_ref())),
                Some(PathBuf::from("/usr/bin/true")),
                "cwd={cwd:?}, PATH={path}"
            );
        }
    }
}

#[test]
fn only_a_bubblewrap_target_execs_through_this_binarys_launcher() {
    let plain = RunCommand {
        command: "claude".into(),
        args: vec!["--resume".into()],
        ..Default::default()
    };
    assert_eq!(
        exec_program_and_args(&plain).unwrap(),
        (PathBuf::from("claude"), vec!["--resume".to_owned()])
    );

    let sandboxed = RunCommand {
        target: yinz_types::command::PaneTarget::Bubblewrap {
            root: "/my project".into(),
        },
        ..plain.clone()
    };
    let (program, args) = exec_program_and_args(&sandboxed).unwrap();
    assert_eq!(Some(&program), yinz_runtime::own_binary::server_binary());
    assert_eq!(
        args,
        [
            yinz_types::agent::BUBBLEWRAP_RUN_FLAG,
            "/my project",
            "--",
            "claude",
            "--resume"
        ]
    );
    // What a sandboxed pane reports for its process reads back to the command as asked.
    let mut argv = vec![program.to_string_lossy().into_owned()];
    argv.extend(args);
    assert_eq!(
        yinz_types::agent::bubblewrap_inner_argv(&argv),
        Some(["claude".to_owned(), "--resume".to_owned()].as_slice())
    );
    // The agent resolves on the sandbox's PATH, not this machine's.
    assert!(can_exec(
        &sandboxed,
        Some(std::ffi::OsStr::new("/nonexistent"))
    ));
    assert!(!can_exec(
        &plain,
        Some(std::ffi::OsStr::new("/nonexistent"))
    ));
}
