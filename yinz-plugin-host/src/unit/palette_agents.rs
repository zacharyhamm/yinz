use super::super::agent_launch::TILED;
use super::*;
use crate::{PaneId, TerminalId};
use std::sync::{atomic::AtomicBool, Arc};
use std::{fs, process, thread};
use yinz_bus::pty::ClientTabIndexOrPaneId;
use yinz_runtime::channels::{unbounded, SenderWithContext};
use yinz_types::agent::control::AgentOutcome;
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::internal_ids::{ClientId, HostId, TabId};

#[test]
fn live_agent_labels_use_panel_project_and_checkout_metadata() {
    use project_data::{
        location::Location,
        projects::{self, Repository, WorktreeStatus},
    };
    use yinz_bus::command_palette::{AgentLabelSource, AgentLabels};

    let mut project = projects::validate("project", "/work/root").unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    let mut registry = [(project.name.clone(), project)].into();
    let local = Location::local("/work/linked");
    let remote = Location::new(Some("builder"), "/work/linked");
    let repository = Repository {
        root: local.path.clone(),
        remote: Some("git@example.com:team/repo.git".into()),
        worktree_status: Some(WorktreeStatus::Linked),
        branch: Some("topic/雪".into()),
        ..Default::default()
    };
    let mut repositories = [
        (local.clone(), repository.clone()),
        (
            remote.clone(),
            Repository {
                host: remote.host.clone(),
                branch: Some("remote-topic".into()),
                ..repository
            },
        ),
    ]
    .into();
    let sources = || {
        [
            ("local", Some(local.clone())),
            ("peer-session", Some(local.clone())),
            ("remote", Some(remote.clone())),
            ("unknown", Some(Location::local("/unknown"))),
            ("no-cwd", None),
        ]
        .into_iter()
        .map(|(id, location)| AgentLabelSource {
            id: id.into(),
            host: location.as_ref().and_then(|location| location.host.clone()),
            cwd: location.map(|location| location.path),
        })
        .collect()
    };
    let labels = resolve_labels(sources(), &registry, &repositories);
    assert_eq!(
        labels["local"],
        AgentLabels {
            project: "project".into(),
            branch: "topic/雪".into()
        }
    );
    assert_eq!(labels["peer-session"], labels["local"]);
    assert_eq!(
        labels["remote"],
        AgentLabels {
            project: "project".into(),
            branch: "remote-topic".into()
        }
    );
    assert_eq!(labels["unknown"], AgentLabels::default());
    assert!(!labels.contains_key("no-cwd"));

    registry.get_mut("project").unwrap().name = "renamed".into();
    repositories.get_mut(&local).unwrap().branch = Some("new-branch".into());
    let labels = resolve_labels(sources(), &registry, &repositories);
    assert_eq!(
        labels["local"],
        AgentLabels {
            project: "renamed".into(),
            branch: "new-branch".into()
        }
    );
    registry.clear();
    let labels = resolve_labels(sources(), &registry, &repositories);
    assert_eq!(labels["local"].project, "");
    assert_eq!(labels["local"].branch, "new-branch");
}

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
        values: vec![],
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
                assert_eq!(placement, TILED);
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
fn palette_projects_offer_creation_first_and_preserve_provider_through_selection() {
    let (repo, populated) = repository();
    let empty_root = tempfile::tempdir().unwrap();
    let empty = DataAccess::sqlite(empty_root.path());
    for definition in project_data::store::shared_defs() {
        empty
            .plugin_data()
            .define_entity(project_data::store::SHARED, "test", &definition, false)
            .unwrap();
    }
    for (data, registered) in [(&empty, false), (&populated, true)] {
        let (reply, commands) = run(
            &request(
                empty_root.path(),
                AgentAction::Projects {
                    kind: "codex".into(),
                },
            ),
            data,
        );
        assert!(commands.is_empty());
        let AgentReply::Choices { title, choices } = reply.unwrap() else {
            panic!()
        };
        assert_eq!(title, "Choose project");
        assert_eq!(choices.len(), if registered { 2 } else { 1 });
        assert_eq!(choices[0].label, "Create new project…");
        assert!(matches!(&choices[0].action, AgentAction::NewProject { kind } if kind == "codex"));
        let (reply, commands) = run(&request(repo.path(), choices[0].action.clone()), data);
        assert!(commands.is_empty());
        assert!(
            matches!(reply.unwrap(), AgentReply::Form { action: AgentAction::NewProject { kind }, .. } if kind == "codex")
        );
        if registered {
            assert!(
                matches!(&choices[1].action, AgentAction::Worktrees { kind, project: AgentProject::Registered(name) } if kind == "codex" && name == "project")
            );
            let (reply, commands) = run(&request(repo.path(), choices[1].action.clone()), data);
            assert!(commands.is_empty());
            let AgentReply::Choices { title, choices } = reply.unwrap() else {
                panic!()
            };
            assert_eq!(title, "project · Worktrees");
            assert_eq!(choices[0].label, "Create new worktree…");
            for choice in choices
                .iter()
                .filter(|choice| choice.label != "Change project…")
            {
                assert!(
                    matches!(&choice.action, AgentAction::Checkout { kind, project: AgentProject::Registered(name), .. } if kind == "codex" && name == "project")
                );
            }
            assert!(choices.iter().any(|choice| choice.label == "main"));
        }
    }
}

#[test]
fn palette_new_project_form_validation_and_launch_use_the_shared_backend() {
    let (repo, data) = repository();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().canonicalize().unwrap().join("new-app");
    let mut create = request(
        repo.path(),
        AgentAction::NewProject {
            kind: "claude".into(),
        },
    );
    let (reply, commands) = run(&create, &data);
    assert!(commands.is_empty());
    let AgentReply::Form { labels, submit, .. } = reply.unwrap() else {
        panic!("expected new project form");
    };
    assert_eq!(labels.len(), 2);
    assert_eq!(submit, "Create and launch");
    for values in [
        vec!["new-app".into()],
        vec![String::new(), destination.display().to_string()],
        vec!["new-app".into(), " ".into()],
    ] {
        create.values = values;
        let (reply, commands) = run(&create, &data);
        assert!(reply.unwrap_err().contains("required"));
        assert!(commands.is_empty());
        assert!(!destination.exists());
    }
    for (name, path, error) in [
        ("new-app", "relative/path".into(), "absolute"),
        (
            "new-app",
            parent.path().display().to_string(),
            "already exists",
        ),
        (
            "project",
            destination.display().to_string(),
            "already exists",
        ),
    ] {
        create.values = vec![name.into(), path];
        let (reply, commands) = run(&create, &data);
        let AgentReply::Launched(response) = reply.unwrap() else {
            panic!("expected backend rejection");
        };
        assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
        assert!(response.message.unwrap().contains(error));
        assert!(commands.is_empty());
        assert!(!destination.exists());
    }
    create.values = vec!["new-app".into(), destination.display().to_string()];
    create
        .active
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(run(&create, &data).0.unwrap_err().contains("cancelled"));
    assert!(!destination.exists());
    create
        .active
        .store(true, std::sync::atomic::Ordering::Release);
    let (reply, commands) = run(&create, &data);
    assert!(
        matches!(&reply, Ok(AgentReply::Launched(r)) if r.outcome == AgentOutcome::Ok),
        "{reply:?}"
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].cwd.as_ref(), Some(&destination));
    assert_eq!(
        git(&destination, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );
    let projects = project_data::projects::from_rows(&agent_control::projects(&data).unwrap());
    assert_eq!(projects["new-app"].root, destination.to_str().unwrap());
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
    create.values = vec![String::new()];
    assert!(run(&create, &data).0.unwrap_err().contains("required"));
    create.values = vec!["review".into()];
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
            create.values = vec!["review".into()];
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
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("XDG_DATA_HOME", storage.path()) };
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
        assert!(!commands[0]
            .args
            .iter()
            .any(|arg| arg.contains("allowedTools") || arg.contains("trust_level")));
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
fn palette_history_automatically_loads_all_retained_revisions() {
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
        let mut loaded = std::collections::BTreeSet::new();
        loop {
            let (reply, _) = run(&request(temp.path(), AgentAction::History(action)), &data);
            let AgentReply::History(HistoryReply::Run { rows, .. }) = reply.unwrap() else {
                panic!("expected timeline")
            };
            loaded.extend(rows.iter().filter_map(|row| match row.action {
                HistoryAction::Event { revision, .. } => Some(revision),
                _ => None,
            }));
            starts.push(
                rows.iter()
                    .find_map(|row| match row.action {
                        HistoryAction::Event { revision, .. } => Some(revision),
                        _ => None,
                    })
                    .unwrap(),
            );
            assert!(
                starts.len() <= revisions.len() + 1,
                "backward navigation failed to terminate: {starts:?}"
            );
            let Some(older) = rows.iter().find(|row| row.id == "continue") else {
                break;
            };
            action = older.action.clone();
        }
        assert_eq!(starts[0], expected_starts[0]);
        assert_eq!(loaded.into_iter().collect::<Vec<_>>(), revisions);
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
fn palette_history_keeps_panel_tool_entries_and_opens_full_retained_tools() {
    use agent_proto::agents::{RunEntry, RunEntryKind};
    use yinz_bus::command_palette::{HistoryAction, HistoryReply};
    use yinz_types::agent::tool_capture::{Payload, ToolCapture};
    let (temp, data) = repository();
    let mut recorded = recorded_run(temp.path());
    recorded.entries = [
        RunEntryKind::Prompt,
        RunEntryKind::ToolCall,
        RunEntryKind::ToolResult,
        RunEntryKind::ToolCall,
        RunEntryKind::Message,
        RunEntryKind::ToolResult,
        RunEntryKind::ToolResult,
        RunEntryKind::Message,
        RunEntryKind::ToolCall,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| RunEntry {
        revision: i as u64 + 1,
        kind,
        text: format!(r"retained text {i}: [ERROR] C:\logs\output *literal* `code`"),
        created_at_ms: Some(1234),
        tool: matches!(kind, RunEntryKind::ToolCall | RunEntryKind::ToolResult).then(|| {
            ToolCapture {
                name: format!("tool {i}"),
                call_id: Some(format!("call {i}")),
                turn_id: None,
                worker_id: None,
                parent_call_id: None,
                source_event: kind.label().into(),
                status: "completed".into(),
                arguments: None,
                output: Some(Payload {
                    text: format!("captured output {i}"),
                    format: "text".into(),
                    original_bytes: 1000 + i,
                    redacted: true,
                    truncated: true,
                    attachments_omitted: true,
                }),
                error: None,
                duration_ms: Some(100 + i as u64),
                output_unavailable: false,
                metadata_truncated: true,
                metadata_redacted: true,
                identity: None,
                payload_omitted: false,
            }
        }),
        event_id: None,
    })
    .collect();
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &agent_proto::agents::rows::run_mutations(&recorded),
        )
        .unwrap();
    let execute = |action| {
        run(&request(temp.path(), AgentAction::History(action)), &data)
            .0
            .unwrap()
    };
    let AgentReply::History(HistoryReply::Run {
        header,
        events,
        rows,
        ..
    }) = execute(HistoryAction::Run {
        id: recorded.id.clone(),
        from: None,
        query: String::new(),
    })
    else {
        panic!("expected conversation");
    };
    assert!(header.session_id.contains(&recorded.session_id));
    let events = yinz_bus::command_palette::collapse_tools(events);
    assert_eq!(events.len(), 6);
    assert_eq!(events[3].kind, "tools");
    assert_eq!(events[3].tool_calls, 0);
    assert_eq!(events[3].revisions, vec![6, 7]);
    assert_eq!(events[5].kind, "tools");
    assert_eq!(events[5].tool_calls, 1);
    assert_eq!(events[5].revisions, vec![9]);
    assert_eq!(events[1].kind, "tools");
    assert_eq!(events[1].tool_calls, 2);
    assert_eq!(events[1].revisions, vec![2, 3, 4]);
    assert!(events[1].markdown.is_empty());
    assert!(rows.iter().any(|row| row.id == "event:2"));
    let action = HistoryAction::Tools {
        id: recorded.id.clone(),
        revisions: events[1].revisions.clone(),
    };
    let expected = |entry: &RunEntry| {
        format!(
            "{} · revision {}:\n{}\n\n{}",
            entry.kind.label(),
            entry.revision,
            entry.text,
            serde_json::to_string_pretty(entry.tool.as_ref().unwrap()).unwrap()
        )
    };
    // A single tool and a group use the same retained captures, in revision order.
    for (action, entries) in [
        (action, &recorded.entries[1..4]),
        (
            HistoryAction::Tools {
                id: recorded.id.clone(),
                revisions: vec![4, 2, 3],
            },
            &recorded.entries[1..4],
        ),
        (
            HistoryAction::Tools {
                id: recorded.id.clone(),
                revisions: vec![2],
            },
            &recorded.entries[1..2],
        ),
    ] {
        let AgentReply::History(HistoryReply::Reader {
            header,
            markdown,
            tool,
        }) = execute(action)
        else {
            panic!("expected tools");
        };
        assert_eq!(header, "tool_call");
        assert_eq!(
            markdown,
            entries
                .iter()
                .map(expected)
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        assert!(tool.is_empty());
    }
    for entry in &recorded.entries[1..4] {
        let AgentReply::History(HistoryReply::Reader { markdown, tool, .. }) =
            execute(HistoryAction::Event {
                id: recorded.id.clone(),
                revision: entry.revision,
            })
        else {
            panic!("expected individual tool");
        };
        assert_eq!(markdown, entry.text);
        assert_eq!(
            serde_json::from_str::<ToolCapture>(&tool).unwrap(),
            *entry.tool.as_ref().unwrap()
        );
    }
    let AgentReply::History(HistoryReply::Run { events, .. }) = execute(HistoryAction::Run {
        id: recorded.id.clone(),
        from: None,
        query: "retained".into(),
    }) else {
        panic!("expected search results");
    };
    assert_eq!(events.len(), recorded.entries.len());
    for (event, entry) in events.iter().zip(&recorded.entries) {
        assert_eq!(event.revisions, vec![entry.revision]);
        assert_eq!(event.kind, entry.kind.label());
        let expected = if matches!(
            entry.kind,
            RunEntryKind::ToolCall | RunEntryKind::ToolResult
        ) {
            entry.text.clone()
        } else {
            yinz_tile_markdown::escape_inline(&entry.text)
        };
        assert_eq!(event.markdown, expected);
    }
}

#[test]
fn palette_prompt_rejects_unpinned_conversations_before_local_or_peer_dispatch() {
    let (temp, data) = repository();
    let (tx, rx) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(tx)),
        ..Default::default()
    };
    for session_id in [None, Some(String::new())] {
        for target_session in [None, Some("unavailable-palette-peer".into())] {
            let mut request = request(
                temp.path(),
                AgentAction::Prompt {
                    pane_id: PaneId::Terminal(TerminalId::new(9)),
                    session_id: session_id.clone(),
                    target_session,
                    submission_id: "palette-retry".into(),
                },
            );
            request.values = vec!["continue\nwith tests".into()];
            assert_eq!(
                execute(&request, &senders, &data).unwrap_err(),
                "Agent has no conversation ID; refresh agent info before sending a prompt"
            );
            assert!(rx.try_recv().is_err());
        }
    }
}

#[test]
fn palette_prompt_routes_guarded_text_without_launching_an_agent() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest as ControlRequest, AgentResponse};
    let (temp, data) = repository();
    let target = PaneId::Terminal(TerminalId::new(9));
    let mut request = request(
        temp.path(),
        AgentAction::Prompt {
            pane_id: target,
            session_id: Some("expected-session".into()),
            target_session: None,
            submission_id: "palette-retry".into(),
        },
    );
    request.values = vec!["continue\nwith tests".into()];
    let (tx, rx) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(tx)),
        ..Default::default()
    };
    thread::scope(|scope| {
        scope.spawn(|| {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let ScreenInstruction::AgentControlReply(control, reply) = instruction else { panic!("expected prompt"); };
            assert!(matches!(control, ControlRequest::Prompt { pane_id, expected_session_id: Some(session), submission_id: Some(submission), prompt, wait: false, .. }
                if pane_id == target && session == "expected-session" && submission == "palette-retry" && prompt == "continue\nwith tests"));
            reply.send(AgentResponse::new(AgentOutcome::Ok)).unwrap();
        });
        assert!(
            matches!(execute(&request, &senders, &data).unwrap(), AgentReply::Prompted(response) if response.outcome == AgentOutcome::Ok)
        );
    });
}

#[test]
fn palette_prompt_peer_uses_owning_session_socket_and_submission_identity() {
    use interprocess::local_socket::prelude::*;
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    use yinz_types::agent::control::{AgentOutcome, AgentRequest as ControlRequest, AgentResponse};
    fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let directory = tempfile::tempdir_in(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let name = format!(
        "{}-prompt",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    let socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
    let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
    let request = ControlRequest::Prompt {
        pane_id: PaneId::Terminal(TerminalId::new(7)),
        expected_session_id: Some("peer-agent".into()),
        submission_id: Some("peer-retry".into()),
        preserve_draft: false,
        prompt: "continue".into(),
        wait: false,
        timeout_ms: 30_000,
    };
    thread::scope(|scope| {
        let expected = request.clone();
        scope.spawn(move || {
            let connection = listener.accept().unwrap();
            let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(connection);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            receiver.set_recv_timeout(Some(Duration::from_secs(5))).unwrap();
            assert!(matches!(receiver.recv_client_msg(), IpcRecv::Msg(ClientToServerMsg::AgentControl(control), _) if control == expected));
            sender.send_server_msg(ServerToClientMsg::AgentControl(AgentResponse::new(AgentOutcome::Ok))).unwrap();
            assert!(matches!(receiver.recv_client_msg(), IpcRecv::Msg(ClientToServerMsg::ClientExited, _)));
        });
        assert_eq!(
            prompt_peer(&name.into(), request).unwrap().outcome,
            AgentOutcome::Ok
        );
    });
    if socket.exists() {
        fs::remove_file(socket).unwrap();
    }
}

#[test]
fn palette_prompt_peer_bounds_sending_and_cleanup_when_peer_does_not_read() {
    use interprocess::local_socket::prelude::*;
    use std::time::Instant;
    use yinz_types::agent::control::AgentRequest as ControlRequest;

    fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let directory = tempfile::tempdir_in(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let name = format!(
        "{}-blocked-prompt",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    let socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
    let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
    let request = ControlRequest::Prompt {
        pane_id: PaneId::Terminal(TerminalId::new(7)),
        expected_session_id: Some("peer-agent".into()),
        submission_id: Some("peer-retry".into()),
        preserve_draft: false,
        // Larger than the Unix socket buffer, forcing transmission to wait.
        prompt: "x".repeat(8 * 1024 * 1024),
        wait: false,
        timeout_ms: 30_000,
    };
    thread::scope(|scope| {
        let (done, result) = crossbeam::channel::bounded(1);
        scope.spawn(move || {
            let budget = Duration::from_millis(500);
            let started = Instant::now();
            let response = prompt_peer_with_deadline(&name.into(), request, started + budget);
            done.send((response, started.elapsed(), budget)).unwrap();
        });
        let connection = listener.accept().unwrap();
        // Keep the socket open without reading until the complete call, including cleanup, ends.
        let completed = result.recv_timeout(Duration::from_secs(3));
        drop(connection);
        let (response, elapsed, budget) = completed.expect("prompt call exceeded its deadline");
        assert!(
            response.unwrap_err().contains("Prompt outcome unknown"),
            "sending a partial prompt must report an uncertain outcome"
        );
        // Allow socket timer granularity and scheduling, but not a second timeout for cleanup.
        assert!(elapsed < budget + Duration::from_millis(250), "{elapsed:?}");
    });
    if socket.exists() {
        fs::remove_file(socket).unwrap();
    }
}
