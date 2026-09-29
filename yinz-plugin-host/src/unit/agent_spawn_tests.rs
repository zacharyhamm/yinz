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
                ClientTabIndexOrPaneId::ClientId(crate::ClientId::new(1))
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
            root.clone(),
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
        root.clone(),
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
        root.clone(),
        crate::ClientId::new(1),
        &senders,
        &data_access,
    )
    .unwrap_err();
    assert!(refused.contains("recognized agent command"));
    assert!(!root.join(".worktrees/invalid").exists());
}
