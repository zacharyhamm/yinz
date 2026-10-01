use super::*;
use yinz_runtime::channels::{self, SenderWithContext};

#[test]
fn cli_agent_spawn_returns_actual_pane_checkout_and_collision_branch() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        let output = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let data_access = DataAccess::sqlite(&root.join("data"));
    let (to_pty, pty_rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_pty: Some(SenderWithContext::new(to_pty)),
        ..Default::default()
    };
    // All three launch preparations must reject before touching Git, a VM, or the pty.
    for (worktree, firecracker) in [(false, None), (true, None), (true, Some("unprepared-vm"))] {
        let response = agent_control::control_agent(
            yinz_types::agent::control::AgentRequest::Spawn {
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some("rejected".into()),
                    worktree: Some(worktree),
                    firecracker: firecracker.map(str::to_owned),
                    resume: Some("last".into()),
                    permission_mode: Some("skip".into()),
                    ..Default::default()
                },
                cwd: Some(root.clone()),
                project: None,
                remote_host: None,
                automatic_firecracker_vm: true,
            },
            crate::ClientId::new(99),
            &senders,
            &data_access,
        );
        assert_eq!(
            response.outcome,
            yinz_types::agent::control::AgentOutcome::Rejected
        );
        assert!(response.message.unwrap().contains("omit --permission-mode"));
        assert!(response.spawned.is_none());
        assert!(pty_rx.try_recv().is_err());
        assert!(!root.join(".worktrees").exists());
        let refs = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["branch", "--format=%(refname:short)"])
            .output()
            .unwrap();
        assert!(refs.status.success());
        assert_eq!(String::from_utf8(refs.stdout).unwrap(), "main\n");
    }
    let worker_root = root.clone();
    let worker = thread::spawn(move || {
        for (pane, branch) in [(7, "review"), (8, "review-2")] {
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(
                Some(TerminalAction::RunCommand(command)),
                _,
                _,
                _,
                placement,
                Some(mut completion),
                _,
            ) = instruction
            else {
                panic!("expected an agent command pane");
            };
            assert_eq!(
                placement,
                ClientTabIndexOrPaneId::ClientIdNoFocus(crate::ClientId::new(1))
            );
            assert_eq!(
                command.cwd,
                Some(worker_root.join(".worktrees").join(branch))
            );
            assert!(command.originating_plugin.is_none());
            assert_eq!(command.command, PathBuf::from("env"));
            assert!(command
                .args
                .contains(&format!("{}=1", yinz_types::agent::AGENT_PANE_ENV)));
            assert_eq!(
                yinz_types::agent::agent_kind_of(&command.command, &command.args).as_deref(),
                Some("codex")
            );
            completion.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(pane)));
        }
    });
    for (pane, branch) in [(7, "review"), (8, "review-2")] {
        let spawned = spawn_agent_for_cli(
            AgentSpawnSpec {
                kind: Some("codex".into()),
                branch: Some("review".into()),
                ..Default::default()
            },
            Some(root.clone()),
            None,
            None,
            false,
            crate::ClientId::new(1),
            &senders,
            &data_access,
        )
        .unwrap();
        assert_eq!(
            spawned.pane_id,
            PaneId::Terminal(crate::TerminalId::new(pane))
        );
        assert_eq!(spawned.branch.as_deref(), Some(branch));
        assert_eq!(spawned.cwd, Some(root.join(".worktrees").join(branch)));
    }
    worker.join().unwrap();
    let refused = spawn_agent_for_cli(
        AgentSpawnSpec {
            kind: Some("codex".into()),
            ..Default::default()
        },
        Some(root.clone()),
        None,
        None,
        false,
        crate::ClientId::new(1),
        &senders,
        &data_access,
    )
    .unwrap_err();
    assert!(refused.contains("not a linked git worktree"));
    let refused = spawn_agent_for_cli(
        AgentSpawnSpec {
            kind: Some("zsh".into()),
            branch: Some("invalid".into()),
            ..Default::default()
        },
        Some(root.clone()),
        None,
        None,
        false,
        crate::ClientId::new(1),
        &senders,
        &data_access,
    )
    .unwrap_err();
    assert!(refused.contains("recognized agent command"));
    assert!(!root.join(".worktrees/invalid").exists());
}

#[test]
fn automation_spawn_rejects_prompts_and_local_options_for_ssh_before_opening_a_pane() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    for field in ["prompt", "reuse_prompt"] {
        let spec =
            serde_json::from_value(serde_json::json!({"kind":"codex",field:"work"})).unwrap();
        let error = spawn_agent_for_cli(
            spec,
            None,
            None,
            None,
            false,
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        )
        .unwrap_err();
        assert!(error.contains("spawn does not accept"));
    }
    for input in [
        serde_json::json!({"kind":"codex"}),
        serde_json::json!({"kind":"codex","allow_outside_worktree":true,"branch":"topic"}),
        serde_json::json!({"kind":"codex","allow_outside_worktree":true,"firecracker":"vm"}),
    ] {
        let mut spec = serde_json::from_value(input).unwrap();
        assert!(agent_control::prepare_automation_agent(
            &mut spec,
            Some("/remote".into()),
            None,
            Some("server".into()),
            false,
            &data
        )
        .is_err());
    }
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        allow_outside_worktree: Some(true),
        ..Default::default()
    };
    assert!(agent_control::prepare_automation_agent(
        &mut spec,
        Some("relative".into()),
        None,
        Some("server".into()),
        false,
        &data
    )
    .is_err());
    let prepared = agent_control::prepare_automation_agent(
        &mut spec,
        Some("/remote/not-on-this-host".into()),
        None,
        Some("server".into()),
        false,
        &data,
    )
    .unwrap();
    assert!(prepared.command.target.is_remote());
    assert_eq!(
        prepared.command.cwd,
        Some("/remote/not-on-this-host".into())
    );
}

#[test]
fn agent_spawn_registration_failure_retains_created_pane_and_checkout() {
    let (tx, rx) = channels::unbounded();
    drop(rx);
    let senders = ThreadSenders {
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(tx)),
        ..Default::default()
    };
    let spawned = yinz_types::agent::SpawnedAgent {
        pane_id: yinz_types::pane_id::PaneId::Terminal(TerminalId::new(17)),
        cwd: Some("/actual/checkout".into()),
        branch: Some("actual-branch".into()),
        ..Default::default()
    };
    let response = agent_control::register_spawned_agent(&senders, spawned.clone());
    assert_eq!(
        response.outcome,
        yinz_types::agent::control::AgentOutcome::Failed
    );
    assert_eq!(response.spawned, Some(spawned));
    assert!(response
        .message
        .unwrap()
        .contains("launch registration failed"));
}

#[test]
fn plugin_agent_control_resolves_only_local_relative_directories() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_config.location = yinz_types::input::layout::RunPluginLocation::Yinz(
        yinz_types::data::PluginTag::new("hocus"),
    );
    env.plugin_cwd = root.clone();
    env.data_access = std::sync::Arc::new(DataAccess::sqlite(&root.join("data")));
    let (to_pty, pty_rx) = channels::unbounded();
    let (to_plugin, plugin_rx) = channels::unbounded();
    env.senders = ThreadSenders {
        to_pty: Some(SenderWithContext::new(to_pty)),
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        ..Default::default()
    }
    .silently_fail_on_send();

    for (cwd, remote_host, expected) in [
        (Some(PathBuf::from(".")), None, Some(root.clone())),
        (Some(PathBuf::from("child")), None, Some(child)),
        (Some(root.clone()), None, Some(root.clone())),
        (None, None, Some(root.clone())),
        (
            Some(PathBuf::from("/host/remote")),
            Some("server".to_owned()),
            Some(PathBuf::from("/host/remote")),
        ),
        (Some(PathBuf::from(".")), Some("server".to_owned()), None),
    ] {
        let request = AgentRequest::Spawn {
            spec: AgentSpawnSpec {
                kind: Some("codex".into()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            },
            cwd,
            project: None,
            remote_host: remote_host.clone(),
            automatic_firecracker_vm: false,
        };
        run_plugin_command(
            &mut env,
            PluginCommand::AgentControl {
                id: "test".into(),
                request: serde_json::to_string(&request).unwrap(),
            },
        )
        .unwrap();
        if let Some(expected) = &expected {
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(
                Some(TerminalAction::RunCommand(command)),
                _,
                _,
                _,
                _,
                Some(mut completion),
                _,
            ) = instruction
            else {
                panic!("expected agent spawn");
            };
            assert_eq!(command.cwd.as_ref(), Some(expected));
            assert_eq!(
                command.target,
                match remote_host {
                    Some(host) => yinz_types::command::PaneTarget::ssh(&host).unwrap(),
                    None => yinz_types::command::PaneTarget::Local,
                }
            );
            completion.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(7)));
        }
        let (instruction, _) = plugin_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let PluginInstruction::Update(events) = instruction else {
            panic!("expected agent response");
        };
        let Event::CustomMessage(name, payload) = &events[0].2 else {
            panic!("expected custom message");
        };
        assert_eq!(name, "agent-response");
        let payload: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            payload["response"]["outcome"],
            serde_json::to_value(if expected.is_some() {
                AgentOutcome::Ok
            } else {
                AgentOutcome::Rejected
            })
            .unwrap(),
            "{payload}"
        );
        if expected.is_none() {
            assert!(payload["response"]["message"]
                .as_str()
                .unwrap()
                .contains("SSH cwd must be absolute"));
            assert!(pty_rx.try_recv().is_err());
        }
    }
}
