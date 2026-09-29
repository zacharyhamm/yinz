use super::*;
use yinz_types::agent::control::AgentStatus;
use yinz_types::command::PaneTarget;
use yinz_types::internal_ids::TabId;
use yinz_types::plugin_data::Mutation;

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
        status_available: false,
        prompt_supported: false,
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
            assert!(result.err().unwrap().contains(error));
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
