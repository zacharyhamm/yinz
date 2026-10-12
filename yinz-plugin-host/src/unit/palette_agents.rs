use super::*;
use std::sync::{atomic::AtomicBool, Arc};
use yinz_runtime::channels::{unbounded, SenderWithContext};
use yinz_types::internal_ids::{ClientId, HostId, TabId};

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
    for definition in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &definition, false)
            .unwrap();
    }
    let mut project =
        project_data::projects::validate("project", temp.path().to_str().unwrap()).unwrap();
    project.remote = Some("git@example.com:team/repository.git".into());
    project.worktree_dir = Some("custom-checkouts".into());
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
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
        let is_mother = matches!(request.action, AgentAction::Mother { .. });
        let worker = scope.spawn(move || {
            let mut commands = Vec::new();
            while let Ok((instruction, _)) = pty_rx.recv() {
                let PtyInstruction::SpawnTerminal(
                    Some(TerminalAction::RunCommand(command)),
                    _,
                    placement,
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
                if is_mother {
                    assert!(matches!(placement, NewPanePlacement::Overlay(_)));
                } else {
                    assert_eq!(placement, TILED);
                }
                assert_eq!(completion.is_mother, is_mother);
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
            project: Some("project".into()),
        },
    );
    let (reply, commands) = run(&start, &data);
    assert!(commands.is_empty());
    let AgentReply::Choices { title, choices } = reply.unwrap() else {
        panic!()
    };
    assert!(title.contains("project"));
    assert!(choices.iter().any(|c| c.label == "Detached HEAD"
        && c.description == checkout.canonicalize().unwrap().to_string_lossy()));
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
        for registered in [false, true] {
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
            let project = if registered {
                AgentProject::Registered("project".into())
            } else {
                AgentProject::Repository(repo.path().to_owned())
            };
            let mut create = request(
                repo.path(),
                AgentAction::Checkout {
                    kind: "codex".into(),
                    project,
                    path: None,
                },
            );
            create.value = Some("review".into());
            let (reply, commands) = run(&create, &data);
            assert!(
                matches!(&reply, Ok(AgentReply::Launched(r)) if r.outcome == AgentOutcome::Ok),
                "base={base}, registered={registered}: {reply:?}"
            );
            assert_eq!(commands.len(), 1);
            assert_eq!(git(repo.path(), &["rev-parse", "review"]), expected);
        }
    }
}

#[test]
fn palette_current_directory_and_fresh_duplicate_targets() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let (repo, data) = repository();
    let storage = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", storage.path());
    let metadata = yinz_runtime::home::xdg_data_dir()
        .join("bubblewrap/environments")
        .join(yinz_config::project_config::root_key(
            &repo.path().canonicalize().unwrap(),
        ));
    fs::create_dir_all(&metadata).unwrap();
    fs::write(metadata.join("accepted.json"), serde_json::to_vec(&serde_json::json!({
        "root": repo.path().canonicalize().unwrap(), "ready": true,
        "declaration": {"mounts": [], "paths": [], "env": {}, "inherit": [], "seeds": [], "setup": [], "checks": []}
    })).unwrap()).unwrap();
    let current = request(
        repo.path(),
        AgentAction::Current {
            kind: "codex".into(),
        },
    );
    let (reply, commands) = run(&current, &data);
    assert!(!commands.is_empty(), "{reply:?}");
    assert_eq!(commands[0].cwd.as_deref(), Some(repo.path()));
    for target in [
        PaneTarget::Local,
        PaneTarget::Ssh {
            host: "host".into(),
        },
        // A sandboxed agent's duplicate is sandboxed: the target is the sandbox.
        PaneTarget::Bubblewrap {
            root: repo.path().to_owned(),
        },
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
            !target.is_remote()
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

fn claude_mother_fixture(home: &Path) {
    std::env::set_var("HOME", home);
    std::env::set_var("XDG_DATA_HOME", home.join("state"));
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join("claude"));
    fs::create_dir_all(home.join("claude")).unwrap();
    fs::write(home.join("claude/.claude.json"), "{}").unwrap();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    yinz_test_support::write_warm_script(&bin.join("bwrap"), "echo 'bubblewrap 0.10.0'");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
}

#[test]
fn unsupported_mother_providers_do_not_provision_or_spawn() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let start = tempfile::tempdir().unwrap();
    let cache = start.path().join("cache");
    std::env::set_var("YINZ_TEST_CACHE_ROOT", &cache);
    std::env::set_var("YINZ_SESSION_NAME", "unsupported-mother");
    let data = DataAccess::sqlite(&start.path().join("data"));
    yinz_types::agent::provider::register(
        9182,
        yinz_types::agent::provider::AgentProvider {
            kind: "mother-test-provider".into(),
            executable: "custom-agent".into(),
            ..Default::default()
        },
    )
    .unwrap();
    for kind in [
        "muse",
        "agy",
        "antigravity",
        "cursor",
        "Cursor-Agent",
        "mother-test-provider",
        "unknown",
    ] {
        let request = request(start.path(), AgentAction::Mother { kind: kind.into() });
        let (screen_tx, screen_rx) = unbounded();
        let (pty_tx, pty_rx) = unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_pty: Some(SenderWithContext::new(pty_tx)),
            ..Default::default()
        };
        assert_eq!(
            execute(&request, &senders, &data).unwrap_err(),
            "Mother supports only Claude, Codex, and Pi"
        );
        assert!(screen_rx.is_empty());
        assert!(pty_rx.is_empty());
        assert!(
            !cache.exists(),
            "unsupported Mother provisioned a directory"
        );
    }
    yinz_types::agent::provider::unregister_owner(9182);
}

#[test]
fn mother_launches_as_an_overlay_in_its_private_directory_without_a_source_pane() {
    if yinz_test_support::reexec_test() {
        return;
    }
    std::env::set_var("YINZ_SESSION_NAME", "mother-test");
    let start = tempfile::Builder::new()
        .prefix("mother space \"quote\".dot")
        .tempdir()
        .unwrap();
    claude_mother_fixture(start.path());
    std::env::set_var("YINZ_TEST_CACHE_ROOT", start.path());
    assert!(yinz_runtime::consts::YINZ_CACHE_DIR.starts_with(start.path()));
    let cwd = yinz_config::project_config::bubblewrap::mother_directory(
        &yinz_runtime::consts::YINZ_CACHE_DIR,
        "mother-test",
    )
    .unwrap();
    let data = DataAccess::sqlite(&start.path().join("data"));
    let launch = |active: bool, kind: &str, expected_title: &str| {
        // The pane the palette opened from is somewhere else, and may be gone by now.
        let request = request(
            Path::new("/nonexistent/source"),
            AgentAction::Mother { kind: kind.into() },
        );
        request.active.store(active, atomic::Ordering::Release);
        let (screen_tx, screen_rx) = unbounded();
        let (pty_tx, pty_rx) = unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_pty: Some(SenderWithContext::new(pty_tx)),
            ..Default::default()
        };
        let (reply, launches) = thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let mut launches = Vec::new();
                while let Ok((instruction, _)) = pty_rx.recv() {
                    let PtyInstruction::SpawnTerminal(
                        Some(TerminalAction::RunCommand(command)),
                        title,
                        placement,
                        _,
                        target,
                        Some(mut completion),
                        _,
                    ) = instruction
                    else {
                        panic!("unexpected pty instruction");
                    };
                    completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(42)));
                    completion.set_affected_tab_id(TabId::new(2));
                    assert!(completion.is_mother);
                    assert_eq!(title.as_deref(), Some(expected_title));
                    launches.push((command, placement, target));
                }
                launches
            });
            let reply = execute(&request, &senders, &data);
            drop(senders);
            (reply, worker.join().unwrap())
        });
        let screen: Vec<_> = screen_rx.try_iter().map(|(i, _)| i).collect();
        (reply, launches, screen)
    };

    let (reply, launches, screen) = launch(true, "claude", "mother: claude");
    assert!(
        matches!(&reply, Ok(AgentReply::Launched(response)) if response.outcome == AgentOutcome::Ok),
        "{reply:?}"
    );
    let [(command, placement, target)] = launches.as_slice() else {
        panic!("expected one launch, got {launches:?}");
    };
    assert_eq!(command.cwd.as_deref(), Some(cwd.as_path()));
    assert_eq!(command.target.is_bubblewrap(), cfg!(target_os = "linux"));
    assert!(
        matches!(placement, NewPanePlacement::Overlay(overlay) if !overlay.is_modal() && overlay.coordinates.is_some()),
        "{placement:?}"
    );
    assert_eq!(
        *target,
        ClientTabIndexOrPaneId::ClientIdNoFocus(ClientId::new(1))
    );
    assert!(!screen
        .iter()
        .any(|instruction| matches!(instruction, ScreenInstruction::GetPaneInfo { .. })));

    if cfg!(target_os = "linux") {
        assert!(command
            .target
            .bubblewrap_root()
            .unwrap()
            .ends_with("profile-claude"));
        std::env::set_var("CLAUDE_CONFIG_DIR", start.path().join("missing"));
        let (reply, launches, _) = launch(true, "claude", "mother: claude");
        assert!(
            matches!(reply, Ok(AgentReply::Launched(response)) if response.outcome == AgentOutcome::Rejected && response.message.as_ref().unwrap().contains("Claude"))
        );
        assert!(launches.is_empty());
        std::env::set_var("CLAUDE_CONFIG_DIR", start.path());
        let (reply, launches, _) = launch(true, "claude", "mother: claude");
        assert!(
            matches!(reply, Ok(AgentReply::Launched(response)) if response.outcome == AgentOutcome::Rejected)
        );
        assert!(launches.is_empty());
        std::env::set_var("CLAUDE_CONFIG_DIR", start.path().join("claude"));
    }

    for (kind, title) in [(" CODEX ", "mother: codex"), ("Pi", "mother: pi")] {
        let (reply, launches, _) = launch(true, kind, title);
        assert!(
            matches!(reply, Ok(AgentReply::Launched(response)) if response.outcome == AgentOutcome::Ok)
        );
        assert_eq!(launches.len(), 1);
        let command = &launches[0].0;
        let ordinary = existing_command(
            kind.trim().to_lowercase().as_str(),
            cwd.to_str().unwrap(),
            PaneTarget::Local,
        )
        .unwrap();
        assert!(!ordinary
            .command
            .args
            .iter()
            .any(|arg| arg == "--approve" || arg.contains("trust_level")));
        let expected = if kind.trim().eq_ignore_ascii_case("codex") {
            format!(
                "projects={{{}={{trust_level=\"trusted\"}}}}",
                serde_json::to_string(cwd.to_str().unwrap()).unwrap()
            )
        } else {
            "--approve".to_owned()
        };
        assert!(command.args.contains(&expected), "{:?}", command.args);
        let mut fallback = ordinary;
        fallback.command.args.clone_from(&command.args);
        mother_target(
            &mut fallback,
            Err(anyhow::anyhow!("namespace setup failed")),
        )
        .unwrap();
        assert!(
            fallback.command.args.contains(&expected),
            "{:?}",
            fallback.command.args
        );
        if kind.trim().eq_ignore_ascii_case("codex") {
            assert!(launches[0]
                .0
                .args
                .iter()
                .any(|arg| arg == "sandbox_workspace_write.network_access=true"));
        }
    }
    let (reply, launches, screen) = launch(false, "claude", "mother: claude");
    assert!(reply.unwrap_err().contains("cancelled"));
    assert!(launches.is_empty());
    assert!(screen.is_empty());
}

fn recorded_run(root: &Path) -> agent_proto::agents::AgentRun {
    agent_proto::agents::AgentRun {
        id: "codex:archived".into(),
        pane_id: 7,
        yinz_session_name: "old-session".into(),
        host_id: yinz_runtime::host_identity::host_id(),
        origin_run_id: None,
        previous_run_id: Some("previous".into()),
        session_id: "archived".into(),
        kind: "codex".into(),
        cwd: Some(root.to_string_lossy().into_owned()),
        host: None,
        bubblewrap: None,
        started_at_ms: 100,
        ended_at_ms: Some(200),
        outcome: agent_proto::agents::RunOutcome::Completed,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        omitted_entries: 0,
        entries: vec![agent_proto::agents::RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: agent_proto::agents::RunEntryKind::Message,
            text: format!("needle\n{}", "日本語 retained text ".repeat(4000)),
            created_at_ms: Some(150),
        }],
    }
}

#[test]
fn palette_history_search_exact_event_predecessor_and_resume() {
    use yinz_bus::command_palette::{HistoryAction, HistoryReply};
    let (temp, data) = repository();
    let recorded = recorded_run(temp.path());
    let write = |recorded: &agent_proto::agents::AgentRun| {
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &agent_proto::agents::rows::run_mutations(recorded),
            )
            .unwrap();
    };
    write(&recorded);
    let execute = |action| run(&request(temp.path(), AgentAction::History(action)), &data);
    let (reply, commands) = execute(HistoryAction::Search(
        yinz_bus::command_palette::query::AgentQuery::parse(
            "needle provider:codex session:old-session",
            yinz_bus::command_palette::query::ARCHIVE_FIELDS,
        )
        .unwrap(),
    ));
    let AgentReply::History(HistoryReply::Rows { rows, .. }) = reply.unwrap() else {
        panic!("expected search rows")
    };
    assert_eq!(rows.len(), 1);
    assert!(commands.is_empty());
    let (reply, _) = execute(rows[0].action.clone());
    let AgentReply::History(HistoryReply::Run { rows, events, .. }) = reply.unwrap() else {
        panic!("expected timeline")
    };
    assert!(events[0].markdown.starts_with("needle\n"));
    assert!(events[0].truncated);
    assert!(events[0].markdown.chars().count() <= agent_proto::run_history::ENTRY_TEXT_CHARS + 1);
    assert!(rows
        .iter()
        .any(|r| matches!(&r.action, HistoryAction::Run { id, .. } if id == "previous")));
    let (reply, _) = execute(HistoryAction::Event {
        id: recorded.id.clone(),
        revision: 1,
    });
    let AgentReply::History(HistoryReply::Reader { markdown: text, .. }) = reply.unwrap() else {
        panic!("expected retained event")
    };
    assert!(text.contains(&recorded.entries[0].text));
    let (reply, commands) = execute(HistoryAction::Resume {
        id: recorded.id.clone(),
    });
    assert!(matches!(reply.unwrap(), AgentReply::Launched(_)));
    assert_eq!(commands.len(), 1);
    assert!(commands[0].args.iter().any(|arg| arg == "archived"));
    assert_eq!(commands[0].cwd.as_deref(), Some(temp.path()));
    assert_eq!(commands[0].target, PaneTarget::Local);
    // A run recorded in a Bubblewrap environment resumes inside it, where its conversation is.
    let mut sandboxed = recorded.clone();
    sandboxed.bubblewrap = Some(temp.path().to_string_lossy().into_owned());
    write(&sandboxed);
    let (reply, commands) = execute(HistoryAction::Resume {
        id: recorded.id.clone(),
    });
    assert!(matches!(reply.unwrap(), AgentReply::Launched(_)));
    assert_eq!(
        commands[0].target,
        PaneTarget::Bubblewrap {
            root: temp.path().to_owned()
        }
    );
    write(&recorded);
    for (outcome, host, host_id, kind, session, directory, error) in [
        (
            agent_proto::agents::RunOutcome::Active,
            None,
            recorded.host_id.clone(),
            "codex",
            "archived",
            Some(temp.path().to_string_lossy().into_owned()),
            "active",
        ),
        (
            agent_proto::agents::RunOutcome::Completed,
            None,
            yinz_types::internal_ids::HostId::new("foreign"),
            "codex",
            "archived",
            recorded.cwd.clone(),
            "foreign host",
        ),
        (
            agent_proto::agents::RunOutcome::Completed,
            None,
            recorded.host_id.clone(),
            "codex",
            "",
            recorded.cwd.clone(),
            "session id",
        ),
        (
            agent_proto::agents::RunOutcome::Completed,
            None,
            recorded.host_id.clone(),
            "codex",
            "archived",
            None,
            "directory",
        ),
        (
            agent_proto::agents::RunOutcome::Completed,
            None,
            recorded.host_id.clone(),
            "gemini",
            "archived",
            recorded.cwd.clone(),
            "resume",
        ),
    ] {
        let mut changed = recorded.clone();
        changed.outcome = outcome;
        changed.host = host;
        changed.host_id = host_id;
        changed.kind = kind.into();
        changed.session_id = session.into();
        changed.cwd = directory;
        write(&changed);
        let (reply, commands) = execute(HistoryAction::Resume {
            id: recorded.id.clone(),
        });
        assert!(reply.unwrap_err().contains(error));
        assert!(commands.is_empty());
    }
}

#[test]
fn palette_history_older_follows_retained_revisions() {
    use yinz_bus::command_palette::{HistoryAction, HistoryReply};
    for (revisions, text, expected_starts) in [
        (
            vec![1].into_iter().chain(100..130).collect::<Vec<_>>(),
            "event",
            vec![100, 1],
        ),
        (
            (1..5).chain(100..130).chain(1000..1030).collect(),
            "event",
            vec![1000, 100, 1],
        ),
        ((1..=65).collect(), "event", vec![36, 6, 1]),
        ((1..=65).collect(), &"x".repeat(4000), vec![52, 22, 1]),
    ] {
        let (temp, data) = repository();
        let mut recorded = recorded_run(temp.path());
        let entry = recorded.entries[0].clone();
        recorded.entries = revisions
            .iter()
            .map(|revision| {
                let mut entry = entry.clone();
                entry.revision = *revision;
                entry.text = text.into();
                entry
            })
            .collect();
        recorded.revision = *revisions.last().unwrap();
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &agent_proto::agents::rows::run_mutations(&recorded),
            )
            .unwrap();
        // Other runs must not contribute revisions to the backward cursor.
        let mut other = recorded.clone();
        other.id = "other-run".into();
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &agent_proto::agents::rows::run_mutations(&other),
            )
            .unwrap();
        let mut action = HistoryAction::Run {
            id: recorded.id.clone(),
            from: None,
            query: String::new(),
        };
        let mut starts = Vec::new();
        loop {
            let (reply, _) = run(&request(temp.path(), AgentAction::History(action)), &data);
            let AgentReply::History(HistoryReply::Run { rows, .. }) = reply.unwrap() else {
                panic!("expected timeline")
            };
            starts.push(
                rows.iter()
                    .find_map(|row| match row.action {
                        HistoryAction::Event { revision, .. } => Some(revision),
                        _ => None,
                    })
                    .unwrap(),
            );
            assert!(
                starts.len() <= expected_starts.len(),
                "backward navigation failed to terminate: {starts:?}"
            );
            let Some(older) = rows.iter().find(|row| row.id == "older") else {
                break;
            };
            action = older.action.clone();
        }
        assert_eq!(starts, expected_starts);
    }
}

#[test]
fn palette_history_filters_match_the_same_runs_with_or_without_text() {
    use yinz_bus::command_palette::{
        query::{AgentQuery, ARCHIVE_FIELDS},
        HistoryAction, HistoryReply,
    };
    let (temp, data) = repository();
    let mut recorded = recorded_run(temp.path());
    recorded.id = "Codex:Archived".into();
    recorded.kind = "Codex".into();
    recorded.yinz_session_name = "Main".into();
    recorded.host_id = HostId::new("Host-A");
    let mut legacy = recorded.clone();
    legacy.id = "Codex:Legacy".into();
    legacy.host_id = HostId::new("");
    let mut active = recorded.clone();
    active.id = "Codex:Active".into();
    active.outcome = agent_proto::agents::RunOutcome::Active;
    for record in [&recorded, &legacy, &active] {
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &agent_proto::agents::rows::run_mutations(record),
            )
            .unwrap();
    }
    for (filters, expected) in [
        ("session:main", vec!["Codex:Archived", "Codex:Legacy"]),
        ("provider:codex", vec!["Codex:Archived", "Codex:Legacy"]),
        ("host:host-a", vec!["Codex:Archived"]),
        ("run:codex:archived", vec!["Codex:Archived"]),
        (
            "session:main host:host-a",
            vec!["Codex:Archived", "Codex:Legacy"],
        ),
        ("session:other", vec![]),
        ("provider:claude", vec![]),
        ("host:host-b", vec![]),
        ("run:absent", vec![]),
        ("since:101", vec![]),
    ] {
        for text in ["", "needle"] {
            let query = AgentQuery::parse(&format!("{text} {filters}"), ARCHIVE_FIELDS).unwrap();
            let (reply, commands) = run(
                &request(
                    temp.path(),
                    AgentAction::History(HistoryAction::Search(query)),
                ),
                &data,
            );
            let AgentReply::History(HistoryReply::Rows { rows, .. }) = reply.unwrap() else {
                panic!("expected search rows")
            };
            let mut ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
            ids.sort_unstable();
            assert_eq!(ids, expected, "{text:?} {filters}");
            assert!(commands.is_empty());
        }
    }
}

#[test]
fn palette_history_run_search_orders_events_and_keeps_time_and_hit_navigation() {
    use agent_proto::agents::{RunEntry, RunEntryKind};
    use yinz_bus::command_palette::{HistoryAction, HistoryReply};
    let (temp, data) = repository();
    let mut recorded = recorded_run(temp.path());
    recorded.revision = 3;
    recorded.entries = (1..=3)
        .map(|revision| RunEntry {
            revision,
            event_id: None,
            kind: RunEntryKind::Message,
            text: format!(
                "**{}**\n\n- retained Markdown",
                "needle ".repeat(revision as usize)
            ),
            created_at_ms: Some(150),
            tool: None,
        })
        .collect();
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &agent_proto::agents::rows::run_mutations(&recorded),
        )
        .unwrap();
    for query in ["", "needle"] {
        let (reply, _) = run(
            &request(
                temp.path(),
                AgentAction::History(HistoryAction::Run {
                    id: recorded.id.clone(),
                    from: None,
                    query: query.into(),
                }),
            ),
            &data,
        );
        let AgentReply::History(HistoryReply::Run {
            header,
            events,
            rows,
            ..
        }) = reply.unwrap()
        else {
            panic!("expected timeline")
        };
        assert_eq!(
            events.iter().map(|e| e.revision).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(header.started, "1970-01-01 00:00 UTC");
        assert!(events.iter().all(|e| e.time == "1970-01-01 00:00 UTC"));
        if query.is_empty() {
            assert_eq!(events[0].markdown, recorded.entries[0].text);
            assert!(!events[0].truncated);
        } else {
            assert_eq!(
                rows.iter()
                    .filter(|r| matches!(r.action, HistoryAction::Hit { .. }))
                    .count(),
                3
            );
        }
    }
}

#[test]
fn mother_launch_errors_preserve_the_placement_marker_pane_and_resources() {
    if yinz_test_support::reexec_test() {
        return;
    }
    std::env::set_var("YINZ_SESSION_NAME", "mother-test");
    let start = tempfile::tempdir().unwrap();
    claude_mother_fixture(start.path());
    std::env::set_var("YINZ_TEST_CACHE_ROOT", start.path());
    assert!(yinz_runtime::consts::YINZ_CACHE_DIR.starts_with(start.path()));
    let cwd = yinz_config::project_config::bubblewrap::mother_directory(
        &yinz_runtime::consts::YINZ_CACHE_DIR,
        "mother-test",
    )
    .unwrap();
    for failure in ["known pane", "missing pane", "registration"] {
        let request = request(
            start.path(),
            AgentAction::Mother {
                kind: "claude".into(),
            },
        );
        let (screen_tx, screen_rx) = unbounded();
        drop(screen_rx);
        let (pty_tx, pty_rx) = unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_pty: Some(SenderWithContext::new(pty_tx)),
            ..Default::default()
        };
        let reply = thread::scope(|scope| {
            scope.spawn(move || {
                let (instruction, _) = pty_rx.recv().unwrap();
                let PtyInstruction::SpawnTerminal(_, _, _, _, _, Some(mut completion), _) =
                    instruction
                else {
                    panic!("expected a launch");
                };
                assert!(completion.is_mother);
                if failure != "missing pane" {
                    completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(42)));
                    completion.complete_pane_placement(TabId::new(2));
                }
                if failure != "registration" {
                    completion.set_error_message(failure.into());
                }
            });
            mother(&request, "claude", &senders).unwrap()
        });
        let AgentReply::Launched(response) = reply else {
            panic!("expected a launch reply")
        };
        assert_eq!(response.outcome, AgentOutcome::Failed);
        assert!(
            response.message.as_ref().unwrap().contains(failure),
            "{response:?}"
        );
        if failure == "missing pane" {
            assert!(response.spawned.is_none());
            assert_eq!(
                response.launch_resources.unwrap().cwd.as_deref(),
                Some(cwd.as_path())
            );
        } else {
            let spawned = response.spawned.unwrap();
            assert_eq!(spawned.pane_id, PaneId::Terminal(TerminalId::new(42)));
            assert_eq!(spawned.tab_id, Some(TabId::new(2)));
            assert_eq!(spawned.cwd.as_deref(), Some(cwd.as_path()));
        }
    }
}

#[test]
fn mother_sandbox_selection_and_fallback_are_independent_of_bubblewrap() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let mut sandboxed = existing_command("codex", cwd, PaneTarget::Local).unwrap();
    let args = sandboxed.command.args.clone();
    let profile = root.path().join("profile-codex");
    mother_target(&mut sandboxed, Ok(profile.clone())).unwrap();
    assert_eq!(
        sandboxed.command.target.bubblewrap_root(),
        Some(profile.as_path())
    );
    assert_eq!(sandboxed.cwd, root.path());
    assert_eq!(sandboxed.command.args, args);

    let mut invalid = existing_command("claude", cwd, PaneTarget::Local).unwrap();
    let original = invalid.command.path.clone();
    let error = anyhow::anyhow!("complete host setup first")
        .context(yinz_config::project_config::bubblewrap::MotherSetupError);
    assert!(mother_target(&mut invalid, Err(error)).is_err());
    assert_eq!(invalid.command.path, original);

    let mut local = existing_command("codex", cwd, PaneTarget::Local).unwrap();
    mother_target(&mut local, Err(anyhow::anyhow!("namespace setup failed"))).unwrap();
    assert_eq!(local.command.target, PaneTarget::Local);
    assert_eq!(local.command.path, Path::new("/bin/sh"));
    assert_eq!(
        yinz_types::agent::agent_kind_of(&local.command.path, &local.command.args).as_deref(),
        Some("codex")
    );
    assert!(local
        .command
        .args
        .iter()
        .any(|arg| arg.contains("unsandboxed") && arg.contains("namespace setup failed")));
    // Run the banner wrapper with a harmless command, proving it prints and execs once.
    let output = process::Command::new(&local.command.path)
        .args(&local.command.args[..4])
        .args(["/bin/sh", "-c", "printf agent-started"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.contains("namespace setup failed\nagent-started"),
        "{output}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn mother_forks_select_provider_profiles_for_palette_and_plugin_panes() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use yinz_config::project_config::bubblewrap::{mother_directory, Environment};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_DATA_HOME", home.join("data"));
    std::env::set_var("CODEX_HOME", home.join("codex-state"));
    std::env::set_var("PI_CODING_AGENT_DIR", home.join("pi-state"));
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join("claude-state"));
    fs::create_dir(home.join("claude-state")).unwrap();
    fs::write(home.join("claude-state/.claude.json"), "{}").unwrap();
    std::env::set_var("XDG_STATE_HOME", home.join("xdg-state"));
    std::env::set_var("XDG_CACHE_HOME", home.join("xdg-cache"));
    std::env::set_var("YINZ_SESSION_NAME", "mother-forks");
    std::env::remove_var("YINZ_PANE_REF");
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    yinz_test_support::write_warm_script(
        &bin.join("bwrap"),
        r#"
if [ "$1" = --version ]; then
    echo 'bubblewrap 0.10.0'
    exit 0
fi
test "${MOTHER_CHECK_FAIL:-0}" = 0
"#,
    );
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    let work = mother_directory(&home.join("cache"), "forks").unwrap();
    let source = Environment::at_root(&work).prepare_mother("codex").unwrap();
    let metadata = |root: &Path| {
        yinz_runtime::home::xdg_data_dir()
            .join("bubblewrap/environments")
            .join(yinz_config::project_config::root_key(root))
    };
    let source_record = metadata(source.root()).join("accepted.json");
    let original = fs::read(&source_record).unwrap();
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(metadata(source.root()).join("lock"))
        .unwrap();
    lease.try_lock_shared().unwrap();
    let source_target = PaneTarget::bubblewrap(source.root()).unwrap();
    let mut plugin = crate::test_env::test_env();
    plugin.plugin_cwd = work.clone();
    plugin.response_sink = ResponseSink::Capture(None);
    let data = DataAccess::sqlite(&home.join("store"));
    for kind in ["codex", "pi", "claude", "cursor"] {
        let mut duplicate = request(
            &work,
            AgentAction::Duplicate {
                kind: Some(kind.into()),
            },
        );
        duplicate.source.target = source_target.clone();
        duplicate.source.agent = Some(
            serde_json::from_value(serde_json::json!({
                "kind": "codex", "state": "Idle", "cwd": work
            }))
            .unwrap(),
        );
        let (reply, commands) = run(&duplicate, &data);
        if kind == "cursor" {
            assert!(commands.is_empty(), "{reply:?}");
            assert!(
                matches!(reply.unwrap(), AgentReply::Launched(response) if response.outcome != AgentOutcome::Ok && response.message.as_deref().is_some_and(|message| message.contains("no supported writable state layout")))
            );
        } else {
            assert_eq!(commands.len(), 1, "{reply:?}");
            let command = &commands[0];
            assert_eq!(command.cwd.as_deref(), Some(work.as_path()));
            let expected = work.with_file_name(format!("profile-{kind}"));
            assert_eq!(command.target.bubblewrap_root(), Some(expected.as_path()));
            let record: serde_json::Value = serde_json::from_slice(
                &fs::read(metadata(&expected).join("accepted.json")).unwrap(),
            )
            .unwrap();
            let mut state = vec![home.join(format!("{kind}-state"))];
            let mut env = serde_json::json!({});
            if kind == "claude" {
                state = vec![home
                    .join("data/yinz/bubblewrap/homes")
                    .join(yinz_config::project_config::root_key(&expected))
                    .join("projects")];
                env = serde_json::json!({
                    "HOME": home,
                    "CLAUDE_CONFIG_DIR": home.join("claude-state"),
                    "DISABLE_AUTOUPDATER": "1",
                    "XDG_CACHE_HOME": "/run/yinz-claude/cache",
                    "XDG_STATE_HOME": "/run/yinz-claude/state",
                });
            }
            assert_eq!(record["declaration"]["mother"], serde_json::json!(state));
            assert_eq!(record["declaration"]["env"], env);
        }
        for rerun in [false, true] {
            let command = yinz_runtime::agent::launch::command_in_context(
                kind,
                Some("/host".into()),
                source_target.clone(),
            )
            .unwrap();
            let (screen_tx, screen_rx) = unbounded();
            let (pty_tx, pty_rx) = unbounded();
            plugin.senders.to_screen = Some(SenderWithContext::new(screen_tx));
            plugin.senders.to_pty = Some(SenderWithContext::new(pty_tx));
            let command = thread::scope(|scope| {
                let worker = scope.spawn(move || {
                    let mut launched = None;
                    while let Ok((instruction, _)) = pty_rx.recv() {
                        let PtyInstruction::SpawnTerminal(
                            Some(TerminalAction::RunCommand(command)),
                            ..,
                        ) = instruction
                        else {
                            panic!("unexpected launch {instruction:?}");
                        };
                        launched = Some(command);
                    }
                    launched
                });
                if rerun {
                    run_command_in_pane(
                        &plugin,
                        PaneId::Terminal(TerminalId::new(7)),
                        command,
                        BTreeMap::new(),
                        false,
                    );
                } else {
                    open_command_pane(&mut plugin, command, BTreeMap::new());
                }
                plugin.senders.to_pty = None;
                let opened = worker.join().unwrap();
                let rerun_command = screen_rx.try_iter().find_map(|(instruction, _)| {
                    if let ScreenInstruction::RunCommandInPane(id, command, _) = instruction {
                        assert_eq!(id, TerminalId::new(7));
                        Some(command)
                    } else {
                        None
                    }
                });
                if rerun {
                    rerun_command
                } else {
                    opened
                }
            });
            if kind == "cursor" {
                assert!(command.is_none());
            } else {
                let command = command.unwrap();
                assert_eq!(command.cwd, Some(work.clone()));
                assert_eq!(
                    command.target.bubblewrap_root(),
                    Some(work.with_file_name(format!("profile-{kind}")).as_path())
                );
            }
        }
        assert_eq!(fs::read(&source_record).unwrap(), original);
    }
    // Recorded resumes and compatible legacy profiles keep their accepted target.
    let mut legacy: serde_json::Value = serde_json::from_slice(&original).unwrap();
    legacy["root"] = serde_json::json!(work);
    legacy["declaration"]
        .as_object_mut()
        .unwrap()
        .remove("mother_work");
    fs::create_dir_all(metadata(&work)).unwrap();
    fs::write(
        metadata(&work).join("accepted.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    for root in [source.root(), work.as_path()] {
        let target = PaneTarget::bubblewrap(root).unwrap();
        let mut command = yinz_runtime::agent::launch::resume_command(
            "codex",
            "archived",
            None,
            None,
            root.to_str(),
        )
        .unwrap();
        command.cwd = Some(work.clone());
        prepare_agent_command(&mut command).unwrap();
        assert_eq!(command.target, target);
        assert_eq!(command.cwd, Some(work.clone()));
    }
    // Project environments and remote commands keep their original targets.
    legacy["declaration"]
        .as_object_mut()
        .unwrap()
        .remove("mother");
    fs::write(
        metadata(&work).join("accepted.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    for target in [
        PaneTarget::bubblewrap(&work).unwrap(),
        PaneTarget::Local,
        PaneTarget::ssh("example.test").unwrap(),
    ] {
        let prepared = existing_command("claude", work.to_str().unwrap(), target.clone()).unwrap();
        assert_eq!(prepared.command.target, target);
    }
    let mut project_command = yinz_runtime::agent::launch::command_in_context(
        "claude",
        None,
        PaneTarget::bubblewrap(&work).unwrap(),
    )
    .unwrap();
    prepare_agent_command(&mut project_command).unwrap();
    assert_eq!(project_command.cwd, None);
    std::env::set_var("MOTHER_CHECK_FAIL", "1");
    // Preparation selects the profile; runtime probes belong to PTY spawning.
    assert!(existing_command("pi", work.to_str().unwrap(), source_target.clone()).is_ok());
    let command =
        yinz_runtime::agent::launch::command_in_context("pi", Some("/host".into()), source_target)
            .unwrap();
    assert!(run_command_action(&plugin, command, false, BTreeMap::new()).is_some());
    assert_eq!(fs::read(source_record).unwrap(), original);
}

#[test]
fn claude_mother_runtime_unavailable_has_visible_local_fallback() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    claude_mother_fixture(temp.path());
    yinz_test_support::write_warm_script(&temp.path().join("bin/bwrap"), "exit 1");
    std::env::set_var("YINZ_TEST_CACHE_ROOT", temp.path());
    std::env::set_var("YINZ_SESSION_NAME", "claude-fallback");
    let request = request(
        temp.path(),
        AgentAction::Mother {
            kind: "claude".into(),
        },
    );
    let (reply, commands) = run(&request, &DataAccess::sqlite(&temp.path().join("data")));
    assert!(
        matches!(reply, Ok(AgentReply::Launched(response)) if response.outcome == AgentOutcome::Ok)
    );
    assert_eq!(commands.len(), 1);
    #[cfg(target_os = "linux")]
    let mut commands = commands;
    #[cfg(target_os = "linux")]
    {
        use yinz_config::project_config::bubblewrap::{Environment, LaunchCheck};
        let command = &mut commands[0];
        let profile = Environment::at_root(command.target.bubblewrap_root().unwrap());
        let LaunchCheck::MotherFallback(reason) = profile
            .check_launch(command.cwd.as_ref().unwrap(), Path::new("claude"))
            .unwrap()
        else {
            panic!("unavailable Bubblewrap must request local fallback")
        };
        yinz_types::agent::mother_fallback(&mut command.command, &mut command.args, &reason);
        command.target = PaneTarget::Local;
    }
    assert_eq!(commands[0].target, PaneTarget::Local);
    assert!(commands[0]
        .args
        .iter()
        .any(|arg| arg.contains("unsandboxed") && arg.contains("Bubblewrap")));
    assert_eq!(
        yinz_types::agent::agent_kind_of(&commands[0].command, &commands[0].args).as_deref(),
        Some("claude")
    );
}
