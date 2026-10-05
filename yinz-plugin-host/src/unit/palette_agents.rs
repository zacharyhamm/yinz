use super::*;
use std::sync::{atomic::AtomicBool, Arc};
use yinz_runtime::channels::{unbounded, SenderWithContext};
use yinz_types::internal_ids::{ClientId, TabId};

fn repository() -> (tempfile::TempDir, DataAccess) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-b", "main"]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let data = DataAccess::sqlite(&temp.path().join("data"));
    for definition in hocus_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(hocus_data::store::SHARED, "test", &definition, false)
            .unwrap();
    }
    let mut project =
        hocus_data::projects::validate("project", temp.path().to_str().unwrap()).unwrap();
    project.remote = Some("git@example.com:team/repository.git".into());
    project.worktree_dir = Some("custom-checkouts".into());
    project.firecracker_image = Some("rust-dev".into());
    data.plugin_data()
        .apply(
            hocus_data::store::SHARED,
            &[hocus_data::projects::to_mutation(&project)],
        )
        .unwrap();
    (temp, data)
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn request(root: &Path, action: AgentAction) -> AgentRequest {
    AgentRequest {
        client: ClientId::new(1),
        opening: 1,
        step: 1,
        source: PaneInfo {
            id: PaneId::Terminal(TerminalId::new(1)),
            cwd: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        },
        action,
        value: None,
        active: Arc::new(AtomicBool::new(true)),
    }
}

fn run(request: &AgentRequest, data: &DataAccess) -> (Result<AgentReply, String>, Vec<RunCommand>) {
    let (screen_tx, screen_rx) = unbounded();
    let (pty_tx, pty_rx) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(screen_tx)),
        to_pty: Some(SenderWithContext::new(pty_tx)),
        ..Default::default()
    };
    thread::scope(|scope| {
        let source = request.source.clone();
        scope.spawn(move || {
            while let Ok((instruction, _)) = screen_rx.recv() {
                match instruction {
                    ScreenInstruction::GetPaneInfo {
                        response_channel, ..
                    } => {
                        response_channel.send(Some(source.clone())).unwrap();
                    },
                    ScreenInstruction::AgentSpawned(_) => {},
                    _ => panic!("unexpected screen instruction {instruction:?}"),
                }
            }
        });
        let worker = scope.spawn(move || {
            let mut commands = Vec::new();
            while let Ok((instruction, _)) = pty_rx.recv() {
                let PtyInstruction::SpawnTerminal(
                    Some(TerminalAction::RunCommand(command)),
                    _,
                    _,
                    _,
                    target,
                    Some(mut completion),
                    _,
                ) = instruction
                else {
                    panic!("unexpected pty instruction");
                };
                assert_eq!(
                    target,
                    ClientTabIndexOrPaneId::ClientIdNoFocus(ClientId::new(1))
                );
                commands.push(command);
                completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(42)));
                completion.set_affected_tab_id(TabId::new(2));
            }
            commands
        });
        let reply = execute(request, &senders, data);
        drop(senders);
        (reply, worker.join().unwrap())
    })
}

#[test]
fn palette_projects_worktrees_creation_and_exact_external_detached_reuse() {
    let (repo, data) = repository();
    let external = tempfile::tempdir().unwrap();
    let checkout = external.path().join("detached");
    git(
        repo.path(),
        &["worktree", "add", "--detach", checkout.to_str().unwrap()],
    );
    let project = AgentProject::Registered("project".into());
    let start = request(
        repo.path(),
        AgentAction::Start {
            kind: "claude".into(),
            vm: false,
            project: Some("project".into()),
        },
    );
    let (reply, commands) = run(&start, &data);
    assert!(commands.is_empty());
    let AgentReply::Choices { title, choices } = reply.unwrap() else {
        panic!()
    };
    assert!(title.contains("project"));
    assert!(choices
        .iter()
        .any(|c| c.label == "Detached HEAD" && c.description == checkout.to_string_lossy()));
    assert!(choices.iter().any(|c| c.label == "Change project…"));
    let selected = choices
        .into_iter()
        .find(|c| c.label == "Detached HEAD")
        .unwrap();
    let (reply, commands) = run(&request(repo.path(), selected.action), &data);
    assert!(
        matches!(&reply, Ok(AgentReply::Launched(r)) if r.outcome == AgentOutcome::Ok),
        "{reply:?}"
    );
    assert_eq!(commands[0].cwd, Some(checkout.canonicalize().unwrap()));
    assert!(commands[0].args.iter().any(|a| a == "claude"));
    let mut create = request(
        repo.path(),
        AgentAction::Checkout {
            kind: "codex".into(),
            vm: false,
            project,
            path: None,
        },
    );
    assert!(matches!(
        run(&create, &data).0.unwrap(),
        AgentReply::Form { .. }
    ));
    create.value = Some(String::new());
    assert!(run(&create, &data).0.unwrap_err().contains("required"));
    create.value = Some("review".into());
    let (reply, commands) = run(&create, &data);
    assert!(
        matches!(&reply, Ok(AgentReply::Launched(r)) if r.outcome == AgentOutcome::Ok),
        "{reply:?}"
    );
    assert_eq!(
        commands[0].cwd,
        Some(
            repo.path()
                .join("custom-checkouts/review")
                .canonicalize()
                .unwrap()
        )
    );
    let (reply, _) = run(
        &request(
            repo.path(),
            AgentAction::Projects {
                kind: "claude".into(),
                vm: false,
            },
        ),
        &data,
    );
    assert!(
        matches!(reply.unwrap(), AgentReply::Choices { choices, .. } if choices.iter().any(|c| c.label == "Caller’s repository"))
    );
    let (reply, commands) = run(
        &request(
            repo.path(),
            AgentAction::Worktrees {
                kind: "claude".into(),
                vm: false,
                project: AgentProject::Registered("missing".into()),
            },
        ),
        &data,
    );
    assert!(reply.unwrap_err().contains("no longer exists"));
    assert!(commands.is_empty());
}

#[test]
fn palette_worktree_base_prefers_main_and_falls_back_to_primary_head() {
    for base in ["main", "master", "trunk", "detached"] {
        for vm in [false, true] {
            let (repo, data) = repository();
            match base {
                "main" => {
                    git(repo.path(), &["switch", "-c", "feature"]);
                    git(
                        repo.path(),
                        &[
                            "-c",
                            "user.name=Test",
                            "-c",
                            "user.email=test@example.com",
                            "commit",
                            "--allow-empty",
                            "-m",
                            "feature",
                        ],
                    );
                },
                "detached" => {
                    git(repo.path(), &["switch", "--detach"]);
                    git(repo.path(), &["branch", "-D", "main"]);
                },
                branch => {
                    git(repo.path(), &["branch", "-m", branch]);
                },
            }
            let expected = git(
                repo.path(),
                &["rev-parse", if base == "main" { "main" } else { "HEAD" }],
            );
            let project = if vm {
                AgentProject::Registered("project".into())
            } else {
                AgentProject::Repository(repo.path().to_owned())
            };
            let mut create = request(
                repo.path(),
                AgentAction::Checkout {
                    kind: "codex".into(),
                    vm,
                    project,
                    path: None,
                },
            );
            create.value = Some("review".into());
            let (reply, commands) = run(&create, &data);
            assert!(
                matches!(&reply, Ok(AgentReply::Launched(r)) if r.outcome == AgentOutcome::Ok),
                "base={base}, vm={vm}: {reply:?}"
            );
            assert_eq!(commands.len(), 1);
            assert_eq!(git(repo.path(), &["rev-parse", "review"]), expected);
        }
    }
}

#[test]
fn palette_current_directory_vm_defaults_and_fresh_duplicate_targets() {
    let (repo, data) = repository();
    let current = request(
        repo.path(),
        AgentAction::Current {
            kind: "codex".into(),
        },
    );
    let (reply, commands) = run(&current, &data);
    assert!(!commands.is_empty(), "{reply:?}");
    assert_eq!(commands[0].cwd.as_deref(), Some(repo.path()));
    let vm = request(
        repo.path(),
        AgentAction::Checkout {
            kind: "claude".into(),
            vm: true,
            project: AgentProject::Registered("project".into()),
            path: Some(repo.path().to_owned()),
        },
    );
    let (reply, commands) = run(&vm, &data);
    assert!(!commands.is_empty(), "{reply:?}");
    assert!(
        matches!(&commands[0].target, PaneTarget::Firecracker { image: Some(image), worktree: Some(path), .. } if image == "rust-dev" && path == repo.path())
    );
    for target in [
        PaneTarget::Local,
        PaneTarget::Ssh {
            host: "host".into(),
        },
        commands[0].target.clone(),
    ] {
        let mut duplicate = request(
            repo.path(),
            AgentAction::Duplicate {
                kind: Some("claude".into()),
            },
        );
        duplicate.source.target = target.clone();
        duplicate.source.agent = Some(
            serde_json::from_value(
                serde_json::json!({"kind": "codex", "state": "Idle", "cwd": duplicate.source.cwd}),
            )
            .unwrap(),
        );
        let (reply, commands) = run(&duplicate, &data);
        assert!(!commands.is_empty(), "{reply:?}");
        assert_eq!(commands[0].target, target);
        assert_eq!(commands[0].cwd.as_deref(), Some(repo.path()));
        assert!(commands[0].args.iter().any(|arg| arg == "claude"));
        assert!(!commands[0]
            .args
            .iter()
            .any(|arg| arg == "--resume" || arg == "--continue"));
        assert_eq!(
            commands[0].args.iter().any(|arg| arg == "--settings"),
            target.is_local()
        );
    }
    let cancelled = request(
        repo.path(),
        AgentAction::Current {
            kind: "claude".into(),
        },
    );
    cancelled.active.store(false, atomic::Ordering::Release);
    let (reply, commands) = run(&cancelled, &data);
    assert!(reply.unwrap_err().contains("cancelled"));
    assert!(commands.is_empty());
}
