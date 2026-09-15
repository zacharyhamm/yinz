use super::*;
use zellij_tile::prelude::HostId;
use zellij_tile::prelude::TabId;

/// A plugin pane is not an agent, a suppressed one is not on screen, and a held
/// one still is - the three cases the snapshot has to tell apart.
#[test]
fn a_snapshot_keeps_the_terminal_panes_that_are_agents() {
    let plugin = PaneInfo {
        id: PaneId::from_legacy(1, true),
        ..pane_info(1, "hocus", None)
    };
    let suppressed = PaneInfo {
        is_suppressed: true,
        ..pane_info(2, "hidden", Some("codex"))
    };
    let held = PaneInfo {
        is_held: true,
        exited: true,
        ..pane_info(3, "codex", Some("codex"))
    };
    let manifest = manifest(vec![plugin, suppressed, held]);

    let panes = terminal_panes(&manifest, &[tab_info(0, TabId::new(9), "work")]);
    assert_eq!(
        panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
        vec![3],
    );
    assert_eq!(
        panes[0].status,
        agents::AgentStatus::Held,
        "held beats exited"
    );
    assert_eq!(panes[0].tab, Some((TabId::new(9), "work".to_owned())));
}

#[test]
fn agent_register_rejects_a_pane_that_is_not_there_and_a_kind_that_is_not_a_word() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":9,"kind":"codex"}"#
        )),
        Some("pane 9 is not available".into()),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":4,"kind":"code review"}"#
        )),
        Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
    );
    assert!(state.manual_kinds.is_empty());
}

#[test]
fn a_manual_kind_lasts_until_its_pane_closes() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":4,"kind":"reviewer"}"#
        )),
        Some("registered pane 4 as reviewer".into()),
    );

    // A retitled pane is still the same pane, so the kind stays.
    state.set_panes(terminal_panes(
        &manifest(vec![pane_info(4, "new title", Some("zsh"))]),
        &[],
    ));
    assert_eq!(
        state.manual_kinds.get(&4).map(String::as_str),
        Some("reviewer")
    );

    state.set_panes(Vec::new());
    assert!(state.manual_kinds.is_empty(), "the pane closed, so it goes");
}

/// Off wasm every host call is a stub, so this checks which panes were listed and
/// what was said about them, not what they were showing.
#[test]
fn agent_list_covers_detected_and_registered_panes_only() {
    let mut state = state_with_panes();
    state.run_local(&agent_call(
        "agent_register",
        r#"{"pane_id":4,"kind":"reviewer"}"#,
    ));

    let list = state.run_local(&agent_call("agent_list", "{}")).unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    let list = list.as_array().unwrap();

    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["pane_id"], 3);
    assert_eq!(list[0]["kind"], "codex");
    assert_eq!(list[0]["source"], "auto");
    assert_eq!(list[0]["status"], "running");
    assert_eq!(list[1]["pane_id"], 4);
    assert_eq!(list[1]["kind"], "reviewer");
    assert_eq!(list[1]["source"], "manual");
}

#[test]
fn agent_list_is_empty_before_the_first_pane_update() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_list", "{}")),
        Some("[]".into()),
    );
}

#[test]
fn agent_spawn_rejects_no_command_and_a_kind_that_is_not_a_word() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_spawn", "{}")),
        Some("an agent or a command is required".into()),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"command":"zsh","kind":"code review"}"#
        )),
        Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
    );
    assert!(state.manual_kinds.is_empty());
}

/// The dispatch itself needs the host, so what is tested here is the half that
/// does not: folding `new_pane`'s answer back into the kinds.
#[test]
fn a_spawned_kind_is_registered_against_the_pane_new_pane_reported() {
    let mut state = idle_state();
    assert_eq!(
        state.register_spawned(
            Some("reviewer"),
            r#"{"opened":true,"pane_id":7,"is_plugin":false}"#
        ),
        r#"{"opened":true,"pane_id":7,"is_plugin":false} (registered as reviewer)"#,
    );
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );
    assert!(state.pending_agent_panes.contains(&7));

    // The subscribe reply can beat the pane-created event back to hocus.
    // That early snapshot must not discard the override it just sent.
    assert!(!state.set_panes(Vec::new()));
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );

    state.set_panes(terminal_panes(
        &manifest(vec![pane_info(7, "zsh", Some("zsh"))]),
        &[],
    ));
    assert!(!state.pending_agent_panes.contains(&7));
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );

    assert!(state.set_panes(Vec::new()));
    assert!(!state.manual_kinds.contains_key(&7));

    // No kind, or no pane in the answer: the result passes through untouched.
    assert_eq!(
        state.register_spawned(None, r#"{"opened":true,"pane_id":8,"is_plugin":false}"#),
        r#"{"opened":true,"pane_id":8,"is_plugin":false}"#,
    );
    assert_eq!(
        state.register_spawned(Some("scout"), "zellij did not open the pane"),
        "zellij did not open the pane",
    );
    assert!(state.manual_kinds.is_empty());
}

/// The whole of `agent_control`, compared as the complete host call it becomes.
/// A wrong pane kind or a wrong tool here is a killed pane.
#[test]
fn every_agent_action_maps_onto_the_tool_that_already_does_it() {
    let state = state_with_panes();
    for (arguments, name, mapped) in [
        (
            r#"{"pane_id":3,"text":"go","keys":["enter"]}"#,
            "send_input",
            r#"{"is_plugin":false,"keys":["enter"],"pane_id":3,"text":"go"}"#,
        ),
        (
            r#"{"pane_id":3,"action":"interrupt"}"#,
            "send_input",
            r#"{"is_plugin":false,"keys":["ctrl_c"],"pane_id":3}"#,
        ),
        (
            r#"{"pane_id":3,"action":"restart"}"#,
            "rerun_pane",
            r#"{"is_plugin":false,"pane_id":3}"#,
        ),
        (
            r#"{"pane_id":3,"action":"close"}"#,
            "close_pane",
            r#"{"is_plugin":false,"pane_id":3}"#,
        ),
    ] {
        let call = agent_call(
            if arguments.contains("action") {
                "agent_control"
            } else {
                "agent_send"
            },
            arguments,
        );
        let args = tools::Args::parse(arguments).unwrap();
        let host = state.agent_host_call(&call, &args).unwrap();
        assert_eq!(host.name, name, "{arguments}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&host.arguments).unwrap(),
            serde_json::from_str::<serde_json::Value>(mapped).unwrap(),
            "{arguments}"
        );
        assert_eq!(host.id, call.id, "the result still answers the call");
    }
}

#[test]
fn agent_tools_refuse_a_pane_that_is_not_an_agent_and_an_action_that_is_not_one() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_control",
            r#"{"pane_id":3,"action":"pause"}"#
        )),
        Some("unknown agent action: pause".into()),
    );
    // Pane 4 is a plain shell that nobody registered.
    assert_eq!(
        state.run_local(&agent_call("agent_send", r#"{"pane_id":4,"text":"go"}"#)),
        Some("pane 4 is not an agent; call agent_list, or agent_register it first".into()),
    );
    assert_eq!(
        state.run_local(&agent_call("agent_send", r#"{"text":"go"}"#)),
        Some("pane_id is required".into()),
    );
}

/// A closed pane leaves the snapshot, and with it every way to send to it.
#[test]
fn a_closed_agent_can_no_longer_be_sent_to() {
    let mut state = state_with_panes();
    assert!(state.is_agent(3));
    state.set_panes(Vec::new());
    assert!(!state.is_agent(3));
}

/// What the latch changes for the tools, stated on purpose: an agent that is
/// busy shelling out is still an agent, so `agent_send` reaches it and the
/// keys land in whatever it is running. That is what a human at the keyboard
/// would get, and it is the only way to interrupt a tool run — losing that
/// would be worse than the surprise.
#[test]
fn a_shelled_out_agent_can_still_be_sent_to() {
    let mut state = state_with_panes();
    let shelled_out = agents::AgentPane {
        title: "zsh".to_owned(),
        command: Some("git status".to_owned()),
        detected: Some("codex".to_owned()),
        ..panes_with_agent()[0].clone()
    };

    state.set_panes(vec![shelled_out]);

    assert!(state.is_agent(3));
}

#[test]
fn agent_changes_do_not_post_to_chat() {
    let mut state = state_with_panes();
    let mut panes = panes_with_agent();
    panes[0].status = agents::AgentStatus::Exited;

    state.apply_poll(
        panes,
        agents::Activity::new(),
        agents::Summaries::new(),
        agents::HookFeeds::new(),
        None,
    );
    state.apply_poll(
        Vec::new(),
        agents::Activity::new(),
        agents::Summaries::new(),
        agents::HookFeeds::new(),
        None,
    );

    assert!(state.messages.is_empty());
}

#[test]
fn a_model_agent_action_is_confirmed_and_a_typed_one_is_not() {
    for (name, arguments) in [
        ("agent_send", r#"{"pane_id":3,"text":"go"}"#),
        ("agent_control", r#"{"pane_id":3,"action":"close"}"#),
    ] {
        let mut state = State {
            stream: state_streaming(Vec::new(), "hocus-1").stream,
            reply_at: Some(1),
            messages: vec![Message::user("hi"), Message::assistant("")],
            ..state_with_panes()
        };
        let frame = tool_call_frame("call_1", name, arguments);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool), "{name}");
        assert!(tool_results(&state).is_empty(), "{name} ran unasked");
    }

    // Typing the command is the confirmation, so it runs on the spot.
    let mut state = state_with_panes();
    state.run_manual(r#"/agent_control pane_id=3 action=close"#);
    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(tool_results(&state).len(), 1);
}

/// The two routes `agent_spawn` takes. Neither can run here — the pane opening is
/// a host call either way — so what is checked is *which* one was chosen, by the
/// error each returns: `new_pane` goes through `tools::dispatch`, the git route
/// through `launch::open`, and their stubs say different things.
#[test]
fn a_branch_is_what_sends_a_spawn_through_the_git_route() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_spawn", r#"{"agent":"claude"}"#)),
        Some("tools are unavailable outside zellij".into()),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"agent":"claude","branch":"fix-parser"}"#
        )),
        Some("tools are unavailable outside zellij".into()),
    );
}

/// Outside a project, `cwd` is the caller's own directory, and the `new_pane` route
/// would pass it into the guest untranslated. The refusal comes before the host
/// call; the same spawn without a cwd reaches it.
#[test]
fn a_firecracker_spawn_outside_a_project_refuses_a_host_cwd() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"agent":"claude","firecracker":true,"cwd":"/home/me/src"}"#
        )),
        Some(
            "cwd is a path on this machine, and a Firecracker agent starts in the VM's \
             workspace; leave cwd unset or name a project"
                .into()
        ),
    );
    assert!(state.manual_kinds.is_empty());
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"agent":"claude","firecracker":true}"#
        )),
        Some("tools are unavailable outside zellij".into()),
    );
}

/// `firecracker=true` promises a VM per agent unless there is a folder to share: the host is
/// told to name the VM after the folder only when a checkout is being cut or a project is
/// selected. Outside both, the unique name hocus generated reaches the host as the VM to run
/// in, and nothing of this machine is mirrored - two agents started from one directory get
/// two VMs.
#[test]
fn a_firecracker_spawn_without_a_checkout_or_project_keeps_its_own_vm() {
    let spawns = || {
        tools::test_seams::SPAWN_AGENT_CALLS.with_borrow(|calls| {
            calls
                .iter()
                .map(|call| {
                    (
                        call.firecracker.clone().unwrap_or_default(),
                        call.automatic_firecracker_vm,
                    )
                })
                .collect::<Vec<_>>()
        })
    };

    let mut state = idle_state();
    tools::test_seams::reset();
    state.run_local(&agent_call(
        "agent_spawn",
        r#"{"agent":"claude","firecracker":true}"#,
    ));
    state.run_local(&agent_call(
        "agent_spawn",
        r#"{"agent":"claude","firecracker":true}"#,
    ));
    let isolated = spawns();
    assert_eq!(isolated.len(), 2, "{isolated:?}");
    for (vm, automatic) in &isolated {
        assert!(vm.starts_with("hocus-agent-"), "{vm}");
        assert!(!automatic, "{vm} would be renamed after the focused folder");
    }
    assert_ne!(isolated[0].0, isolated[1].0, "two agents, two VMs");

    tools::test_seams::reset();
    state.run_local(&agent_call(
        "agent_spawn",
        r#"{"agent":"claude","firecracker":true,"branch":"fix-parser"}"#,
    ));
    assert_eq!(
        spawns()
            .iter()
            .map(|(_, automatic)| *automatic)
            .collect::<Vec<_>>(),
        vec![true],
        "a checkout is a folder to name the VM after"
    );

    let mut state = state_with_project();
    tools::test_seams::reset();
    state.run_local(&agent_call(
        "agent_spawn",
        r#"{"agent":"claude","firecracker":true}"#,
    ));
    assert_eq!(
        spawns()
            .iter()
            .map(|(_, automatic)| *automatic)
            .collect::<Vec<_>>(),
        vec![true],
        "a project's root is a folder to name the VM after"
    );

    tools::test_seams::reset();
    state.run_local(&agent_call("agent_spawn", r#"{"agent":"claude"}"#));
    assert_eq!(spawns(), vec![(String::new(), false)], "no VM at all");
}

#[test]
fn project_images_apply_only_to_firecracker_spawns_and_explicit_images_win() {
    let mut local = launch::Spec {
        kind: Some("codex".to_owned()),
        ..Default::default()
    };
    crate::tool_loop::apply_project_firecracker_image(&mut local, Some("rust".to_owned()));
    assert_eq!(local.firecracker_image, None);

    let mut firecracker = launch::Spec {
        firecracker: Some("review".to_owned()),
        ..local.clone()
    };
    crate::tool_loop::apply_project_firecracker_image(&mut firecracker, Some("rust".to_owned()));
    assert_eq!(firecracker.firecracker_image.as_deref(), Some("rust"));

    firecracker.firecracker_image = Some("custom".to_owned());
    crate::tool_loop::apply_project_firecracker_image(&mut firecracker, Some("ignored".to_owned()));
    assert_eq!(firecracker.firecracker_image.as_deref(), Some("custom"));
}

/// The image is one field of a project and answers to the project tools like the
/// rest: a call that says nothing about it keeps what is stored, an empty one
/// clears it, and one with no root updates a project already known in place.
#[test]
fn a_projects_firecracker_image_is_kept_cleared_or_updated_by_the_project_tools() {
    let mut state = idle_state();
    let remember = |state: &mut State, arguments: &str| {
        state.remember_project(&tools::Args::parse(arguments).unwrap())
    };
    let stored_image = |state: &mut State| {
        let rows = state
            .store()
            .fetch(&FetchRequest::entity("project"))
            .expect("fetched");
        projects::from_rows(&rows)["hocus"]
            .firecracker_image
            .clone()
    };

    let project = remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":"rust"}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("rust"));
    assert_eq!(stored_image(&mut state).as_deref(), Some("rust"));

    // Remembered again with its root and nothing said about the image.
    let project = remember(&mut state, r#"{"name":"hocus","root":"/work/hocus"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("rust"));
    assert_eq!(
        state.projects["hocus"].firecracker_image.as_deref(),
        Some("rust")
    );

    // The image alone, on a project already known.
    let project = remember(&mut state, r#"{"name":"hocus","firecracker_image":"ruby"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("ruby"));
    assert_eq!(stored_image(&mut state).as_deref(), Some("ruby"));

    // Empty clears it, with or without the root.
    let project = remember(&mut state, r#"{"name":"hocus","firecracker_image":""}"#).unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(stored_image(&mut state), None);
    remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":"rust"}"#,
    )
    .unwrap();
    let project = remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":" "}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(stored_image(&mut state), None);

    // Renaming a discovered project and clearing its image in one call: the
    // clear wins over the discovered image, and the answer is what is stored.
    let discovered = |state: &mut State, root: &str, image: &str| {
        let mut registry = state.projects.clone();
        projects::discover(
            &mut registry,
            &[projects::Repository {
                root: root.to_owned(),
                worktree_status: Some(projects::WorktreeStatus::Primary),
                ..projects::Repository::default()
            }],
        );
        let name = registry
            .values()
            .find(|project| project.root == root)
            .map(|project| project.name.clone())
            .expect("discovered");
        registry.get_mut(&name).unwrap().firecracker_image = Some(image.to_owned());
        state.projects = registry;
        name
    };
    let found = discovered(&mut state, "/work/found", "rust");
    let project = remember(
        &mut state,
        r#"{"name":"renamed","root":"/work/found","firecracker_image":""}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(state.projects["renamed"].firecracker_image, None);
    assert!(!state.projects.contains_key(&found));
    // With nothing said about the image, the discovered one comes along and
    // is what the tool reports.
    let other = discovered(&mut state, "/work/other", "go");
    let project = remember(&mut state, r#"{"name":"named","root":"/work/other"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("go"));
    assert_eq!(
        state.projects["named"].firecracker_image.as_deref(),
        Some("go")
    );
    assert!(!state.projects.contains_key(&other));

    // Neither a bad name nor an image for a project nobody has named yet.
    let error = remember(
        &mut state,
        r#"{"name":"hocus","firecracker_image":"no spaces"}"#,
    )
    .expect_err("a space is not an image name");
    assert!(error.contains("invalid Firecracker VM name"), "{error}");
    assert!(remember(&mut state, r#"{"name":"other","firecracker_image":"rust"}"#).is_err());
}

/// `agent` and `command` are alternatives, and the schema cannot say so — this is
/// where a call naming neither is turned back.
#[test]
fn a_spawn_naming_no_agent_and_no_command_is_refused_before_any_git_work() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_spawn", r#"{"branch":"fix-parser"}"#)),
        Some("an agent or a command is required".into()),
    );
    assert!(state.manual_kinds.is_empty());
}

/// The prompt and the model are the agent's own arguments, not hocus's: they reach
/// the pane as argv, in the order `launch` puts them in.
#[test]
fn a_prompt_and_a_model_reach_the_agent_as_its_own_arguments() {
    let args =
        tools::Args::parse(r#"{"agent":"claude","model":"opus","prompt":"fix it","hooks":false}"#)
            .unwrap();
    let (command, _) = launch::plan(&args.launch_spec()).unwrap();
    assert_eq!(command.path, std::path::PathBuf::from("claude"));
    assert_eq!(command.args, ["--model", "opus", "fix it"]);

    // Left to its default, a claude pane is also handed its hook wiring, which the model
    // may pass through untouched as `env`, `permission_mode` and `resume` too.
    let args = tools::Args::parse(
        r#"{"agent":"claude","permission_mode":"skip","resume":"last","env":{"A":"1"}}"#,
    )
    .unwrap();
    let (command, _) = launch::plan(&args.launch_spec()).unwrap();
    assert_eq!(command.path, std::path::PathBuf::from("env"));
    assert_eq!(
        &command.args[..4],
        [
            "A=1",
            "claude",
            "--dangerously-skip-permissions",
            "--continue"
        ]
    );
    assert_eq!(command.args[4], "--settings");
}

/// A kind set by hand outlives the hocus that set it, as long as the pane does.
#[test]
fn a_registered_kind_survives_open_store() {
    let mut state = state_with_panes();
    state.run_local(&agent_call(
        "agent_register",
        r#"{"pane_id":4,"kind":"reviewer"}"#,
    ));
    let rows = state
        .store()
        .fetch(&FetchRequest::entity(agents::rows::LATCH_ENTITY))
        .expect("fetched");
    assert_eq!(
        agents::rows::latches_from_rows(&rows),
        BTreeMap::from([(4, "reviewer".to_owned())])
    );

    // Named, because a latch belongs to a session as well as a pane - see
    // `store::agent_latch_def`. A hocus that has not heard a snapshot yet reads none, and
    // restores them when the first one names the session.
    let mut restarted = State {
        zellij_session_name: "test-session".to_owned().into(),
        host_id: HostId::new("test-host"),
        ..State::default()
    };
    restarted
        .store()
        .apply(vec![agents::rows::latch_mutation(
            &restarted.owner(),
            4,
            "reviewer",
        )])
        .expect("copied");
    restarted.open_store();
    assert_eq!(
        restarted.manual_kinds.get(&4).map(String::as_str),
        Some("reviewer")
    );

    // And the pane closing forgets the row, as it forgets the kind.
    state.set_panes(Vec::new());
    let rows = state
        .store()
        .fetch(&FetchRequest::entity(agents::rows::LATCH_ENTITY))
        .expect("fetched");
    assert!(agents::rows::latches_from_rows(&rows).is_empty());
}

#[test]
fn project_worktree_defaults_reach_the_host_only_for_worktrees() {
    let mut state = idle_state();
    let mut project = projects::validate("trees", "/work/trees").unwrap();
    project.worktree_dir = Some("../saved trees".into());
    state.projects.insert(project.name.clone(), project);
    state.active_project = Some("trees".into());
    tools::test_seams::reset();
    for args in [
        r#"{"agent":"codex","branch":"topic"}"#,
        r#"{"agent":"codex","branch":"other","worktree_dir":"/override"}"#,
        r#"{"agent":"codex","branch":"local","worktree":false}"#,
        r#"{"agent":"codex"}"#,
    ] {
        assert_eq!(
            state.run_local(&agent_call("agent_spawn", args)),
            Some("tools are unavailable outside zellij".into())
        );
    }
    tools::test_seams::SPAWN_AGENT_CALLS.with_borrow(|calls| {
        assert_eq!(
            calls
                .iter()
                .map(|call| call.worktree_dir.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("../saved trees"), Some("/override"), None, None]
        );
    });
}
