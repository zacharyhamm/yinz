use super::*;

#[test]
fn provisioning_refreshes_instructions_and_preserves_operational_memory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("memory.json"), "my goals").unwrap();
    provision(dir.path(), "test session").unwrap();
    fs::write(dir.path().join("AGENTS.md"), "old instructions").unwrap();
    provision(dir.path(), "test session").unwrap();
    let bootstrap = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(bootstrap.starts_with(BOOTSTRAP));
    assert!(bootstrap.contains(&format!(
        "{:?}",
        crate::own_binary::server_binary().unwrap()
    )));
    assert_eq!(
        fs::read_to_string(dir.path().join("memory.json")).unwrap(),
        "my goals"
    );
    for root in [".agents/skills", ".claude/skills"] {
        for name in ["yinz-mother", "yinz-agents", "yinz-sessions"] {
            assert!(dir.path().join(root).join(name).join("SKILL.md").is_file());
        }
    }
}

#[cfg(unix)]
#[test]
fn provisioning_refuses_a_link_to_global_agent_configuration() {
    let work = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(global.path(), work.path().join(".agents")).unwrap();
    assert!(provision(work.path(), "test session").is_err());
    assert_eq!(fs::read_dir(global.path()).unwrap().count(), 0);
}

#[test]
fn coordination_rules_are_explicit_and_quote_paths_and_sessions() {
    let executable = "/opt/Yinz build's/bin/yinz";
    let session = "mother's \"test\" session";
    let rules = codex_rules(executable, session).unwrap();
    let patterns: Vec<Vec<String>> = rules
        .lines()
        .skip(1)
        .map(|line| {
            serde_json::from_str(
                line.strip_prefix("prefix_rule(pattern=")
                    .unwrap()
                    .strip_suffix(", decision=\"allow\")")
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(patterns.len(), COORDINATION_COMMANDS.len() * 4);
    for binary in [executable, "yinz"] {
        for command in COORDINATION_COMMANDS {
            let mut implicit = vec![binary];
            implicit.extend_from_slice(command);
            assert!(patterns.iter().any(|pattern| pattern == &implicit));
            let mut explicit = vec![binary, "--session", session];
            explicit.extend_from_slice(command);
            assert!(patterns.iter().any(|pattern| pattern == &explicit));
        }
    }
    for denied in [
        vec!["yinz", "agent", "claude"],
        vec!["yinz", "agent", "codex"],
        vec!["yinz", "agent", "clear-draft"],
        vec!["yinz", "hooks", "install"],
        vec!["yinz", "action", "agent-report"],
        vec!["yinz", "setup"],
        vec!["yinz", "--session", "another session", "agent", "send-keys"],
        vec!["yinz-other", "agent", "send-keys"],
        vec!["sh", "-c", "yinz agent send-keys terminal_3 Enter"],
    ] {
        assert!(
            !patterns.iter().any(|pattern| denied
                .starts_with(&pattern.iter().map(String::as_str).collect::<Vec<_>>())),
            "{denied:?}"
        );
    }
}

#[test]
fn claude_permissions_reject_rule_injection_in_paths_and_sessions() {
    for value in [
        "s) Bash (",
        "s(",
        "s)",
        "s,Bash",
        "s*",
        "s\0",
        "s\n",
        "s\r",
        "s\t",
        "s\u{1b}",
        "s\u{7f}",
        "s\u{85}",
    ] {
        let error = claude_args("/opt/yinz", value).unwrap_err();
        assert!(
            error.to_string().contains("session name"),
            "{value:?}: {error}"
        );
        let executable = format!("/opt/{value}/yinz");
        let error = claude_args(&executable, "test").unwrap_err();
        assert!(
            error.to_string().contains("executable path"),
            "{executable:?}: {error}"
        );
    }
    assert!(coordination_args("claude", Path::new("."), "s) Bash (").is_err());
}

#[test]
fn claude_permissions_preserve_quoted_paths_and_sessions() {
    let args = claude_args("/opt/Yinz build's/bin/yinz", "mother's \"test\" session").unwrap();
    assert_eq!(args.len(), COORDINATION_COMMANDS.len() * 4);
    assert!(args.contains(&"--allowedTools=Bash(yinz agent list:*)".into()));
    assert!(args.contains(
        &"--allowedTools=Bash('/opt/Yinz build'\\''s/bin/yinz' --session 'mother'\\''s \"test\" session' agent list:*)".into()
    ));
    assert!(args
        .iter()
        .all(|arg| arg.starts_with("--allowedTools=Bash(")
            && arg.ends_with(":*)")
            && arg.matches('(').count() == 1
            && arg.matches(')').count() == 1
            && arg.matches('*').count() == 1));
}

#[test]
fn permissions_are_launch_scoped_and_pi_needs_no_override() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("Mother's workspace");
    fs::create_dir(&work).unwrap();
    let args = coordination_args("codex", &work, "test").unwrap();
    assert_eq!(&args[..2], &crate::agent::launch::codex_yinz_args());
    assert_eq!(args[2], "-c");
    assert_eq!(
        args[3],
        format!(
            "projects={{{}={{trust_level=\"trusted\"}}}}",
            serde_json::to_string(&work.canonicalize().unwrap()).unwrap()
        )
    );
    assert!(!work.join(".codex/config.toml").exists());
    assert!(coordination_args("pi", &work, "test").unwrap().is_empty());
    let args = coordination_args("claude", &work, "Mother's session").unwrap();
    assert!(args.contains(&"--allowedTools=Bash(yinz agent send-keys:*)".into()));
    assert!(args.contains(
        &"--allowedTools=Bash(yinz --session 'Mother'\\''s session' agent send-keys:*)".into()
    ));
    assert!(args
        .iter()
        .all(|arg| arg.starts_with("--allowedTools=Bash(")));
    assert!(coordination_args("claude", &work, "other*session").is_err());
    for kind in ["claude", "codex", "pi"] {
        let (mut command, _) = crate::agent::launch::plan(&crate::agent::launch::Spec {
            kind: Some(kind.into()),
            permission_mode: (kind == "claude").then(|| "auto".into()),
            ..Default::default()
        })
        .unwrap();
        let original = command.args.clone();
        command
            .args
            .extend(coordination_args(kind, &work, "test").unwrap());
        assert!(command.args.starts_with(&original));
        assert_eq!(
            crate::agent::restart_args(&command.path, &command.args).unwrap(),
            command.args
        );
        if kind == "pi" {
            assert_eq!(command.args, original);
        }
    }
}

#[cfg(unix)]
#[test]
fn codex_provisioning_preserves_other_files_and_rejects_linked_directories() {
    for relative in [".codex", ".codex/rules"] {
        let work = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        fs::create_dir_all(work.path().join(relative).parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(global.path(), work.path().join(relative)).unwrap();
        assert!(provision(work.path(), "test").is_err());
        assert_eq!(fs::read_dir(global.path()).unwrap().count(), 0);
    }
    let work = tempfile::tempdir().unwrap();
    provision(work.path(), "first").unwrap();
    let custom = work.path().join(".codex/rules/custom.rules");
    fs::write(&custom, "# keep me").unwrap();
    let rules = work.path().join(".codex/rules/yinz-mother.rules");
    let external = tempfile::NamedTempFile::new().unwrap();
    fs::write(external.path(), "external").unwrap();
    fs::remove_file(&rules).unwrap();
    std::os::unix::fs::symlink(external.path(), &rules).unwrap();
    provision(work.path(), "second").unwrap();
    let generated = fs::read_to_string(&rules).unwrap();
    provision(work.path(), "second").unwrap();
    assert_eq!(fs::read_to_string(&rules).unwrap(), generated);
    assert!(generated.contains("second"));
    assert!(!generated.contains("first"));
    assert_eq!(fs::read_to_string(&custom).unwrap(), "# keep me");
    assert_eq!(fs::read_to_string(external.path()).unwrap(), "external");
    assert!(!rules.is_symlink());
}

#[test]
#[ignore = "requires the installed Codex CLI"]
fn codex_cli_checks_generated_rules() {
    let work = tempfile::tempdir().unwrap();
    let rules = work.path().join("mother.rules");
    fs::write(
        &rules,
        codex_rules("/opt/Yinz build/bin/yinz", "Mother's session").unwrap(),
    )
    .unwrap();
    for (command, allowed) in [
        (
            vec!["yinz", "agent", "send-keys", "terminal_3", "Enter"],
            true,
        ),
        (
            vec![
                "/opt/Yinz build/bin/yinz",
                "--session",
                "Mother's session",
                "agent",
                "send-keys",
                "terminal_3",
                "Enter",
            ],
            true,
        ),
        (vec!["yinz", "mother", "checkpoint"], true),
        (vec!["yinz", "hooks", "install"], false),
        (vec!["yinz", "agent", "codex"], false),
        (
            vec!["yinz", "--session", "other", "agent", "send-keys"],
            false,
        ),
        (vec!["/opt/other/yinz", "agent", "send-keys"], false),
        (vec!["yinz-extra", "agent", "send-keys"], false),
        (vec!["python3", "-c", "print('hello')"], false),
        (
            vec![
                "bash",
                "-lc",
                "yinz agent send-keys terminal_3 Enter && unrelated-command",
            ],
            false,
        ),
    ] {
        let output = std::process::Command::new("codex")
            .args(["execpolicy", "check", "--rules"])
            .arg(&rules)
            .args(&command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result["decision"] == "allow",
            allowed,
            "{command:?}: {result}"
        );
    }
}
