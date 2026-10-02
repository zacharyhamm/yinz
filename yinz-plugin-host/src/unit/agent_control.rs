use super::*;
use yinz_types::agent::control::AgentStatus;
use yinz_types::command::PaneTarget;
use yinz_types::internal_ids::TabId;
use yinz_types::plugin_data::Mutation;

#[test]
fn lost_host_control_replies_keep_the_submission_identity_sent_to_screen() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let prompt = |submission_id| AgentRequest::Prompt {
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
        background_tasks: Vec::new(),
    }
}

#[test]
fn project_filter_respects_host_session_assignment_and_remote_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-b", "main"]);
    let data = DataAccess::sqlite(temp.path());
    let store = data.plugin_data();
    for def in hocus_data::store::shared_defs() {
        store
            .define_entity(hocus_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = hocus_data::projects::validate("work", root.to_str().unwrap()).unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    let id = store
        .apply(
            hocus_data::store::SHARED,
            &[hocus_data::projects::to_mutation(&project)],
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
    assert!(listed(AgentStatus {
        target: PaneTarget::firecracker("vm", None, Some(&root)).unwrap(),
        cwd: Some("/workspace".into()),
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
        filter_project(&mut response, "work", &data, |pane, cwd| {
            assert_eq!(pane, remote.pane_id);
            assert_eq!(cwd, "/work/src");
            probe.clone()
        })
        .unwrap();
        !response.agents.is_empty()
    };
    assert!(remote_listed(Probe::Ok(probe.clone())));
    for unavailable in [
        Probe::Pending,
        Probe::Err("unavailable".into()),
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
        assert!(!remote_listed(unavailable));
    }
    // An explicit assignment wins in both directions and never needs a probe.
    for (project_id, expected) in [(id.get() + 1, false), (id.get(), true)] {
        store
            .apply(
                hocus_data::store::SHARED,
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
        filter_project(&mut response, "work", &data, |_, _| {
            panic!("explicit assignment wins")
        })
        .unwrap();
        assert_eq!(!response.agents.is_empty(), expected);
    }
    store
        .apply(
            hocus_data::store::SHARED,
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
                hocus_data::store::SHARED,
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
fn project_launch_defaults_and_firecracker_placement_are_shared() {
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
        assert!(process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let data = DataAccess::sqlite(&root.join("data"));
    let store = data.plugin_data();
    for def in hocus_data::store::shared_defs() {
        store
            .define_entity(hocus_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut project = hocus_data::projects::validate("work", root.to_str().unwrap()).unwrap();
    project.worktree_dir = Some("custom-trees".into());
    project.firecracker_image = Some("rust".into());
    store
        .apply(
            hocus_data::store::SHARED,
            &[hocus_data::projects::to_mutation(&project)],
        )
        .unwrap();
    for (reuse, branch) in [(false, "topic"), (false, "topic-2"), (true, "topic")] {
        let mut spec = AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("topic".into()),
            reuse: Some(reuse),
            ..Default::default()
        };
        let prepared =
            prepare_automation_agent(&mut spec, None, Some("work".into()), None, false, &data)
                .unwrap();
        assert_eq!(prepared.cwd, root.join("custom-trees").join(branch));
        assert_eq!(prepared.branch.as_deref(), Some(branch));
    }
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        firecracker: Some("vm".into()),
        allow_outside_worktree: Some(true),
        ..Default::default()
    };
    let prepared =
        prepare_automation_agent(&mut spec, None, Some("work".into()), None, false, &data).unwrap();
    assert!(
        matches!(prepared.command.target, PaneTarget::Firecracker { vm, image: Some(image), worktree: Some(path) } if vm == "vm" && image == "rust" && path == root)
    );
    assert!(prepare_automation_agent(
        &mut spec,
        Some("/elsewhere".into()),
        Some("work".into()),
        None,
        false,
        &data
    )
    .is_err());
}

#[test]
fn branch_vm_names_are_explicit_unless_automatic() {
    let temp = tempfile::tempdir().unwrap();
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
        assert!(process::Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let root = temp.path().canonicalize().unwrap();
    let data = DataAccess::sqlite(&root.join("data"));
    for (automatic, branch) in [(false, "topic"), (true, "topic-2")] {
        let mut spec = AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("topic".into()),
            firecracker: Some("review".into()),
            ..Default::default()
        };
        let prepared =
            prepare_automation_agent(&mut spec, Some(root.clone()), None, None, automatic, &data)
                .unwrap();
        let folder = root.join(".worktrees").join(branch);
        assert_eq!(prepared.cwd, folder);
        let PaneTarget::Firecracker { vm, worktree, .. } = prepared.command.target else {
            panic!("VM");
        };
        assert_eq!(worktree, Some(folder.clone()));
        assert_eq!(
            vm,
            if automatic {
                crate::firecracker::worktree_vm_name(&folder)
            } else {
                "review".into()
            }
        );
    }
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        firecracker: Some("standalone".into()),
        ..Default::default()
    };
    let prepared = prepare_automation_agent(&mut spec, None, None, None, true, &data).unwrap();
    assert!(
        matches!(prepared.command.target, PaneTarget::Firecracker { vm, worktree: None, .. } if vm == "standalone")
    );
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
    for def in hocus_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(hocus_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    for project_root in [&root, &base.join("alias")] {
        let project =
            hocus_data::projects::validate("work", project_root.to_str().unwrap()).unwrap();
        data.plugin_data()
            .apply(
                hocus_data::store::SHARED,
                &[hocus_data::projects::to_mutation(&project)],
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
                false,
                &data,
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
                false,
                &data,
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
    for def in hocus_data::store::shared_defs()
        .into_iter()
        .filter(|def| def.name == "project")
    {
        store
            .define_entity(hocus_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let outer = hocus_data::projects::validate("outer", root.to_str().unwrap()).unwrap();
    let mut project = hocus_data::projects::validate("work", "/registered/elsewhere").unwrap();
    project.remote = Some("git@example.com:team/repo.git".into());
    project.common_dir = Some(checkout.join(".git").to_string_lossy().into_owned());
    let check = |cwd: &std::path::Path, name: &str, expected: bool| {
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
                hocus_data::store::SHARED,
                &[
                    hocus_data::projects::to_mutation(&outer),
                    hocus_data::projects::to_mutation(&project),
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
    for def in hocus_data::store::shared_defs() {
        store
            .define_entity(hocus_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let project = hocus_data::projects::validate("work", "/work").unwrap();
    let id = store
        .apply(
            hocus_data::store::SHARED,
            &[hocus_data::projects::to_mutation(&project)],
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
        let mutations: Vec<_> = (0..=hocus_data::store::LIST_FETCH_LIMIT)
            .map(|tab| assignment(host, session, i64::from(tab)))
            .collect();
        store.apply(hocus_data::store::SHARED, &mutations).unwrap();
    }
    store
        .apply(
            hocus_data::store::SHARED,
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
    let mutations: Vec<_> = (0..=hocus_data::store::LIST_FETCH_LIMIT)
        .map(|tab| assignment("host", "session", i64::from(tab)))
        .collect();
    store.apply(hocus_data::store::SHARED, &mutations).unwrap();
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
            false,
            &data,
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
            false,
            &data
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
        prepare_automation_agent(&mut spec, Some(root.clone()), None, None, false, &data).unwrap()
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
        false,
        &DataAccess::sqlite(&base.join("data")),
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
        false,
        &DataAccess::sqlite(&base.join("data")),
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
