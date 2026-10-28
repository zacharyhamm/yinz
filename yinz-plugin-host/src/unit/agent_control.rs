use super::super::agent_launch::{
    agent_folder_tracked, apply_project_worktree_dir, prepare_git_checkout_in_tracked,
    WorktreeGuard,
};
use super::*;
use std::thread;
use std::time::Duration;
use yinz_bus::pty::ClientTabIndexOrPaneId;
use yinz_types::agent::control::AgentStatus;
use yinz_types::command::GitCheckout;
use yinz_types::command::PaneTarget;
use yinz_types::input::command::TerminalAction;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::plugin_data::Mutation;

#[test]
fn provider_usage_replies_keep_the_request_client_and_generation_without_screen_work() {
    use std::collections::HashSet;
    use yinz_runtime::channels::{self, SenderWithContext};
    use yinz_types::agent::control::ProviderUsageStatus;
    use yinz_types::data::{Event, PermissionType, PluginCommand};

    let mut env = crate::test_env::test_env();
    let directory = tempfile::tempdir().unwrap();
    env.data_access = std::sync::Arc::new(DataAccess::sqlite(directory.path()));
    env.client_id = crate::ClientId::new(7);
    env.generation = Some(91);
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    let (to_plugin, plugin_rx) = channels::unbounded();
    let (to_screen, screen_rx) = channels::unbounded();
    env.senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        to_screen: Some(SenderWithContext::new(to_screen)),
        ..Default::default()
    };
    crate::yinz_exports::run_plugin_command(
        &mut env,
        PluginCommand::AgentControl {
            id: "usage-fixture".into(),
            request: serde_json::to_string(&AgentRequest::Usage {
                provider: "unsupported-fixture".into(),
            })
            .unwrap(),
        },
    )
    .unwrap();
    let (instruction, _) = plugin_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    let PluginInstruction::PluginEvent(
        plugin,
        client,
        generation,
        Event::CustomMessage(name, body),
    ) = instruction
    else {
        panic!("expected provider usage reply");
    };
    assert_eq!(plugin, env.plugin_id);
    assert_eq!(client, env.client_id);
    assert_ne!(client, env.owner_client_id);
    assert_eq!(generation, 91);
    assert_eq!(name, "agent-response");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["id"], "usage-fixture");
    let response: AgentResponse = serde_json::from_value(body["response"].clone()).unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok);
    let usage = response.usage.unwrap();
    assert_eq!(usage.provider, "unsupported-fixture");
    assert_eq!(usage.status, ProviderUsageStatus::Unsupported);
    assert!(usage.windows.is_empty());
    assert!(screen_rx.is_empty());
}

#[test]
fn lost_host_control_replies_keep_the_submission_identity_sent_to_screen() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let prompt = |submission_id| AgentRequest::Prompt {
        preserve_draft: false,
        pane_id: PaneId::Terminal(TerminalId::new(7)),
        expected_session_id: Some("conversation".into()),
        submission_id,
        prompt: "Review the checkout".into(),
        wait: true,
        timeout_ms: 30_000,
    };
    for (request, expected_id) in [
        (prompt(None), None),
        (
            prompt(Some("explicit-prompt".into())),
            Some("explicit-prompt"),
        ),
        (
            AgentRequest::WaitTurn {
                pane_id: PaneId::Terminal(TerminalId::new(7)),
                expected_session_id: Some("conversation".into()),
                submission_id: "prior-turn".into(),
                timeout_ms: 30_000,
            },
            Some("prior-turn"),
        ),
    ] {
        let (to_screen, screen_rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(yinz_runtime::channels::SenderWithContext::new(to_screen)),
            ..Default::default()
        };
        let screen = thread::spawn(move || {
            let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let ScreenInstruction::AgentControlReply(request, reply) = instruction else {
                panic!("expected control request");
            };
            let submission_id = match request {
                AgentRequest::Prompt { submission_id, .. } => submission_id.unwrap(),
                AgentRequest::WaitTurn { submission_id, .. } => submission_id,
                _ => panic!("expected a prompt or turn wait"),
            };
            // Dropping the reply exercises the same fallback as its deadline, immediately.
            drop(reply);
            submission_id
        });
        let response = control_agent(request, crate::ClientId::new(1), &senders, &data);
        let sent_id = screen.join().unwrap();
        assert_eq!(response.outcome, AgentOutcome::TimedOut);
        assert_eq!(response.submission_id.as_deref(), Some(sent_id.as_str()));
        assert!(response.message.unwrap().contains("outcome unknown"));
        if let Some(expected_id) = expected_id {
            assert_eq!(sent_id, expected_id);
        } else {
            assert!(uuid::Uuid::parse_str(&sent_id).is_ok());
        }
    }
}

#[test]
fn host_control_routing_rejection_keeps_the_prompt_submission_identity() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let response = control_agent(
        AgentRequest::Prompt {
            preserve_draft: false,
            pane_id: PaneId::Terminal(TerminalId::new(7)),
            expected_session_id: None,
            submission_id: Some("not-routed".into()),
            prompt: "Review the checkout".into(),
            wait: false,
            timeout_ms: 30_000,
        },
        crate::ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(response.outcome, AgentOutcome::Rejected);
    assert_eq!(response.submission_id.as_deref(), Some("not-routed"));
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn agent_status(cwd: &std::path::Path) -> AgentStatus {
    AgentStatus {
        pane_id: PaneId::Terminal(crate::TerminalId::new(7)),
        session: "session".into(),
        host_id: "host".into(),
        tab_id: Some(TabId::new(3)),
        target: PaneTarget::Local,
        agent: yinz_types::agent::AgentInfo {
            kind: "codex".into(),
            state: yinz_types::agent::AgentState::Idle,
            source: yinz_types::agent::AgentSource::Spawned,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        },
        cwd: Some(cwd.to_string_lossy().into_owned()),
        branch: None,
        ready: false,
        not_ready_reason: Some(yinz_types::agent::control::NotReadyReason::PromptUnsupported),
        status_available: false,
        prompt_supported: false,
        readiness: None,
        input_revision: None,
        is_suppressed: false,
        clear_draft_supported: false,
        background_tasks: Vec::new(),
    }
}

#[test]
fn project_event_membership_recovers_after_failed_probes_with_identical_inputs() {
    use yinz_types::data::{GetPaneGitProbeResponse as Probe, PaneGitProbe};

    let temp = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(temp.path());
    let store = data.plugin_data();
    for def in project_data::store::shared_defs() {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = project_data::projects::validate("work", "/work").unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    store
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
        )
        .unwrap();
    let remote = AgentStatus {
        target: PaneTarget::ssh("server").unwrap(),
        ..agent_status(std::path::Path::new("/work/src"))
    };
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_pty: Some(yinz_runtime::channels::SenderWithContext::new(tx)),
        ..Default::default()
    };
    let pane = remote.pane_id;
    let pty = std::thread::spawn(move || {
        for response in [
            Some(Probe::Err("unavailable".into())),
            None,
            Some(Probe::Ok(PaneGitProbe {
                host: "server".into(),
                cwd: "/work/src".into(),
                exit_code: Some(0),
                metadata: b"/work\n/work/.git\n/work/.git\nmain\n".to_vec(),
                config: b"remote.origin.url\nhttps://example.com/team/repo\0".to_vec(),
            })),
        ] {
            let (instruction, _) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            let PtyInstruction::GetPaneGitProbe {
                pane_id,
                cwd,
                response_channel,
                ..
            } = instruction
            else {
                panic!("expected a git probe");
            };
            assert_eq!(pane_id, pane);
            assert_eq!(cwd, "/work/src");
            if let Some(response) = response {
                response_channel.send(response).unwrap();
            }
            // A dropped reply exercises the receive-error path without waiting for its deadline.
        }
    });
    for expected in [None, None, Some(vec![pane])] {
        assert_eq!(
            agent_event_projects(vec![remote.clone()], vec!["work".into()], &data, &senders),
            BTreeMap::from([("work".into(), Ok(expected))])
        );
    }
    pty.join().unwrap();
}

#[test]
fn project_containment_uses_canonical_paths() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("project");
    let sibling = base.join("project-other");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::os::unix::fs::symlink(&root, base.join("alias")).unwrap();
    std::os::unix::fs::symlink(&sibling, root.join("escape")).unwrap();
    let data = DataAccess::sqlite(&base.join("data"));
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    for project_root in [&root, &base.join("alias")] {
        let project =
            project_data::projects::validate("work", project_root.to_str().unwrap()).unwrap();
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &[project_data::projects::to_mutation(&project)],
            )
            .unwrap();
        for cwd in [root.join("src"), base.join("alias/src")] {
            let mut spec = AgentSpawnSpec {
                kind: Some("codex".into()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            };
            let prepared = prepare_automation_agent(
                &mut spec,
                Some(cwd),
                Some("work".into()),
                None,
                &data,
                &Default::default(),
            )
            .unwrap();
            assert_eq!(prepared.cwd, root.join("src"));
        }
        for (cwd, error) in [
            (sibling.clone(), "cwd must be inside"),
            (root.join("escape"), "cwd must be inside"),
            (root.join("missing"), "cannot resolve agent directory"),
        ] {
            let mut spec = AgentSpawnSpec {
                kind: Some("codex".into()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            };
            let result = prepare_automation_agent(
                &mut spec,
                Some(cwd),
                Some("work".into()),
                None,
                &data,
                &Default::default(),
            );
            assert!(result.err().unwrap().to_string().contains(error));
        }
    }
}

#[test]
fn project_filter_uses_repository_identity_before_containment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let checkout = root.join("checkout");
    let clone = root.join("clone");
    let linked = root.join("linked");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(
        &checkout,
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
    git(
        &root,
        &["clone", checkout.to_str().unwrap(), clone.to_str().unwrap()],
    );
    git(
        &checkout,
        &["worktree", "add", linked.to_str().unwrap(), "-b", "linked"],
    );
    git(&clone, &["remote", "remove", "origin"]);
    for path in [&checkout, &clone] {
        git(
            path,
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/team/repo.git",
            ],
        );
    }
    let data = DataAccess::sqlite(&root.join("data"));
    let store = data.plugin_data();
    // Missing assignments are allowed; repository ownership still works.
    for def in project_data::store::shared_defs()
        .into_iter()
        .filter(|def| def.name == "project")
    {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut outer = project_data::projects::validate("outer", root.to_str().unwrap()).unwrap();
    outer.discovered = true;
    let mut project = project_data::projects::validate("work", "/registered/elsewhere").unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    project.common_dir = Some(checkout.join(".git").to_string_lossy().into_owned());
    let check = |cwd: &std::path::Path, name: &str, expected: bool| {
        let probe = yinz_process::pty_host::remote::status::probe_git(cwd.to_str().unwrap());
        let repository = match project_data::git_probe::repository(
            probe.exit_code,
            &probe.metadata,
            &probe.config,
        ) {
            project_data::git_probe::ProbeOutcome::Checkout(repository) => repository,
            _ => project_data::projects::Repository::folder(cwd.to_string_lossy()),
        };
        crate::project_provider::publish(
            1,
            serde_json::json!({"providers":[], "locations": {cwd.to_str().unwrap():repository}}),
        )
        .unwrap();
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.agents.push(agent_status(cwd));
        filter_project(&mut response, name, &data, |_, _| panic!("local probe")).unwrap();
        assert_eq!(!response.agents.is_empty(), expected, "{cwd:?}: {name}");
    };
    for remote in [true, false] {
        if !remote {
            git(&checkout, &["remote", "remove", "origin"]);
            project.remote = None;
        }
        store
            .apply(
                project_data::store::SHARED,
                &[
                    project_data::projects::to_mutation(&outer),
                    project_data::projects::to_mutation(&project),
                ],
            )
            .unwrap();
        for path in [&checkout, &linked] {
            check(path, "work", true);
            check(path, "outer", false);
        }
        if remote {
            check(&clone, "work", true);
            check(&clone, "outer", false);
        }
        check(&root, "outer", false);
    }
}

#[test]
fn project_filter_limits_assignments_per_host_and_session() {
    let temp = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(temp.path());
    let store = data.plugin_data();
    for def in project_data::store::shared_defs() {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let project = project_data::projects::validate("work", "/work").unwrap();
    let id = store
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
        )
        .unwrap()
        .report
        .ids[0];
    let assignment = |host: &str, session: &str, tab: i64| {
        Mutation::put("agent_assignment")
            .uid(format!("{host}/{session}/{tab}"))
            .set("host_id", host)
            .set("yinz_session_name", session)
            .set("tab_id", tab)
            .set("project", "work")
            .set("project_id", id.get())
            .build()
    };
    // Each unrelated owner exceeds the limit independently: neither half may be omitted.
    for (host, session) in [("other", "session"), ("host", "other")] {
        let mutations: Vec<_> = (0..=project_data::store::LIST_FETCH_LIMIT)
            .map(|tab| assignment(host, session, i64::from(tab)))
            .collect();
        store
            .apply(project_data::store::SHARED, &mutations)
            .unwrap();
    }
    store
        .apply(
            project_data::store::SHARED,
            &[assignment("host", "session", 3)],
        )
        .unwrap();
    let mut response = AgentResponse::new(AgentOutcome::Ok);
    response
        .agents
        .push(agent_status(std::path::Path::new("/outside")));
    filter_project(&mut response, "work", &data, |_, _| {
        panic!("assignment wins")
    })
    .unwrap();
    assert_eq!(response.agents.len(), 1);
    let mutations: Vec<_> = (0..=project_data::store::LIST_FETCH_LIMIT)
        .map(|tab| assignment("host", "session", i64::from(tab)))
        .collect();
    store
        .apply(project_data::store::SHARED, &mutations)
        .unwrap();
    assert_eq!(
        filter_project(&mut response, "work", &data, |_, _| panic!("truncated")),
        Err("project assignments are incomplete".into())
    );
}

#[test]
fn spawn_guidance_lists_recognized_kinds_before_any_checkout_work() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    for (kind, command) in [(None, None), (Some("bogus"), None), (None, Some("zsh"))] {
        let mut spec = AgentSpawnSpec {
            kind: kind.map(str::to_owned),
            command: command.map(str::to_owned),
            branch: Some("must-not-exist".into()),
            ..Default::default()
        };
        let error = prepare_automation_agent(
            &mut spec,
            Some(directory.path().into()),
            None,
            None,
            &data,
            &Default::default(),
        )
        .err()
        .unwrap()
        .to_string();
        for kind in yinz_types::agent::KINDS {
            assert!(error.contains(kind), "{error}");
        }
        assert!(error.contains("--file PATH"), "{error}");
        assert!(error.contains("command"), "{error}");
        assert!(!directory.path().join(".worktrees").exists());
    }
    for (kind, command, args) in [
        (Some("CoDeX"), None, vec![]),
        (Some("gemini"), None, vec![]),
        (Some("cursor-agent"), None, vec![]),
        (None, Some("/opt/bin/claude.js"), vec![]),
        (None, Some("node"), vec!["/opt/bin/gemini.js".into()]),
    ] {
        let mut spec = AgentSpawnSpec {
            kind: kind.map(str::to_owned),
            command: command.map(str::to_owned),
            args,
            allow_outside_worktree: Some(true),
            ..Default::default()
        };
        assert!(prepare_automation_agent(
            &mut spec,
            Some(directory.path().into()),
            None,
            None,
            &data,
            &Default::default()
        )
        .is_ok());
    }
}

#[test]
fn spawn_reports_renamed_and_reused_worktrees_and_keeps_them_out_of_git_status() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["commit", "--allow-empty", "-q", "-m", "initial"]);
    let data = DataAccess::sqlite(&base.join("data"));
    let spawn = |reuse: bool| {
        let mut spec = AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("topic".into()),
            reuse: reuse.then_some(true),
            ..Default::default()
        };
        prepare_automation_agent(
            &mut spec,
            Some(root.clone()),
            None,
            None,
            &data,
            &Default::default(),
        )
        .unwrap()
    };

    let first = spawn(false);
    assert_eq!(first.branch.as_deref(), Some("topic"));
    assert_eq!(first.requested_branch, None);
    assert!(!first.reused);

    let second = spawn(false);
    assert_eq!(second.branch.as_deref(), Some("topic-2"));
    assert_eq!(second.requested_branch.as_deref(), Some("topic"));
    assert!(!second.reused);

    let third = spawn(true);
    assert_eq!(third.cwd, root.join(".worktrees").join("topic"));
    assert_eq!(third.requested_branch, None);
    assert!(third.reused);

    let status = process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    for entry in ["/.worktrees/topic/\n", "/.worktrees/topic-2/\n"] {
        assert_eq!(exclude.matches(entry).count(), 1, "{exclude}");
    }
}

#[test]
fn a_nested_worktree_dir_excludes_only_the_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["commit", "--allow-empty", "-q", "-m", "initial"]);
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        branch: Some("topic".into()),
        worktree_dir: Some("src/worktrees".into()),
        ..Default::default()
    };
    let prepared = prepare_automation_agent(
        &mut spec,
        Some(root.clone()),
        None,
        None,
        &DataAccess::sqlite(&base.join("data")),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(prepared.cwd, root.join("src/worktrees/topic"));
    std::fs::write(root.join("src/new.rs"), "").unwrap();
    let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("\n/src/worktrees/topic/\n"), "{exclude}");
    assert!(!exclude.contains("\n/src/\n"), "{exclude}");
    let status = process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "?? src/new.rs\n");
}

#[test]
fn a_worktree_ignored_by_the_project_adds_no_exclude_entry() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join(".gitignore"), ".worktrees/\n").unwrap();
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "-q", "-m", "initial"]);
    let before = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default();
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        branch: Some("topic".into()),
        ..Default::default()
    };
    prepare_automation_agent(
        &mut spec,
        Some(root.clone()),
        None,
        None,
        &DataAccess::sqlite(&base.join("data")),
        &Default::default(),
    )
    .unwrap();
    let after = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default();
    assert_eq!(before, after);
}

#[test]
fn reused_worktrees_name_the_live_agents_already_in_them() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().canonicalize().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(tx)),
        ..Default::default()
    };
    let new_pane = PaneId::Terminal(crate::TerminalId::new(9));
    let screen = {
        let checkout = checkout.clone();
        let other = other.path().to_owned();
        std::thread::spawn(move || {
            let (instruction, _) = rx.recv().unwrap();
            let ScreenInstruction::AgentControlReply(AgentRequest::List { project: None }, reply) =
                instruction
            else {
                panic!("expected an agent listing");
            };
            let mut response = AgentResponse::new(AgentOutcome::Ok);
            let mut same = agent_status(&checkout);
            same.pane_id = PaneId::Terminal(crate::TerminalId::new(7));
            let mut itself = agent_status(&checkout);
            itself.pane_id = new_pane;
            let mut elsewhere = agent_status(&other);
            elsewhere.pane_id = PaneId::Terminal(crate::TerminalId::new(8));
            response.agents = vec![same, itself, elsewhere];
            reply.send(response).unwrap();
        })
    };
    let mut spawned = SpawnedAgent {
        pane_id: new_pane,
        branch: Some("topic".into()),
        cwd: Some(checkout),
        reused: true,
        ..Default::default()
    };
    spawned.occupied_by = occupants(&senders, &spawned);
    screen.join().unwrap();
    assert_eq!(
        spawned.occupied_by,
        vec![PaneId::Terminal(crate::TerminalId::new(7))]
    );
    let notes = spawn_notes(&spawned).unwrap();
    assert!(notes.contains("already used by terminal_7"), "{notes}");
}

#[test]
fn spawn_notes_explain_renames_and_stay_quiet_otherwise() {
    let mut spawned = SpawnedAgent {
        branch: Some("topic-2".into()),
        requested_branch: Some("topic".into()),
        ..Default::default()
    };
    assert_eq!(
        spawn_notes(&spawned).as_deref(),
        Some("branch topic was taken; created topic-2")
    );
    spawned.requested_branch = None;
    assert_eq!(spawn_notes(&spawned), None);
    spawned.reused = true;
    assert_eq!(
        spawn_notes(&spawned).as_deref(),
        Some("reused an existing worktree")
    );
}

#[test]
fn bubblewrap_rejects_remote_targets_before_checkout_changes() {
    let temp = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(temp.path());
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        bubblewrap: true,
        allow_outside_worktree: Some(true),
        ..Default::default()
    };
    let result = prepare_automation_agent(
        &mut spec,
        Some(temp.path().into()),
        None,
        Some("host".into()),
        &data,
        &Default::default(),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Bubblewrap cannot be combined"));
    assert!(!temp.path().join(".worktrees").exists());
}

#[test]
fn bubblewrap_checkout_rejections_leave_git_and_paths_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["commit", "--allow-empty", "-qm", "initial"]);
    std::os::unix::fs::symlink(&base, root.join("escape")).unwrap();
    std::os::unix::fs::symlink(base.join("missing"), root.join("dangling")).unwrap();
    let state = || {
        let outputs: Vec<_> = [
            vec!["show-ref"],
            vec!["rev-parse", "HEAD"],
            vec!["worktree", "list", "--porcelain"],
        ]
        .into_iter()
        .map(|args| {
            process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
                .stdout
        })
        .collect();
        (
            outputs,
            std::fs::read(root.join(".git/info/exclude")).unwrap(),
        )
    };
    let before = state();
    let data = DataAccess::sqlite(&base.join("data"));
    let store = data.plugin_data();
    for def in project_data::store::shared_defs() {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = project_data::projects::validate("work", root.to_str().unwrap()).unwrap();
    project.worktree_dir = Some(base.join("saved-external").to_string_lossy().into_owned());
    store
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
        )
        .unwrap();
    for explicit in [None, Some(base.join("explicit-external"))] {
        let spec = AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("configured".into()),
            worktree_dir: explicit
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let (_, mut checkout) = yinz_runtime::agent::launch::plan(&spec).unwrap();
        apply_project_worktree_dir(&mut checkout, spec.worktree_dir.as_deref(), &root, || {
            store.fetch(
                project_data::store::SHARED,
                &FetchRequest::entity("project"),
            )
        })
        .unwrap();
        let mut attempted = None;
        assert!(agent_folder_tracked(
            checkout,
            Some(base.clone()),
            root.clone(),
            false,
            WorktreeGuard::Required,
            &mut attempted,
            Some(&root),
            &Default::default()
        )
        .is_err());
        assert!(attempted.is_none());
        assert_eq!(state(), before);
    }
    assert!(!base.join("saved-external").exists());
    assert!(!base.join("explicit-external").exists());
    std::fs::create_dir(root.join("suffix")).unwrap();
    std::os::unix::fs::symlink(base.join("missing-suffix"), root.join("suffix-2")).unwrap();
    let mut attempted = None;
    assert!(prepare_git_checkout_in_tracked(
        root.clone(),
        GitCheckout::NewWorktree {
            branch: "suffix".into(),
            base: "HEAD".into(),
            path: root.join("suffix")
        },
        false,
        WorktreeGuard::Required,
        &mut attempted,
        Some(&root),
        &Default::default()
    )
    .is_err());
    assert!(attempted.is_none());
    assert_eq!(state(), before);
    assert!(!base.join("missing-suffix").exists());

    for path in [
        base.join("external"),
        PathBuf::from("../relative"),
        PathBuf::from("escape/symlink"),
        PathBuf::from("dangling/tree"),
    ] {
        for reuse in [false, true] {
            let mut attempted = None;
            assert!(prepare_git_checkout_in_tracked(
                root.clone(),
                GitCheckout::NewWorktree {
                    branch: "topic".into(),
                    base: "HEAD".into(),
                    path: path.clone()
                },
                reuse,
                WorktreeGuard::Required,
                &mut attempted,
                Some(&root),
                &Default::default()
            )
            .is_err());
            assert!(attempted.is_none());
            assert_eq!(state(), before);
        }
    }
    assert!(!base.join("external").exists());
    assert!(!base.join("relative").exists());
    assert!(!base.join("symlink").exists());
    let mut attempted = None;
    let prepared = prepare_git_checkout_in_tracked(
        root.clone(),
        GitCheckout::NewWorktree {
            branch: "inside".into(),
            base: "HEAD".into(),
            path: PathBuf::from("new/parents/inside"),
        },
        false,
        WorktreeGuard::Required,
        &mut attempted,
        Some(&root),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(prepared.path, root.join("new/parents/inside"));
}

#[test]
fn bubblewrap_checks_reused_and_collision_suffixed_destinations() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["commit", "--allow-empty", "-qm", "initial"]);
    let outside = base.join("outside");
    git(
        &root,
        &["worktree", "add", "-b", "reused", outside.to_str().unwrap()],
    );
    let mut attempted = None;
    for checkout in [
        GitCheckout::NewBranch {
            branch: "reused".into(),
            base: "HEAD".into(),
        },
        GitCheckout::NewWorktree {
            branch: "reused".into(),
            base: "HEAD".into(),
            path: root.join("unused"),
        },
        GitCheckout::NewWorktree {
            branch: "other".into(),
            base: "HEAD".into(),
            path: outside.clone(),
        },
    ] {
        // Reusing a worktree writes nothing, so it is not refused here. The folder it
        // answers with is outside the checkout, which the launch then refuses.
        let reused = prepare_git_checkout_in_tracked(
            root.clone(),
            checkout,
            true,
            WorktreeGuard::Required,
            &mut attempted,
            Some(&root),
            &Default::default(),
        )
        .unwrap();
        assert!(reused.reused_worktree);
        assert_eq!(reused.path, outside);
        assert!(attempted.is_none());
        assert!(
            yinz_config::project_config::bubblewrap::Environment::at_root(&root)
                .validate_cwd(&reused.path)
                .is_err()
        );
    }
    std::fs::create_dir(root.join("collision")).unwrap();
    std::os::unix::fs::symlink(&base, root.join("collision-2")).unwrap();
    // Force the suffix search past an existing path and an escaping symlink.
    let checkout = GitCheckout::NewWorktree {
        branch: "collision".into(),
        base: "HEAD".into(),
        path: root.join("collision"),
    };
    let prepared = prepare_git_checkout_in_tracked(
        root.clone(),
        checkout,
        false,
        WorktreeGuard::Required,
        &mut attempted,
        Some(&root),
        &Default::default(),
    )
    .unwrap();
    assert!(prepared.path.starts_with(&root));
    assert_ne!(prepared.path, root.join("collision-2"));
    let linked = prepared.path;
    let contained = prepare_git_checkout_in_tracked(
        linked.clone(),
        GitCheckout::NewWorktree {
            branch: "from-linked".into(),
            base: "HEAD".into(),
            path: PathBuf::from("trees/from-linked"),
        },
        false,
        WorktreeGuard::Required,
        &mut attempted,
        Some(&root),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(contained.path, root.join("trees/from-linked"));
    // The caller's own directory is answered as it is, and refused as the launch folder.
    let current = agent_folder_tracked(
        GitCheckout::Current,
        Some(outside.clone()),
        linked,
        false,
        WorktreeGuard::Waived,
        &mut None,
        Some(&root),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(current.path, outside);
    assert!(
        yinz_config::project_config::bubblewrap::Environment::at_root(&root)
            .validate_cwd(&current.path)
            .is_err()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn bubblewrap_preparation_validates_each_distinct_checkout_once() {
    const CHILD: &str = "YINZ_TEST_BWRAP_VALIDATION";
    if let Some(directory) = std::env::var_os(CHILD) {
        let directory = PathBuf::from(directory);
        let root = directory.join("repo").canonicalize().unwrap();
        let metadata = yinz_runtime::home::xdg_data_dir()
            .join("bubblewrap/environments")
            .join(yinz_config::project_config::root_key(&root));
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::write(
            metadata.join("accepted.json"),
            serde_json::to_vec(&serde_json::json!({
                "root": root, "ready": true,
                "declaration": {"mounts": [], "paths": [], "env": {}, "inherit": [],
                    "seeds": [], "setup": [], "checks": []}
            }))
            .unwrap(),
        )
        .unwrap();
        let data = DataAccess::sqlite(&directory.join("store"));
        for def in project_data::store::shared_defs() {
            data.plugin_data()
                .define_entity(project_data::store::SHARED, "test", &def, false)
                .unwrap();
        }
        for (branch, cwd, expected) in [
            (None, root.clone(), 1),
            (None, directory.join("alias"), 1),
            (Some("inside"), root.clone(), 2),
        ] {
            std::fs::write(directory.join("calls"), "").unwrap();
            let spec = AgentSpawnSpec {
                kind: Some("codex".into()),
                bubblewrap: true,
                branch: branch.map(str::to_owned),
                worktree_dir: branch.map(|_| root.join("trees").to_string_lossy().into_owned()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            };
            let prepared =
                prepare_agent(&spec, root.clone(), Some(cwd), &data, &Default::default()).unwrap();
            assert!(prepared.command.target.is_bubblewrap());
            assert_eq!(
                std::fs::read_to_string(directory.join("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                expected
            );
        }
        let environment = yinz_config::project_config::bubblewrap::Environment::at_root(&root);
        for malformed in ["", "/tmp", "/tmp\n/tmp\n/tmp", "relative\nrelative"] {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("YINZ_TEST_GIT_OUTPUT", malformed) };
            assert!(environment.validate_cwd(&root).is_err());
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    let root = directory.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    std::os::unix::fs::symlink(&root, directory.join("alias")).unwrap();
    let git_binary = process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(git_binary.status.success());
    let bin = directory.join("bin");
    std::fs::create_dir(&bin).unwrap();
    yinz_test_support::write_warm_script(
        &bin.join("git"),
        r#"
case "$*" in
    *--path-format=absolute*)
        printf 'metadata\n' >> "$YINZ_TEST_BWRAP_VALIDATION/calls"
        if [ "${YINZ_TEST_GIT_OUTPUT+x}" = x ]; then
            printf '%s\n' "$YINZ_TEST_GIT_OUTPUT"
            exit 0
        fi
        ;;
esac
exec "$YINZ_TEST_REAL_GIT" "$@"
"#,
    );
    let output = process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "yinz_exports::agent_control::tests::bubblewrap_preparation_validates_each_distinct_checkout_once",
        ])
        .env(CHILD, &directory)
        .env("XDG_DATA_HOME", directory.join("data"))
        .env(
            "YINZ_TEST_REAL_GIT",
            String::from_utf8(git_binary.stdout).unwrap().trim(),
        )
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn project_filter_respects_host_session_assignment_and_remote_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-b", "main"]);
    let data = DataAccess::sqlite(temp.path());
    let store = data.plugin_data();
    for def in project_data::store::shared_defs() {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = project_data::projects::validate("work", root.to_str().unwrap()).unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    let id = store
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
        )
        .unwrap()
        .report
        .ids[0];
    let status = agent_status(&root);
    let listed = |status: AgentStatus| {
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.agents.push(status);
        filter_project(&mut response, "work", &data, |_, _| {
            yinz_types::data::GetPaneGitProbeResponse::Pending
        })
        .unwrap();
        !response.agents.is_empty()
    };
    assert!(listed(status.clone()));
    assert!(!listed(AgentStatus {
        target: PaneTarget::ssh("other-host").unwrap(),
        ..status.clone()
    }));
    let remote = AgentStatus {
        target: PaneTarget::ssh("server").unwrap(),
        cwd: Some("/work/src".into()),
        ..status.clone()
    };
    let probe = yinz_types::data::PaneGitProbe {
        host: "server".into(),
        cwd: "/work/src".into(),
        exit_code: Some(0),
        metadata: b"/work\n/work/.git\n/work/.git\nmain\n".to_vec(),
        config: b"remote.origin.url\nhttps://example.com/team/repo\0".to_vec(),
    };
    use yinz_types::data::GetPaneGitProbeResponse as Probe;
    let remote_listed = |probe: Probe| {
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.agents.push(remote.clone());
        let pending = filter_project(&mut response, "work", &data, |pane, cwd| {
            assert_eq!(pane, remote.pane_id);
            assert_eq!(cwd, "/work/src");
            probe.clone()
        })
        .unwrap();
        (!response.agents.is_empty(), pending)
    };
    assert_eq!(remote_listed(Probe::Ok(probe.clone())), (true, false));
    assert_eq!(remote_listed(Probe::Pending), (false, true));
    assert_eq!(
        remote_listed(Probe::Err("unavailable".into())),
        (false, true)
    );
    for unavailable in [
        Probe::Ok(yinz_types::data::PaneGitProbe {
            host: "stale".into(),
            ..probe.clone()
        }),
        Probe::Ok(yinz_types::data::PaneGitProbe {
            cwd: "/old".into(),
            ..probe.clone()
        }),
        Probe::Ok(yinz_types::data::PaneGitProbe {
            metadata: vec![],
            ..probe.clone()
        }),
        Probe::Ok(yinz_types::data::PaneGitProbe {
            exit_code: None,
            metadata: vec![],
            ..probe.clone()
        }),
        Probe::Ok(yinz_types::data::PaneGitProbe {
            config: vec![],
            ..probe.clone()
        }),
        Probe::Ok(yinz_types::data::PaneGitProbe {
            config: b"remote.origin.url\nhttps://example.com/team/other\0".to_vec(),
            ..probe.clone()
        }),
    ] {
        assert_eq!(remote_listed(unavailable), (false, false));
    }
    // An explicit assignment wins in both directions and never needs a probe.
    for (project_id, expected) in [(id.get() + 1, false), (id.get(), true)] {
        store
            .apply(
                project_data::store::SHARED,
                &[Mutation::put("agent_assignment")
                    .uid("remote-override")
                    .set("host_id", "host")
                    .set("yinz_session_name", "session")
                    .set("tab_id", 3i64)
                    .set("project", "assigned")
                    .set("project_id", project_id)
                    .build()],
            )
            .unwrap();
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.agents.push(remote.clone());
        let pending = filter_project(&mut response, "work", &data, |_, _| {
            panic!("explicit assignment wins")
        })
        .unwrap();
        assert!(!pending);
        assert_eq!(!response.agents.is_empty(), expected);
    }
    store
        .apply(
            project_data::store::SHARED,
            &[Mutation::delete(
                "agent_assignment",
                yinz_types::plugin_data::Pred::eq("uid", "remote-override"),
            )],
        )
        .unwrap();
    let outside = AgentStatus {
        cwd: Some("/other".into()),
        ..status
    };
    for (host, session, expected) in [
        ("other", "session", false),
        ("host", "other", false),
        ("host", "session", true),
    ] {
        store
            .apply(
                project_data::store::SHARED,
                &[Mutation::put("agent_assignment")
                    .uid(format!("{host}/{session}"))
                    .set("host_id", host)
                    .set("yinz_session_name", session)
                    .set("tab_id", 3i64)
                    .set("project", "work")
                    .set("project_id", id.get())
                    .build()],
            )
            .unwrap();
        assert_eq!(listed(outside.clone()), expected);
    }
}

#[test]
fn project_launch_uses_worktree_defaults_collision_names_and_reuse() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
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
        git(&root, &args);
    }
    let data = DataAccess::sqlite(&root.join("data"));
    let store = data.plugin_data();
    for def in project_data::store::shared_defs() {
        store
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = project_data::projects::validate("work", root.to_str().unwrap()).unwrap();
    project.worktree_dir = Some("custom-trees".into());
    store
        .apply(
            project_data::store::SHARED,
            &[project_data::projects::to_mutation(&project)],
        )
        .unwrap();
    for (reuse, branch) in [(false, "topic"), (false, "topic-2"), (true, "topic")] {
        let mut spec = AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("topic".into()),
            reuse: Some(reuse),
            ..Default::default()
        };
        let prepared = prepare_automation_agent(
            &mut spec,
            None,
            Some("work".into()),
            None,
            &data,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(prepared.cwd, root.join("custom-trees").join(branch));
        assert_eq!(prepared.branch.as_deref(), Some(branch));
    }
}

#[test]
fn project_agent_list_rejects_partial_membership_until_local_probes_finish() {
    use project_data::projects::{self, Repository};
    let temp = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(temp.path());
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = projects::validate("work", "/work").unwrap();
    project.discovered = true;
    project.provider_identity = Some("[\"marker\",\"work\"]".into());
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &[projects::to_mutation(&project)],
        )
        .unwrap();
    let project_id = projects::row_ids(&super::projects(&data).unwrap())["work"];
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &[Mutation::put("agent_assignment")
                .uid("assigned")
                .set("host_id", "host")
                .set("yinz_session_name", "session")
                .set("tab_id", 3i64)
                .set("project", "work")
                .set("project_id", project_id.get())
                .build()],
        )
        .unwrap();
    let assigned = agent_status(std::path::Path::new("/outside"));
    let mut probed = agent_status(std::path::Path::new("/work/child"));
    probed.pane_id = PaneId::Terminal(TerminalId::new(8));
    probed.tab_id = Some(TabId::new(4));
    for resolved in [false, true] {
        if resolved {
            let repository = Repository {
                root: "/work".into(),
                provider_identity: project.provider_identity.clone(),
                ..Default::default()
            };
            crate::project_provider::publish(
                99,
                serde_json::json!({"providers":[], "locations":{"/work/child":repository}}),
            )
            .unwrap();
        } else {
            crate::project_provider::invalidate();
        }
        let agents = vec![assigned.clone(), probed.clone()];
        let (to_screen, screen_rx) = yinz_runtime::channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(yinz_runtime::channels::SenderWithContext::new(to_screen)),
            ..Default::default()
        };
        let screen = thread::spawn(move || {
            let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let ScreenInstruction::AgentControlReply(AgentRequest::List { .. }, reply) =
                instruction
            else {
                panic!("expected list request");
            };
            let mut response = AgentResponse::new(AgentOutcome::Ok);
            response.agents = agents;
            reply.send(response).unwrap();
        });
        let response = control_agent(
            AgentRequest::List {
                project: Some("work".into()),
            },
            crate::ClientId::new(1),
            &senders,
            &data,
        );
        screen.join().unwrap();
        if resolved {
            assert_eq!(response.outcome, AgentOutcome::Ok);
            assert_eq!(response.agents.len(), 2);
        } else {
            assert_eq!(response.outcome, AgentOutcome::Rejected);
            assert!(response.agents.is_empty());
            assert!(response
                .message
                .unwrap()
                .contains("membership is incomplete"));
        }
    }
    crate::project_provider::clear(99);
}

#[test]
fn project_provider_snapshot_drives_native_filter_and_membership_events() {
    use project_data::projects::{self, Repository};
    let temp = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(temp.path());
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = projects::validate("work", "/first").unwrap();
    project.provider_identity = Some("[\"marker\",\"work\"]".into());
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &[projects::to_mutation(&project)],
        )
        .unwrap();
    let status = agent_status(std::path::Path::new("/second"));
    let repository = Repository {
        root: "/second".into(),
        provider_identity: project.provider_identity.clone(),
        display_name: Some("Work".into()),
        ..Default::default()
    };
    crate::project_provider::publish(
        99,
        serde_json::json!({"providers":[], "locations":{"/second":repository}}),
    )
    .unwrap();
    let mut response = AgentResponse::new(AgentOutcome::Ok);
    response.agents = vec![status.clone()];
    assert!(!filter_project(&mut response, "work", &data, |_, _| panic!(
        "must use service snapshot"
    ))
    .unwrap());
    assert_eq!(response.agents.len(), 1);
    let memberships = agent_event_projects(
        vec![status.clone()],
        vec!["work".into()],
        &data,
        &ThreadSenders::default(),
    );
    assert_eq!(memberships["work"], Ok(Some(vec![status.pane_id])));
    crate::project_provider::clear(99);
    let memberships = agent_event_projects(
        vec![status],
        vec!["work".into()],
        &data,
        &ThreadSenders::default(),
    );
    assert_eq!(memberships["work"], Ok(None));
}

fn new_project_data(directory: &std::path::Path) -> DataAccess {
    let data = DataAccess::sqlite(&directory.join("data"));
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    data
}

fn new_project_request(destination: PathBuf) -> AgentRequest {
    AgentRequest::SpawnNewProject {
        name: "new-app".into(),
        destination,
        spec: AgentSpawnSpec {
            kind: Some("codex".into()),
            ..Default::default()
        },
        placement: Default::default(),
    }
}

#[test]
fn new_project_spawn_creates_unborn_main_remembers_root_and_launches_there() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("new-app");
    let data = new_project_data(temp.path());
    let (to_pty, pty_rx) = yinz_runtime::channels::unbounded();
    let (to_screen, screen_rx) = yinz_runtime::channels::unbounded();
    let (to_plugin, plugin_rx) = yinz_runtime::channels::unbounded();
    let senders = ThreadSenders {
        to_pty: Some(yinz_runtime::channels::SenderWithContext::new(to_pty)),
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(to_screen)),
        to_plugin: Some(yinz_runtime::channels::SenderWithContext::new(to_plugin)),
        ..Default::default()
    };
    let expected_root = root.clone();
    let worker = thread::spawn(move || {
        let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
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
            panic!("expected agent launch");
        };
        assert_eq!(command.cwd, Some(expected_root));
        assert!(matches!(target, ClientTabIndexOrPaneId::ClientIdNoFocus(_)));
        completion.set_affected_tab_id(TabId::new(0));
        completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(17)));
    });
    let response = control_agent(
        new_project_request(root.clone()),
        crate::ClientId::new(1),
        &senders,
        &data,
    );
    worker.join().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
    assert_eq!(response.spawned.unwrap().cwd, Some(root.clone()));
    let (notification, _) = plugin_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        matches!(notification, PluginInstruction::RefreshDataWatches { namespace, entities }
        if namespace == project_data::store::SHARED && entities.contains("project"))
    );
    assert!(matches!(
        screen_rx.recv_timeout(Duration::from_secs(30)).unwrap().0,
        ScreenInstruction::AgentSpawned(_)
    ));
    let projects = project_data::projects::from_rows(&projects(&data).unwrap());
    assert_eq!(projects["new-app"].root, root.to_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(root.join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
    assert!(!root.join(".git/refs/heads/main").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn new_project_spawn_rejects_existing_paths_names_roots_and_unavailable_registry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("new-app");
    let data = new_project_data(temp.path());
    let spawn = |data: &DataAccess| {
        control_agent(
            new_project_request(root.clone()),
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            data,
        )
    };
    let missing_parent = control_agent(
        new_project_request(root.join("child")),
        crate::ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(missing_parent.outcome, AgentOutcome::Rejected);
    assert!(!root.exists());
    std::fs::create_dir(&root).unwrap();
    assert_eq!(spawn(&data).outcome, AgentOutcome::Rejected);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::remove_dir(&root).unwrap();
    std::fs::write(&root, "keep this file").unwrap();
    assert_eq!(spawn(&data).outcome, AgentOutcome::Rejected);
    assert_eq!(std::fs::read_to_string(&root).unwrap(), "keep this file");
    std::fs::remove_file(&root).unwrap();
    std::os::unix::fs::symlink(temp.path().join("missing"), &root).unwrap();
    assert_eq!(spawn(&data).outcome, AgentOutcome::Rejected);
    assert!(std::fs::symlink_metadata(&root)
        .unwrap()
        .file_type()
        .is_symlink());
    std::fs::remove_file(&root).unwrap();
    let uninitialized = DataAccess::sqlite(&temp.path().join("uninitialized"));
    assert_eq!(spawn(&uninitialized).outcome, AgentOutcome::Rejected);
    assert!(!root.exists());
    for (name, registered_root) in [("new-app", "/elsewhere"), ("other", root.to_str().unwrap())] {
        let project = project_data::projects::validate(name, registered_root).unwrap();
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &[project_data::projects::to_mutation(&project)],
            )
            .unwrap();
        let response = spawn(&data);
        assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
        assert!(!root.exists());
        assert_eq!(
            project_data::projects::from_rows(&projects(&data).unwrap())[name].root,
            registered_root
        );
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &[project_data::projects::delete_mutation(name)],
            )
            .unwrap();
    }
}

#[test]
fn new_project_spawn_validates_before_creation_and_retains_failed_launches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("new-app");
    let data = new_project_data(temp.path());
    for spec in [
        AgentSpawnSpec::default(),
        AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("topic".into()),
            ..Default::default()
        },
        AgentSpawnSpec {
            kind: Some("codex".into()),
            resume: Some("last".into()),
            ..Default::default()
        },
        AgentSpawnSpec {
            kind: Some("codex".into()),
            reuse: Some(false),
            ..Default::default()
        },
        AgentSpawnSpec {
            kind: Some("codex".into()),
            prompt: Some("work".into()),
            ..Default::default()
        },
    ] {
        let mut request = new_project_request(root.clone());
        if let AgentRequest::SpawnNewProject { spec: launch, .. } = &mut request {
            *launch = spec;
        }
        let response = control_agent(
            request,
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        );
        assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
        assert!(!root.exists());
    }
    let mut request = new_project_request(root.clone());
    for invalid_name in [false, true] {
        if let AgentRequest::SpawnNewProject {
            placement, name, ..
        } = &mut request
        {
            if invalid_name {
                *placement = Default::default();
                *name = " ".into();
            } else {
                *placement = yinz_types::agent::control::AgentPlacement::NewTab(Some("\n".into()));
            }
        }
        assert_eq!(
            control_agent(
                request.clone(),
                crate::ClientId::new(1),
                &ThreadSenders::default(),
                &data
            )
            .outcome,
            AgentOutcome::Rejected
        );
    }
    assert!(!root.exists());
    let response = control_agent(
        new_project_request(root.clone()),
        crate::ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(response.outcome, AgentOutcome::Failed, "{response:?}");
    let message = response.message.unwrap();
    assert!(message.contains(root.to_str().unwrap()), "{message}");
    assert!(message.contains("registration succeeded"), "{message}");
    assert!(message.contains("--allow-outside-worktree"), "{message}");
    assert_eq!(response.launch_resources.unwrap().cwd, Some(root.clone()));
    assert!(root.join(".git").is_dir());
    assert!(project_data::projects::from_rows(&projects(&data).unwrap()).contains_key("new-app"));
}

#[test]
fn new_project_spawn_git_failure_retains_directory_without_registration() {
    // Use a child process so Git's deliberately invalid environment cannot affect other tests.
    if std::env::var_os("YINZ_NEW_PROJECT_GIT_FAILURE").is_none() {
        let output = process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread::current().name().unwrap(), "--nocapture"])
            .env("YINZ_NEW_PROJECT_GIT_FAILURE", "1")
            .env("GIT_CONFIG_COUNT", "invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("new-app");
    let data = new_project_data(temp.path());
    let response = control_agent(
        new_project_request(root.clone()),
        crate::ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(response.outcome, AgentOutcome::Failed, "{response:?}");
    let message = response.message.unwrap();
    assert!(
        message.contains("initialize project repository"),
        "{message}"
    );
    assert!(
        message.contains("registration did not succeed"),
        "{message}"
    );
    assert!(root.is_dir());
    assert!(projects(&data).unwrap().rows.is_empty());
}

#[test]
fn new_project_spawn_never_overwrites_a_concurrent_name_or_root_registration() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_storage::{
        data_access::PluginDataBackend, plugin_data_store::SqlitePluginDataBackend,
    };
    use yinz_types::plugin_data::*;

    struct RegisterDuringFetch {
        inner: SqlitePluginDataBackend,
        project: project_data::projects::Project,
        injected: AtomicBool,
    }
    impl PluginDataBackend for RegisterDuringFetch {
        fn define_entity(
            &self,
            namespace: &str,
            owner: &str,
            def: &EntityDef,
            destructive: bool,
        ) -> Result<DataSchemaInfo, DataError> {
            self.inner.define_entity(namespace, owner, def, destructive)
        }
        fn catalog(&self, namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
            self.inner.catalog(namespace)
        }
        fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
            let rows = self.inner.fetch(namespace, request)?;
            // Another writer commits after our snapshot, before creation's guarded batch.
            if !self.injected.swap(true, Ordering::SeqCst) {
                self.inner.apply(
                    namespace,
                    &[project_data::projects::to_mutation(&self.project)],
                )?;
            }
            Ok(rows)
        }
        fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
            self.inner.count(namespace, request)
        }
        fn apply(&self, namespace: &str, mutations: &[Mutation]) -> Result<ApplyReport, DataError> {
            self.inner.apply(namespace, mutations)
        }
    }
    for same_name in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("new-app");
        let other = project_data::projects::validate(
            if same_name { "new-app" } else { "other" },
            if same_name {
                "/other"
            } else {
                root.to_str().unwrap()
            },
        )
        .unwrap();
        let data = DataAccess::from_backend(Arc::new(RegisterDuringFetch {
            inner: SqlitePluginDataBackend::new(temp.path()),
            project: other.clone(),
            injected: AtomicBool::new(false),
        }));
        for def in project_data::store::shared_defs() {
            data.plugin_data()
                .define_entity(project_data::store::SHARED, "test", &def, false)
                .unwrap();
        }
        let response = control_agent(
            new_project_request(root.clone()),
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        );
        assert_eq!(response.outcome, AgentOutcome::Failed, "{response:?}");
        assert!(response
            .message
            .unwrap()
            .contains("registration did not succeed"));
        assert!(root.join(".git").is_dir());
        let registry = project_data::projects::from_rows(&projects(&data).unwrap());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[&other.name].root, other.root);
    }
}
